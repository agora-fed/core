# ADR-0003 — IPv6-first networking

- **Status:** Accepted · **Context:** PLAN.md principle 4.

## Decision

Every bind address, config default, example, Service, Ingress, and health probe defaults to IPv6
(`[::1]` / `::`). IPv4 is configured only as an explicit, documented fallback.

## Rationale

The sovereign park is IPv6-native (the production VM is reachable only over IPv6:
`2804:710:d0:9::a000`; Forgejo at `2804:710:d0:5::20`). Defaulting to IPv6 avoids dual-stack drift
and matches the real network.

## Consequences

- `DATABASE_URL`, `REDIS_URL`, SMTP, and embeddings URLs in `settings.env.example` use `[::1]`.
- Helm values and probes use IPv6; load tests and docs follow suit.
