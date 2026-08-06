//! Citizen feed — the subscriber that turns civic events into
//! `user_notification` (0.25.0-fediverso).
//!
//! O loop tese ("propor → cluster → threshold → SLA → resposta OU silêncio")
//! already emitted the right events (`ProposalThresholdCrossed`, `ConsequenceSla*`);
//! all that was missing was closing the feedback loop to the author: **you do not know
//! your proposal crossed the trigger until this lands.** This subscriber is that.
//!
//! Strategy: for each civic event we resolve the proposal's `author_citizen_id`
//! proposta associada (via `SlaId → consequence_sla.proposal_id → proposal`)
//! and insert a row into `user_notification` with a civic kind from migration
//! 0411. The insert is idempotent via the UNIQUE `(citizen_id, kind, source_actor_url, object_uri)`
//! — we use `object_uri` = the proposal's local URI so the keys stay distinct per
//! kind (`sla_started` vs `sla_response` vs `sla_expired` never collide).

use async_trait::async_trait;
use dsoc_auth::profile::ProfileService;
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::ids::CitizenId;
use dsoc_core::Result;
use dsoc_events::EventHandler;
use sqlx::PgPool;
use uuid::Uuid;

use crate::notifications::{self, NewNotification};

pub struct CivicNotifySub {
    pub db: PgPool,
    pub public_origin: String,
    /// Complete phase E (0.26.24): auto-federation at the threshold must publish
    /// a Note on the author's behalf — `create_public_note` lives here.
    pub profiles: ProfileService,
}

// Manual because `ProfileService` does not derive Debug (it holds a pool + clock).
impl std::fmt::Debug for CivicNotifySub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CivicNotifySub")
            .field("public_origin", &self.public_origin)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl EventHandler for CivicNotifySub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        match envelope.event {
            Event::ProposalThresholdCrossed { proposal, .. } => {
                self.notify_proposal_author(
                    proposal.as_uuid(),
                    "proposal_threshold",
                    "sua proposta cruzou o gatilho da consequência — o SLA vai começar",
                )
                .await;
                // Complete phase E: automatic amplification on the fediverse, in
                // the author's name. Best-effort — a failure here never brings down the
                // neither the in-app notification above nor the dispatch loop.
                self.auto_federate_threshold(proposal.as_uuid()).await;
            }
            Event::ConsequenceSlaStarted { sla, .. } => {
                self.notify_via_sla(
                    sla.as_uuid(),
                    "sla_started",
                    "o mandato tem prazo pra responder sua proposta",
                )
                .await;
                // 0.32.0: D0 of the "digital registered mail" — the 1st formal warning to the cabinet
                // goes out HERE (with the answer-without-an-account link) and records
                // receipt #1 of the chain. Without it the worker's D+1/D+2 ladder never
                // disparava: a query exige `count(receipts) BETWEEN 1 AND 2`.
                self.email_gabinete_d0(sla.as_uuid()).await;
            }
            Event::ConsequenceOfficialResponded { sla, .. } => {
                self.notify_via_sla(
                    sla.as_uuid(),
                    "sla_response",
                    "o mandato respondeu sua proposta — accountability registrada",
                )
                .await;
                // Block C (C3): the ANSWER federates positively — symmetric to
                // `auto_federate_silence`. The plan's golden rule: every
                // amplified negative consequence must have its positive
                // counterpart. Silence already became a Note; so does the answer.
                self.auto_federate_response(sla.as_uuid()).await;
            }
            Event::ConsequenceSlaExpired { sla, .. } => {
                self.notify_via_sla(
                    sla.as_uuid(),
                    "sla_expired",
                    "o SLA venceu sem resposta — silêncio público registrado",
                )
                .await;
                // 0.29.1: silence federates WITH THE PROOF — the Note carries the
                // chain of warning receipts (digital registered mail, item 2 slice 2).
                self.auto_federate_silence(sla.as_uuid()).await;
            }
            _ => {}
        }
        Ok(())
    }
}

impl CivicNotifySub {
    async fn notify_via_sla(&self, sla_id: Uuid, kind: &str, preview: &str) {
        let proposal_id = match sqlx::query_scalar::<_, Uuid>(
            "SELECT proposal_id FROM consequence_sla WHERE id = $1",
        )
        .bind(sla_id)
        .fetch_optional(&self.db)
        .await
        {
            Ok(Some(p)) => p,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(sla = %sla_id, error = ?err, "civic_notify: SLA lookup falhou");
                return;
            }
        };
        self.notify_proposal_author(proposal_id, kind, preview)
            .await;
    }

    /// D0 of the "digital registered mail of silence" (0.32.0): when the SLA starts, the
    /// office receives the 1st formal warning by e-mail — with the signed
    /// answer-without-an-account link — and receipt #1 enters the hash-chained chain.
    /// Idempotent: when the proposal already has a receipt (an at-least-once dispatch
    /// redelivery), it does not resend. Best-effort — a failure becomes a warn.
    async fn email_gabinete_d0(&self, sla_id: Uuid) {
        type D0Row = (
            Uuid,
            String,
            Option<Uuid>,
            Option<String>,
            String,
            chrono::DateTime<chrono::Utc>,
        );
        let row: Option<D0Row> = match sqlx::query_as(
            r"SELECT p.id, p.title, s.mandate_id, m.public_email,
                     COALESCE(m.display_name, 'gabinete'), s.due_at
                FROM consequence_sla s
                JOIN proposal p ON p.id = s.proposal_id
                JOIN mandate m ON m.id = s.mandate_id
               WHERE s.id = $1",
        )
        .bind(sla_id)
        .fetch_optional(&self.db)
        .await
        {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(sla = %sla_id, error = ?err, "civic_notify: D0 lookup falhou");
                return;
            }
        };
        let Some((proposal_id, title, mandate_id, email, mandate_name, due_at)) = row else {
            return;
        };
        let Some(email) = email else {
            // A mandate with no public e-mail — the ladder never starts; the silence
            // still expires normally through the sweep.
            return;
        };
        // Idempotency: receipt 1 already exists → a redelivery, do not resend.
        let already: i64 =
            sqlx::query_scalar("SELECT count(*) FROM notification_receipt WHERE proposal_id = $1")
                .bind(proposal_id)
                .fetch_one(&self.db)
                .await
                .unwrap_or(0);
        if already > 0 {
            return;
        }
        let origin = self.public_origin.trim_end_matches('/');
        let respond_url = match crate::respond_link::issue_token(&self.db, sla_id).await {
            Some(token) => format!("{origin}/responder/?sla={sla_id}&t={token}"),
            None => format!("{origin}/propostas/{proposal_id}"),
        };
        let proposal_url = format!("{origin}/propostas/{proposal_id}");
        let mut ctx: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        ctx.insert("mandate_name", mandate_name.clone());
        ctx.insert("proposal_title", title.clone());
        ctx.insert("due_date", due_at.format("%d/%m/%Y").to_string());
        ctx.insert("respond_url", respond_url.clone());
        ctx.insert("proposal_url", proposal_url.clone());
        let (subject, body) =
            crate::email_templates::render(&self.db, "sla_started_mandate", &ctx)
                .await
                .unwrap_or_else(|| {
                    (
                        format!("[DemocraciaBR] Prazo de resposta iniciado — {title}"),
                        format!(
                            "Prezado(a) {mandate_name},\n\nA proposta cidadã \"{title}\" atingiu \
                             o número de apoios necessário e o prazo público de resposta começou.\n\n\
                             Responder agora (sem cadastro): {respond_url}\n\n\
                             Ver a demanda: {proposal_url}\n\n— DemocraciaBR",
                        ),
                    )
                });
        let outcome = match crate::proposal_delivery::smtp_from_env() {
            Some(cfg) => {
                match crate::proposal_delivery::send_email(&cfg, &email, &subject, &body).await {
                    Ok(()) => "accepted".to_owned(),
                    Err(err) => {
                        let mut msg = format!("failed: {err}");
                        msg.truncate(200);
                        msg
                    }
                }
            }
            None => "dev-logged".to_owned(),
        };
        crate::notification_receipts::record(
            &self.db,
            proposal_id,
            mandate_id,
            &email,
            &subject,
            &outcome,
        )
        .await;
    }

    async fn notify_proposal_author(&self, proposal_id: Uuid, kind: &str, preview: &str) {
        let (author, title): (Option<Uuid>, String) = match sqlx::query_as(
            "SELECT author_citizen_id, title FROM proposal WHERE id = $1",
        )
        .bind(proposal_id)
        .fetch_optional(&self.db)
        .await
        {
            Ok(Some(row)) => row,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(proposal = %proposal_id, error = ?err, "civic_notify: proposal lookup falhou");
                return;
            }
        };
        let Some(author) = author else {
            // Legacy proposal with no author (no citizen to notify).
            return;
        };
        // The preview includes a short title for recognition in the list.
        let title_short: String = title.chars().take(80).collect();
        let full_preview = format!("{preview} — \"{title_short}\"");
        // object_uri is the proposal's public-ish local URL. It serves both the
        // front-end link and as part of the idempotency UNIQUE key.
        let object_uri = format!(
            "{}/propostas/{}",
            self.public_origin.trim_end_matches('/'),
            proposal_id
        );
        let n = NewNotification {
            citizen_id: author,
            kind,
            source_actor_url: None,
            source_handle: "DemocraciaBR",
            source_display_name: Some("DemocraciaBR"),
            source_avatar_url: None,
            object_uri: Some(&object_uri),
            object_preview: Some(&full_preview),
        };
        if let Err(err) = notifications::insert(&self.db, n).await {
            tracing::warn!(citizen = %author, error = ?err, "civic_notify: insert falhou");
        }
        // Real push (RFC 8291) — never blocks the dispatch loop, internal spawn.
        let title = match kind {
            "proposal_threshold" => "🚨 Sua proposta cruzou o gatilho",
            "sla_started" => "⏳ SLA do mandato começou",
            "sla_response" => "✅ O mandato respondeu você",
            "sla_expired" => "🔇 Silêncio público registrado",
            _ => "DemocraciaBR",
        };
        let payload = serde_json::json!({
            "title": title,
            "body": full_preview,
            "url": object_uri,
        });
        crate::web_push::send_to_citizen(&self.db, author, &payload.to_string()).await;
        // 0.32.0: beyond in-app + push, the 3 civic milestones also go out by
        // e-mail to the author (threshold crossed, answer, silence). The
        // `sla_started` one stays in-app only — it arrives seconds after the threshold
        // and would become a duplicate e-mail. Opt-out via `email_prefs` (key =
        // kind; ausente = ligado).
        self.email_author(author, proposal_id, kind).await;
    }

    /// Civic e-mail to the proposal's author (0.32.0). Best-effort + spawn —
    /// never holds the dispatch loop. A kind with no mapped template is a no-op.
    async fn email_author(&self, author: Uuid, proposal_id: Uuid, kind: &str) {
        let template_key = match kind {
            "proposal_threshold" => "proposal_threshold_author",
            "sla_response" => "sla_response_author",
            "sla_expired" => "sla_expired_author",
            _ => return,
        };
        type AuthorRow = (Option<String>, Option<serde_json::Value>, String, String);
        let row: Option<AuthorRow> = match sqlx::query_as(
            r"SELECT ac.email, c.email_prefs, p.title,
                     COALESCE(m.display_name, 'o mandato')
                FROM proposal p
                LEFT JOIN mandate m ON m.id = p.mandate_id
                LEFT JOIN auth_credential ac ON ac.citizen_id = $2
                LEFT JOIN citizen c ON c.id = $2
               WHERE p.id = $1",
        )
        .bind(proposal_id)
        .bind(author)
        .fetch_optional(&self.db)
        .await
        {
            Ok(r) => r,
            Err(err) => {
                tracing::warn!(proposal = %proposal_id, error = ?err, "civic_notify: author e-mail lookup falhou");
                return;
            }
        };
        let Some((email, prefs, proposal_title, mandate_name)) = row else {
            return;
        };
        let Some(email) = email else { return };
        // Opt-out: email_prefs is `{"follow":true,...}`; an absent key = on.
        let enabled = prefs
            .as_ref()
            .and_then(|p| p.get(kind))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        if !enabled {
            return;
        }
        let Some(cfg) = crate::proposal_delivery::smtp_from_env() else {
            tracing::info!(to = %email, kind, "DEV: SMTP unconfigured; e-mail cívico logado em vez de enviado.");
            return;
        };
        let origin = self.public_origin.trim_end_matches('/');
        let proposal_url = format!("{origin}/propostas/{proposal_id}");
        let mut ctx: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
        ctx.insert("proposal_title", proposal_title.clone());
        ctx.insert("proposal_url", proposal_url.clone());
        ctx.insert("mandate_name", mandate_name);
        let (subject, body) = crate::email_templates::render(&self.db, template_key, &ctx)
            .await
            .unwrap_or_else(|| {
                (
                    format!("DemocraciaBR — atualização da sua proposta \"{proposal_title}\""),
                    format!(
                        "Olá,\n\nSua proposta \"{proposal_title}\" tem uma atualização.\n\n\
                         Acompanhe: {proposal_url}\n\n— DemocraciaBR"
                    ),
                )
            });
        tokio::spawn(async move {
            if let Err(err) =
                crate::proposal_delivery::send_email(&cfg, &email, &subject, &body).await
            {
                tracing::warn!(?err, "civic_notify: e-mail ao autor falhou");
            }
        });
    }

    /// Complete phase E (0.26.24): publishes a public Note on the author's behalf
    /// when their proposal crosses the trigger. Gates, in order:
    ///
    /// 1. the proposal has an author;
    /// 2. the author is federable (`is_public = true` + `handle`) and has not turned
    ///    `auto_federate_threshold` em /configuracoes;
    /// 3. no Note by this author citing this proposal exists yet
    ///    (idempotency — dispatch is at-least-once and the `user_notification`
    ///    UNIQUE does not hold NULLs in `source_actor_url`).
    ///
    /// All best-effort: any failure becomes a `warn` and returns, without
    /// propagating `Err` (otherwise the subscriber's whole batch stalls).
    async fn auto_federate_threshold(&self, proposal_id: Uuid) {
        let row: Option<(Option<Uuid>, String)> =
            sqlx::query_as("SELECT author_citizen_id, title FROM proposal WHERE id = $1")
                .bind(proposal_id)
                .fetch_optional(&self.db)
                .await
                .unwrap_or_default();
        let Some((Some(author), title)) = row else {
            return; // proposta sumiu ou é legacy sem autor — nada a federar.
        };

        let gate: Option<(Option<String>, bool, bool)> = sqlx::query_as(
            "SELECT handle, is_public, auto_federate_threshold
               FROM citizen WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(author)
        .fetch_optional(&self.db)
        .await
        .unwrap_or_default();
        let Some((Some(handle), true, true)) = gate else {
            tracing::debug!(
                citizen = %author,
                proposal = %proposal_id,
                "auto_federate: autor não federável ou preferência off; pulando"
            );
            return;
        };

        let origin = self.public_origin.trim_end_matches('/');
        let proposal_url = format!("{origin}/propostas/{proposal_id}");

        // Idempotency: have we already published a Note by this author citing this URL?
        let already: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM federation_outbox_entry
                 WHERE citizen_id = $1 AND payload::text LIKE '%' || $2 || '%')",
        )
        .bind(author)
        .bind(&proposal_url)
        .fetch_one(&self.db)
        .await
        .unwrap_or(true); // erro no check → assume que sim (não duplicar).
        if already {
            return;
        }

        let citizen = CitizenId::from_uuid(author);
        // A citizen's first post may not have a keypair yet — generate it lazily.
        if let Err(err) = self.profiles.ensure_actor_public_key(citizen).await {
            tracing::warn!(citizen = %author, error = ?err, "auto_federate: keypair falhou");
            return;
        }
        let actor_url = format!("{origin}/actors/{handle}");
        let content = build_threshold_note(&title, &proposal_url);
        match self
            .profiles
            .create_public_note(
                citizen,
                &actor_url,
                origin,
                &content,
                &[],
                None,
                false,
                None,
            )
            .await
        {
            Ok((activity_id, fanout)) => {
                tracing::info!(
                    citizen = %author,
                    proposal = %proposal_id,
                    activity = %activity_id,
                    fanout,
                    "auto_federate: Note do threshold publicada"
                );
            }
            Err(err) => {
                tracing::warn!(
                    citizen = %author,
                    proposal = %proposal_id,
                    error = ?err,
                    "auto_federate: create_public_note falhou"
                );
            }
        }
    }
}

impl CivicNotifySub {
    /// Federated Note of SILENCE with the proof embedded (0.29.1, digital
    /// registered mail slice 2). Same gates as the threshold Note (federable author +
    /// preference on); idempotency via the `#SilêncioRegistrado` hashtag
    /// alongside the URL — the threshold Note cites the same URL, so the URL
    /// alone does not distinguish them. Best-effort: a failure becomes a warn.
    async fn auto_federate_silence(&self, sla_id: Uuid) {
        let row: Option<(Uuid, Option<Uuid>, String)> = sqlx::query_as(
            "SELECT p.id, p.author_citizen_id, p.title
               FROM consequence_sla s JOIN proposal p ON p.id = s.proposal_id
              WHERE s.id = $1",
        )
        .bind(sla_id)
        .fetch_optional(&self.db)
        .await
        .unwrap_or_default();
        let Some((proposal_id, Some(author), title)) = row else {
            return;
        };
        let gate: Option<(Option<String>, bool, bool)> = sqlx::query_as(
            "SELECT handle, is_public, auto_federate_threshold
               FROM citizen WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(author)
        .fetch_optional(&self.db)
        .await
        .unwrap_or_default();
        let Some((Some(handle), true, true)) = gate else {
            return;
        };

        let origin = self.public_origin.trim_end_matches('/');
        let proposal_url = format!("{origin}/propostas/{proposal_id}");
        let already: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM federation_outbox_entry
                 WHERE citizen_id = $1
                   AND payload::text LIKE '%' || $2 || '%'
                   AND payload::text LIKE '%#SilêncioRegistrado%')",
        )
        .bind(author)
        .bind(&proposal_url)
        .fetch_one(&self.db)
        .await
        .unwrap_or(true);
        if already {
            return;
        }

        // The proof: hash-chained receipts of the warnings sent to the office.
        let receipts: Vec<(i32, chrono::DateTime<chrono::Utc>, String, String)> = sqlx::query_as(
            "SELECT attempt, sent_at, outcome, hash
               FROM notification_receipt WHERE proposal_id = $1 ORDER BY attempt",
        )
        .bind(proposal_id)
        .fetch_all(&self.db)
        .await
        .unwrap_or_default();

        let citizen = CitizenId::from_uuid(author);
        if let Err(err) = self.profiles.ensure_actor_public_key(citizen).await {
            tracing::warn!(citizen = %author, error = ?err, "silence note: keypair falhou");
            return;
        }
        let actor_url = format!("{origin}/actors/{handle}");
        let content = build_silence_note(&title, &proposal_url, &receipts);
        match self
            .profiles
            .create_public_note(
                citizen,
                &actor_url,
                origin,
                &content,
                &[],
                None,
                false,
                None,
            )
            .await
        {
            Ok((activity_id, fanout)) => {
                tracing::info!(
                    citizen = %author,
                    proposal = %proposal_id,
                    activity = %activity_id,
                    fanout,
                    receipts = receipts.len(),
                    "silence note: publicada com cadeia de recibos"
                );
            }
            Err(err) => {
                tracing::warn!(citizen = %author, proposal = %proposal_id, error = ?err,
                    "silence note: create_public_note falhou");
            }
        }
    }

    /// POSITIVE federated Note of the answer (C3, Block C). Symmetric to
    /// `auto_federate_silence`: when the mandate answers within the deadline, the
    /// proposal's AUTHOR (already federable) publishes a celebrating Note — giving
    /// the official positive REACH, not just a threat. Same gates as the silence
    /// Note (federable author + preference on); idempotency via the
    /// `#RespostaRegistrada` hashtag alongside the URL (the silence/threshold Note
    /// cites the same URL, so the URL alone does not distinguish). Best-effort: a failure
    /// becomes a warn and never breaks the dispatch loop nor the in-app notifications.
    async fn auto_federate_response(&self, sla_id: Uuid) {
        let row: Option<(Uuid, Option<Uuid>, String, Option<String>)> = sqlx::query_as(
            "SELECT p.id, p.author_citizen_id, p.title,
                    COALESCE(m.display_name, 'o mandato')
               FROM consequence_sla s
               JOIN proposal p ON p.id = s.proposal_id
               LEFT JOIN mandate m ON m.id = s.mandate_id
              WHERE s.id = $1",
        )
        .bind(sla_id)
        .fetch_optional(&self.db)
        .await
        .unwrap_or_default();
        let Some((proposal_id, Some(author), title, mandate_name)) = row else {
            return;
        };
        let mandate_name = mandate_name.unwrap_or_else(|| "o mandato".to_owned());

        let gate: Option<(Option<String>, bool, bool)> = sqlx::query_as(
            "SELECT handle, is_public, auto_federate_threshold
               FROM citizen WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(author)
        .fetch_optional(&self.db)
        .await
        .unwrap_or_default();
        let Some((Some(handle), true, true)) = gate else {
            return;
        };

        let origin = self.public_origin.trim_end_matches('/');
        let proposal_url = format!("{origin}/propostas/{proposal_id}");
        let already: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM federation_outbox_entry
                 WHERE citizen_id = $1
                   AND payload::text LIKE '%' || $2 || '%'
                   AND payload::text LIKE '%#RespostaRegistrada%')",
        )
        .bind(author)
        .bind(&proposal_url)
        .fetch_one(&self.db)
        .await
        .unwrap_or(true);
        if already {
            return;
        }

        let citizen = CitizenId::from_uuid(author);
        if let Err(err) = self.profiles.ensure_actor_public_key(citizen).await {
            tracing::warn!(citizen = %author, error = ?err, "response note: keypair falhou");
            return;
        }
        let actor_url = format!("{origin}/actors/{handle}");
        let content = build_response_note(&title, &mandate_name, &proposal_url);
        match self
            .profiles
            .create_public_note(
                citizen,
                &actor_url,
                origin,
                &content,
                &[],
                None,
                false,
                None,
            )
            .await
        {
            Ok((activity_id, fanout)) => {
                tracing::info!(
                    citizen = %author,
                    proposal = %proposal_id,
                    activity = %activity_id,
                    fanout,
                    "response note: resposta positiva federada"
                );
            }
            Err(err) => {
                tracing::warn!(citizen = %author, proposal = %proposal_id, error = ?err,
                    "response note: create_public_note falhou");
            }
        }
    }
}

/// Body of the POSITIVE answer Note (C3). Celebrates the mandate that answered — the positive
/// counterpart of the silence note. The title is capped to fit comfortably within the 3000-char
/// limit of `create_public_note`; the `#RespostaRegistrada` hashtag distinguishes it from the
/// silence note in the idempotency check and feeds the positive timeline.
fn build_response_note(title: &str, mandate_name: &str, proposal_url: &str) -> String {
    let title_short: String = title.chars().take(120).collect();
    format!(
        "✅ #RespostaRegistrada: {mandate_name} respondeu à demanda \"{title_short}\" dentro do \
         prazo público na #DemocraciaBR. Accountability funciona quando o gabinete participa.\n\n\
         {proposal_url}"
    )
}

/// Body of the silence Note — the denunciation travels WITH the proof: each dated
/// warning and the chain's final hash (verifiable at the public endpoint).
fn build_silence_note(
    title: &str,
    proposal_url: &str,
    receipts: &[(i32, chrono::DateTime<chrono::Utc>, String, String)],
) -> String {
    let title_short: String = title.chars().take(120).collect();
    let mut avisos = String::new();
    for (attempt, sent_at, outcome, _) in receipts {
        let ok = if outcome == "accepted" {
            "entregue"
        } else {
            outcome.as_str()
        };
        avisos.push_str(&format!(
            "• aviso {attempt}: {} ({ok})\n",
            sent_at.format("%d/%m/%Y")
        ));
    }
    let chain = receipts
        .last()
        .map(|(_, _, _, hash)| {
            let short: String = hash.chars().take(16).collect();
            format!(
                "Cadeia de recibos sha256 …{short} — verifique em \
                 {proposal_url} (recibos públicos).\n"
            )
        })
        .unwrap_or_default();
    format!(
        "🔇 #SilêncioRegistrado: o mandato não respondeu à demanda \
         \"{title_short}\" dentro do prazo público na #DemocraciaBR.\n\n\
         {avisos}{chain}\n{proposal_url}"
    )
}

/// Body of the threshold Note. The title is capped to fit comfortably within the
/// of `create_public_note`'s 3000 chars; the hashtag feeds the timeline
/// local `#DemocraciaBR` and of the remote instances.
fn build_threshold_note(title: &str, proposal_url: &str) -> String {
    let title_short: String = title.chars().take(140).collect();
    format!(
        "🚨 Minha proposta \"{title_short}\" cruzou o gatilho de consequência \
         na #DemocraciaBR — o mandato agora tem prazo pra responder, ou o \
         silêncio fica registrado no placar público.\n\nAcompanhe: {proposal_url}"
    )
}

#[cfg(test)]
mod tests {
    use super::{build_response_note, build_silence_note, build_threshold_note};

    #[test]
    fn response_note_is_positive_and_names_the_mandate() {
        let note = build_response_note(
            "Ciclovia na Av. Central",
            "Dep. Fulana",
            "https://x.br/propostas/abc",
        );
        // A positive marker distinct from silence (idempotency + timeline).
        assert!(note.contains("#RespostaRegistrada"));
        assert!(!note.contains("#SilêncioRegistrado"));
        // Credits the mandate by name (positive reach for the official).
        assert!(note.contains("Dep. Fulana"));
        assert!(note.contains("Ciclovia na Av. Central"));
        assert!(note.contains("https://x.br/propostas/abc"));
        // An absurd title must not blow the 3000-char limit of create_public_note.
        let long = build_response_note(&"x".repeat(5_000), "M", "https://x.br/p/1");
        assert!(long.chars().count() <= 3_000);
    }

    #[test]
    fn silence_note_carries_receipts_chain_and_marker() {
        let t = chrono::DateTime::parse_from_rfc3339("2026-07-10T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let receipts = vec![
            (1, t, "accepted".to_owned(), "aaaa1111".repeat(8)),
            (2, t, "accepted".to_owned(), "bbbb2222".repeat(8)),
        ];
        let note = build_silence_note("Ciclovia", "https://x.br/propostas/abc", &receipts);
        assert!(note.contains("#SilêncioRegistrado"));
        assert!(note.contains("aviso 1: 10/07/2026 (entregue)"));
        assert!(note.contains("aviso 2"));
        // The truncated final hash is present — the proof travels with the denunciation.
        assert!(note.contains(&"bbbb2222".repeat(2)));
        assert!(note.contains("https://x.br/propostas/abc"));
        // Without receipts (old proposals) the note still goes out, minus the chain section.
        let bare = build_silence_note("Ciclovia", "https://x.br/p/1", &[]);
        assert!(bare.contains("#SilêncioRegistrado"));
        assert!(!bare.contains("Cadeia de recibos"));
    }

    #[test]
    fn threshold_note_carries_title_url_and_hashtag() {
        let note = build_threshold_note("Ciclovia na Av. Central", "https://x.br/propostas/abc");
        assert!(note.contains("Ciclovia na Av. Central"));
        assert!(note.contains("https://x.br/propostas/abc"));
        assert!(note.contains("#DemocraciaBR"));
    }

    #[test]
    fn threshold_note_caps_long_titles_within_note_limit() {
        let long_title = "x".repeat(5_000);
        let note = build_threshold_note(&long_title, "https://x.br/p/1");
        assert!(note.chars().count() <= 3_000);
        assert!(note.contains(&"x".repeat(140)));
        assert!(!note.contains(&"x".repeat(141)));
    }
}
