# ADR-0008 — Sovereign credential auth: e-mail + senha + CPF (reverses Zitadel for citizens)

- **Status:** Accepted · **Supersedes (partially):** PLAN.md principle 7 ("Sovereign auth via Zitadel
  OIDC") for the **citizen** path. Justified per principle 12.

## (a) Why the previous approach fails
Zitadel is a generic OIDC IdP; it does **not** verify a Brazilian **CPF**, which is the identity
anchor the product requires ("a autenticação tem que ser verificada por CPF"). Routing every citizen
through an external IdP also adds a dependency for the most sensitive, highest-volume flow.

## (b) Can it be salvaged?
Partially: Zitadel/OIDC remains available (dormant, env-gated) and may be used for **staff/admin SSO**.
But the citizen identity is now self-hosted and CPF-anchored.

## (c) Why the new approach is better
Self-hosted **e-mail + senha (Argon2id) + CPF** is more sovereign (no foreign IdP in the citizen hot
path) and lets us anchor identity on CPF. CPF is **validated algorithmically** (check digits — offline,
free) now; a pluggable `CpfVerifier` allows confirming against an official source (Serpro Datavalid /
KYC) later to reach the `verified` assurance, without changing the contract.

## Decision
- `dsoc-auth` gains `register(org, email, password, cpf)` and `login(org, email, password)` →
  `IssuedSession`. New endpoints `POST /auth/register`, `POST /auth/login` (alongside the existing
  `/auth/me`). Passwords are **Argon2id** PHC strings (never plaintext). CPF is normalized (11 digits),
  unique per org, with a `cpf_status` of `validated`/`verified`.
- `CpfVerifier` trait + `AlgorithmicCpfVerifier` (returns `validated`); a future Serpro/KYC impl returns
  `verified` and raises the citizen to `Strong`.
- Migration `0101_auth_credentials.sql`: `citizen.oidc_subject` becomes nullable; new `auth_credential`
  table (FK only to core `citizen`/`org`).

## Consequences
- `argon2` added as a dependency. The `Authorization` port (require/level) is unchanged — it reads the
  DB, so it works regardless of the auth method.
- Real anti-fraud CPF verification (Serpro/KYC) is a follow-up needing a provider + credentials.
