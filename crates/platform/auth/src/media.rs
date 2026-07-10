//! # Profile media uploads — avatar + cover (ADR-0010 W1.2).
//!
//! Multipart upload, server-side validation + resize, persistence to the [`Storage`] port
//! (S3-compatible). The caller never controls the storage key — the service picks
//! `avatars/<citizen_id>/<random>.png` so an upload always overwrites the citizen's avatar in
//! their own scoped sub-prefix, and the random suffix changes per upload so CDN caches stay
//! valid forever (the prior object is then deleted out of band; see the `update_*_for_citizen`
//! queries that swap `*_object_key`).
//!
//! ## Validation
//! - **MIME guess from bytes**, not the multipart's `Content-Type` (clients lie). The decoder
//!   refuses anything that isn't a valid PNG/JPEG/WebP.
//! - **Size cap** at 5 MiB on the raw upload (rejected before decode).
//! - **Resize** to a hard maximum (avatar 512x512, cover 1500x500) preserving aspect ratio so
//!   we never pay storage/bandwidth for a phone photo dumped raw.
//! - **Re-encode as PNG** so the storage object is uniform — clients always know what to expect
//!   and we drop any embedded EXIF (privacy: location, device id, etc.).

use std::io::Cursor;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use dsoc_core::ids::CitizenId;
use dsoc_core::{Error, Result, Storage};
use image::imageops::FilterType;
use image::ImageFormat;
use rand::RngCore;

/// Max accepted raw upload size before decode. Keeps a malicious 10 GB upload from filling RAM.
const MAX_UPLOAD_BYTES: usize = 5 * 1024 * 1024;
/// Avatar bounding box (the result is the largest contained square, kept square via crop).
pub const AVATAR_SIZE: u32 = 512;
/// Cover bounding box. Landscape — the front-end displays it as a wide banner.
pub const COVER_WIDTH: u32 = 1500;
pub const COVER_HEIGHT: u32 = 500;

/// What kind of profile picture we're uploading. Drives the resize target and the storage prefix.
#[derive(Debug, Clone, Copy)]
pub enum MediaKind {
    Avatar,
    Cover,
}

impl MediaKind {
    fn prefix(self) -> &'static str {
        match self {
            Self::Avatar => "avatars",
            Self::Cover => "covers",
        }
    }
}

/// Storage key picked by the service for a given citizen + media kind + this specific upload.
/// The random suffix means previous URLs stay cache-valid until the prior object is deleted.
pub fn object_key(citizen: CitizenId, kind: MediaKind) -> String {
    let mut buf = [0u8; 9]; // 12-char base64 — plenty of entropy for collision-free overwrite.
    rand::rngs::OsRng.fill_bytes(&mut buf);
    let suffix = URL_SAFE_NO_PAD.encode(buf);
    let id = citizen.as_uuid().simple();
    format!("{}/{}/{}.png", kind.prefix(), id, suffix)
}

/// Validate, resize, and upload an image. Returns the object key the gateway must persist on the
/// citizen row so subsequent `GET /me` calls resolve to it via `MEDIA_BASE_URL`.
///
/// # Errors
/// [`Error::Validation`] for too-large / not-an-image / decode-failed inputs;
/// [`Error::Storage`] for an S3/MinIO failure; [`Error::Dependency`] when storage is unwired.
pub async fn upload_image(
    storage: Option<&std::sync::Arc<dyn Storage>>,
    citizen: CitizenId,
    kind: MediaKind,
    raw: Vec<u8>,
) -> Result<String> {
    let storage = storage.ok_or_else(|| Error::Dependency {
        dependency: "storage",
        source: "not configured".into(),
    })?;
    if raw.len() > MAX_UPLOAD_BYTES {
        return Err(Error::Validation(format!(
            "imagem maior que {} MB",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        )));
    }
    // Decode-from-bytes; this enforces "it must be a real, supported image" regardless of what
    // Content-Type the client claims. CPU-bound: run on the blocking pool so we don't stall the
    // tokio reactor on a big JPEG.
    let processed = tokio::task::spawn_blocking(move || resize_and_encode(&raw, kind))
        .await
        .map_err(|join| Error::Storage(Box::new(join)))??;

    let key = object_key(citizen, kind);
    storage.put(&key, "image/png", processed).await?;
    Ok(key)
}

/// CPU-bound: decode → resize/crop to the target → re-encode as PNG (strips EXIF).
fn resize_and_encode(raw: &[u8], kind: MediaKind) -> Result<Vec<u8>> {
    let format = image::guess_format(raw)
        .map_err(|_| Error::Validation("formato de imagem não suportado".to_owned()))?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP
    ) {
        return Err(Error::Validation(
            "use uma imagem PNG, JPEG ou WebP".to_owned(),
        ));
    }
    let img = image::load_from_memory_with_format(raw, format)
        .map_err(|e| Error::Validation(format!("imagem inválida: {e}")))?;

    let out = match kind {
        // Avatar: scale-to-fill the smaller dimension then center-crop to a square. Avoids the
        // letterbox/pillarbox a plain `resize` would produce.
        MediaKind::Avatar => img.resize_to_fill(AVATAR_SIZE, AVATAR_SIZE, FilterType::Lanczos3),
        // Cover: same scale-to-fill into the landscape banner box.
        MediaKind::Cover => img.resize_to_fill(COVER_WIDTH, COVER_HEIGHT, FilterType::Lanczos3),
    };

    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    out.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
        .map_err(|e| Error::Storage(Box::new(e)))?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    fn fake_png(w: u32, h: u32) -> Vec<u8> {
        let buf: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_fn(w, h, |_, _| Rgba([10, 200, 80, 255]));
        let mut out = Vec::new();
        buf.write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn resize_avatar_produces_square_png() {
        let input = fake_png(1024, 600);
        let out = resize_and_encode(&input, MediaKind::Avatar).unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!(decoded.width(), AVATAR_SIZE);
        assert_eq!(decoded.height(), AVATAR_SIZE);
    }

    #[test]
    fn resize_cover_produces_banner_png() {
        let input = fake_png(3000, 1000);
        let out = resize_and_encode(&input, MediaKind::Cover).unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!(decoded.width(), COVER_WIDTH);
        assert_eq!(decoded.height(), COVER_HEIGHT);
    }

    #[test]
    fn unsupported_format_is_validation_error() {
        let garbage = vec![1u8, 2, 3, 4, 5];
        let r = resize_and_encode(&garbage, MediaKind::Avatar);
        assert!(matches!(r, Err(Error::Validation(_))));
    }

    #[test]
    fn object_key_scopes_to_citizen_and_kind() {
        let id = CitizenId::new();
        let k = object_key(id, MediaKind::Avatar);
        assert!(k.starts_with("avatars/"));
        assert!(k.ends_with(".png"));
        let k2 = object_key(id, MediaKind::Cover);
        assert!(k2.starts_with("covers/"));
    }
}
