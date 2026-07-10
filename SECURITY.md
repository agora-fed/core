# Security Policy

This is **critical democratic infrastructure** under coordinated political/legal/infrastructural
pressure. Security is a first-class, non-negotiable concern (PLAN.md principles 8, 11; section 3).

## Reporting a vulnerability

Report privately via the contact form at
<https://democracia.social.br/contato/?setor=seguranca>. Do **not** open a public issue.
We acknowledge within 72 hours and aim to remediate critical issues before public disclosure.

## Hard rules (enforced in CI)

- **No hardcoded secrets.** Secrets live only in `.config/settings.env` (gitignored). CI runs a
  secret scan and a `cargo-deny` supply-chain audit on every push.
- **No `unsafe` Rust.** `unsafe_code = "forbid"` workspace-wide.
- **Explicit SQL only.** `sqlx` compile-time-checked queries; no string-concatenated SQL.
- **Vote privacy.** Individual vote-to-citizen linkage is minimized and never queryable by an
  official; officials see aggregates only (LGPD; future zero-knowledge track).
- **Auditable moderation.** No opaque third-party classifiers decide civic-speech visibility.

## Threat model

See [docs/ops/DEPLOYMENT.md](./docs/ops/DEPLOYMENT.md) — "no single point of takedown": multi-region
intent, redundant DNS/ingress, append-only audit log (Phase 3).
