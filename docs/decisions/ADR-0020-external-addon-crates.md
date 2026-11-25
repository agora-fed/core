# ADR-0020 — External add-on crates (first: DIOGENES articulation rooms)

- **Status:** Accepted — DIOGENES in production since 2026-11-25 (image `0.83.0-diogenes`).
- **Context:** ADR-0011 (core & modules: manifest, per-org gate), ADR-0013 (English API),
  ADR-0015 (l10n as a pluggable layer, external repo `agora-fed/l10n-brazil`), ADR-0007
  (authenticated caller). Feature request: a vote-count and articulation tool for the Chamber
  of Deputies, based on the "Placar de Votações" design by Lucas Leffa, built "as a separate
  module of ÁGORA" in its own repository (`agora-fed/diogenes`).

## Decision

1. **An add-on is a crate in its own repository** with **no dependency on any `dsoc-*` crate**.
   It exposes a small host contract:
   - `api_router(state) -> Router`, mounted under `/api/v1`. **The add-on serves JSON only;
     the site draws its pages** (see "Revision 2026-11-25" below);
   - an `Identity` trait the host implements (caller, user lookup by handle, module gate,
     and `member_added` so the host can tell a new member — bell, web push, e-mail);
   - `migrate(&PgPool)` with its own migration table (`diogenes_migration`), its own advisory
     lock and table prefix (`diogenes_*`). It never touches host tables.
2. **The host owns all wiring in one file**, `crates/gateway/src/<addon>_addon.rs`, which
   implements the trait on top of `inject_identity` headers, `AdminService` permissions and
   `module_gate`. The API router is merged **inside** the `/api/v1` chain, so the identity,
   signup-gate and rate-limit layers cover it.
3. **Compile-time switch:** a cargo feature per add-on (`diogenes`, on by default). The
   dependency is a git dependency **pinned by `rev`**, allowed in `deny.toml`.
4. **Run-time switch:** a module manifest in `module_catalog.rs` with
   `default_enabled: false` (ADR-0011 D5). The add-on is off in every org until an admin turns
   it on at `/admin/modulos`. Navigation and pages follow the optional-module pattern of
   `groups` (#78): `UI_OPTIONAL_MODULES` and `OPTIONAL_MODULE_ROUTES` in `instance_html.rs`,
   `module-gate.css`, `OPTIONAL_MODULES` in `web/src/lib/modules.ts`, links in the header,
   left rail, command palette and footer, and the page itself under `web/src/pages/`
   built from the design system (`web/src/components/ui/`).
5. **Background work is opt-in per installation** by environment
   (`DIOGENES_SYNC_ENABLED=true`). Shared reference data has no org, so only one operator org
   (`DIOGENES_OPERATOR_ORG`) may trigger the sync by hand.
6. **Installation-specific wiring lives on a deploy branch**, not on core `main`
   (`deploy/pindorama`, like `deploy/organica`). Core stays free of add-ons that only one
   installation runs. A fix found while wiring (for example the bearer-scope fix) goes to core
   through its own PR.
7. **Documentation:** the add-on repository keeps its own ADRs. DIOGENES writes them in
   ASD-STE100 (its `docs/STYLE.md`). This ADR records the host side.

## Revision 2026-11-25 — the site draws the pages

The first deploy mounted the add-on's own HTML/CSS/JS at `/articulacao/`. In production it
looked like a different product: buttons escaped their cards, inputs cut their text, the
header did not match, members without a profile name showed a bare generated handle.

- The pages now live in the site (`web/src/pages/articulacao.astro`,
  `web/src/components/articulacao/`, logic in `web/src/lib/articulacao.ts`) and use the
  design system only; a `ui/Select.svelte` was added for the missing native select.
- The gateway no longer mounts `diogenes::web_router()`; the static page is gated by
  `OPTIONAL_MODULE_ROUTES` like `/grupos`. The hash routes of the first version are kept.
- A member without a profile name is shown as `@handle`.
- New members are told: `user_notification` kind `room_member` (migration 0900, add-on
  range 0900–0949), web push, and the editable e-mail template `diogenes_room_member`
  (opt-out `email_prefs.room_member`). Code: `crates/gateway/src/diogenes_notice.rs`.
- Add-on side: diogenes ADR-0007 (`member_added`; `web_router` deprecated for one release).

## Rationale

- ADR-0011 already gives per-org gating and permissions by key; an add-on reuses them instead
  of inventing a plugin runtime. Rust links statically, so the only real choice is where the
  wiring commit lives (decision 6).
- No `dsoc-*` dependency keeps the add-on testable alone (its CI runs against PostgreSQL with
  a fake identity) and lets other ÁGORA installations mount it without our internal crates.
- An own migration table avoids collisions with `migrations/REGISTRY.md` ranges and lets the
  add-on evolve its schema on its own release cycle.

## If this reverses a prior decision (PLAN.md principle 12)

The architecture extends ADR-0011 to modules outside the workspace, as ADR-0015 did for
`agora-fed/l10n-brazil`. The 2026-11-25 revision reverses the first version's "add-on
ships its own pages":
- (a) Why it failed: a second, hand-written UI outside the design system drifted at once
  (broken layout in production, inconsistent header, no dark-mode/mobile guarantees).
- (b) Salvage: the JSON API, the hash routes and the behaviour were kept; only the
  rendering moved. `web_router` stays one release for a one-line rollback.
- (c) Why the new is better: one design system, one navigation, the site's tests
  (vitest, svelte-check ratchet, Playwright) cover the add-on like any other page.

## Consequences

- The host must keep the add-on's `rev` current; a bump is a normal PR with tests.
- Every add-on ships the full definition of done of CONTRIBUTING.md: backend unit +
  integration tests (the host adapter has its own suite, `crates/gateway/tests/diogenes.rs`),
  vitest contract tests for navigation, security cases (spoofed identity headers → 401, module
  off → 404, permission → 403), ≥95% patch coverage, and a production Playwright spec
  (`web/tests/ui/articulacao.spec.ts`).
- Incident 2026-11-25: the first deploy wired the module into the catalog but not the header,
  so turning it on changed nothing visible. Decision 4 and the vitest contract
  (`web/tests/modules.contract.test.ts`) now pin every navigation surface.
- Add-on data that is large and changes rarely (the TSE campaign accounts, diogenes ADR-0009)
  is an operator-started job, not part of the hourly sync. After a deploy that adds such a
  job, the operator starts it once and checks `GET /api/v1/diogenes/sync/status`. The
  Receita CNPJ job (`companies`, diogenes ADR-0010) depends on `donations`: run it after
  each donations run.
- A route that the add-on makes public (the campaign reads, diogenes ADR-0009 change) has no
  session, so the mutating-only limiter of the host does not cover it. The host wraps the
  add-on router with a read limiter for those routes (`diogenes_addon::public_read_limit`).
- Follow-up: a generic `Addon` trait in the gateway when a second add-on arrives (YAGNI until
  then).
