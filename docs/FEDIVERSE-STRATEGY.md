# AGORA on the Fediverse — current state, gaps, and strategy

> **Scope:** technical diagnosis of the "Fediverse + Decidim" hybrid AGORA already is,
> the concrete gaps against the relevant FEPs, and a wave-based plan forward.
> **Sources:** code reading of `crates/` (2026-08-05) + Fediverse knowledge base
> (FEP-1b12, FEP-67ff, FEP-9fde, FEP-6481, FEP-ef61, relays, NodeInfo).
> **Normative context:** ADR-0005 (ActivityPub), ADR-0010 (civic-social platform),
> ADR-0011 (modules), ADR-0013 (AGORA = framework / Pindorama = installation / English API),
> ADR-0015 (`l10n_<cc>` layer), ADR-0019 (karma).

---

## 1. The thesis: what AGORA already is (and nobody else is)

Three families of democratic software exist today, and none does what the others do:

| Family | Examples | Has | Lacks |
|---|---|---|---|
| **Institutional participation** | Decidim, Consul, CitizenLab | Spaces × Components, processes, participatory budgeting | Federation (zero), social network, consequence |
| **Federated forum** | Lemmy, PieFed, Mbin, NodeBB | `Group` FEP-1b12, threads, federated moderation | Structured deliberation, mandates, accountability |
| **Federated microblog** | Mastodon, Misskey, Friendica | Social graph, feed, client API, reach | Deliberation, decision, consequence |

AGORA is the **intersection of all three, plus a fourth layer that exists nowhere
else**: the *consequence loop* (proposal → threshold → SLA against a mandate → public
silence / answer → scorecard). That fourth layer is the strategic differentiator and is
exactly what has **no Fediverse standard yet**. That is a leadership opportunity, not
debt — see §7.

```
                    ┌─────────────────────────────────────────────┐
                    │            AGORA (framework)                │
                    ├─────────────────────────────────────────────┤
  Mastodon-like  ─▶ │  FEED      note · Question · Like · Boost    │ ← federated ✅
  Lemmy-like     ─▶ │  FORUM     forum · forum_topic · arguments   │ ← federated ⚠️ (outbound only)
  Decidim-like   ─▶ │  SPACES ×  processes/assemblies/consultations│ ← NOT federated ❌
                    │  COMPONENTS proposals/budgets/meetings/...   │
  (AGORA only)   ─▶ │  CONSEQUENCE  mandate · SLA · scorecard      │ ← NOT federated ❌
                    └─────────────────────────────────────────────┘
```

---

## 2. Current architecture — the map

### 2.1 Tiers (docs/ARCHITECTURE.md §1)

```
Tier 0  core ─ db ─ api-contract
Tier 1  platform/{auth,notify,events,consensus,moderation,admin,storage,l10n-br} + gateway
Tier 2  spaces/{processes,assemblies,consultations,initiatives,mandates}
        components/{proposals,votes,comments,forums,budgets,surveys,meetings,
                    accountability,consequence,scorecard}
Tier 3  clients/federation   (+ web, mobile)
```

This **is** Decidim's model (*Participatory Spaces* × *Components*) expressed as Rust
crates with CI-enforced boundaries (`scripts/check-crate-boundaries.sh`). It is a real
asset and must be preserved — every proposal below keeps it.

### 2.2 Actual code size

| Crate | LOC | Note |
|---|---:|---|
| `gateway` | **41,369** | ⚠️ monolith — see §5.1 |
| `platform/auth` | 8,292 | |
| `spaces/mandates` | 3,240 | includes `parties.rs` |
| `components/forums` | 3,186 | |
| `platform/consensus` | 2,667 | pgvector / clustering |
| `clients/federation` | **2,348** | ⚠️ pure builders only — see §5.1 |
| `components/scorecard` | 2,221 | |
| `components/proposals` | 2,131 | |
| … | | 128 migrations |

Inside the gateway, the federation files:

| File | LOC | What it is |
|---|---:|---|
| `federation.rs` | 4,147 | **the real AP implementation** (Person, inbox, Follow/Accept, Like/Announce, signed delivery) |
| `mastodon_api.rs` | 1,976 | **Mastodon Client API** (`/api/v1/*`, `/oauth/*`) — Ivory/Tusky/Elk work |
| `socrates_mirror.rs` | 1,862 | idea mirror |
| `social_graph.rs` | 1,657 | social graph |
| `federation_feed.rs` | 987 | timeline |
| `fediverso_admin.rs` | 462 | instance admin |
| `forum_federation.rs` | **427** | **`Group` FEP-1b12 — partial** |
| `mastodon_dto.rs` / `mastodon_oauth.rs` | 923 | Mastodon translation |

---

## 3. What ALREADY works on the Fediverse (honest inventory)

### 3.1 Person / microblog layer — mature ✅

This is a genuine Mastodon-compatible implementation, not a skeleton:

- **Discovery:** WebFinger (`/.well-known/webfinger`), NodeInfo 2.1
  (`/.well-known/nodeinfo`), `/actors/instance` (instance actor).
- **Actor:** `Person` with `publicKey`, absolutized `icon`/`image`, HTML `summary`,
  human `url`, lazy RSA key generation.
- **Signed inbox:** `Signature` header parse → fetch signer actor → canonical-string
  verification → **insert-before-act** idempotency log → dispatch.
- **Activities:** `Follow`/`Accept`, `Undo{Follow}`, `Create{Note}`, bare `Note`
  `Create`, `Like`, `Announce`, `Undo{Like|Announce}`, `Delete`, `Update`.
- **Objects:** `Note` with hashtags, WebFinger-resolved mentions, media, and
  **`Question`** (polls — `oneOf`/`anyOf`, Mastodon parity, `polls.rs`).
- **Collections:** populated `outbox`, `followers` (Person).
- **Client-side:** `/federation/lookup`, `/me/follow`, `/me/bulk_follow`, `/me/feed`,
  `/me/like`, `/me/boost`, `/me/notifications`, `/timelines/tag/{name}`, `/search`,
  `/trends/hashtags`, `/directory`, `/suggestions/follow`.
- **Full Mastodon Client API** — third-party clients already work against Pindorama.
  **This is under-leveraged as a market argument.**

### 3.2 Group / forum layer — partial ⚠️

`crates/gateway/src/forum_federation.rs` implements FEP-1b12 **outbound only**:

| FEP-1b12 | Status | Where |
|---|---|---|
| `Group` actor with `publicKey` | ✅ | `group_actor_json()` |
| WebFinger resolution of forum handles | ✅ | `webfinger_jrd()`, delegated by `federation.rs` |
| Territorial reversed handle (`saude.santos.sp`) | ✅ | `handle_to_path()`/`path_to_handle()` |
| HTTP-Signature verification on the Group inbox | ✅ | `federation.rs` §4b (issue #6, v0.71.0) |
| Inbox idempotency (`forum_inbox_seen`, 0678) | ✅ | insert-before-act |
| `Follow` → `Accept` signed with the Group key | ✅ | `inbox()` |
| `Undo{Follow}` | ✅ | `inbox()` |
| `audience` on the object | ✅ | `topic_object_json()` |
| Dereferenceable object (`/actors/{h}/objects/{id}`) | ✅ | |
| `Announce` fan-out with idempotent retry | ✅ | `announce_sweep()`, `attempts < 5` |
| **Inbound `Create{Page\|Note}`** | ❌ | `inbox()` falls through `_ => ACCEPTED` — **dropped** |
| **Remote comment (`Create{Note}` + `inReplyTo`)** | ❌ | same |
| `attributedTo` collection (moderators) | ❌ | absent from the actor doc |
| Moderator `Add`/`Remove` | ❌ | |
| Content `Remove` (federated moderation) | ❌ | |
| Topic `Update`/`Delete` | ❌ | |
| `Page` type for top-level posts | ❌ | uses `Note` + `name` |
| `Announce(Create(Page))` | ❌ | sends `Announce` with `object` = **URL string** |
| Group `outbox` | ❌ | route resolves citizens → 404 for forums |
| Group `followers` | ❌ | `followers_get()` resolves citizens → 404 for forums |
| `properties.type = Group` in the JRD | ❌ | Lemmy/PieFed use it to detect groups |
| `endpoints.sharedInbox` (ours) | ❌ | we consume the remote one, never advertise ours |

**Practical consequence:** today a Pindorama forum is functionally a *Guppe* — a boost
bot. A Lemmy/Mbin/PieFed user **cannot post or comment** in an AGORA forum. The graph
comes in, content goes out, nothing comes back. This is gap #1 of this whole document.

### 3.3 Decidim layer (spaces/components) — not federated ❌

`proposals`, `budgets`, `meetings`, `surveys`, `consultations`, `assemblies`,
`initiatives`, `accountability`, `consequence`, `scorecard`: **zero ActivityPub
surface.** Mapping DTOs exist in `clients/federation/src/mapping.rs`
(`proposal_to_ap`, `sla_to_ap`, `scorecard_to_ap`, `cluster_to_ap`, `support_tally`) —
**but nothing mounts them on a route.** They are unit-tested pure builders, never served.

---

## 4. Naming: "party" in AGORA

Short answer: today it is `party`, and **that violates ADR-0013/0015** — it must be
fixed before it grows.

### 4.1 What exists today (migrations 0204 + 0673)

```sql
party              (org_id, sigla PK, name, tse_number, logo_url, founded_year, website)
party_directory    (id, org_id, party_sigla, esfera, uf, municipio, name, parent_directory_id)
party_administrator(id, org_id, party_sigla, directory_id, citizen_id, role, invited_by, accepted_at)
```

Three Brazilian-localization leaks inside what should be country-agnostic core:

1. **`party`** — "political party" is an electoral concept. Not every AGORA
   installation will have parties (a union, a cooperative, a university have *member
   organizations*).
2. **`esfera text CHECK (esfera IN ('federal','estadual','municipal'))`** — the
   Brazilian federative hierarchy **hard-coded in a CHECK constraint**. Spain has 4
   levels, France 5. ADR-0015 explicitly says this comes from the
   `TerritorialProvider`. This CHECK is the most expensive leak in the schema.
3. **`sigla` / `municipio` / `tse_number`** — pt-BR / TSE-specific fields in the contract.

### 4.2 Proposed naming

| Pindorama (pt-BR UI) | AGORA (core, EN) | ActivityPub | Note |
|---|---|---|---|
| Partido | `Organization` (`kind = "party"`) | `Group` | `kind` supplied by `l10n_<cc>` |
| Diretório (nacional/estadual/municipal) | `Chapter` | `Group` | `territorial_level: i16` |
| Sigla (PT, PSOL) | `short_name` / `slug` | `preferredUsername` | |
| Esfera | `TerritorialLevel` (from `TerritorialProvider`) | — | **not a 3-value enum** |
| Número TSE | `registry_ref` (opaque) | — | `l10n_br` reads it as the TSE number |
| Fórum institucional | `Forum` | `Group` | already exists |
| Bancada / frente parlamentar | `Caucus` | `Group` | does not exist yet |
| Mandato | `Mandate` | `Person` + `dsoc:role` | already exists |
| Grupo de campanha | `CampaignGroup` | `Group` | does **not** federate today |

**Central insight:** `Organization`, `Chapter`, `Forum`, `Caucus` and `CampaignGroup`
are *all* `Group` in ActivityPub. Today only `Forum` has an actor. A **single `Group`
registry** (§6.2) solves all five at once and is the precondition for group-scoped
voting (§6.3).

---

## 5. Structural gaps (non-federation)

### 5.1 The gateway is a monolith and federation lives in the wrong place

`crates/gateway` has **41k LOC** — more than all domain crates combined. Worse: the
*real* federation lives in `gateway/src/federation.rs` (4,147 LOC) while
`crates/clients/federation` (2,348 LOC, Tier 3) contains only pure builders and a
`routes()` that is **dead** (the gateway mounts its own `public_routes()`).

This breaks the `dsoc-federation` crate's own `CRATE.md` contract and means:
- federation cannot be tested without booting the whole gateway;
- a third-party module has no way to register its own AP objects;
- the "Tier 3 depends only on api-contract" rule was bypassed by moving the code to Tier 1.

**Federation must become Tier 1 (`platform/federation`), with state and DB**, and Tier
3 becomes the SDK/client. It is a reclassification, not a rewrite.

### 5.2 Modularity: today the "plugin" is a `&[...]` inside the gateway

`crates/gateway/src/module_catalog.rs` declares `pub static CATALOG: &[ModuleManifest]`
with 22 modules. The file itself admits route mounting still lives in `api_router()`.

So: **installing a module today = editing a file inside the gateway and recompiling.**
`ModuleManifest` already lives in the right place (`dsoc_app::manifest`) and already
has `permissions`, `nav`, `depends_on`, `flag_key` — the abstraction is ready; the
dependency inversion is missing.

Also: manifest `title`s are hard-coded pt-BR strings — in an international framework
those must be i18n keys.

### 5.3 Fragmentation: 8 voting subsystems, 3 comment stacks, 3 content stacks

**Voting** (8 mechanisms, no shared abstraction, only 1 federates):

| # | Where | Method | Federates? | Privacy |
|---|---|---|---|---|
| 1 | `components/votes` | proposal support | ❌ | aggregate (LGPD) |
| 2 | `spaces/initiatives` | signature + threshold | ❌ | nominal signature |
| 3 | `gateway/polls.rs` | poll on a Note | ✅ `Question` | public |
| 4 | `gateway/campaign_groups.rs` | agree/neutral/disagree | ❌ | own table |
| 5 | `spaces/consultations` | windowed question | ❌ | — |
| 6 | `components/surveys` | typed questionnaire | ❌ | 1 per citizen |
| 7 | `components/forums` (ADR-0019) | signed score + karma | partial | public |
| 8 | `components/budgets` | participatory budgeting | ❌ | — |

**Comments:** `components/comments`, `forum_comment` (arguments), `note` with
`inReplyTo`. **Content:** `note`, `forum_topic`, `proposal`. Three parallel stacks,
three moderation paths.

This is organic-growth debt, not a design error — but it blocks federation: federating
proposal/budget/consultation would require writing the same AP layer three times.

### 5.4 Localization leaks outside `l10n_br`

The `platform/l10n-br` crate is correct and well built (`IdentityVerifier`,
`TerritorialProvider`, `VoterRegistration`, `Localization`) — and is now also published
standalone as [agora-fed/l10n-brazil](https://github.com/agora-fed/l10n-brazil). The
problem is what did **not** go through it:

- the `esfera` CHECK in `party_directory` and `mandate` (§4.1)
- `municipio_ibge`, `titulo_eleitor`, `cpf`, `residencia_*` in the API contract
- pt routes: `/api/v1/municipios`, `/campanha`, `/consultas`, `/politicos`
- `forum.full_path` assumes a 3-level BR territory; `handle_to_path()` caps `segs.len() <= 3`
- `DEFAULT_ORG_UUID` hard-coded in several handlers
- web UI without an i18n catalog (no `Accept-Language`, no `web/src/i18n/<locale>`)
- `module_catalog` with literal pt-BR titles
- pt-BR doc comments across core crates (policy now: English only in core;
  Portuguese only in l10n-brazil — sweep tracked in §9 wave R)

---

## 6. Where to advance — the five structural moves

### 6.1 M1 — Bidirectional `Group` (complete FEP-1b12)

The highest return per line of code in the whole project. Closes §3.2's ❌ list:

- accept `Create{Page|Note}` and `Create{Note}+inReplyTo` in the Group inbox,
  validating `audience`;
- **reject** anything whose `audience` does not point at the group (the FEP requires it);
- emit `Page` for top-level posts and `Announce(Create(Page))` with the object
  **embedded** (accept both forms inbound — Lemmy sends both,
  Friendica/Hubzilla/NodeBB send `Announce(Object)`);
- serve the Group `outbox` and `followers`;
- `attributedTo` → moderators collection + `Add`/`Remove`;
- `Remove` (not `Delete`) for moderation, also wrapped in `Announce`;
- `properties: {"…#type": "Group"}` in the JRD;
- advertise `endpoints.sharedInbox` (1 delivery per remote instance, not per follower).

Result: a Pindorama forum becomes a first-class community visible and usable from
Lemmy, PieFed, Mbin, NodeBB and Friendica — tens of thousands of already-installed users.

### 6.2 M2 — Unified `Group` registry (`Organization`, `Chapter`, `Forum`, `Caucus`, `CampaignGroup`)

One table `fed_group(id, kind, slug, owner_ref, territorial_level, territory_ref,
public_key_pem, private_key_pem, …)` with `kind` discriminating. All five get an AP
actor, followers, inbox and Announce through **one** code path. Consequences:

- a party becomes `@pt@democracia.social.br`, a chapter `@pt.sp@`, a caucus
  `@bancada-feminista.psol.sp@`;
- resolves §4.2 and kills the `esfera` CHECK (becomes `territorial_level: i16` +
  `TerritorialProvider`);
- it is the **precondition for group-scoped ballots** (§6.3): "who may vote" =
  followers/members of a `Group`.

### 6.3 M3 — The `Ballot` primitive (referendum / plebiscite)

Replace §5.3's 8 mechanisms with **one** component with four pluggable axes:

```
Ballot {
  scope:      Group | Space | Territory | Everyone       ← who may vote ("per group")
  method:     YesNo | SingleChoice | Approval | Ranked   ← how it is counted
            | Score | Quadratic
  privacy:    PublicTally | SecretBallot | AggregateOnly ← LGPD; default AggregateOnly
  threshold:  quorum + threshold → triggers consequence (SLA)
  federation: Question (open) | dsoc:Ballot + dsoc:Tally (secret)
}
```

- **Plebiscite/referendum** = `Ballot { scope: Territory, method: YesNo, privacy: AggregateOnly }`.
- **Group vote** = `scope: Group(id)` — only members/followers of that `Group` vote.
- **Feed poll** (today `polls.rs`) = `scope: Everyone, method: SingleChoice,
  privacy: PublicTally` → keeps federating as `Question`, no regression.
- **Initiative signature** = `method: Approval, privacy: PublicTally` (signatures are nominal).

The `privacy: AggregateOnly` axis **is** ADR-0005's constraint ("support is an
aggregate, never a per-actor `Like`") — generalized and reusable instead of
reimplemented per component.

### 6.4 M4 — Federated object spine (Decidim enters the Fediverse)

Every authored piece of content (`note`, `forum_topic`, `proposal`, `budget_project`,
`consultation_question`, `meeting`) gets a row in a spine table:

```
fed_object(object_id PK, kind, local_ref, attributed_to, audience_group_id,
           in_reply_to, published_at, visibility, deleted_at)
```

The AP object derives from the spine; `note`/`forum_topic`/`proposal` become **views**
over it. Gains: one moderation path, one threading path, one delivery path, and the
already-written builders in `clients/federation/src/mapping.rs` (`proposal_to_ap`,
`sla_to_ap`, `scorecard_to_ap`) finally get routes. The most expensive move and the
highest long-term leverage.

### 6.5 M5 — Real modularity (`AgoraModule`)

```rust
// crates/core/src/module.rs
pub trait AgoraModule: Send + Sync {
    fn manifest(&self) -> &ModuleManifest;          // already exists in dsoc_app::manifest
    fn routes(&self, state: AppState) -> Router<()>;
    fn migrations(&self) -> &'static [Migration];
    fn subscriptions(&self) -> &'static [EventTopic];
    fn federation(&self) -> Option<&dyn FederatedModule>;  // AP types the module owns
}
```

- each module crate exposes `pub fn module() -> &'static dyn AgoraModule`;
- the gateway composes `Vec<&dyn AgoraModule>` instead of the static `CATALOG` +
  `.merge()` chain;
- **Cargo feature flags per module** (`--features forums,budgets,l10n-br`) → a real
  installation build (Odoo's "addons path" applied to Rust);
- third-party modules outside the workspace named `agora-module-*`.

On dynamic runtime loading (`.so`/wasm): **do not**. Rust has no stable ABI; the
cost/benefit is bad. Build-time feature composition + a distribution catalog delivers
95% of the value at 5% of the risk. What **is** runtime-dynamic is already right:
which modules are active per org, via `admin_feature_flag`.

---

## 7. The Fediverse leadership play

Interoperating well (M1) puts AGORA *in* the Fediverse. This puts AGORA *in front*:

### 7.1 Write the FEPs that do not exist

There is **no** Fediverse standard for deliberation, voting or accountability. Lemmy
became the forum reference because nutomic wrote FEP-1b12 and implemented it. The same
space is open:

| Proposed FEP | Content | AGORA's state |
|---|---|---|
| `Ballot & Tally` | federated voting with an **aggregate tally, no per-actor `Like`** — privacy by design | already ADR-0005's constraint; M3 formalizes it |
| `Proposal & Deliberation` | `Proposal` object, `Argument` (with ± stance), consensus cluster | `mapping.rs` already has the builders |
| `Accountability` | `Commitment`, `Sla`, `Scorecard` — institutional response and public silence | `sla_to_ap`/`scorecard_to_ap` already exist |

AGORA would be simultaneously author and reference implementation. It is the cheapest
and most durable route to technical leadership — and ADR-0005's `dsoc:` vocabulary is
already the draft.

### 7.2 Quick credibility wins

- **`FEDERATION.md` (FEP-67ff)** at the repo root — the document describing what the
  implementation supports. Does not exist today. It is the first thing another
  implementer looks for. Cost: 1 day.
- **NodeInfo with capabilities (FEP-9fde / FEP-6481)** —
  `metadata.activitypub.extensions` with the `dsoc:` vocabulary IRIs. Today NodeInfo
  only advertises `accountabilityVocabulary`. Cost: 1 day.
- **Publicize the Mastodon Client API that already exists** — Ivory, Tusky, Elk and
  Ice Cubes already work against Pindorama and nobody knows. Cost: communication, zero code.

### 7.3 Civic relay

A new municipal instance is born with an empty federated timeline. An AGORA relay
(`Follow` on `as:Public` → `Accept` → `Announce` back), possibly **filtered by
territory or topic** (FediBuzz model), solves the cold start of every new
installation. It is network infrastructure — and whoever runs the network
infrastructure leads the network.

### 7.4 Research track: portable identity (FEP-ef61)

Portable objects / nomadic identity mean a citizen **does not lose their civic
identity if their municipality's instance is shut down**. Connects directly to
PLAN.md §1.6 ("survive coordinated pressure"). Expensive and immature in the
ecosystem — position as R&D, not as a deliverable.

---

## 8. Constraints that must not be violated

1. **Vote privacy (LGPD / ADR-0005).** Per-actor `Like` on a vote is forbidden.
   `AggregateOnly` is `Ballot`'s default. Any vote federation goes through `dsoc:Tally`.
2. **Identity headers.** `x-dsoc-citizen-id` et al. **must** be stripped at Caddy *and*
   at the gateway (incident 0.36.2). Every new inbox enters through an
   HTTP-Signature-authenticated path, never via header.
3. **An open Group = spam surface.** Accepting remote `Create` (M1) opens the forum to
   the whole Fediverse. It must ship with: instance allow/denylist, optional
   follower-before-post requirement, moderator approval queue, and the existing
   `moderation` crate in the path.
4. **Federated interactions never trigger consequence.** Already the rule in
   `components/forums` (federated interactions count separately and never trigger
   institutional mail). The same must hold for `Ballot`, or the institutional SLA
   becomes a remote-brigade toy.
5. **`api-contract` is frozen.** Contract changes require an ADR and a transition
   window (canonical EN alias + temporary pt), as defined in ADR-0013.

---

## 9. The wave plan

Sequenced by *(network value) ÷ (risk × cost)*. Each wave is shippable and reversible.

### Wave R — Repo split & GitOps ✅ (largely DONE, 2026-08-05)

| # | Delivery | Status |
|---|---|---|
| R.1 | Core published to [github.com/agora-fed/core](https://github.com/agora-fed/core) (history secret-scanned) | ✅ |
| R.2 | [l10n-brazil](https://github.com/agora-fed/l10n-brazil) standalone (git-dep on core; 20 unit tests) | ✅ seeded |
| R.3 | GitHub Actions CI (fmt/clippy/boundaries/tests/coverage/web/helm) | ✅ |
| R.4 | GitOps mandatory: tag → GHCR image → pull-based sync agent (`deploy/gitops/`), docs/GITOPS.md | ✅ |
| R.5 | Helm chart `agora-core` (generic) + `values-pindorama.yaml` | ✅ |
| R.6 | Dev quickstart `docker-compose.dev.yml` (pgvector + migrations + gateway) | ✅ |
| R.7 | Core drops the embedded `platform/l10n-br` in favor of the git dep | pending (wave 2, needs `AgoraModule` wiring) |
| R.8 | Plugin repos (`agora-module-*`) | pending — **blocked by M5**: without the `AgoraModule` trait a standalone module repo cannot build |
| R.9 | English-only sweep of legacy pt-BR doc comments in core | in progress (new files + README/docs done; crate sweep queued) |

### Wave 0 — Credibility (days) · zero risk

| # | Delivery | Depends on |
|---|---|---|
| 0.1 | `FEDERATION.md` at the root (FEP-67ff) | — |
| 0.2 | NodeInfo: `metadata.activitypub.extensions` with `dsoc:` IRIs (FEP-9fde) | — |
| 0.3 | Forum JRD with `properties.type = Group` | — |
| 0.4 | Advertise `endpoints.sharedInbox` in the actor doc | — |
| 0.5 | ADR-0020 — *Group registry & naming* (§4.2 + §6.2) | — |
| 0.6 | ADR-0021 — *`Ballot` primitive* (§6.3) | — |

### Wave 1 — Bidirectional `Group` (2–4 weeks) · **highest return**

| # | Delivery | Depends on |
|---|---|---|
| 1.1 | Inbound `Create{Page\|Note}` with `audience` validation | 0.3 |
| 1.2 | `Create{Note}+inReplyTo` (remote comment) → `forum_comment` | 1.1 |
| 1.3 | Anti-spam guard: instance allow/denylist + approval queue | 1.1 |
| 1.4 | Emit `Page` + embedded `Announce(Create(Page))`; accept both forms | — |
| 1.5 | Group `outbox` and `followers` | — |
| 1.6 | `attributedTo` (moderators) + `Add`/`Remove` + content `Remove`, all in `Announce` | 1.5 |
| 1.7 | Real interop test against Lemmy, PieFed, Mbin and NodeBB | 1.1–1.6 |

### Wave 2 — Reclassification and modularity (3–5 weeks) · medium risk

| # | Delivery | Depends on |
|---|---|---|
| 2.1 | Move federation to `crates/platform/federation` (Tier 1); Tier 3 becomes the SDK | Wave 1 |
| 2.2 | `AgoraModule` trait in `core` + `pub fn module()` per crate | — |
| 2.3 | Gateway composes `Vec<&dyn AgoraModule>`; `CATALOG` leaves the gateway | 2.2 |
| 2.4 | Cargo feature flags per module | 2.3 |
| 2.5 | Manifest titles/labels become i18n keys | 2.2 |
| 2.6 | Core consumes `l10n-brazil` as a git dep; embedded copy removed (R.7) | 2.2 |
| 2.7 | First `agora-module-*` template repo published (R.8) | 2.4 |

### Wave 3 — `Group` registry + naming (3–4 weeks) · **breaks contract**

| # | Delivery | Depends on |
|---|---|---|
| 3.1 | Unified `fed_group` table; `Forum` migrates onto it | 2.1 |
| 3.2 | `party` → `Organization(kind)`, `party_directory` → `Chapter` | 0.5 |
| 3.3 | `esfera` CHECK → `territorial_level: i16` + `TerritorialProvider` | 3.2 |
| 3.4 | `Organization`/`Chapter`/`CampaignGroup` get `Group` actors | 3.1 |
| 3.5 | `Caucus` as a new `kind` | 3.4 |
| 3.6 | EN/pt alias window on affected routes (ADR-0013) | 3.2 |

### Wave 4 — `Ballot` (4–6 weeks)

| # | Delivery | Depends on |
|---|---|---|
| 4.1 | `components/ballot`: 4 axes, `AggregateOnly` default | 0.6 |
| 4.2 | `polls.rs` reimplemented on `Ballot` (no `Question` regression) | 4.1 |
| 4.3 | `scope: Group` — vote restricted to members/followers | 3.1 |
| 4.4 | `scope: Territory` — **plebiscite** with quorum | 3.3 |
| 4.5 | Migration of `campaign_groups` polls, `initiatives`, `consultations` | 4.1 |
| 4.6 | Federation: `dsoc:Ballot` + `dsoc:Tally` (aggregate) | 4.1 |

### Wave 5 — `fed_object` spine + federated Decidim (6–10 weeks)

| # | Delivery | Depends on |
|---|---|---|
| 5.1 | `fed_object` table + backfill of `note`/`forum_topic` | Wave 3 |
| 5.2 | `proposal` on the spine → `Proposal` AP served (uses `proposal_to_ap`) | 5.1 |
| 5.3 | One moderation path and one threading path over the spine | 5.1 |
| 5.4 | Federated `Sla`/`Scorecard`/`Commitment` (`sla_to_ap`, `scorecard_to_ap`) | 5.2 |

### Wave 6 — Leadership (continuous, parallel from Wave 1)

| # | Delivery | Depends on |
|---|---|---|
| 6.1 | FEP draft *Ballot & Tally* → SocialHub | 4.6 |
| 6.2 | FEP draft *Proposal & Deliberation* | 5.2 |
| 6.3 | FEP draft *Accountability* | 5.4 |
| 6.4 | Civic relay (`Follow as:Public`), territory/topic filtered | Wave 1 |
| 6.5 | Public campaign "Pindorama works in your Mastodon client" | — |
| 6.6 | R&D: FEP-ef61 portable identity | — |

---

## 10. Decision queue

What needs deciding before code is written:

1. **`Organization` vs `Group` as the core name for the party concept** (§4.2) —
   changes schema, contract and routes. Marcos decides.
2. **Unified `Ballot` vs keeping the 8 mechanisms** (§6.3) — keeping them means
   plebiscite and group voting become #9 and #10, and vote federation gets written
   N times.
3. **`fed_object` spine (§6.4)** — the most expensive item. Deferrable until after
   Wave 4 without blocking anything, but the later it lands the more it costs.
4. **Relay: operate one or not** (§7.3) — an infrastructure and operating-cost
   decision, not a code decision.
