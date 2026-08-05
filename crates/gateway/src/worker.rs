//! # In-process event worker — the composition root's background runtime.
//!
//! The platform writes every cross-crate signal to a durable event log (the transactional outbox,
//! ADR-0006), but the log is inert until something *drains* it. This module is that something: it
//! revives the **consequence loop** at runtime by (1) draining the log into each subscriber's
//! idempotent [`EventHandler`] and (2) sweeping expired SLA clocks on a fixed interval.
//!
//! It lives in the gateway because the gateway is the only **composition root** allowed to touch more
//! than one crate (PLAN.md section 6): a subscriber handler may, for example, read a proposal's body
//! from `dsoc-proposals` to feed `dsoc-consensus`, which no component crate could do without breaching
//! the boundary rules. The handlers themselves contain no business logic — they forward each delivery
//! into the owning crate's existing, tested service method.
//!
//! Every loop is **supervised**: a transient error (a momentary DB blip) is logged and the loop
//! retries on its next tick rather than killing the gateway process. Delivery is at-least-once, so
//! every handler is idempotent (the owning crates dedupe by their natural keys / `claim_consumed`).
//!
//! This runs in-process for the lean single-node deployment; it is structured so it can later be
//! lifted into a dedicated worker Deployment for HA without changing the handlers (Phase 3).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dsoc_app::AppState;
use dsoc_auth::profile::ProfileService;
use dsoc_core::events::{Event, EventEnvelope, EventTopic};
use dsoc_core::Result;
use dsoc_events::{dispatch_batch, EventHandler, EventQueue, SubscriberName};
use tokio::time::{interval, MissedTickBehavior};

/// Max deliveries pulled per poll. Bounds memory and query cost; the loop drains in repeated batches.
const POLL_LIMIT: u32 = 500;
/// Default cadence of the dispatch poll (ms). Low enough that the loop feels live, high enough to keep
/// idle DB load negligible. Override with `WORKER_DISPATCH_MS`.
const DEFAULT_DISPATCH_MS: u64 = 1_000;
/// Default cadence of the SLA expiry sweep (ms). Override with `WORKER_SWEEP_MS`.
const DEFAULT_SWEEP_MS: u64 = 60_000;
/// Default cadence of the federation delivery drain (ms). Tighter than the sweep because each
/// tick handles up to `DELIVERY_BATCH` pending shipments; idle ticks are cheap (one indexed
/// SELECT). Override with `WORKER_DELIVERY_MS`.
const DEFAULT_DELIVERY_MS: u64 = 2_000;
/// Max deliveries claimed per delivery tick. Bounds the burst when a Note fans out to many
/// followers; the queue drains over subsequent ticks.
const DELIVERY_BATCH: u32 = 50;
/// Default cadence do re-embed do backlog (fatia 2a, 0.28.4). Override com
/// `WORKER_REEMBED_MS`. Backlog vazio = um SELECT indexado por tick.
const DEFAULT_REEMBED_MS: u64 = 60_000;
/// Cadence do escalonamento de avisos ao gabinete (0.29 — AR digital).
/// Reenvia D+1 e D+2 enquanto o SLA está pendente; 1 tick/hora basta
/// (a granularidade da escada é diária). Override: `WORKER_ESCALATION_MS`.
const DEFAULT_ESCALATION_MS: u64 = 60 * 60 * 1000;
/// Propostas re-embedadas por tick. O modelo roda em CPU (~centenas de ms
/// por texto) — lote pequeno pra não competir com o ingest ao vivo.
const REEMBED_BATCH: i64 = 8;
/// Drop a delivery row after this many failed attempts (the worker stops claiming it; the row
/// stays in DB for ops introspection). Matches the longest reasonable Mastodon retry window.
const DELIVERY_MAX_ATTEMPTS: i32 = 10;
/// Cadência do cleanup de pending_signups vencidos. Uma vez por hora é suficiente —
/// pending TTL é 24h e o volume esperado é baixíssimo. Override com
/// `WORKER_SIGNUP_CLEANUP_MS`.
const DEFAULT_SIGNUP_CLEANUP_MS: u64 = 3_600_000;
/// Idade (em dias) que uma pending expirada precisa ter pra ser apagada. Guardamos
/// uns dias após o vencimento pra auditoria/ops. Override com `AUTH_SIGNUP_CLEANUP_DAYS`.
const DEFAULT_SIGNUP_CLEANUP_DAYS: i64 = 7;
/// Cadência default do sweep do SOCRATES (6 h). A coleção do e-Cidadania muda
/// devagar (é um ranking de apoios) — bater mais que isso só gasta a paciência
/// do portal do Senado. Override com `SOCRATES_SWEEP_MS`.
const DEFAULT_SOCRATES_SWEEP_MS: u64 = 6 * 60 * 60 * 1000;

/// Read a millisecond interval from the environment, falling back to `default`.
fn env_ms(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v| v > 0)
        .unwrap_or(default)
}

// --- Subscriber handlers ----------------------------------------------------------------------
//
// One struct per durable subscriber. Each forwards the matching events into the owning crate's
// service; unrelated events are an idempotent no-op there, so a handler is total over the catalog.

/// `consensus`: on `proposals.created`, embed and cluster the proposal. The slim catalog event
/// carries ids only, so the composition root fetches the proposal body to feed the embedder.
struct ConsensusSub {
    consensus: dsoc_consensus::ClusterService,
    proposals: dsoc_proposals::ProposalService,
}

#[async_trait]
impl EventHandler for ConsensusSub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        if let Event::ProposalCreated { proposal, .. } = envelope.event {
            let row = self.proposals.get(proposal).await?;
            let text = format!("{}\n{}", row.title, row.body);
            self.consensus.consume(envelope, &text).await?;
        }
        Ok(())
    }
}

/// `moderation`: on `proposals.created`, evaluate the proposal against rules
/// and emit `moderation.cleared`/`moderation.flagged`. Sem este subscriber, a
/// proposta ficava presa em `draft` pra sempre — não havia trigger natural
/// pra publicar. Fecha o gap "não estão 100%" das propostas.
struct ModerationEvaluateSub {
    moderation: dsoc_moderation::ModerationService,
    proposals: dsoc_proposals::ProposalService,
}

#[async_trait]
impl EventHandler for ModerationEvaluateSub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        if let Event::ProposalCreated { proposal, .. } = envelope.event {
            let row = self.proposals.get(proposal).await?;
            let text = format!("{}\n{}", row.title, row.body);
            let org = dsoc_core::ids::OrgId::from_uuid(row.org_id);
            let target = dsoc_moderation::ModerationTarget::Proposal(proposal);
            self.moderation.evaluate(org, target, &text).await?;
        }
        Ok(())
    }
}

/// `proposals`: link to clusters, publish on moderation clearance, fold vote tallies, and fire the
/// threshold crossing. Drives the front half of the loop toward `proposals.threshold.crossed`.
struct ProposalsSub {
    proposals: dsoc_proposals::ProposalService,
}

#[async_trait]
impl EventHandler for ProposalsSub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        self.proposals.consume(envelope).await?;
        Ok(())
    }
}

/// `consequence`: on `proposals.threshold.crossed`, start the SLA clock against the official.
struct ConsequenceSub {
    consequence: dsoc_consequence::ConsequenceService,
}

#[async_trait]
impl EventHandler for ConsequenceSub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        self.consequence.consume(envelope).await?;
        Ok(())
    }
}

/// `scorecard`: project consequence/mandate events into the public record. A `responded` event needs
/// the response latency, which the slim catalog event does not carry — the composition root derives it
/// from the SLA's own `started_at` (the trusted source) and the event time.
struct ScorecardSub {
    scorecard: dsoc_scorecard::ScorecardService,
    consequence: dsoc_consequence::ConsequenceService,
}

#[async_trait]
impl EventHandler for ScorecardSub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        let response_hours = match envelope.event {
            Event::ConsequenceOfficialResponded { sla, .. } => {
                let row = self.consequence.get_sla(sla).await?;
                let seconds = (envelope.at - row.started_at).num_seconds().max(0) as f64;
                Some(seconds / 3_600.0)
            }
            _ => None,
        };
        dsoc_scorecard::handle_event(&self.scorecard, envelope, response_hours).await?;
        Ok(())
    }
}

/// `comments`: on `moderation.flagged`, flag the targeted proposal's still-visible comments.
struct CommentsSub {
    comments: dsoc_comments::CommentService,
}

#[async_trait]
impl EventHandler for CommentsSub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        dsoc_comments::handle_event(&self.comments, envelope).await?;
        Ok(())
    }
}

/// `notify`: fan-out for the SLA-lifecycle events. Looks up the operator citizen on the mandate's
/// identity binding and enqueues a channel-appropriate notification (`dsoc-notify` picks the
/// channel via its plan). Unbound mandates are info-logged and skipped — no error, no retry.
struct NotifySlaSub {
    notify: dsoc_notify::NotifyService,
    db: dsoc_db::Db,
}

#[async_trait]
impl EventHandler for NotifySlaSub {
    async fn handle(&self, envelope: &EventEnvelope) -> Result<()> {
        dsoc_notify::handle_sla_event(&self.notify, &self.db, envelope).await?;
        Ok(())
    }
}

// --- Subscription wiring ----------------------------------------------------------------------

/// One durable subscription: a named cursor over a topic, driving a handler. The same topic may be
/// consumed by several subscriptions (distinct names → independent cursors): `consensus` and
/// `consequence` both watch `proposals`, each reacting to its own events.
struct Subscription {
    name: SubscriberName,
    topic: EventTopic,
    handler: Arc<dyn EventHandler>,
}

/// Build the full subscription set from application state. Each service is constructed from the shared
/// ports (`from_state`), so the worker holds no privileged access the HTTP layer lacks.
fn subscriptions(state: &AppState) -> Vec<Subscription> {
    // The names are fixed routing identifiers, not user input. `parse` is fallible, so a name that
    // somehow failed validation is dropped (logged) rather than panicking the process — but in
    // practice every literal below is valid, so the set is complete.
    let sub = |name: &str, topic: EventTopic, handler: Arc<dyn EventHandler>| {
        match SubscriberName::parse(name) {
            Ok(name) => Some(Subscription {
                name,
                topic,
                handler,
            }),
            Err(err) => {
                tracing::error!(subscriber = name, error = %err, "invalid subscriber name; skipping");
                None
            }
        }
    };

    [
        // Front half of the loop: created → clustered → (tally) → threshold crossed.
        sub(
            "consensus-worker",
            EventTopic::Proposals,
            Arc::new(ConsensusSub {
                consensus: dsoc_consensus::ClusterService::from_state(state),
                proposals: dsoc_proposals::ProposalService::from_state(state),
            }),
        ),
        sub(
            "proposals-consensus-worker",
            EventTopic::Consensus,
            Arc::new(ProposalsSub {
                proposals: dsoc_proposals::ProposalService::from_state(state),
            }),
        ),
        sub(
            "proposals-moderation-worker",
            EventTopic::Moderation,
            Arc::new(ProposalsSub {
                proposals: dsoc_proposals::ProposalService::from_state(state),
            }),
        ),
        // NOVO: avalia propostas recém-criadas contra as regras de moderação
        // e emite ModerationCleared/Flagged. Sem isso, o proposal ficava
        // eterno em `draft` (o publish só dispara em `ModerationCleared`).
        sub(
            "moderation-evaluate-worker",
            EventTopic::Proposals,
            Arc::new(ModerationEvaluateSub {
                moderation: dsoc_moderation::ModerationService::from_state(state),
                proposals: dsoc_proposals::ProposalService::from_state(state),
            }),
        ),
        sub(
            "proposals-votes-worker",
            EventTopic::Votes,
            Arc::new(ProposalsSub {
                proposals: dsoc_proposals::ProposalService::from_state(state),
            }),
        ),
        // Back half (the thesis spine): threshold crossed → SLA → expiry/response → scorecard.
        sub(
            "consequence-worker",
            EventTopic::Proposals,
            Arc::new(ConsequenceSub {
                consequence: dsoc_consequence::ConsequenceService::from_state(state),
            }),
        ),
        sub(
            "scorecard-consequence-worker",
            EventTopic::Consequence,
            Arc::new(ScorecardSub {
                scorecard: dsoc_scorecard::ScorecardService::from_state(state),
                consequence: dsoc_consequence::ConsequenceService::from_state(state),
            }),
        ),
        sub(
            "scorecard-mandates-worker",
            EventTopic::Mandates,
            Arc::new(ScorecardSub {
                scorecard: dsoc_scorecard::ScorecardService::from_state(state),
                consequence: dsoc_consequence::ConsequenceService::from_state(state),
            }),
        ),
        // Moderation fan-out into comments.
        sub(
            "comments-moderation-worker",
            EventTopic::Moderation,
            Arc::new(CommentsSub {
                comments: dsoc_comments::CommentService::from_state(state),
            }),
        ),
        // SLA-lifecycle notifications to the operator of the targeted mandate. Two subscriptions
        // (distinct cursors) because the three events live under two topics: `proposals` carries
        // `proposals.threshold.crossed`, `consequence` carries the SLA started/expired events.
        sub(
            "notify-sla-proposals-worker",
            EventTopic::Proposals,
            Arc::new(NotifySlaSub {
                notify: dsoc_notify::NotifyService::from_state(state),
                db: state.db.clone(),
            }),
        ),
        sub(
            "notify-sla-consequence-worker",
            EventTopic::Consequence,
            Arc::new(NotifySlaSub {
                notify: dsoc_notify::NotifyService::from_state(state),
                db: state.db.clone(),
            }),
        ),
        // Feed cidadão (0.25.0-fediverso): notifica o AUTOR da proposta em cada
        // marco cívico (threshold, sla started/response/expired). NotifySlaSub
        // acima cobre o operador do mandato; este cobre o cidadão que propôs.
        sub(
            "civic-notify-proposals-worker",
            EventTopic::Proposals,
            Arc::new(crate::civic_notify::CivicNotifySub {
                db: state.db.clone(),
                public_origin: std::env::var("PUBLIC_ORIGIN")
                    .unwrap_or_else(|_| "https://democracia.social.br".to_owned()),
                profiles: ProfileService::from_state(state),
            }),
        ),
        sub(
            "civic-notify-consequence-worker",
            EventTopic::Consequence,
            Arc::new(crate::civic_notify::CivicNotifySub {
                db: state.db.clone(),
                public_origin: std::env::var("PUBLIC_ORIGIN")
                    .unwrap_or_else(|_| "https://democracia.social.br".to_owned()),
                profiles: ProfileService::from_state(state),
            }),
        ),
        // Recibo de entrega da proposta (autor + gabinete). No ProposalCreated,
        // dispara dois e-mails via SMTP e grava os timestamps em
        // proposal.notified_{author,mandate}_at.
        sub(
            "proposal-delivery-worker",
            EventTopic::Proposals,
            Arc::new(crate::proposal_delivery::ProposalDeliverySub {
                db: state.db.clone(),
                public_origin: std::env::var("PUBLIC_ORIGIN")
                    .unwrap_or_else(|_| "https://democracia.social.br".to_owned()),
            }),
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

// --- Loops ------------------------------------------------------------------------------------

/// Spawn the background runtime: one supervised dispatch loop per subscription plus the SLA sweep.
/// Returns immediately; the loops run for the lifetime of the process. Set `WORKER_ENABLED=false` to
/// disable (tests / read-only replicas).
pub fn spawn(state: AppState) {
    if std::env::var("WORKER_ENABLED")
        .map(|v| v == "false")
        .unwrap_or(false)
    {
        tracing::info!("event worker disabled (WORKER_ENABLED=false)");
        return;
    }

    let queue = EventQueue::new(state.db.clone(), state.clock.clone());
    let dispatch_ms = env_ms("WORKER_DISPATCH_MS", DEFAULT_DISPATCH_MS);
    let sweep_ms = env_ms("WORKER_SWEEP_MS", DEFAULT_SWEEP_MS);

    // Ator de instância (0539): prima a chave que assina os fetches ActivityPub
    // (AUTHORIZED_FETCH). Best-effort — sem ela, fetches saem sem assinatura.
    {
        let db = state.db.clone();
        let origin = std::env::var("PUBLIC_ORIGIN")
            .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
        tokio::spawn(async move {
            crate::federation::prime_instance_key(&db, &origin).await;
        });
    }

    // Carteiro dos fóruns (F3): envia os dispatches de patamar pendentes (1/min).
    {
        let db = state.db.clone();
        let origin = std::env::var("PUBLIC_ORIGIN")
            .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_millis(60_000));
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                crate::forum_mailer::sweep(&db, &origin).await;
                // Daily consolidated representative alert (0676, issue #3):
                // self-gating — the (mandate, day) claim makes every extra
                // tick a no-op, so riding the 60s loop is free.
                crate::topic_representatives::daily_alert_sweep(&db, &origin).await;
            }
        });
    }

    let subs = subscriptions(&state);
    let count = subs.len();
    for subscription in subs {
        let queue = queue.clone();
        tokio::spawn(dispatch_loop(queue, subscription, dispatch_ms));
    }

    let consequence = dsoc_consequence::ConsequenceService::from_state(&state);
    let clock = state.clock.clone();
    tokio::spawn(sweep_loop(consequence, clock, sweep_ms));

    // Federation outbound delivery (ADR-0010 W2.5). Drains `federation_delivery` rows that are
    // due, signs each with the author citizen's key, POSTs to the recipient inbox. Failures get
    // exponential backoff inside the DB row; the worker only ships, never schedules.
    let delivery_ms = env_ms("WORKER_DELIVERY_MS", DEFAULT_DELIVERY_MS);
    tokio::spawn(federation_delivery_loop(state.clone(), delivery_ms));

    // Cleanup de auth_pending_signup vencidos (P3.3). Deletar bem depois do
    // vencimento — a auditoria pode querer ver quem tentou o quê.
    let signup_cleanup_ms = env_ms("WORKER_SIGNUP_CLEANUP_MS", DEFAULT_SIGNUP_CLEANUP_MS);
    let signup_cleanup_days = std::env::var("AUTH_SIGNUP_CLEANUP_DAYS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&v: &i64| v > 0)
        .unwrap_or(DEFAULT_SIGNUP_CLEANUP_DAYS);
    tokio::spawn(signup_cleanup_loop(
        state.clone(),
        signup_cleanup_ms,
        signup_cleanup_days,
    ));

    // 0.26.19: apaga notas próprias com idade > auto_delete_notes_older_than_days.
    // 1 tick por hora — barato, indexed. Não bloqueia outros loops.
    tokio::spawn(auto_delete_notes_loop(state.clone()));

    // Retry de entrega de proposta: reenvia os e-mails (autor/gabinete) que NÃO
    // saíram porque o SMTP falhou na 1ª tentativa — o envio é fire-and-forget e o
    // cursor do evento avança mesmo em falha, então sem isto o e-mail some. Idempotente
    // (só reenvia o lado com notified_*_at NULL). Reusa a cadência do sweep de SLA.
    tokio::spawn(proposal_delivery_retry_loop(
        crate::proposal_delivery::ProposalDeliverySub {
            db: state.db.clone(),
            public_origin: std::env::var("PUBLIC_ORIGIN")
                .unwrap_or_else(|_| "https://democracia.social.br".to_owned()),
        },
        sweep_ms,
    ));

    // 0.28.4 (fatia 2a): re-embeda o backlog da era do stub FNV — rows de
    // consensus_embedding sem text_sample ganham vetor real + assinatura de
    // direção + amostra NLI, e o centroide do cluster é recomputado. Some
    // sozinho quando o backlog seca.
    tokio::spawn(reembed_backlog_loop(state.clone()));

    // 0.29 (AR digital do silêncio): enquanto o SLA está pendente, o aviso
    // ao gabinete escala D+1 e D+2 — cada reenvio vira recibo hash-encadeado.
    tokio::spawn(notification_escalation_loop(state.clone()));

    // SOCRATES v2 (0671): sweep das Ideias Legislativas em alta no e-Cidadania.
    // DESLIGADO por default — este loop PUBLICA conteúdo no fórum em nome do
    // bot, então precisa de um "sim" explícito por instalação, não de um deploy.
    let socrates_enabled = std::env::var("SOCRATES_SWEEP_ENABLED")
        .map(|v| v == "true")
        .unwrap_or(false);
    if socrates_enabled {
        tokio::spawn(socrates_sweep_loop(state.clone()));
    } else {
        tracing::info!("socrates sweep desligado (defina SOCRATES_SWEEP_ENABLED=true pra ligar)");
    }

    tracing::info!(
        subscriptions = count,
        dispatch_ms,
        sweep_ms,
        delivery_ms,
        signup_cleanup_ms,
        "event worker started: consequence loop is live"
    );
}

/// Fatia 2a (0.28.4): drena o backlog de embeddings da era do stub. Cada
/// tick pega até [`REEMBED_BATCH`] propostas com `text_sample` vazio, busca
/// o texto no composition root (tabela de outro crate — mesmo padrão do
/// [`ConsensusSub`]) e re-embeda via [`dsoc_consensus::ClusterService::re_embed`].
/// Proposta cujo fetch falhe fica no backlog e é retentada no próximo tick
/// (log em warn); o loop nunca derruba o worker.
async fn reembed_backlog_loop(state: AppState) {
    let reembed_ms = env_ms("WORKER_REEMBED_MS", DEFAULT_REEMBED_MS);
    let consensus = dsoc_consensus::ClusterService::from_state(&state);
    let proposals = dsoc_proposals::ProposalService::from_state(&state);
    let mut ticker = interval(Duration::from_millis(reembed_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let batch = match consensus.stale_backlog(REEMBED_BATCH).await {
            Ok(batch) => batch,
            Err(err) => {
                tracing::error!(?err, "re-embed: backlog fetch failed");
                continue;
            }
        };
        if batch.is_empty() {
            continue;
        }
        let mut done = 0usize;
        for proposal in batch {
            let row = match proposals.get(proposal).await {
                Ok(row) => row,
                Err(dsoc_core::Error::NotFound(_)) => {
                    // Proposta apagada (purge/LGPD) — a embedding é órfã e
                    // sairia do backlog só via purge, senão retry eterno.
                    match consensus.purge_orphan(proposal).await {
                        Ok(()) => tracing::info!(
                            %proposal,
                            "re-embed: purged orphan embedding (proposal was deleted)"
                        ),
                        Err(err) => {
                            tracing::error!(%proposal, ?err, "re-embed: orphan purge failed");
                        }
                    }
                    continue;
                }
                Err(err) => {
                    tracing::warn!(%proposal, ?err, "re-embed: proposal fetch failed; will retry");
                    continue;
                }
            };
            let text = format!("{}\n{}", row.title, row.body);
            match consensus.re_embed(proposal, &text).await {
                Ok(cluster) => {
                    done += 1;
                    tracing::info!(%proposal, ?cluster, "re-embedded stub-era proposal");
                }
                Err(err) => tracing::error!(%proposal, ?err, "re-embed failed"),
            }
        }
        tracing::info!(done, "re-embed backlog tick");
    }
}

/// 0.29 — escada de avisos do "AR digital do silêncio": SLA `pending`
/// dentro do prazo, com 1–2 recibos e o último há mais de 24h, recebe um
/// lembrete (D+1, depois D+2) — e cada reenvio vira recibo hash-encadeado
/// via [`crate::notification_receipts::record`]. Para quando o gabinete
/// responde (status muda), quando o SLA vence, ou na 3ª tentativa.
async fn notification_escalation_loop(state: AppState) {
    let escalation_ms = env_ms("WORKER_ESCALATION_MS", DEFAULT_ESCALATION_MS);
    let mut ticker = interval(Duration::from_millis(escalation_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        type Due = (
            uuid::Uuid,
            String,
            Option<uuid::Uuid>,
            String,
            String,
            i64,
            uuid::Uuid,
        );
        let due: Vec<Due> = match sqlx::query_as(
            r"SELECT p.id,
                     p.title,
                     s.mandate_id,
                     m.public_email,
                     COALESCE(m.display_name, 'gabinete'),
                     (SELECT max(r.attempt)::bigint FROM notification_receipt r
                       WHERE r.proposal_id = p.id),
                     s.id
                FROM consequence_sla s
                JOIN proposal p ON p.id = s.proposal_id
                JOIN mandate m ON m.id = s.mandate_id
               WHERE s.status = 'pending'
                 AND now() < s.due_at
                 AND (SELECT count(*) FROM notification_receipt r
                       WHERE r.proposal_id = p.id) BETWEEN 1 AND 2
                 AND (SELECT max(r.sent_at) FROM notification_receipt r
                       WHERE r.proposal_id = p.id) < now() - interval '24 hours'
               LIMIT 20",
        )
        .fetch_all(&state.db)
        .await
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::error!(?err, "escalation: due query failed");
                continue;
            }
        };
        if due.is_empty() {
            continue;
        }
        let smtp = crate::proposal_delivery::smtp_from_env();
        let public_origin = std::env::var("PUBLIC_ORIGIN")
            .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
        for (proposal_id, title, mandate_id, email, display_name, last_attempt, sla_id) in due {
            let attempt = last_attempt + 1;
            // Reply-to-respond (0.30): link assinado responde SEM conta —
            // atrito zero pro gabinete. Sem RESPOND_LINK_SECRET, cai no
            // link da proposta (que exige operador logado).
            let origin = public_origin.trim_end_matches('/');
            let respond_url = match crate::respond_link::respond_token(sla_id) {
                Some(token) => format!("{origin}/responder/?sla={sla_id}&t={token}"),
                None => format!("{origin}/propostas/{proposal_id}"),
            };
            let proposal_url = format!("{origin}/propostas/{proposal_id}");
            // Template editável pelo admin (0.32.0); fallback = texto original.
            let mut ctx: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
            ctx.insert("mandate_name", display_name.clone());
            ctx.insert("proposal_title", title.clone());
            ctx.insert("attempt", attempt.to_string());
            ctx.insert("respond_url", respond_url.clone());
            ctx.insert("proposal_url", proposal_url.clone());
            let (subject, body) =
                crate::email_templates::render(&state.db, "sla_reminder_mandate", &ctx)
                    .await
                    .unwrap_or_else(|| {
                        (
                    format!("[Lembrete {attempt}/3] Demanda cidadã aguardando resposta — {title}"),
                    format!(
                        "Prezado(a) {display_name},\n\nA demanda cidadã \"{title}\" segue \
                                 aguardando resposta do gabinete. Este é o {attempt}º aviso; cada \
                                 aviso fica registrado publicamente com recibo verificável.\n\n\
                                 Responder agora (sem cadastro): {respond_url}\n\n\
                                 Ver a demanda: {proposal_url}\n\n— DemocraciaBR",
                    ),
                )
                    });
            let outcome = match &smtp {
                Some(cfg) => {
                    match crate::proposal_delivery::send_email(cfg, &email, &subject, &body).await {
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
                &state.db,
                proposal_id,
                mandate_id,
                &email,
                &subject,
                &outcome,
            )
            .await;
        }
    }
}

/// SOCRATES v2 (0671): a cada `SOCRATES_SWEEP_MS` (default 6 h) descobre as
/// Ideias Legislativas em alta no e-Cidadania, espelha as novas como tópicos do
/// fórum `senado` (até `SOCRATES_SWEEP_MAX` por rodada) e re-sincroniza o
/// contador de apoios das já espelhadas. Só roda com `SOCRATES_SWEEP_ENABLED=true`.
///
/// Rodada com erro é LOGADA e retentada no próximo tick — o portal do Senado é
/// de terceiros e cair não pode derrubar o worker. O log durável fica em
/// `socrates_sweep_run` (o painel admin lê de lá).
async fn socrates_sweep_loop(state: AppState) {
    let period_ms = env_ms("SOCRATES_SWEEP_MS", DEFAULT_SOCRATES_SWEEP_MS);
    let mut ticker = interval(Duration::from_millis(period_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tracing::info!(period_ms, "socrates sweep ligado");
    loop {
        ticker.tick().await;
        match crate::socrates_mirror::sweep_once(&state).await {
            Ok(stats) => tracing::info!(
                found = stats.found,
                mirrored = stats.mirrored,
                skipped = stats.skipped,
                updated = stats.updated,
                errors = stats.errors.len(),
                "socrates sweep tick"
            ),
            Err(err) => {
                tracing::warn!(error = %err, "socrates sweep falhou; retenta no próximo tick");
            }
        }
    }
}

/// Percorre todas as contas com preferência `auto_delete_notes_older_than_days`
/// setada e marca `deleted_at = now()` nas notas próprias que passam do prazo.
/// Idempotente; uma nota já marcada como deleted não é remexida.
async fn auto_delete_notes_loop(state: AppState) {
    let mut ticker = interval(Duration::from_secs(60 * 60)); // 1h
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let res = sqlx::query(
            r"UPDATE federation_outbox_entry oe
                 SET deleted_at = now()
                FROM citizen c
               WHERE c.id = oe.citizen_id
                 AND c.auto_delete_notes_older_than_days IS NOT NULL
                 AND oe.deleted_at IS NULL
                 AND oe.created_at < now() - make_interval(days => c.auto_delete_notes_older_than_days)",
        )
        .execute(&state.db)
        .await;
        match res {
            Ok(r) if r.rows_affected() > 0 => {
                tracing::info!(deleted = r.rows_affected(), "auto_delete_notes tick");
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(error = ?err, "auto_delete_notes falhou"),
        }
    }
}

/// Loop de cleanup do `auth_pending_signup` + `auth_login_attempt`. Faz
/// dois DELETEs por tick (barato, indexed); nunca falha o processo.
/// Reenvia a entrega de propostas cujo e-mail não confirmou (`notified_*_at` NULL).
/// Ver [`crate::proposal_delivery::ProposalDeliverySub::sweep_undelivered`].
async fn proposal_delivery_retry_loop(
    sub: crate::proposal_delivery::ProposalDeliverySub,
    period_ms: u64,
) {
    let mut ticker = interval(Duration::from_millis(period_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        ticker.tick().await;
        sub.sweep_undelivered().await;
    }
}

async fn signup_cleanup_loop(state: AppState, period_ms: u64, cutoff_days: i64) {
    let mut ticker = interval(Duration::from_millis(period_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let svc = dsoc_auth::signup_verify::SignupVerifyService::from_state(&state);
    loop {
        ticker.tick().await;
        match svc.cleanup_expired(cutoff_days).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(deleted = n, cutoff_days, "signup_verify cleanup"),
            Err(err) => tracing::warn!(error = ?err, "signup_verify cleanup falhou"),
        }
        // login_attempt: TTL curto (mesmo cutoff) — só interessa pra rate + audit.
        match dsoc_auth::signup_verify::SignupVerifyService::cleanup_login_attempts_via(
            &state,
            cutoff_days,
        )
        .await
        {
            Ok(0) => {}
            Ok(n) => tracing::info!(deleted = n, cutoff_days, "login_attempt cleanup"),
            Err(err) => tracing::warn!(error = ?err, "login_attempt cleanup falhou"),
        }
    }
}

/// Drain one subscription forever, polling on its interval. Each tick drains the topic in batches
/// until empty; a handler/storage error is logged and the delivery is retried on the next tick (the
/// cursor only advances past fully-handled deliveries).
async fn dispatch_loop(queue: EventQueue, subscription: Subscription, period_ms: u64) {
    let mut ticker = interval(Duration::from_millis(period_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        loop {
            match dispatch_batch(
                &queue,
                &subscription.name,
                subscription.topic,
                POLL_LIMIT,
                subscription.handler.as_ref(),
            )
            .await
            {
                Ok(0) => break,
                Ok(n) => {
                    tracing::debug!(
                        subscriber = %subscription.name.as_str(),
                        handled = n,
                        "dispatched batch"
                    );
                    if (n as u32) < POLL_LIMIT {
                        break;
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        subscriber = %subscription.name.as_str(),
                        error = %err,
                        "dispatch batch failed; retrying next tick"
                    );
                    break;
                }
            }
        }
    }
}

/// Drain `federation_delivery` rows that are due (ADR-0010 W2.5). On every tick:
///   1. Claim up to `DELIVERY_BATCH` pending rows (`FOR UPDATE SKIP LOCKED`, so future
///      multi-worker setups don't race for the same shipment).
///   2. For each, sign the body with the author's private PEM and POST to the recipient inbox.
///   3. 2xx → mark delivered (the row becomes inert). Non-2xx / network error → schedule the
///      next attempt with exponential backoff; after `DELIVERY_MAX_ATTEMPTS` the worker stops
///      pulling and the row becomes a permanent audit trail.
///
/// Concurrent within a batch: each delivery is a fresh tokio task, so a slow remote does not
/// block the rest of the batch.
async fn federation_delivery_loop(state: AppState, period_ms: u64) {
    let svc = ProfileService::from_state(&state);
    let mut ticker = interval(Duration::from_millis(period_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let tasks = match svc.claim_deliveries(DELIVERY_BATCH).await {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!(error = %err, "delivery claim failed; retrying next tick");
                continue;
            }
        };
        if tasks.is_empty() {
            continue;
        }
        tracing::debug!(claimed = tasks.len(), "claimed delivery batch");
        // Per-task spawn keeps a slow remote from holding up the rest.
        for task in tasks {
            let svc = svc.clone();
            tokio::spawn(async move {
                if task.attempts > DELIVERY_MAX_ATTEMPTS {
                    tracing::warn!(
                        delivery = %task.delivery_id,
                        attempts = task.attempts,
                        target = %task.recipient_inbox,
                        "delivery exceeded max attempts; abandoning"
                    );
                    return;
                }
                let result = crate::federation::deliver_signed(
                    &task.actor_url,
                    &task.private_pem,
                    &task.recipient_inbox,
                    &task.payload,
                )
                .await;
                match result {
                    Ok(()) => {
                        if let Err(err) = svc.delivery_succeeded(task.delivery_id).await {
                            tracing::error!(error = %err, "mark delivery succeeded failed");
                        }
                    }
                    Err(err) => {
                        let msg = err.to_string();
                        tracing::info!(
                            delivery = %task.delivery_id,
                            attempts = task.attempts,
                            target = %task.recipient_inbox,
                            error = %msg,
                            "delivery failed; backoff scheduled"
                        );
                        if let Err(err2) = svc
                            .delivery_failed(task.delivery_id, task.attempts, &msg)
                            .await
                        {
                            tracing::error!(error = %err2, "mark delivery failed errored");
                        }
                    }
                }
            });
        }
    }
}

/// Sweep expired SLA clocks across every due tenant on a fixed interval — this is what makes "silence
/// is permanent" real in production. Time comes from the injected clock, never ambiently.
async fn sweep_loop(
    consequence: dsoc_consequence::ConsequenceService,
    clock: Arc<dyn dsoc_core::clock::Clock>,
    period_ms: u64,
) {
    let mut ticker = interval(Duration::from_millis(period_ms));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        match consequence.sweep_all_due(clock.now()).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(expired = n, "swept SLA clocks into public silence"),
            Err(err) => tracing::warn!(error = %err, "sla sweep failed; retrying next tick"),
        }
    }
}
