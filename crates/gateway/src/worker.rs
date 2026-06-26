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

    tracing::info!(
        subscriptions = count,
        dispatch_ms,
        sweep_ms,
        "event worker started: consequence loop is live"
    );
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
