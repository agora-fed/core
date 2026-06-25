# CI/CD — the audit instrument

> CI/CD is **the main object of reliability and auditability** for this project (explicit project
> requirement). Because the platform carries political weight, every merge to `main` must be
> reproducible, attributable, and provable. The pipeline — not human goodwill — is the gate.

Pipelines run on **Forgejo Actions** (workflows in `.forgejo/workflows/`). They are
GitHub-Actions-compatible YAML, executed by a self-hosted Forgejo runner in the sovereign park.

## Workflows

| File | Trigger | Purpose (gate) |
|------|---------|----------------|
| `ci.yml` | push / PR | fmt, `clippy -D warnings`, build, **tests against real PostgreSQL+pgvector**, coverage ≥ 80%, crate-boundary check |
| `security.yml` | push / PR / weekly | `cargo-deny` (advisories + licenses + sources), `cargo audit`, secret scan |
| `helm.yml` | changes under `deploy/helm/**` | `helm lint` + `helm template` + `kubeconform` schema validation |
| `release.yml` | tag `v*` | build container images, sign, push to sovereign registry, package Helm chart |

## The reliability contract

1. **Real database, not containers-as-toys.** Tests run against an actual PostgreSQL 16 service
   with `pgvector` (the `postgres` service in `ci.yml`). `sqlx` queries are verified against the
   committed `.sqlx/` offline cache and re-checked live.
2. **Deny-by-default.** `clippy -D warnings`, `cargo-deny`, and the boundary check are hard
   failures, not advisories.
3. **No green-by-skipping.** Skipped/ignored tests fail the coverage gate. The E2E thesis test
   (`tests/e2e/core_loop.rs`) is required for release.
4. **Provenance.** Release images are signed; the chart records the exact commit. Every deploy is
   traceable to a reviewed, tested commit — the audit trail a politically targeted platform needs.
5. **English + Conventional Commits** are linted, keeping history machine-auditable.

## Pipeline as evidence

The CI run is the public evidence that a given version was built and tested as claimed. Logs and
coverage reports are retained as artifacts. This is deliberate: in a contested political
environment, "the tests passed" must be independently verifiable, not asserted.

## Local parity

`scripts/ci-local.sh` runs the same gates locally so contributors reproduce CI before pushing.
