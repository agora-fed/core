//! # Polls on Notes — persistence, AP mapping, voting (migration 0408).
//!
//! When a Note carries a poll the AP object type flips from `Note` to
//! `Question` (Mastodon parity) and the options ride as `oneOf` (single
//! choice) or `anyOf` (multi-select) arrays. This module hides the SQL
//! behind that logic and exposes the DTOs the feed handler bundles.
//!
//! Runtime-unchecked `sqlx::query*` throughout — see `federation_feed.rs`
//! for the offline-cache policy.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

/// Max number of options per poll (Mastodon parity). Enforced at the API
/// layer; the DB schema has no hard upper bound.
pub const MAX_OPTIONS: usize = 8;
/// Min votable window (5 minutes). Rejects `expires_in_minutes < 5`.
pub const MIN_EXPIRES_MINUTES: i64 = 5;
/// Max votable window (7 days). Longer polls are lopped at insert.
pub const MAX_EXPIRES_MINUTES: i64 = 7 * 24 * 60;

/// Input shape for `create_from_input` — mirrors what the composer sends on
/// `POST /me/notes` when `poll` is present.
#[derive(Debug, Clone, Deserialize)]
pub struct PollInput {
    pub options: Vec<String>,
    #[serde(default)]
    pub multiple: bool,
    /// Author-picked expiration window (5–10080 minutes = 5min–7d).
    pub expires_in_minutes: i64,
}

/// One option in a poll DTO.
#[derive(Debug, Clone, Serialize)]
pub struct PollOptionDto {
    pub id: Uuid,
    pub sort_order: i32,
    pub text: String,
    pub vote_count: i32,
}

/// Poll DTO returned alongside a `FeedItemDto`.
#[derive(Debug, Clone, Serialize)]
pub struct PollDto {
    pub id: Uuid,
    pub multiple: bool,
    pub expires_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<DateTime<Utc>>,
    pub options: Vec<PollOptionDto>,
    /// Sum of `vote_count` — cheaper for the front to render "N votes" than a
    /// per-option reduce.
    pub total_votes: i32,
    /// Which option ids the viewer already voted for. Empty if not-voted or
    /// anonymous. Frontend uses this to lock the ballot and highlight bars.
    pub voted_option_ids: Vec<Uuid>,
}

#[derive(Debug, sqlx::FromRow)]
#[allow(dead_code)]
struct OptionRow {
    id: Uuid,
    sort_order: i32,
    text: String,
    vote_count: i32,
}

/// Validate + persist a poll for a Note that was just created. Called from
/// `post_my_note` right after the outbox INSERT. Returns the DB poll id.
pub async fn create_from_input(
    db: &PgPool,
    object_uri: &str,
    input: &PollInput,
) -> Result<Uuid, PollError> {
    let opts: Vec<String> = input
        .options
        .iter()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(200).collect())
        .collect();
    if opts.len() < 2 {
        return Err(PollError::TooFewOptions);
    }
    if opts.len() > MAX_OPTIONS {
        return Err(PollError::TooManyOptions);
    }
    if input.expires_in_minutes < MIN_EXPIRES_MINUTES {
        return Err(PollError::WindowTooShort);
    }
    let minutes = input.expires_in_minutes.min(MAX_EXPIRES_MINUTES);
    let now = Utc::now();
    let expires_at = now + chrono::Duration::minutes(minutes);
    let poll_id = Uuid::now_v7();
    sqlx::query(
        r"INSERT INTO note_poll (id, object_uri, multiple, expires_at, closed_at, created_at)
          VALUES ($1, $2, $3, $4, NULL, $5)",
    )
    .bind(poll_id)
    .bind(object_uri)
    .bind(input.multiple)
    .bind(expires_at)
    .bind(now)
    .execute(db)
    .await
    .map_err(PollError::Db)?;
    for (i, text) in opts.iter().enumerate() {
        sqlx::query(
            r"INSERT INTO note_poll_option (id, poll_id, sort_order, text, vote_count)
              VALUES ($1, $2, $3, $4, 0)",
        )
        .bind(Uuid::now_v7())
        .bind(poll_id)
        .bind(i as i32)
        .bind(text)
        .execute(db)
        .await
        .map_err(PollError::Db)?;
    }
    Ok(poll_id)
}

/// Rewrite the outbox payload of a Note that just became a Question. Flips
/// `object.type` to "Question", inserts oneOf/anyOf + endTime, and preserves
/// the rest of the Note fields. Idempotent — running twice writes the same
/// object graph.
pub async fn update_outbox_payload_with_question(
    db: &PgPool,
    activity_id: &str,
    object_uri: &str,
) -> Result<(), sqlx::Error> {
    let Some(question) = ap_question_fields(db, object_uri).await? else {
        return Ok(());
    };
    // Fetch the current payload, mutate its object, write back.
    let Some((mut payload,)): Option<(serde_json::Value,)> =
        sqlx::query_as(r"SELECT payload FROM federation_outbox_entry WHERE activity_id = $1")
            .bind(activity_id)
            .fetch_optional(db)
            .await?
    else {
        return Ok(());
    };
    if let Some(object) = payload.get_mut("object").and_then(|o| o.as_object_mut()) {
        object.insert("type".into(), serde_json::Value::String("Question".into()));
        if let Some(q) = question.as_object() {
            for (k, v) in q {
                object.insert(k.clone(), v.clone());
            }
        }
    }
    sqlx::query(r"UPDATE federation_outbox_entry SET payload = $2 WHERE activity_id = $1")
        .bind(activity_id)
        .bind(&payload)
        .execute(db)
        .await?;
    Ok(())
}

/// Build the AP `Question` sub-object for a Note's payload. `options` are
/// looked up fresh from the DB so the emitted vocab matches what the vote
/// endpoint will accept.
pub async fn ap_question_fields(
    db: &PgPool,
    object_uri: &str,
) -> Result<Option<serde_json::Value>, sqlx::Error> {
    let Some(row) =
        sqlx::query_as::<_, (bool, DateTime<Utc>, Option<DateTime<Utc>>)>(
            r"SELECT multiple, expires_at, closed_at FROM note_poll WHERE object_uri = $1",
        )
        .bind(object_uri)
        .fetch_optional(db)
        .await?
    else {
        return Ok(None);
    };
    let (multiple, expires_at, closed_at) = row;
    let opts = list_options(db, object_uri).await?;
    // Each option becomes a Note item under oneOf/anyOf per Mastodon convention.
    let items: Vec<serde_json::Value> = opts
        .iter()
        .map(|o| {
            serde_json::json!({
                "type": "Note",
                "name": o.text,
                "replies": {
                    "type": "Collection",
                    "totalItems": o.vote_count,
                },
            })
        })
        .collect();
    let mut out = serde_json::json!({
        "endTime": expires_at.to_rfc3339(),
    });
    let key = if multiple { "anyOf" } else { "oneOf" };
    out[key] = serde_json::Value::Array(items);
    if let Some(cl) = closed_at {
        out["closed"] = serde_json::Value::String(cl.to_rfc3339());
    }
    Ok(Some(out))
}

async fn list_options(
    db: &PgPool,
    object_uri: &str,
) -> Result<Vec<OptionRow>, sqlx::Error> {
    sqlx::query_as::<_, OptionRow>(
        r"SELECT o.id, o.sort_order, o.text, o.vote_count
            FROM note_poll_option o
            JOIN note_poll p ON p.id = o.poll_id
           WHERE p.object_uri = $1
           ORDER BY o.sort_order",
    )
    .bind(object_uri)
    .fetch_all(db)
    .await
}

/// Batch-fetch poll DTOs for a page of Notes. Empty when none of the URIs
/// carry a poll; consumers merge into `FeedItemDto.poll` before returning.
pub async fn list_for_notes(
    db: &PgPool,
    object_uris: &[String],
    viewer_actor_url: Option<&str>,
) -> Result<HashMap<String, PollDto>, sqlx::Error> {
    if object_uris.is_empty() {
        return Ok(HashMap::new());
    }
    // Poll headers.
    let polls: Vec<(String, Uuid, bool, DateTime<Utc>, Option<DateTime<Utc>>)> =
        sqlx::query_as::<_, (String, Uuid, bool, DateTime<Utc>, Option<DateTime<Utc>>)>(
            r"SELECT object_uri, id, multiple, expires_at, closed_at
                FROM note_poll
               WHERE object_uri = ANY($1::text[])",
        )
        .bind(object_uris)
        .fetch_all(db)
        .await?;
    if polls.is_empty() {
        return Ok(HashMap::new());
    }
    let poll_ids: Vec<Uuid> = polls.iter().map(|p| p.1).collect();
    // All options for those polls.
    let opts: Vec<(Uuid, Uuid, i32, String, i32)> =
        sqlx::query_as::<_, (Uuid, Uuid, i32, String, i32)>(
            r"SELECT poll_id, id, sort_order, text, vote_count
                FROM note_poll_option
               WHERE poll_id = ANY($1::uuid[])
               ORDER BY poll_id, sort_order",
        )
        .bind(&poll_ids)
        .fetch_all(db)
        .await?;
    // The viewer's votes, if any.
    let voted: Vec<(Uuid, Vec<String>)> = if let Some(actor) = viewer_actor_url {
        sqlx::query_as::<_, (Uuid, Vec<String>)>(
            r"SELECT poll_id, option_ids
                FROM note_poll_vote
               WHERE poll_id = ANY($1::uuid[]) AND actor_url = $2",
        )
        .bind(&poll_ids)
        .bind(actor)
        .fetch_all(db)
        .await?
    } else {
        Vec::new()
    };
    let voted_map: HashMap<Uuid, Vec<Uuid>> = voted
        .into_iter()
        .map(|(pid, ids)| {
            (
                pid,
                ids.into_iter()
                    .filter_map(|s| Uuid::parse_str(&s).ok())
                    .collect(),
            )
        })
        .collect();
    // Bucket options by poll_id.
    let mut opts_by_poll: HashMap<Uuid, Vec<PollOptionDto>> = HashMap::new();
    for (pid, oid, order, text, count) in opts {
        opts_by_poll.entry(pid).or_default().push(PollOptionDto {
            id: oid,
            sort_order: order,
            text,
            vote_count: count,
        });
    }
    // Assemble DTOs keyed by object_uri.
    let mut out: HashMap<String, PollDto> = HashMap::new();
    for (uri, poll_id, multiple, expires_at, closed_at) in polls {
        let options = opts_by_poll.remove(&poll_id).unwrap_or_default();
        let total_votes: i32 = options.iter().map(|o| o.vote_count).sum();
        let voted_option_ids = voted_map.get(&poll_id).cloned().unwrap_or_default();
        out.insert(
            uri,
            PollDto {
                id: poll_id,
                multiple,
                expires_at,
                closed_at,
                options,
                total_votes,
                voted_option_ids,
            },
        );
    }
    Ok(out)
}

/// Register a vote from `actor_url`. Returns the refreshed DTO. Idempotent
/// on (poll_id, actor_url) — a repeat vote either 409s (single-choice) or
/// no-ops (same ballot on multi-select).
pub async fn cast_vote(
    db: &PgPool,
    object_uri: &str,
    actor_url: &str,
    option_ids: &[Uuid],
    media_base_url_for_dto: &str,
) -> Result<PollDto, PollError> {
    let _ = media_base_url_for_dto; // reserved for later media-related read paths
    // Defense-in-depth: apenas cidadãos DESTA instância podem votar. O único
    // caller hoje (`POST /me/notes/vote`) constrói o voter_url do CallerId
    // autenticado, mas guardamos o invariante aqui pra qualquer futuro
    // hook de inbox federado que tente enfiar voto remoto (ex.: Create(Note)
    // com `name`+inReplyTo, convenção Mastodon). Regra: o texto do site é
    // "enquete propaga no fediverso mas voto é restrito a democracia.social.br".
    let po = std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
    let expected_prefix = format!("{}/actors/", po.trim_end_matches('/'));
    if !actor_url.starts_with(&expected_prefix) {
        tracing::warn!(actor_url, "cast_vote: rejeitado — voter_url não é local");
        return Err(PollError::RemoteVoterForbidden);
    }
    // Fetch poll header.
    let Some((poll_id, multiple, expires_at, closed_at)): Option<(
        Uuid,
        bool,
        DateTime<Utc>,
        Option<DateTime<Utc>>,
    )> = sqlx::query_as(
        r"SELECT id, multiple, expires_at, closed_at FROM note_poll WHERE object_uri = $1",
    )
    .bind(object_uri)
    .fetch_optional(db)
    .await
    .map_err(PollError::Db)?
    else {
        return Err(PollError::NotFound);
    };
    if closed_at.is_some() || expires_at < Utc::now() {
        return Err(PollError::Closed);
    }
    if option_ids.is_empty() {
        return Err(PollError::EmptyBallot);
    }
    if !multiple && option_ids.len() > 1 {
        return Err(PollError::TooManyForSingle);
    }
    // Validate that every option_id belongs to this poll.
    let valid_ids: Vec<Uuid> = sqlx::query_scalar::<_, Uuid>(
        r"SELECT id FROM note_poll_option WHERE poll_id = $1 AND id = ANY($2::uuid[])",
    )
    .bind(poll_id)
    .bind(option_ids)
    .fetch_all(db)
    .await
    .map_err(PollError::Db)?;
    if valid_ids.len() != option_ids.len() {
        return Err(PollError::UnknownOption);
    }
    // Serialize ballot as text[] of uuid.
    let ballot_text: Vec<String> = option_ids.iter().map(|u| u.to_string()).collect();
    // INSERT vote; on conflict abort so the client learns their previous vote is authoritative.
    let inserted = sqlx::query(
        r"INSERT INTO note_poll_vote
             (id, poll_id, actor_url, option_ids, created_at)
          VALUES ($1, $2, $3, $4, $5)
          ON CONFLICT (poll_id, actor_url) DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(poll_id)
    .bind(actor_url)
    .bind(&ballot_text)
    .bind(Utc::now())
    .execute(db)
    .await
    .map_err(PollError::Db)?;
    if inserted.rows_affected() == 0 {
        return Err(PollError::AlreadyVoted);
    }
    // Bump vote_count on each chosen option.
    for oid in option_ids {
        let _ = sqlx::query(
            r"UPDATE note_poll_option SET vote_count = vote_count + 1 WHERE id = $1",
        )
        .bind(oid)
        .execute(db)
        .await
        .map_err(PollError::Db)?;
    }
    // Return the refreshed DTO.
    let uris = vec![object_uri.to_owned()];
    let map = list_for_notes(db, &uris, Some(actor_url))
        .await
        .map_err(PollError::Db)?;
    Ok(map
        .into_values()
        .next()
        .ok_or(PollError::NotFound)?)
}

#[derive(Debug)]
pub enum PollError {
    TooFewOptions,
    TooManyOptions,
    WindowTooShort,
    NotFound,
    Closed,
    EmptyBallot,
    TooManyForSingle,
    UnknownOption,
    AlreadyVoted,
    /// Voto vindo de instância federada — a política é: enquete propaga
    /// pelo fediverso, mas a apuração vale apenas para cidadãos com conta
    /// nesta instância.
    RemoteVoterForbidden,
    Db(sqlx::Error),
}

impl PollError {
    #[must_use]
    pub fn user_message(&self) -> String {
        match self {
            Self::TooFewOptions => "envie pelo menos 2 opções".into(),
            Self::TooManyOptions => {
                format!("no máximo {MAX_OPTIONS} opções")
            }
            Self::WindowTooShort => format!(
                "a enquete precisa durar pelo menos {MIN_EXPIRES_MINUTES} minutos"
            ),
            Self::NotFound => "enquete não encontrada".into(),
            Self::Closed => "esta enquete já foi encerrada".into(),
            Self::EmptyBallot => "escolha ao menos uma opção".into(),
            Self::TooManyForSingle => "esta enquete permite apenas 1 opção".into(),
            Self::UnknownOption => "opção inválida".into(),
            Self::AlreadyVoted => "você já votou nesta enquete".into(),
            Self::RemoteVoterForbidden =>
                "enquetes podem circular no fediverso, mas a apuração só conta \
                 votos de cidadãos com conta em democracia.social.br"
                    .into(),
            Self::Db(_) => "erro ao registrar o voto".into(),
        }
    }
}
