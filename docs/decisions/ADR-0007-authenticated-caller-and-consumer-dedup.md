# ADR-0007 — Authenticated caller + consumer idempotency

- **Status:** Accepted · **Context:** Wave-1/Wave-2 reviews repeatedly found two defect classes.

## Problem 1 — trusting `citizen_id` from the request body

Several handlers read `citizen_id` from the JSON body and passed it to `Authorization::require`,
which checks whether *that* id has a level — not whether the **caller** is that id. An authenticated
attacker could act as any citizen (vote/notify/propose/respond as someone else).

## Decision 1 — `dsoc_app::CallerId` extractor

Handlers take the acting identity from the `CallerId` Axum extractor (the verified caller from the
gateway's OIDC middleware; until that lands, from the trusted gateway-set `x-dsoc-citizen-id` /
`x-dsoc-org-id` headers that the public ingress strips). **Never** take the acting citizen from the
body. `authz.require(caller.org, caller.citizen, level)` uses the caller, not body input.

## Problem 2 — consumer idempotency under at-least-once delivery

ADR-0006 requires idempotent consumers. Some used only state-based guards.

## Decision 2 — `dsoc_db::consumed::claim_consumed`

A general dedup ledger `events_consumed(consumer, event_id)`; `claim_consumed(tx, consumer, id)`
returns whether this delivery is the first. Use it inside the effect transaction. Naturally-idempotent
state guards (e.g. `WHERE threshold_crossed_at IS NULL`, monotonic `max`) remain acceptable; this is
the general mechanism for everything else.

## Consequences

- `dsoc-app` gains the extractor (+axum dep); `dsoc-db` gains `consumed` + migration `0002`.
- Handlers stop trusting body identity; consumers can claim events explicitly.
