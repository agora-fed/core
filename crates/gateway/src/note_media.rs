//! # Note media attachments — upload, listing, and AP mapping (migration 0407).
//!
//! MinIO put + Postgres insert for one image at a time. Videos and audio are
//! reserved in the schema but rejected at the API for now (0.18.0-gamma
//! scope: images only). Reuses the [`dsoc_core::Storage`] port already wired
//! into `AppState`, so no new dependency wiring — the same MinIO bucket that
//! serves avatars serves note media, under a `notes/YYYY/MM/{uuid}.png` key
//! prefix.
//!
//! Runtime-unchecked `sqlx::query*` calls — mirrors the policy in
//! `federation_feed.rs` (no `.sqlx/` regeneration on schema growth).

use chrono::{DateTime, Datelike, Utc};
use dsoc_core::Storage;
use image::ImageFormat;
#[cfg(test)]
use image::{ImageBuffer, Rgba};
use serde::Serialize;
use sqlx::PgPool;
use std::io::Cursor;
use std::sync::Arc;
use uuid::Uuid;

/// Max accepted raw upload size before decode. Above this we short-circuit.
pub const MAX_UPLOAD_BYTES: usize = 8 * 1024 * 1024;
/// Bounding box for the re-encoded image. Portrait phones easily hit 12 Mpx —
/// we downscale to something a feed card can render without a jank spike.
pub const MAX_DIMENSION: u32 = 1600;
/// Max attachments the API accepts per Note (Mastodon parity).
pub const MAX_PER_NOTE: usize = 4;

#[derive(Debug, Clone, Serialize)]
pub struct MediaDto {
    pub id: Uuid,
    pub url: String,
    pub kind: String,
    pub content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
}

#[derive(Debug, sqlx::FromRow)]
struct MediaRow {
    id: Uuid,
    kind: String,
    object_key: Option<String>,
    remote_url: Option<String>,
    content_type: String,
    alt_text: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
}

fn url_of(row: &MediaRow, media_base_url: &str) -> String {
    if let Some(key) = row.object_key.as_deref() {
        let base = media_base_url.trim_end_matches('/');
        if base.is_empty() {
            format!("/media/{key}")
        } else {
            format!("{base}/{key}")
        }
    } else {
        row.remote_url.clone().unwrap_or_default()
    }
}

/// Result of a fresh upload — the id the caller ties to their Note, plus the
/// derived public URL so the client can render a preview right away.
#[derive(Debug, Clone)]
pub struct Uploaded {
    pub id: Uuid,
    pub url: String,
    pub kind: String,
    pub content_type: String,
    pub alt_text: Option<String>,
    pub width: i32,
    pub height: i32,
}

/// Validate → resize → re-encode as PNG → put in storage → INSERT the media row.
/// The caller passes their own `actor_url` so the row is auditable back to who
/// uploaded it (used later for GC of orphaned uploads).
pub async fn upload_image(
    db: &PgPool,
    storage: Option<&Arc<dyn Storage>>,
    actor_url: &str,
    raw: Vec<u8>,
    alt_text: Option<String>,
    media_base_url: &str,
) -> Result<Uploaded, UploadError> {
    if raw.len() > MAX_UPLOAD_BYTES {
        return Err(UploadError::TooLarge);
    }
    let storage = storage.ok_or(UploadError::StorageUnwired)?;
    // Decode + resize on the blocking pool: this is CPU-bound and we do not
    // want to stall the tokio reactor on a big JPEG.
    let processed =
        tokio::task::spawn_blocking(move || decode_and_shrink(&raw))
            .await
            .map_err(|_| UploadError::Runtime)??;
    // key: notes/YYYY/MM/{uuid}.png
    let id = Uuid::now_v7();
    let now = Utc::now();
    let key = format!(
        "notes/{:04}/{:02}/{}.png",
        now.year(),
        now.month(),
        id.simple()
    );
    storage
        .put(&key, "image/png", processed.bytes)
        .await
        .map_err(UploadError::Storage)?;
    let alt_owned = alt_text
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(1500).collect::<String>());
    sqlx::query(
        r"
        INSERT INTO media_attachment
            (id, actor_url, kind, object_key, remote_url, content_type,
             alt_text, width, height, blurhash, size_bytes, created_at)
        VALUES ($1, $2, 'image', $3, NULL, 'image/png', $4, $5, $6, NULL, $7, $8)
        ",
    )
    .bind(id)
    .bind(actor_url)
    .bind(&key)
    .bind(alt_owned.as_deref())
    .bind(processed.width as i32)
    .bind(processed.height as i32)
    .bind(processed.byte_len as i64)
    .bind(now)
    .execute(db)
    .await
    .map_err(UploadError::Db)?;
    let url = {
        let base = media_base_url.trim_end_matches('/');
        if base.is_empty() {
            format!("/media/{key}")
        } else {
            format!("{base}/{key}")
        }
    };
    Ok(Uploaded {
        id,
        url,
        kind: "image".into(),
        content_type: "image/png".into(),
        alt_text: alt_owned,
        width: processed.width as i32,
        height: processed.height as i32,
    })
}

/// Update the alt_text on a media row. Best-effort — trims + caps at 1500
/// chars; a missing id is not an error. Runs from the note publish path.
pub async fn update_alt_text(
    db: &PgPool,
    media_id: Uuid,
    alt: &str,
) -> Result<(), sqlx::Error> {
    let trimmed: String = alt.trim().chars().take(1500).collect();
    sqlx::query(r"UPDATE media_attachment SET alt_text = $2 WHERE id = $1")
        .bind(media_id)
        .bind(&trimmed)
        .execute(db)
        .await?;
    Ok(())
}

/// Bind a set of uploaded attachments to a Note. Order-preserving via
/// `sort_order`. Idempotent on (object_uri, media_id).
pub async fn attach_to_note(
    db: &PgPool,
    object_uri: &str,
    media_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let now = Utc::now();
    for (order, mid) in media_ids.iter().take(MAX_PER_NOTE).enumerate() {
        sqlx::query(
            r"
            INSERT INTO note_media (id, object_uri, media_id, sort_order, created_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (object_uri, media_id) DO NOTHING
            ",
        )
        .bind(Uuid::now_v7())
        .bind(object_uri)
        .bind(mid)
        .bind(order as i32)
        .bind(now)
        .execute(db)
        .await?;
    }
    Ok(())
}

/// Row used only by `list_for_notes` — flattens the join so sqlx can decode
/// via `FromRow`. Not exposed; consumers get `MediaDto` values.
#[derive(Debug, sqlx::FromRow)]
struct BatchMediaRow {
    object_uri: String,
    id: Uuid,
    kind: String,
    object_key: Option<String>,
    remote_url: Option<String>,
    content_type: String,
    alt_text: Option<String>,
    width: Option<i32>,
    height: Option<i32>,
}

/// Batch-fetch media for a set of Notes. Returns a map keyed by `object_uri`.
/// Avoids N+1 when the feed handler renders 20+ Notes.
pub async fn list_for_notes(
    db: &PgPool,
    object_uris: &[String],
    media_base_url: &str,
) -> Result<std::collections::HashMap<String, Vec<MediaDto>>, sqlx::Error> {
    if object_uris.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let rows = sqlx::query_as::<_, BatchMediaRow>(
        r"
        SELECT nm.object_uri,
               m.id,
               m.kind,
               m.object_key,
               m.remote_url,
               m.content_type,
               m.alt_text,
               m.width,
               m.height
          FROM note_media nm
          JOIN media_attachment m ON m.id = nm.media_id
         WHERE nm.object_uri = ANY($1::text[])
         ORDER BY nm.object_uri, nm.sort_order
        ",
    )
    .bind(object_uris)
    .fetch_all(db)
    .await?;
    let mut out: std::collections::HashMap<String, Vec<MediaDto>> =
        std::collections::HashMap::new();
    for r in rows {
        let media_row = MediaRow {
            id: r.id,
            kind: r.kind.clone(),
            object_key: r.object_key,
            remote_url: r.remote_url,
            content_type: r.content_type.clone(),
            alt_text: r.alt_text.clone(),
            width: r.width,
            height: r.height,
        };
        let url = url_of(&media_row, media_base_url);
        out.entry(r.object_uri).or_default().push(MediaDto {
            id: r.id,
            url,
            kind: r.kind,
            content_type: r.content_type,
            alt_text: r.alt_text,
            width: r.width,
            height: r.height,
        });
    }
    Ok(out)
}

/// Read the media attached to a Note, in the same order the author placed them.
pub async fn list_for_note(
    db: &PgPool,
    object_uri: &str,
    media_base_url: &str,
) -> Result<Vec<MediaDto>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaRow>(
        r"
        SELECT m.id,
               m.kind,
               m.object_key,
               m.remote_url,
               m.content_type,
               m.alt_text,
               m.width,
               m.height
          FROM note_media nm
          JOIN media_attachment m ON m.id = nm.media_id
         WHERE nm.object_uri = $1
         ORDER BY nm.sort_order
        ",
    )
    .bind(object_uri)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| MediaDto {
            id: r.id,
            url: url_of(&r, media_base_url),
            kind: r.kind.clone(),
            content_type: r.content_type.clone(),
            alt_text: r.alt_text.clone(),
            width: r.width,
            height: r.height,
        })
        .collect())
}

/// Persist a remote attachment (received via inbound Create(Note).attachment[]).
/// Not proxied yet — we cache the remote URL as-is. Best-effort: any failure
/// (unknown mime, bad shape) is silently ignored by the caller.
pub async fn upsert_remote_media(
    db: &PgPool,
    actor_url: &str,
    remote_url: &str,
    content_type: &str,
    alt_text: Option<&str>,
    width: Option<i32>,
    height: Option<i32>,
) -> Result<Uuid, sqlx::Error> {
    let kind = if content_type.starts_with("image/") {
        "image"
    } else if content_type.starts_with("video/") {
        "video"
    } else if content_type.starts_with("audio/") {
        "audio"
    } else {
        return Err(sqlx::Error::Protocol("unsupported media type".into()));
    };
    let id = Uuid::now_v7();
    let now: DateTime<Utc> = Utc::now();
    sqlx::query(
        r"
        INSERT INTO media_attachment
            (id, actor_url, kind, object_key, remote_url, content_type,
             alt_text, width, height, blurhash, size_bytes, created_at)
        VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, NULL, NULL, $9)
        ",
    )
    .bind(id)
    .bind(actor_url)
    .bind(kind)
    .bind(remote_url)
    .bind(content_type)
    .bind(alt_text)
    .bind(width)
    .bind(height)
    .bind(now)
    .execute(db)
    .await?;
    Ok(id)
}

#[derive(Debug)]
pub enum UploadError {
    TooLarge,
    NotAnImage,
    StorageUnwired,
    Storage(dsoc_core::Error),
    Db(sqlx::Error),
    Runtime,
}

impl UploadError {
    /// User-facing PT-BR message for the API envelope.
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::TooLarge => format!(
                "arquivo maior que {} MB",
                MAX_UPLOAD_BYTES / (1024 * 1024)
            ),
            Self::NotAnImage => "envie uma imagem PNG, JPEG ou WebP".into(),
            Self::StorageUnwired => {
                "armazenamento não configurado no servidor".into()
            }
            Self::Storage(_) => "não foi possível salvar o arquivo agora".into(),
            Self::Db(_) => "erro ao registrar a mídia".into(),
            Self::Runtime => "erro interno ao processar a imagem".into(),
        }
    }
}

struct Processed {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    byte_len: usize,
}

fn decode_and_shrink(raw: &[u8]) -> Result<Processed, UploadError> {
    let format = image::guess_format(raw).map_err(|_| UploadError::NotAnImage)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    ) {
        return Err(UploadError::NotAnImage);
    }
    let img = image::load_from_memory_with_format(raw, format)
        .map_err(|_| UploadError::NotAnImage)?;
    // Keep aspect: only shrink if either dimension exceeds MAX_DIMENSION.
    let (w, h) = (img.width(), img.height());
    let shrunk = if w > MAX_DIMENSION || h > MAX_DIMENSION {
        img.resize(MAX_DIMENSION, MAX_DIMENSION, image::imageops::FilterType::Lanczos3)
    } else {
        img
    };
    let (fw, fh) = (shrunk.width(), shrunk.height());
    let mut buf: Vec<u8> = Vec::with_capacity(256 * 1024);
    shrunk
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .map_err(|_| UploadError::Runtime)?;
    let byte_len = buf.len();
    Ok(Processed {
        bytes: buf,
        width: fw,
        height: fh,
        byte_len,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn fake_png(w: u32, h: u32) -> Vec<u8> {
        let img: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_fn(w, h, |_, _| Rgba([20, 180, 90, 255]));
        let mut out = Vec::new();
        img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).unwrap();
        out
    }

    #[test]
    fn shrinks_large_images() {
        let big = fake_png(4000, 2000);
        let p = decode_and_shrink(&big).unwrap();
        assert!(p.width <= MAX_DIMENSION && p.height <= MAX_DIMENSION);
    }

    #[test]
    fn keeps_small_images_untouched_in_size() {
        let small = fake_png(600, 400);
        let p = decode_and_shrink(&small).unwrap();
        assert_eq!((p.width, p.height), (600, 400));
    }

    #[test]
    fn rejects_junk() {
        let junk = vec![0u8; 10];
        assert!(matches!(
            decode_and_shrink(&junk),
            Err(UploadError::NotAnImage)
        ));
    }
}
