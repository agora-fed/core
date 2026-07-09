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
            }
            Event::ConsequenceOfficialResponded { sla, .. } => {
                self.notify_via_sla(
                    sla.as_uuid(),
                    "sla_response",
                    "o mandato respondeu sua proposta — accountability registrada",
                )
                .await;
            }
            Event::ConsequenceSlaExpired { sla, .. } => {
                self.notify_via_sla(
                    sla.as_uuid(),
                    "sla_expired",
                    "o SLA venceu sem resposta — silêncio público registrado",
                )
                .await;
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
        self.notify_proposal_author(proposal_id, kind, preview).await;
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
        let row: Option<(Option<Uuid>, String)> = sqlx::query_as(
            "SELECT author_citizen_id, title FROM proposal WHERE id = $1",
        )
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
            .create_public_note(citizen, &actor_url, origin, &content, None, false, None)
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
    use super::build_threshold_note;

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
