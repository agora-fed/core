//! # Citizen profile — the social-substrate layer atop credential auth (ADR-0010).
//!
//! This module implements the `GET /me` and `PATCH /me` endpoints' service layer: read and update
//! the authenticated citizen's profile (display name, bio, handle, privacy toggle). Avatar / cover
//! upload routes go through the storage crate (W1.2) and only land their object keys here.
//!
//! Profile mutations NEVER touch credentials — the CPF and password hash live in
//! `auth_credential` and remain inaccessible from this surface, so a profile patch cannot
//! escalate identity. The opposite is also true: a credential operation never modifies the
//! social face of the citizen. That separation is what lets us federate (W2) without leaking
//! sovereign credentials.

use std::sync::Arc;

use dsoc_api_contract::{ProfileDto, SessionInfoDto};
use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Clock, Error, Result, Storage};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain;
use crate::media::{self, MediaKind};
use crate::queries::{self, ProfileRow};

/// Max accepted bio length from the API (DB caps at 1000 as the last-line guard).
const BIO_MAX: usize = 500;
/// Max accepted display-name length.
const DISPLAY_NAME_MAX: usize = 80;
/// Min/max handle length (matches the DB CHECK and Mastodon's de-facto range).
const HANDLE_MIN: usize = 3;
const HANDLE_MAX: usize = 32;

/// The profile service. Holds the database handle and the avatar/cover URL base used to project
/// stored object keys into publicly resolvable URLs (the storage crate owns the actual bytes;
/// this only renders the URL prefix).
#[derive(Clone)]
pub struct ProfileService {
    db: Db,
    media_base_url: String,
    storage: Option<Arc<dyn Storage>>,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ProfileService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProfileService")
            .field("media_base_url", &self.media_base_url)
            .field("storage_configured", &self.storage.is_some())
            .finish()
    }
}

impl ProfileService {
    /// Build the service. `media_base_url` is the public origin under which avatar/cover object
    /// keys resolve, e.g. `https://democracia.social.br/media`. Trailing slash is normalized.
    #[must_use]
    pub fn new(
        db: Db,
        media_base_url: impl Into<String>,
        storage: Option<Arc<dyn Storage>>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let raw = media_base_url.into();
        let trimmed = raw.trim_end_matches('/').to_owned();
        Self {
            db,
            media_base_url: trimmed,
            storage,
            clock,
        }
    }

    /// Build from `AppState`. The media base comes from `MEDIA_BASE_URL` (default
    /// `/media` — relative same-origin path, so Caddy serves it). Storage is whatever the gateway
    /// wired (S3/MinIO when configured, `None` otherwise — handlers map missing storage to a
    /// dependency error).
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        let base = std::env::var("MEDIA_BASE_URL").unwrap_or_else(|_| "/media".to_owned());
        Self::new(
            state.db.clone(),
            base,
            state.storage.clone(),
            state.clock.clone(),
        )
    }

    /// List the caller's live sessions. `current_session_id`, when present, is the session id
    /// the inbound request carried (i.e. "this device"); the DTO flag lets the UI mark it and
    /// disable the revoke button for it (revoking your current cookie is just logout).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_sessions(
        &self,
        caller: CitizenId,
        current_session_id: Option<Uuid>,
    ) -> Result<Vec<SessionInfoDto>> {
        let now = self.clock.now();
        let rows = queries::list_sessions_for_citizen(&self.db, caller.as_uuid(), now)
            .await
            .map_err(map_sqlx)?;
        Ok(rows
            .into_iter()
            .map(|r| SessionInfoDto {
                id: r.id,
                issued_at: r.issued_at,
                expires_at: r.expires_at,
                current: current_session_id == Some(r.id),
            })
            .collect())
    }

    /// Revoke one of the caller's sessions. The DB query is gated by `citizen_id` so a logged-in
    /// user cannot guess another user's session id and revoke it. Refusing to revoke the
    /// CURRENT session keeps the surface non-confusing (the user should call `POST /auth/logout`
    /// for that — different semantics: logout clears the cookie too).
    ///
    /// # Errors
    /// [`Error::Validation`] if `session_id == current_session_id` (call logout instead);
    /// [`Error::NotFound`] if the session is not the caller's (the surface conflates "not
    /// yours" with "doesn't exist" to avoid leaking which it was);
    /// [`Error::Storage`] on persistence failure.
    pub async fn revoke_session(
        &self,
        caller: CitizenId,
        session_id: Uuid,
        current_session_id: Option<Uuid>,
    ) -> Result<()> {
        if current_session_id == Some(session_id) {
            return Err(Error::Validation(
                "use POST /auth/logout para encerrar este dispositivo".to_owned(),
            ));
        }
        let affected = queries::delete_session_for_citizen(&self.db, session_id, caller.as_uuid())
            .await
            .map_err(map_sqlx)?;
        if affected == 0 {
            return Err(Error::NotFound("session not found".to_owned()));
        }
        Ok(())
    }

    /// Upload + persist the caller's avatar. Returns the refreshed profile.
    ///
    /// # Errors
    /// [`Error::Validation`] for an invalid image; [`Error::Dependency`] if storage is not
    /// wired; [`Error::Storage`] on a persistence failure; [`Error::NotFound`] if the citizen
    /// row vanished out from under the upload.
    pub async fn set_avatar(
        &self,
        caller: CitizenId,
        expected_org: OrgId,
        raw: Vec<u8>,
    ) -> Result<ProfileDto> {
        self.set_media(caller, expected_org, MediaKind::Avatar, raw)
            .await
    }

    /// Upload + persist the caller's cover image. Same shape as [`Self::set_avatar`].
    ///
    /// # Errors
    /// See [`Self::set_avatar`].
    pub async fn set_cover(
        &self,
        caller: CitizenId,
        expected_org: OrgId,
        raw: Vec<u8>,
    ) -> Result<ProfileDto> {
        self.set_media(caller, expected_org, MediaKind::Cover, raw)
            .await
    }

    /// Read the citizen's PRIVATE federation PEM. Internal-use only: this is a credential and
    /// MUST NOT cross any HTTP response. Used by the outbound delivery path to sign Accept etc.
    ///
    /// # Errors
    /// [`Error::NotFound`] when no keypair exists yet (caller should `ensure_actor_public_key`
    /// first to materialize one); [`Error::Storage`] on persistence failure.
    pub async fn read_actor_private_key(&self, citizen: CitizenId) -> Result<String> {
        queries::find_actor_private_key(&self.db, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("actor key not found".to_owned()))
    }

    /// Mark an inbound activity id as seen (insert-before-act idempotency log). Returns `true`
    /// for a fresh delivery, `false` for a duplicate.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn mark_inbox_seen(&self, activity_id: &str, citizen: CitizenId) -> Result<bool> {
        let now = self.clock.now();
        queries::mark_inbox_activity_seen(&self.db, activity_id, citizen.as_uuid(), now)
            .await
            .map_err(map_sqlx)
    }

    /// Persist a (validated) inbound Follow activity from a remote actor.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn record_inbound_follow(
        &self,
        citizen: CitizenId,
        remote_actor_url: &str,
        remote_inbox_url: &str,
        follow_activity_id: &str,
    ) -> Result<()> {
        let now = self.clock.now();
        queries::insert_inbound_follow(
            &self.db,
            uuid::Uuid::now_v7(),
            citizen.as_uuid(),
            remote_actor_url,
            remote_inbox_url,
            follow_activity_id,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    /// Mark a previously-recorded inbound follow as ACK'd (Accept delivered).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn accept_inbound_follow(
        &self,
        citizen: CitizenId,
        remote_actor_url: &str,
    ) -> Result<()> {
        let now = self.clock.now();
        queries::mark_inbound_follow_accepted(&self.db, citizen.as_uuid(), remote_actor_url, now)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    /// Claim a batch of pending deliveries for the worker to ship. Each task carries everything
    /// needed (recipient inbox, signing material, body) so the worker doesn't need to JOIN.
    /// The DB-level update bumps `attempts` and stages a short pessimistic next-attempt window
    /// inside the same statement, so a worker crash before success/failure leaves the row
    /// re-claimable after ~5 minutes (not lost, not immediately re-tried).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn claim_deliveries(&self, limit: u32) -> Result<Vec<DeliveryTask>> {
        let now = self.clock.now();
        let claims = queries::claim_pending_deliveries(&self.db, now, i64::from(limit))
            .await
            .map_err(map_sqlx)?;
        Ok(claims.into_iter().map(DeliveryTask::from).collect())
    }
    // (signature already passes `&self.db`)

    /// Mark a delivery as successfully shipped (worker got a 2xx).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn delivery_succeeded(&self, delivery_id: uuid::Uuid) -> Result<()> {
        let now = self.clock.now();
        queries::mark_delivery_done(&self.db, delivery_id, now)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    /// Mark a delivery as failed; schedules the next attempt with exponential backoff.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn delivery_failed(
        &self,
        delivery_id: uuid::Uuid,
        attempts: i32,
        error: &str,
    ) -> Result<()> {
        let now = self.clock.now();
        queries::mark_delivery_failed(&self.db, delivery_id, attempts, error, now)
            .await
            .map_err(map_sqlx)?;
        Ok(())
    }

    /// Post a public Note authored by the citizen. Wraps the content in a `Create(Note)`
    /// activity addressed to `as:Public` + the citizen's followers collection (the Mastodon
    /// Publish a public Note. Wraps `content` in a `Create(Note)` (with mentions/hashtags in the
    /// AP `tag[]` array), persists into the outbox, indexes extracted #hashtags and @mentions,
    /// and fans out one delivery row per ACK'd inbound follower. The delivery worker drains the
    /// queue asynchronously, so this call returns fast.
    ///
    /// 0.18.0 additions: `in_reply_to_uri`, `sensitive`, `spoiler_text` (CW). All optional.
    /// `public_origin` is needed to build local mention/hashtag hrefs. `resolved_mentions`
    /// carries the WebFinger-resolved actors for remote @mentions (gateway does the network
    /// round-trips): their real actor ids go into the `tag[]` hrefs and `cc`, and their
    /// inboxes get a direct delivery row — without a resolved entry a mention falls back to
    /// the guessed URL and is delivered only to followers (pre-resolution behavior). Returns
    /// `(activity_id_url, fanout_count)`.
    ///
    /// # Errors
    /// [`Error::Validation`] for empty/oversized content; [`Error::Storage`] on persistence.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_public_note(
        &self,
        citizen: CitizenId,
        actor_url: &str,
        public_origin: &str,
        content: &str,
        resolved_mentions: &[dsoc_federation::ResolvedMention],
        in_reply_to_uri: Option<&str>,
        sensitive: bool,
        spoiler_text: Option<&str>,
    ) -> Result<(String, u64)> {
        let trimmed = content.trim().to_owned();
        if trimmed.is_empty() {
            return Err(Error::Validation(
                "digite alguma coisa antes de publicar".to_owned(),
            ));
        }
        if trimmed.chars().count() > 3_000 {
            return Err(Error::Validation(
                "o texto está muito longo (máx 3000 caracteres)".to_owned(),
            ));
        }
        // Trim + cap CW at 500 chars (matches migration 0404 CHECK on the local side).
        let spoiler = spoiler_text
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(500).collect::<String>());
        // Extract mentions and hashtags from the raw content BEFORE we wrap it — the AP tag[]
        // array carries them for federated instances that render tag links, and note_hashtag /
        // note_mention index them locally for the future tag timeline and mention notifications.
        let mentions = dsoc_federation::extract_mentions(&trimmed);
        let hashtags = dsoc_federation::extract_hashtags(&trimmed);
        // Authoritative actor URL when the gateway resolved the handle via WebFinger;
        // conventional guess otherwise (unreachable instance, local mention).
        let mention_href = |m: &dsoc_federation::Mention| -> String {
            resolved_mentions
                .iter()
                .find(|r| r.handle == m.handle)
                .map(|r| r.actor_url.clone())
                .unwrap_or_else(|| m.best_actor_url(public_origin))
        };
        let mut mention_hrefs: Vec<String> = Vec::new();
        let mut tag_json: Vec<serde_json::Value> = Vec::new();
        for m in &mentions {
            let href = mention_href(m);
            tag_json.push(serde_json::json!({
                "type": "Mention",
                "href": href,
                "name": format!("@{}", m.handle),
            }));
            if !mention_hrefs.contains(&href) {
                mention_hrefs.push(href);
            }
        }
        for h in &hashtags {
            tag_json.push(serde_json::json!({
                "type": "Hashtag",
                "href": format!("{}/tag/{}", public_origin.trim_end_matches('/'), h.normalized),
                "name": format!("#{}", h.original),
            }));
        }

        let now = self.clock.now();
        let entry_id = uuid::Uuid::now_v7();
        let activity_id = format!("{actor_url}/activities/note-{entry_id}");
        let object_id = format!("{actor_url}/objects/{entry_id}");
        // `cc` = followers + every mentioned actor. Mastodon only notifies a mention when
        // the actor is addressed AND present in `tag[]` — the tag alone is not enough.
        let mut cc: Vec<String> = vec![format!("{actor_url}/followers")];
        cc.extend(mention_hrefs.iter().cloned());
        // Build the Note object. Optional AP fields (`inReplyTo`, `sensitive`, `summary`,
        // `tag`) are only added when they carry content — Mastodon skips them likewise.
        let mut note_object = serde_json::json!({
            "id": object_id,
            "type": "Note",
            "attributedTo": actor_url,
            "published": now.to_rfc3339(),
            "content": trimmed,
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": cc.clone(),
        });
        if let Some(reply) = in_reply_to_uri.filter(|s| !s.is_empty()) {
            note_object["inReplyTo"] = serde_json::Value::String(reply.to_owned());
        }
        if sensitive {
            note_object["sensitive"] = serde_json::Value::Bool(true);
        }
        if let Some(cw) = spoiler.as_deref() {
            note_object["summary"] = serde_json::Value::String(cw.to_owned());
        }
        if !tag_json.is_empty() {
            note_object["tag"] = serde_json::Value::Array(tag_json);
        }
        let payload = serde_json::json!({
            "@context": "https://www.w3.org/ns/activitystreams",
            "id": activity_id,
            "type": "Create",
            "actor": actor_url,
            "published": now.to_rfc3339(),
            "to": ["https://www.w3.org/ns/activitystreams#Public"],
            "cc": cc,
            "object": note_object,
        });
        // INSERT outbox + fanout — two statements; if the fanout fails we leave the outbox
        // entry behind (visible via the Outbox endpoint, just not delivered) so a manual
        // re-fanout is possible.
        queries::insert_outbox_entry(
            &self.db,
            entry_id,
            citizen.as_uuid(),
            &activity_id,
            "Create",
            "public",
            &payload,
            now,
            in_reply_to_uri.filter(|s| !s.is_empty()),
            sensitive,
            spoiler.as_deref(),
        )
        .await
        .map_err(map_sqlx)?;
        // Index #hashtags and @mentions. Best-effort; a failure here should not block delivery
        // (the Note is already visible in the outbox and en route to followers).
        for m in &mentions {
            let _ = queries::insert_note_mention(
                &self.db,
                &object_id,
                &mention_href(m),
                &m.handle,
                now,
            )
            .await;
        }
        for h in &hashtags {
            let _ =
                queries::insert_note_hashtag(&self.db, &object_id, &h.normalized, &h.original, now)
                    .await;
        }
        let fanout =
            queries::fanout_delivery_to_followers(&self.db, entry_id, citizen.as_uuid(), now)
                .await
                .map_err(map_sqlx)?;
        // Direct delivery to each mentioned actor's inbox — without this, a mention only
        // reaches instances that already follow the author and Mastodon never notifies.
        // The UNIQUE on (entry, inbox) dedups against the follower fanout above.
        let mut mention_inboxes: Vec<String> = Vec::new();
        for r in resolved_mentions {
            if let Some(inbox) = r.inbox_url.as_deref() {
                if !mention_inboxes.iter().any(|i| i == inbox) {
                    mention_inboxes.push(inbox.to_owned());
                }
            }
        }
        let mention_fanout =
            queries::fanout_delivery_to_inboxes(&self.db, entry_id, &mention_inboxes, now)
                .await
                .map_err(map_sqlx)?;
        Ok((activity_id, fanout + mention_fanout))
    }

    /// List the citizen's public outbox entries, newest first. Drives the `/actors/{handle}/
    /// outbox` OrderedCollection.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_public_outbox(
        &self,
        citizen: CitizenId,
        limit: u32,
    ) -> Result<(Vec<serde_json::Value>, u64)> {
        let items = queries::list_public_outbox(&self.db, citizen.as_uuid(), i64::from(limit))
            .await
            .map_err(map_sqlx)?;
        let total = queries::count_public_outbox(&self.db, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((items, total as u64))
    }

    /// Remove an inbound follow row (called from the inbox handler on `Undo(Follow)`).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn remove_inbound_follow(
        &self,
        citizen: CitizenId,
        remote_actor_url: &str,
    ) -> Result<u64> {
        queries::delete_inbound_follow(&self.db, citizen.as_uuid(), remote_actor_url)
            .await
            .map_err(map_sqlx)
    }

    /// Persist a fresh outbound follow (our citizen → remote actor). Caller has already done the
    /// signed network delivery and persists into the DB so a re-click is idempotent and the
    /// `/following` collection has a record to display. Returns the activity id URL the caller
    /// used to sign the Follow (passed in unchanged — convenience so callers don't lose track).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn record_outbound_follow(
        &self,
        citizen: CitizenId,
        remote_actor_url: &str,
        remote_inbox_url: &str,
        follow_activity_id: &str,
    ) -> Result<()> {
        let now = self.clock.now();
        queries::upsert_outbound_follow(
            &self.db,
            uuid::Uuid::now_v7(),
            citizen.as_uuid(),
            remote_actor_url,
            remote_inbox_url,
            follow_activity_id,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        Ok(())
    }

    /// Mark an outbound follow as ACK'd by the remote (called by the inbox handler when an
    /// `Accept(Follow)` arrives). Returns `true` if a pending row matched, `false` for a stray
    /// Accept that doesn't match anything we sent (treated as a no-op).
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn accept_outbound_follow(
        &self,
        citizen: CitizenId,
        remote_actor_url: &str,
    ) -> Result<bool> {
        let now = self.clock.now();
        let affected = queries::mark_outbound_follow_accepted(
            &self.db,
            citizen.as_uuid(),
            remote_actor_url,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        Ok(affected > 0)
    }

    /// List who the citizen follows (ACK'd outbound follows). Drives the `/actors/{handle}/
    /// following` OrderedCollection.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_following(
        &self,
        citizen: CitizenId,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<String>, u64)> {
        let urls = queries::list_outbound_following(
            &self.db,
            citizen.as_uuid(),
            i64::from(limit),
            i64::from(offset),
        )
        .await
        .map_err(map_sqlx)?;
        let total = queries::count_outbound_following(&self.db, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((urls, total as u64))
    }

    /// List ACK'd inbound followers of a citizen (page). Used by the OrderedCollection at
    /// `/actors/{handle}/followers`.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure.
    pub async fn list_followers(
        &self,
        citizen: CitizenId,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<String>, u64)> {
        let urls = queries::list_inbound_followers(
            &self.db,
            citizen.as_uuid(),
            i64::from(limit),
            i64::from(offset),
        )
        .await
        .map_err(map_sqlx)?;
        let total = queries::count_inbound_followers(&self.db, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        Ok((urls, total as u64))
    }

    /// Look up a public citizen by user-chosen handle (only when `is_public = true`). Used by the
    /// federation surface (`/.well-known/webfinger`, `/actors/{handle}`) to resolve `@handle`. Returns
    /// the profile DTO so the federation layer can build the Actor document from it.
    ///
    /// # Errors
    /// [`Error::NotFound`] for unknown / private handles (the surface conflates the two to keep the
    /// member directory opaque); [`Error::Storage`] on persistence failure.
    pub async fn find_public_by_handle(&self, org: OrgId, handle: &str) -> Result<ProfileDto> {
        let row = queries::find_public_citizen_by_handle(&self.db, org.as_uuid(), handle)
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("public citizen not found".to_owned()))?;
        Ok(self.row_to_dto(row))
    }

    /// Read (or LAZY-GENERATE) the citizen's federation actor public key. Called by the federation
    /// layer when materializing the Actor document. Generation runs on a blocking task (RSA-2048
    /// gen is ≈100ms CPU) and is wrapped in an `INSERT ... ON CONFLICT DO NOTHING` so a concurrent
    /// double-call never duplicates.
    ///
    /// Only public citizens (`is_public = true`) ever land here — the upstream federation surface
    /// already filters by that flag — so the storage footprint of keypairs is paid for exactly by
    /// those who actually federate.
    ///
    /// # Errors
    /// [`Error::Storage`] on persistence failure or RSA-gen failure (effectively never; the
    /// only realistic source of RSA failure is an exhausted OS RNG).
    pub async fn ensure_actor_public_key(&self, citizen: CitizenId) -> Result<String> {
        if let Some(pem) = queries::find_actor_public_key(&self.db, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?
        {
            return Ok(pem);
        }
        // No key yet — generate, persist, re-read (the second SELECT is needed because the INSERT
        // may be a no-op under contention; the surviving row's PEM is what we return).
        let kp = tokio::task::spawn_blocking(dsoc_federation::generate_actor_keypair)
            .await
            .map_err(|e| Error::Storage(Box::new(e)))?
            .map_err(|e| {
                Error::Storage(Box::new(std::io::Error::other(format!("rsa gen: {e}"))))
            })?;
        let now = self.clock.now();
        queries::insert_actor_keypair(
            &self.db,
            citizen.as_uuid(),
            &kp.private_pem,
            &kp.public_pem,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        let pem = queries::find_actor_public_key(&self.db, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| {
                Error::Storage(Box::new(std::io::Error::other("post-insert lookup empty")))
            })?;
        Ok(pem)
    }

    /// Shared body of [`Self::set_avatar`] / [`Self::set_cover`]. Validates the caller's
    /// citizen row, reads the prior object key (for cleanup), uploads the new image, updates
    /// the row, then best-effort deletes the prior object from storage.
    async fn set_media(
        &self,
        caller: CitizenId,
        expected_org: OrgId,
        kind: MediaKind,
        raw: Vec<u8>,
    ) -> Result<ProfileDto> {
        // Org binding (same protection as get/update).
        let current = queries::find_profile(&self.db, caller.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("citizen not found".to_owned()))?;
        if current.org_id != expected_org.as_uuid() {
            return Err(Error::Unauthorized);
        }

        let keys = queries::current_media_keys(&self.db, caller.as_uuid())
            .await
            .map_err(map_sqlx)?;
        let prior = match (keys, kind) {
            (Some((avatar, _)), MediaKind::Avatar) => avatar,
            (Some((_, cover)), MediaKind::Cover) => cover,
            (None, _) => None,
        };

        let new_key = media::upload_image(self.storage.as_ref(), caller, kind, raw).await?;

        match kind {
            MediaKind::Avatar => {
                queries::update_avatar_object_key(&self.db, caller.as_uuid(), &new_key)
                    .await
                    .map_err(map_sqlx)?;
            }
            MediaKind::Cover => {
                queries::update_cover_object_key(&self.db, caller.as_uuid(), &new_key)
                    .await
                    .map_err(map_sqlx)?;
            }
        }

        // Best-effort cleanup of the prior object. A failure here is logged but doesn't fail the
        // upload — the bytes are already swapped in the DB; the worst case is one orphan.
        if let (Some(prior), Some(storage)) = (prior, self.storage.as_ref()) {
            if let Err(err) = storage.delete(&prior).await {
                tracing::warn!(error = ?err, key = %prior, "failed to delete prior media object");
            }
        }

        // Re-read so the DTO reflects the persisted state (and renders the new URL).
        let row = queries::find_profile(&self.db, caller.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("citizen not found".to_owned()))?;
        Ok(self.row_to_dto(row))
    }

    /// Read the caller's profile. The org is taken from the authenticated `CallerId` (never from
    /// the request) so a caller cannot read another tenant's profile by id-guessing.
    ///
    /// # Errors
    /// [`Error::NotFound`] if the citizen row was deleted out from under the session;
    /// [`Error::Storage`] on persistence failure.
    pub async fn get(&self, caller: CitizenId, expected_org: OrgId) -> Result<ProfileDto> {
        let row = queries::find_profile(&self.db, caller.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("citizen not found".to_owned()))?;
        if row.org_id != expected_org.as_uuid() {
            // The session is wired to a different org than the citizen row — this should be
            // impossible under normal operation, but if it happens we treat it as a hard auth
            // failure rather than leak the other org's view.
            return Err(Error::Unauthorized);
        }
        Ok(self.row_to_dto(row))
    }

    /// Patch the caller's profile. Validates lengths and handle format; rejects a handle
    /// collision inside the org with a tagged Conflict (`handle_taken`).
    ///
    /// # Errors
    /// [`Error::Validation`] for a bad field; [`Error::Conflict`] (`"handle_taken"`) if the
    /// handle is in use by another citizen in the same org; [`Error::NotFound`] if the row was
    /// deleted; [`Error::Storage`] on persistence failure.
    pub async fn update(
        &self,
        caller: CitizenId,
        expected_org: OrgId,
        update: ProfileUpdate,
    ) -> Result<ProfileDto> {
        // Re-read first to enforce the org-binding (same protection as in `get`) and to give the
        // PATCH a stable error path when the underlying row has vanished.
        let current = queries::find_profile(&self.db, caller.as_uuid())
            .await
            .map_err(map_sqlx)?
            .ok_or_else(|| Error::NotFound("citizen not found".to_owned()))?;
        if current.org_id != expected_org.as_uuid() {
            return Err(Error::Unauthorized);
        }

        let normalized = normalize_update(update)?;
        let row = queries::update_profile(
            &self.db,
            caller.as_uuid(),
            normalized.display_name,
            normalized.bio,
            normalized.handle,
            normalized.is_public,
        )
        .await
        .map_err(map_update_sqlx)?
        .ok_or_else(|| Error::NotFound("citizen not found".to_owned()))?;
        Ok(self.row_to_dto(row))
    }

    fn row_to_dto(&self, row: ProfileRow) -> ProfileDto {
        let citizen = CitizenId::from_uuid(row.citizen_id);
        ProfileDto {
            citizen_id: row.citizen_id,
            org_id: row.org_id,
            handle: row.handle,
            public_handle: domain::public_handle(citizen),
            display_name: row.display_name,
            bio: row.bio,
            avatar_url: self.object_url(row.avatar_object_key.as_deref()),
            cover_url: self.object_url(row.cover_object_key.as_deref()),
            is_public: row.is_public,
            verification_level: row.verification_level,
            titulo_status: row.titulo_status,
            created_at: row.created_at,
        }
    }

    fn object_url(&self, key: Option<&str>) -> Option<String> {
        let key = key?;
        if self.media_base_url.is_empty() {
            // Storage backend not configured yet (pre-W1.2). The handler tolerates `None`.
            return None;
        }
        Some(format!("{}/{key}", self.media_base_url))
    }
}

/// Public mirror of `queries::DeliveryClaim` — what the worker needs to ship a pending row.
/// Lives in the service surface so the queries type can stay `pub(crate)` and the gateway
/// worker never imports query internals.
#[derive(Debug, Clone)]
pub struct DeliveryTask {
    /// The `federation_delivery.id` to mark done/failed after the attempt.
    pub delivery_id: uuid::Uuid,
    /// Where to POST.
    pub recipient_inbox: String,
    /// The signer (our actor URL — the `keyId` of the signing key is `<actor>/#main-key`).
    pub actor_url: String,
    /// PKCS#8 PEM of the citizen's private key. Sensitive: never log.
    pub private_pem: String,
    /// The wire-ready Activity to POST as the body.
    pub payload: serde_json::Value,
    /// Attempts so far (for the backoff schedule on failure).
    pub attempts: i32,
}

impl From<queries::DeliveryClaim> for DeliveryTask {
    fn from(c: queries::DeliveryClaim) -> Self {
        Self {
            delivery_id: c.delivery_id,
            recipient_inbox: c.recipient_inbox,
            actor_url: c.actor_url,
            private_pem: c.private_pem,
            payload: c.payload,
            attempts: c.attempts,
        }
    }
}

/// Input for [`ProfileService::update`]. Mirrors [`dsoc_api_contract::ProfileUpdateDto`] but uses
/// the two-level Option convention internally: outer `None` = leave alone, `Some(None)` = clear,
/// `Some(Some(v))` = set.
#[derive(Debug, Clone, Default)]
pub struct ProfileUpdate {
    pub display_name: Option<Option<String>>,
    pub bio: Option<Option<String>>,
    pub handle: Option<Option<String>>,
    pub is_public: Option<bool>,
}

impl From<dsoc_api_contract::ProfileUpdateDto> for ProfileUpdate {
    fn from(dto: dsoc_api_contract::ProfileUpdateDto) -> Self {
        // Empty string from the wire = clear (NULL) the column. This is the only way the JSON
        // body can express "remove the bio" given a flat optional field.
        let into_clear_or_set =
            |v: Option<String>| v.map(|s| if s.is_empty() { None } else { Some(s) });
        Self {
            display_name: into_clear_or_set(dto.display_name),
            bio: into_clear_or_set(dto.bio),
            handle: into_clear_or_set(dto.handle),
            is_public: dto.is_public,
        }
    }
}

fn normalize_update(update: ProfileUpdate) -> Result<ProfileUpdate> {
    let display_name = match update.display_name {
        Some(Some(v)) => {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Some(None)
            } else if trimmed.chars().count() > DISPLAY_NAME_MAX {
                return Err(Error::Validation(format!(
                    "display_name longer than {DISPLAY_NAME_MAX} chars"
                )));
            } else {
                Some(Some(trimmed))
            }
        }
        other => other,
    };

    let bio = match update.bio {
        Some(Some(v)) => {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Some(None)
            } else if trimmed.chars().count() > BIO_MAX {
                return Err(Error::Validation(format!(
                    "bio longer than {BIO_MAX} chars"
                )));
            } else {
                Some(Some(trimmed))
            }
        }
        other => other,
    };

    let handle = match update.handle {
        Some(Some(v)) => {
            let trimmed = v.trim().to_owned();
            if trimmed.is_empty() {
                Some(None)
            } else {
                validate_handle(&trimmed)?;
                Some(Some(trimmed))
            }
        }
        other => other,
    };

    Ok(ProfileUpdate {
        display_name,
        bio,
        handle,
        is_public: update.is_public,
    })
}

/// Validate a handle's format. Matches the DB CHECK: 3–32 chars from `[A-Za-z0-9_.-]`, no `..`.
fn validate_handle(handle: &str) -> Result<()> {
    let len = handle.chars().count();
    if !(HANDLE_MIN..=HANDLE_MAX).contains(&len) {
        return Err(Error::Validation(format!(
            "handle must be {HANDLE_MIN}–{HANDLE_MAX} chars"
        )));
    }
    if handle.contains("..") {
        return Err(Error::Validation("handle cannot contain '..'".to_owned()));
    }
    if !handle
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
    {
        return Err(Error::Validation(
            "handle may only contain letters, digits, '_', '.', '-'".to_owned(),
        ));
    }
    Ok(())
}

fn map_sqlx(error: sqlx::Error) -> Error {
    match error {
        sqlx::Error::RowNotFound => Error::NotFound("citizen not found".to_owned()),
        other => Error::Storage(Box::new(other)),
    }
}

/// Same as `map_sqlx`, but specifically catches the `(org_id, handle)` unique-violation from the
/// citizen profile migration (`citizen_org_handle_unique`) and surfaces it as a tagged Conflict
/// the http layer renders as "Esse nome de usuário já está em uso." — distinct from the bare
/// "Conflito de estado" the user otherwise can't act on.
fn map_update_sqlx(error: sqlx::Error) -> Error {
    if let sqlx::Error::Database(db) = &error {
        if db.is_unique_violation() && db.constraint().unwrap_or("").contains("handle") {
            return Error::Conflict("handle_taken".to_owned());
        }
    }
    map_sqlx(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_format_accepts_normal() {
        assert!(validate_handle("ana").is_ok());
        assert!(validate_handle("ana.lima_99").is_ok());
        assert!(validate_handle("a-b-c").is_ok());
    }

    #[test]
    fn handle_rejects_bad() {
        assert!(validate_handle("ab").is_err()); // too short
        assert!(validate_handle("a".repeat(33).as_str()).is_err()); // too long
        assert!(validate_handle("ana..lima").is_err()); // double dot
        assert!(validate_handle("ana@lima").is_err()); // illegal char
        assert!(validate_handle("ana lima").is_err()); // space
    }

    #[test]
    fn empty_string_clears() {
        let update = ProfileUpdate {
            display_name: Some(Some(String::new())),
            ..Default::default()
        };
        let normalized = normalize_update(update).unwrap();
        assert_eq!(normalized.display_name, Some(None));
    }

    #[test]
    fn whitespace_only_clears() {
        let update = ProfileUpdate {
            bio: Some(Some("   \n  ".to_owned())),
            ..Default::default()
        };
        let normalized = normalize_update(update).unwrap();
        assert_eq!(normalized.bio, Some(None));
    }

    #[test]
    fn long_bio_is_rejected() {
        let update = ProfileUpdate {
            bio: Some(Some("a".repeat(BIO_MAX + 1))),
            ..Default::default()
        };
        assert!(normalize_update(update).is_err());
    }
}
