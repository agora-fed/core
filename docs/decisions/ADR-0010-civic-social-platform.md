# ADR-0010 — Civic-social platform: extending the surface from "consequence loop only" to a full social fabric

- **Status:** Accepted
- **Context:** PLAN.md §1 (North Star), §3 (anti-patterns — "DO NOT rebuild all of Decidim", "DO NOT ship a generic everything-for-everyone UI"), principle 12 (justify every reversal); ADR-0005 (ActivityPub federation foundation), ADR-0008 (CPF credential auth).

## Decision

We **broaden the product surface** from the hyperspecialized "propose → cluster → vote → notify
official → response-or-public-silence" loop to a full **civic-social platform** that includes the
mainstream social-network features citizens expect from any 2026 platform:

1. **Profile management** — display name, bio, avatar, cover image, user-chosen handle.
2. **Password reset and account recovery** via e-mail; session list + revoke; 2FA (TOTP).
3. **Federation extended to citizens** — every citizen becomes an ActivityPub `Person` Actor with
   Inbox/Outbox/Followers/Following collections; can be discovered via Webfinger
   (`@handle@democracia.social.br`).
4. **Follow / unfollow** other citizens — locally AND across instances (Mastodon, other Pindorama
   instances, future PINDORAMA Brazilian hub).
5. **Home timeline** — feed of posts/proposals from followed accounts, paginated.
6. **Search** — by handle, by content (full-text + pgvector for semantic when warranted).

This expansion is delivered in **four sequential waves**, each independently deployable:

- **W1 — profile + basic security** (2-3 weeks): citizen schema extension, profile edit,
  avatar/cover upload (MinIO), password reset, sessions list.
- **W2 — citizens as ActivityPub Actors** (2-3 weeks): per-citizen Inbox/Outbox, Webfinger,
  outbound HTTP-signed delivery, delivery worker.
- **W3 — social layer** (2-3 weeks): follow/unfollow, follower lists, home timeline, search.
- **W4 — security extras** (1 week): TOTP 2FA, audit log, backup recovery codes.

## Rationale

The original PLAN was a contrarian bet on hyperspecialization: most civic-tech failures came from
the "everything for everyone" trap (Decidim being the canonical example), and we'd win by doing one
thing — visible, time-bound, public accountability — exceptionally well.

Real-user contact (first-test pre-flight, 2026-06-26) surfaced a different failure mode: a citizen
who **lacks** the mental affordances of a normal social network — no profile, no way to recover a
password, no way to follow or be followed, no feed of who you care about — does not experience
"focused civic platform"; she experiences "broken or abandoned product". The hyperspecialization
that protected us from feature bloat also made the product feel **inert** to people whose baseline
expectation was set by Instagram and Twitter.

The consequence loop remains the **anchor and differentiator** — what makes us not-Mastodon — but
the social fabric is now treated as the **substrate** the loop runs on, not as feature creep. A
proposal addressed to a mandate is more credible when the proposer has a recognizable identity
others can vouch for. A scorecard is more shareable when citizens follow each other and reshare.
Federation across instances multiplies the consequence: a politician ignored on his city's
instance is *also* ignored in everyone's federated timeline.

## If this reverses a prior decision (PLAN.md principle 12)

**(a) Why the previous approach fails.** PLAN.md §3 ("DO NOT rebuild all of Decidim", "DO NOT ship
a generic everything-for-everyone UI") was defensive against feature-bloat scope creep — an
absolutely correct guardrail in 2025 when the team was small and the foundations weren't yet
real. The failure mode it *prevented* — Decidim's never-shipped pile of half-built features —
is real and we should still hate it. But the *opposite* failure mode it created — a platform so
narrow citizens can't form the basic social trust that gives the consequence loop its bite — has
now surfaced empirically.

**(b) Whether it can be salvaged.** Partially. The principle's *spirit* — hyperspecialize,
refuse generic abstraction, don't port Decidim 1:1 — still holds and binds every wave below.
What we are *not* doing: blogs, polls-for-polls'-sake, generic CMS, marketplaces, monetization,
or anything else outside the "civic accountability + social trust" axis. The principle's *letter*
("nothing social") is what we are reversing, because the social *is* the trust substrate.

**(c) Why the new one is better.** A profile + follow + feed isn't "everything for everyone" —
it's the **minimum identity substrate** below which accountability mechanics don't activate.
Empirically (Mastodon, Bluesky, Twitter all the way back) you cannot demand a public response
from a politician if the demand has no public face. We adopt those mechanics deliberately,
under the consequence-loop product thesis, not as catch-all social-network features.

## Consequences

**Architectural ripples**
- New crate `dsoc-storage` (platform tier) wrapping a `Storage` port (`Storage::put`, `::url`,
  `::delete`) with a MinIO-backed implementation. The port stays in `dsoc-core` so component
  crates depend on the trait, not on MinIO. Future swap to AWS S3 / Cloudflare R2 is one
  implementation change.
- New tables under the `0100` (auth) range: `0102_citizen_profile.sql`,
  `0103_auth_password_reset.sql`. Future federation tables claim the `0400` slot already
  reserved in `migrations/REGISTRY.md`.
- `dsoc-federation` extends from "mandate-only Actor" to "mandate + citizen Actor", with a new
  per-citizen `Outbox` and `Inbox` table set under the `0400` range and a delivery worker hung
  off the existing `dsoc-gateway::worker` runtime (no new pod).
- `lettre` (already a workspace dep) is wired into `dsoc-notify` for transactional e-mail
  (reset, confirmation). SMTP credentials come from `SMTP_HOST/PORT/USER/PASS/FROM` env vars
  (PLAN.md principle 8: no hardcoded secrets); the operator brings their own SMTP relay.

**Privacy posture (LGPD)**
- Citizen profiles are **private by default** (W1). A `citizen.is_public` flag defaults `false`;
  ActivityPub Actor materialization (W2) and federated discoverability are gated behind it.
  This protects test users, minors, and anyone who registers without intending to become a
  public figure — the consequence loop continues to work for private profiles, but their face
  is not federated.
- The CPF is **never** exposed in the federation surface — Actor URIs / handles never embed it,
  the `preferredUsername` is the user-chosen handle, and the audit log surfaces only opaque
  citizen ids.

**Scope guardrails preserved**
- We still do **not** build: blogs, marketplaces, ads, monetization, generic CMS, "page builders",
  custom emoji, polls-as-game.
- We still do **not** import another component's internals (the crate-boundary rules of
  CONTRIBUTING.md hold for every new crate below).
- We still do **not** ship binding-grade cryptographic voting (PLAN.md §3 — still the TSE's job).
- ADR-0008 (sovereign CPF auth) is unaffected: federation handles are an additional *display*
  identity, not an additional credential.

**Roll-out**
- Each wave commits independently and deploys to `democracia.social.br` (image bump). The wife-
  test moves from "post-W1" (profile + reset) to a sequence of staged tests, gating each wave's
  ship on her feedback.
- PLAN.md §3 will be amended (next commit alongside the W1 migration) to reference this ADR
  next to the "DO NOT rebuild all of Decidim" line, so future readers understand the
  scoped-and-justified reversal rather than treating it as drift.
