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
    let sub = |name: &str, topic: EventTopic, handler: Arc<dyn EventHandler>| match SubscriberName::parse(name) {
        Ok(name) => Some(Subscription { name, topic, handler }),
        Err(err) => {
            tracing::error!(subscriber = name, error = %err, "invalid subscriber name; skipping");
            None
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
            }),
        ),
        sub(
            "civic-notify-consequence-worker",
            EventTopic::Consequence,
            Arc::new(crate::civic_notify::CivicNotifySub {
                db: state.db.clone(),
                public_origin: std::env::var("PUBLIC_ORIGIN")
                    .unwrap_or_else(|_| "https://democracia.social.br".to_owned()),
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
    tokio::spawn(signup_cleanup_loop(state.clone(), signup_cleanup_ms, signup_cleanup_days));

    tracing::info!(
        subscriptions = count,
        dispatch_ms,
        sweep_ms,
        delivery_ms,
        signup_cleanup_ms,
        "event worker started: consequence loop is live"
    );
}

/// Loop de cleanup do `auth_pending_signup` + `auth_login_attempt`. Faz
/// dois DELETEs por tick (barato, indexed); nunca falha o processo.
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
            &state, cutoff_days,
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
    use dsoc_auth::profile::ProfileService;
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
