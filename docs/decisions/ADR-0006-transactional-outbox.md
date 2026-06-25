# ADR-0006 — Transactional outbox for event emission

- **Status:** Accepted · **Context:** Wave-1 review found a dual-write hazard.

## Problem

Crates were emitting events by committing the domain transaction and then calling
`EventBus::publish` on a separate connection. If publish fails after the commit, the domain change is
permanently persisted but the event is lost — and a UNIQUE constraint on the domain row makes a retry
return `Conflict`, permanently suppressing the event. For the consequence/scorecard loop this could
silently drop an SLA-started or public-silence signal — unacceptable for an accountability platform.

## Decision

Emit events from domain mutations via a **transactional outbox**: write the event row into
`events_log` **inside the same transaction** as the domain change, using
`dsoc_db::outbox::publish_tx(&mut *tx, &envelope)`. The domain change and the event commit atomically.
The `dsoc-events` dispatcher delivers from `events_log` (at-least-once; consumers must be idempotent).

The `dsoc_core::EventBus` port remains for fire-and-forget emission **outside** a domain transaction
(e.g. from an event handler). Any state change that emits an event MUST use the outbox.

## Consequences

- `dsoc-db` gains `outbox::publish_tx` (tested against real Postgres).
- CONTRIBUTING updated; crate-owner agents adopt the outbox for emitting from mutations.
- Consumers must dedupe by the originating id (idempotency), which the thesis crates already require.
