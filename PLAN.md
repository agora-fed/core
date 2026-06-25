# PLAN.md — Sovereign Participatory Democracy Platform

> **Codename:** `PINDORAMA` *(provisional — pending final call. Fits the sovereignty/indigenous naming line: Pindorama = pre-colonial name for the land now called Brazil.)*
> **Stewardship:** PopSolutions Cooperativa de Tecnologia
> **License:** AGPLv3 + Social Contract clause (Decidim-style democratic guarantees). CHARRUA v1.2 to be evaluated for sovereign-only components.
> **Status:** Foundational plan. Frozen sections are marked `[FROZEN]`. Everything else is open for technical challenge — but reversals must be justified (why the old approach fails, whether it can be fixed, why the new one is better).

---

## 0. How to read this document

This is the **engineering north star** for migrating Decidim (Ruby on Rails) to a Rust-native, mobile-first, sovereign platform — and for correcting Decidim's known structural failures *in the architecture itself*, not as bolt-ons.

It is written so that **a large number of autonomous agents can work in parallel** without colliding. The parallelization model is in section 6. Read sections 1–3 before writing a single line of code.

Language policy: all code, comments, commits, PR descriptions, and this documentation are in **English**. End-user UI and civic content are in **Portuguese** (and later other LATAM languages).

---

## 1. Long-term objective (North Star)

Build a **sovereign, scalable, mobile-first participatory infrastructure** that connects the entire Brazilian political chain — vereadores, prefeitos, deputados (estaduais e federais), senadores, governadores, a Presidência da República, **and the candidates for every one of those offices** — directly to the population.

The platform's defining thesis, and the reason it differs from 20 years of failed civic-tech:

> **Participation without consequence is theater.** The platform exists to convert citizen demand into *visible, time-bound, public accountability* that an elected official cannot silently ignore.

Concretely, at maturity the platform must:

1. Let citizens **propose, debate, and vote** on ideas directed at a specific mandate or campaign.
2. **Automatically cluster** similar proposals into genuine consensus signals (not a noisy backlog).
3. **Force a response loop**: the official is notified, has a public SLA to respond, and the platform publicly records silence.
4. Maintain a **public scorecard** per politician: promises vs. delivery, proposals answered vs. ignored.
5. Reach citizens **where they are** — native mobile with push notifications, not a web bubble.
6. Survive **coordinated pressure** (legal, infrastructural, political) through federation and geographic resilience.
7. Remain **auditable, sovereign, and self-hostable** end to end.

This is critical democratic infrastructure, not a SaaS product. Architecture decisions are judged against that bar.

---

## 2. Guiding principles (diretrizes) `[FROZEN]`

These are non-negotiable and bind every agent and every crate.

1. **Process over result. Path over destination. Preserve history.** If the process is broken, fix the process — never work around it. Keep a real CHANGELOG; credit Decidim concepts we port.
2. **Contract-first.** Domain types, database schema, event contracts, and the public API are designed and frozen *before* parallel implementation begins.
3. **Explicit over magic.** No ORM metaprogramming, no hidden callbacks. Use `sqlx` with explicit, auditable SQL. This is a sovereignty/auditability requirement, not a style preference.
4. **IPv6-first.** Every config, example, bind address, and service discovery defaults to IPv6. IPv4 only as explicit fallback.
5. **Deployment platform.** *Originally `[FROZEN]` as "No Docker / Proxmox LXC + TurnKey + systemd". Superseded by ADR-0002 (Kubernetes + Helm) per principle 12 — see `docs/decisions/ADR-0002-kubernetes-helm.md` for the justified reversal.*
6. **PostgreSQL + pgvector is the database.** Single-node Postgres through Phase 2. CockroachDB (or sharding) only in Phase 3, only for the transactional core, only when the must-not-be-taken-down threat model outweighs operational simplicity. Analytical/vector workloads stay on Postgres+pgvector regardless.
7. **Sovereign auth.** Identity via the self-hosted Zitadel (OIDC). No dependency on foreign identity providers.
8. **Secrets only in `.config/settings.env` (gitignored). Never hardcoded.** Ever.
9. **Mobile is a first-class client, not an afterthought.** Web and native mobile are peers consuming the same contract.
10. **Consequence is a core subsystem, not a feature.** Accountability loops are built into the domain model from day one (see section 4).
11. **Moderation must be auditable.** Rules + statistics + optional *local* models. No opaque third-party black-box classifiers.
12. **Justify every reversal of technical direction.** State (a) why the previous approach fails, (b) whether it can be salvaged, (c) why the new one is better.

---

## 3. DO NOT — explicit anti-scope and anti-patterns `[FROZEN]`

**Migration discipline**
- **DO NOT** attempt a big-bang rewrite. Migrate by strangler-fig (section 8).
- **DO NOT** port Ruby idioms 1:1. STI, `ActiveRecord` callbacks, polymorphic magic, and runtime metaprogramming must be **re-architected**, not translated.
- **DO NOT** begin Tier-2/Tier-3 work before the Tier-0 contracts are `[FROZEN]`.

**Architecture**
- **DO NOT** let more than one agent own the `core` / contract crate.
- **DO NOT** import another component crate's internals. Crates talk only through `core` traits, the event bus, and the public API.
- **DO NOT** introduce an ORM or query builder that hides SQL. `sqlx` checked queries only.
- **DO NOT** add CockroachDB, sharding, Kafka, or service mesh in the MVP.
- **DO NOT** default any socket, example, or doc to IPv4.

**Product / scope traps**
- **DO NOT** rebuild "all of Decidim." Port the consequence-and-accountability thesis first.
- **DO NOT** build cryptographic, election-grade binding voting in the MVP. This is for *pressure and accountability*, not for replacing the TSE ballot.
- **DO NOT** attempt to integrate every legacy government system in Phase 1.
- **DO NOT** moderate civic speech with opaque foreign AI.
- **DO NOT** ship a generic "everything for everyone" UI. Hyperspecialize the core loop: *propose → cluster → vote → notify official → response-or-public-silence*.

**Operations / security**
- **DO NOT** hardcode credentials, tokens, or keys. `.config/settings.env`, gitignored, always.
- **DO NOT** store individual vote-to-citizen linkage in a way the official can query. Officials see aggregates; individual linkage stays minimized/protected (LGPD + future zero-knowledge track).
- **DO NOT** create a single point of takedown.

---

## 4. Decidim's structural failures → built-in corrections `[FROZEN]`

| # | Decidim failure | Root cause | Correction | Owning subsystem |
|---|---|---|---|---|
| 1 | Officials ignore proposals | No consequence | **Consequence Engine**: SLA timers; auto-publish "ignored"; public pressure surface | `consequence` (NEW) |
| 2 | Proposals drown in noise | No semantic clustering | **Consensus Engine**: pgvector dedupe/cluster into one signal | `consensus` (NEW) |
| 3 | Moderation doesn't scale | Manual-only | Rules + statistical detection + optional local model; auditable | `moderation` |
| 4 | Confusing generic UI | "Everything for everyone" | Hyperspecialized civic loop; one primary action per screen | `web` / `mobile` |
| 5 | Citizens never see it | Web-only | **Mobile-first + push**; WhatsApp/Chatwoot reach | `notify` + `mobile` |
| 6 | No link to real power | Voluntary participation | **Mandate Registry**: mandatory onboarding via public email | `mandates` (NEW) |
| 7 | No accountability artifact | No persistent record | **Scorecard**: public promise-vs-delivery, answered-vs-ignored | `scorecard` (NEW) |
| 8 | Pretends to be neutral | Indefensible | Explicit thesis: the platform *is* an accountability instrument | governance / docs |
| 9 | Slow & RAM-heavy | Rails runtime | Rust (Axum + Tokio), explicit SQL, low memory | whole stack |

The four NEW subsystems (`consequence`, `consensus`, `mandates`, `scorecard`) are what make this *not* "Decidim in Rust." They are the point.

---

## 5. Target architecture

### 5.1 Stack `[FROZEN for MVP]`
- **Language:** Rust (stable). **HTTP:** Axum + Tokio.
- **DB access:** `sqlx` (compile-time checked) against PostgreSQL 16+ with `pgvector`.
- **Async jobs / event bus:** Postgres-backed queue (pgmq) for Phases 1–2. NATS only if Phase 3 demands it.
- **Auth:** Zitadel (OIDC), self-hosted. **Cache / real-time:** Redis.
- **Embeddings:** local model served on the cluster (no external API for civic content).
- **Web client:** SPA. **Mobile client:** Flutter (native push), separate repo `git.pop.coop/brasil/democracia-social-app`.
- **Deploy:** Kubernetes + Helm (ADR-0002), IPv6-first, behind HAProxy/ingress.

### 5.2 Workspace decomposition `[FROZEN]`

A **Cargo workspace of independent crates**. The crate boundary is the agent boundary.

```
crates/
├── core/          # [TIER 0] domain primitives, IDs, errors, traits, event contracts
├── db/            # [TIER 0] migrations, schema, sqlx setup, query conventions
├── api-contract/  # [TIER 0] OpenAPI spec + shared DTOs
├── platform/      # [TIER 1] auth, notify, events, consensus, moderation, admin
├── spaces/        # [TIER 2] processes, assemblies, initiatives, consultations, mandates
├── components/    # [TIER 2] proposals, votes, comments, debates, meetings,
│                  #          budgets, surveys, accountability, consequence, scorecard
├── gateway/       # [TIER 1] Axum router; the public API surface
└── clients/       # [TIER 3] web, mobile, federation
```

**Rule:** a crate exposes a service trait (from `core`) and HTTP handlers; it owns its tables; it never reaches into another crate's internals. Cross-crate effects happen via events or the gateway.

### 5.3 Contract-first methodology `[FROZEN]`
`core`, `db`, and `api-contract` are **frozen** before any Tier-1+ work. Changing them after freeze requires an RFC.

### 5.4 New core subsystems (the differentiators)
- **`mandates`** — ingests public official/candidate directories; binds each to a public email; manages verification levels.
- **`consensus`** — embeds each proposal (pgvector); finds near-duplicates; surfaces real consensus.
- **`consequence`** — SLA clock on threshold-crossing proposals; notifies officials; records answered/acted/ignored.
- **`scorecard`** — persistent, public, per-politician accountability record.

---

## 6. Parallelization strategy `[FROZEN]`

### 6.1 Dependency tiers
- **Tier 0 — sequential, single owner, FREEZE first.** `core`, `db`, `api-contract`.
- **Tier 1 — fans out after Tier 0 freeze.** `auth`, `notify`, `events`, `consensus`, `moderation`, `admin`, `gateway`.
- **Tier 2 — fans out massively.** Every space/component crate = one agent each.
- **Tier 3 — fans out after `api-contract` frozen.** `web`, `mobile`, `federation` (against mocks).

### 6.2 Agent ownership model
- **One crate = one owner agent.** Each ships domain model, `sqlx` queries, service impl, HTTP handlers, **and its own tests**, plus a `CRATE.md`.

### 6.3 Stub / mock strategy
Tier-2/3 agents code against trait mocks from `core`; they never block on another agent.

### 6.4 CI / coordination guardrails `[FROZEN]`
- `cargo fmt` + `clippy -D warnings` required to merge.
- `sqlx` query checks against a real Postgres CI runner.
- Per-crate coverage gate. No cross-component path dependencies.
- Conventional Commits, English only. Trunk-based, short-lived branches per crate.

---

## 7. Roadmap (engineering)

- **Phase 0 — Foundations & Freeze.** Tier 0 frozen; CI live; empty gateway boots over IPv6 and authenticates via Zitadel.
- **Phase 1 — Core accountability loop / MVP.** Tier 1 + thesis-path Tier 2 (`mandates`, `proposals`, `votes`, `comments`, `consequence`, `scorecard`) + minimal web/mobile. **Gate:** an official is onboarded by public email; a clustered proposal crosses threshold, starts an SLA, notifies on mobile, and publicly records silence.
- **Phase 2 — Component breadth & regional scale.** Remaining components/spaces; harden moderation/consensus; embeddable scorecard.
- **Phase 3 — National scale, resilience & federation.** Evaluate CockroachDB for the transactional core only; multi-region; federation SDK + hub; external security audit; zero-knowledge vote-privacy track.

---

## 8. Migration methodology: strangler-fig, not big bang `[FROZEN]`

Stand up the Rust gateway + core contract as the new front door; implement one component crate at a time behind it; port good Decidim *concepts* re-architected (credited in CHANGELOG); never run Ruby in the hot path long-term.

---

## 9. Design principles — PLACEHOLDER (to be filled by Marcos)

- **Mobile-first, push-native.** The phone is the primary surface; web is secondary.
- **Hyperspecialized civic loop**, one clear primary action per screen.
- **The SLA clock and the scorecard are the emotional core** of the UI — consequence must be *felt*, visible, shareable.
- Visual identity, palette, typography, component library, accessibility targets → **[Marcos to define].**

---

## 10. Open decisions pending
- [ ] Final codename (Pindorama provisional).
- [ ] License split: AGPLv3 baseline vs. CHARRUA v1.2 for sovereign-only crates.
- [x] Mobile: **Flutter** (separate repo, iOS + Android native).
- [ ] Web SPA framework.
- [ ] Embedding model choice for `consensus` (local).
- [ ] Identity verification levels in `mandates` (email-only → TSE-backed).
- [x] Deployment/ops platform: **Kubernetes + Helm** (ADR-0002).

---

*Path over destination. Process over result. Preserve history. Fix the process, never work around it.*
