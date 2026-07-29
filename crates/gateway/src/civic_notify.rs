//! Feed cidadão — subscriber que converte eventos cívicos em
//! `user_notification` (0.25.0-fediverso).
//!
//! O loop tese ("propor → cluster → threshold → SLA → resposta OU silêncio")
//! já emitia os eventos certos (`ProposalThresholdCrossed`, `ConsequenceSla*`);
//! só faltava fechar o feedback pro autor: **você não sabe que sua proposta
//! cruzou o gatilho até chegar aqui.** Este subscriber é isso.
//!
//! Estratégia: para cada evento cívico, resolvemos o `author_citizen_id` da
//! proposta associada (via `SlaId → consequence_sla.proposal_id → proposal`)
//! e inserimos uma linha em `user_notification` com kind cívico da migration
//! 0411. A insert é idempotente via UNIQUE `(citizen_id, kind, source_actor_url, object_uri)`
//! — usamos `object_uri` = URI local do proposal pra chaves distintas por
//! kind (`sla_started` vs `sla_response` vs `sla_expired` não colidem).

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
    /// Fase E completa (0.26.24): auto-federação no threshold precisa publicar
    /// uma Note em nome do autor — `create_public_note` mora aqui.
    pub profiles: ProfileService,
}

// Manual porque `ProfileService` não deriva Debug (segura um pool + clock).
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
                // Fase E completa: amplificação automática no fediverso, em
                // nome do autor. Best-effort — falha aqui nunca derruba a
                // notificação in-app acima nem o dispatch loop.
                self.auto_federate_threshold(proposal.as_uuid()).await;
            }
            Event::ConsequenceSlaStarted { sla, .. } => {
                self.notify_via_sla(
                    sla.as_uuid(),
                    "sla_started",
                    "o mandato tem prazo pra responder sua proposta",
                )
                .await;
                // 0.32.0: D0 do "AR digital" — o 1º aviso formal ao gabinete
                // sai AQUI (com link responder-sem-conta) e grava o recibo
                // nº 1 da cadeia. Sem ele a escada D+1/D+2 do worker nunca
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
                // Bloco C (C3): a RESPOSTA federa positivamente — simétrico ao
                // `auto_federate_silence`. A regra de ouro do plano: toda
                // consequência negativa amplificada deve ter sua versão
                // positiva. O silêncio já virava Note; a resposta também vira.
                self.auto_federate_response(sla.as_uuid()).await;
            }
            Event::ConsequenceSlaExpired { sla, .. } => {
                self.notify_via_sla(
                    sla.as_uuid(),
                    "sla_expired",
                    "o SLA venceu sem resposta — silêncio público registrado",
                )
                .await;
                // 0.29.1: o silêncio federa com a PROVA — a Note carrega a
                // cadeia de recibos dos avisos (AR digital, item 2 fatia 2).
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

    /// D0 do "AR digital do silêncio" (0.32.0): quando o SLA começa, o
    /// gabinete recebe o 1º aviso formal por e-mail — com o link assinado de
    /// responder-sem-conta — e o recibo nº 1 entra na cadeia hash-encadeada.
    /// Idempotente: se a proposta já tem recibo (redelivery do dispatch
    /// at-least-once), não reenvia. Best-effort — falha vira warn.
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
            // Mandato sem e-mail público — a escada nem começa; o silêncio
            // ainda expira normalmente pelo sweep.
            return;
        };
        // Idempotência: recibo 1 já existe → redelivery, não reenvia.
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
        let respond_url = match crate::respond_link::respond_token(sla_id) {
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
            // Proposta legacy sem autor (sem cidadão pra notificar).
            return;
        };
        // Preview inclui título curto pra reconhecimento na lista.
        let title_short: String = title.chars().take(80).collect();
        let full_preview = format!("{preview} — \"{title_short}\"");
        // object_uri é o URL público-ish do proposal (local). Serve tanto pro
        // link no front quanto como parte da UNIQUE key da idempotência.
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
        // Push real (RFC 8291) — não bloqueia o dispatch loop, spawn interno.
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
        // 0.32.0: além do in-app + push, os 3 marcos cívicos também saem por
        // e-mail pro autor (threshold cruzado, resposta, silêncio). O
        // `sla_started` fica só in-app — chega segundos depois do threshold
        // e viraria e-mail duplicado. Opt-out por `email_prefs` (chave =
        // kind; ausente = ligado).
        self.email_author(author, proposal_id, kind).await;
    }

    /// E-mail cívico ao autor da proposta (0.32.0). Best-effort + spawn —
    /// nunca segura o dispatch loop. Kind sem template mapeado é no-op.
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
        // Opt-out: email_prefs é `{"follow":true,...}`; chave ausente = on.
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

    /// Fase E completa (0.26.24): publica uma Note pública em nome do autor
    /// quando a proposta dele cruza o gatilho. Gates, na ordem:
    ///
    /// 1. proposta tem autor;
    /// 2. autor é federável (`is_public = true` + `handle`) e não desligou
    ///    `auto_federate_threshold` em /configuracoes;
    /// 3. ainda não existe Note deste autor citando esta proposta
    ///    (idempotência — o dispatch é at-least-once e a UNIQUE de
    ///    `user_notification` não segura NULLs em `source_actor_url`).
    ///
    /// Tudo best-effort: qualquer falha vira `warn` e retorna, sem
    /// propagar `Err` (senão o batch inteiro do subscriber trava).
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

        // Idempotência: já publicamos uma Note deste autor citando esta URL?
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
        // Primeiro post do cidadão pode não ter keypair ainda — gera lazy.
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
    /// Nota federada do SILÊNCIO com a prova embutida (0.29.1, AR digital
    /// fatia 2). Mesmos gates da Note de threshold (autor federável +
    /// preferência ligada); idempotência pela hashtag `#SilêncioRegistrado`
    /// junto da URL — a Note de threshold cita a mesma URL, então a URL
    /// sozinha não distingue. Best-effort: falha vira warn.
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

        // A prova: recibos hash-encadeados dos avisos ao gabinete.
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

    /// Nota federada POSITIVA da resposta (C3, Bloco C). Simétrica ao
    /// `auto_federate_silence`: quando o mandato responde dentro do prazo, o
    /// AUTOR da proposta (já federável) publica uma Note celebrando — dando
    /// ALCANCE positivo ao político, não só ameaça. Mesmos gates da Note de
    /// silêncio (autor federável + preferência ligada); idempotência pela
    /// hashtag `#RespostaRegistrada` junto da URL (a Note de silêncio/threshold
    /// cita a mesma URL, então a URL sozinha não distingue). Best-effort: falha
    /// vira warn e nunca derruba o dispatch loop nem as notificações in-app.
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
                citizen, &actor_url, origin, &content, &[], None, false, None,
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

/// Corpo da Note POSITIVA da resposta (C3). Celebra o mandato que respondeu — a versão positiva da
/// nota de silêncio. Título capado pra caber com folga no limite de 3000 chars do
/// `create_public_note`; a hashtag `#RespostaRegistrada` distingue da nota de silêncio na
/// idempotência e alimenta a timeline positiva.
fn build_response_note(title: &str, mandate_name: &str, proposal_url: &str) -> String {
    let title_short: String = title.chars().take(120).collect();
    format!(
        "✅ #RespostaRegistrada: {mandate_name} respondeu à demanda \"{title_short}\" dentro do \
         prazo público na #DemocraciaBR. Accountability funciona quando o gabinete participa.\n\n\
         {proposal_url}"
    )
}

/// Corpo da Note do silêncio — a denúncia viaja COM a prova: cada aviso
/// datado e o hash final da cadeia (verificável no endpoint público).
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

/// Corpo da Note de threshold. Título capado pra caber com folga no limite
/// de 3000 chars do `create_public_note`; a hashtag alimenta a timeline
/// `#DemocraciaBR` local e das instâncias remotas.
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
        // Marcador positivo distinto do silêncio (idempotência + timeline).
        assert!(note.contains("#RespostaRegistrada"));
        assert!(!note.contains("#SilêncioRegistrado"));
        // Credita o mandato pelo nome (alcance positivo ao político).
        assert!(note.contains("Dep. Fulana"));
        assert!(note.contains("Ciclovia na Av. Central"));
        assert!(note.contains("https://x.br/propostas/abc"));
        // Título absurdo não estoura o limite de 3000 chars do create_public_note.
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
        // Hash final truncado presente — a prova viaja com a denúncia.
        assert!(note.contains(&"bbbb2222".repeat(2)));
        assert!(note.contains("https://x.br/propostas/abc"));
        // Sem recibos (propostas antigas) a nota ainda sai, sem seção de cadeia.
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
