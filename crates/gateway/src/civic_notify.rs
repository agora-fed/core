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
use dsoc_core::events::{Event, EventEnvelope};
use dsoc_core::Result;
use dsoc_events::EventHandler;
use sqlx::PgPool;
use uuid::Uuid;

use crate::notifications::{self, NewNotification};

#[derive(Debug)]
pub struct CivicNotifySub {
    pub db: PgPool,
    pub public_origin: String,
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
}
