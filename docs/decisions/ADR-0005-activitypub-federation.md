# ADR-0005 — Federation via ActivityPub (+ a custom accountability vocabulary)

- **Status:** Proposed (target: Phase 3; owning crate `clients/federation`).
- **Context:** PLAN.md §1.6 (survive coordinated pressure through federation), §7 Phase 3
  (federation SDK + hub), and the `federation` crate. This ADR commits the federation protocol to
  **ActivityPub (AP)** so the platform interoperates with the wider fediverse instead of inventing a
  closed protocol.

## Decision

Federate over **ActivityPub** (W3C), extended with a small, namespaced **accountability vocabulary**
(JSON-LD `@context`), the way Lemmy/Mobilizon/Mastodon extend AP for their domains.

- **Actors.** A citizen/voter is an AP `Actor` (`Person`) with a public profile, `inbox`/`outbox`,
  WebFinger handle (`@user@instance`), and an HTTP-Signatures keypair. A **candidacy/mandate is the
  same Actor evolving role** — "voter → candidate → official → governor" is a progression of
  `mandate` bindings on one stable Actor identity, which the scorecard follows. This is the federated
  expression of the platform's core promise.
- **Objects/Activities.** Proposals are a custom `Proposal` object (`Note`-compatible); deliberation
  uses `Note`/`Create`; consensus clusters, SLAs, and scorecards are custom objects under our
  vocabulary so remote instances can render and relay the accountability loop.
- **Federated consequence loop.** A municipality runs a local instance; its signals (clustered
  proposals crossing thresholds against an official) federate into the central hub, and outcomes
  (answered / public silence / scorecard updates) federate back — the consequence loop spans instances.
- **Interop surface.** WebFinger, NodeInfo, actor `inbox`/`outbox`, HTTP Signatures, `Follow`/`Accept`.

## Hard constraints (these shape the design, and the MVP must not preclude them)

1. **Vote privacy is non-negotiable and conflicts with naive AP.** Support is a **public aggregate**,
   never a per-actor public `Like`. Individual vote→citizen linkage MUST NOT be federated (LGPD,
   PLAN.md DO-NOT). So "support" is an aggregate signal object, not an attributed activity.
2. **Sybil resistance.** Open federation enables vote-stuffing. Voting weight is gated by the
   `mandates`/`auth` **verification levels**; remote/low-assurance actors carry lower trust. Federation
   and identity-assurance are co-designed.
3. **Auditable moderation applies to inbound federated content** (PLAN.md principle 11) — no opaque
   relay of unmoderated civic speech.
4. **Sovereignty.** Federation must not create a single point of takedown or capture; instances remain
   self-hostable and self-governing.

## MVP "AP-readiness" seams (reserve now, build in Phase 3 — do NOT implement AP in the MVP)

To avoid a costly retrofit, the Wave-0 contract and MVP crates reserve, without implementing AP:
- A **stable, public Actor identifier + handle** for citizens and mandates (the existing `CitizenId`/
  `MandateId` are stable; add a public handle field when `auth`/`mandates` land).
- An **Actor keypair** slot (for future HTTP Signatures) in the identity model — storage reserved,
  not populated in the MVP.
- Keep proposals/scorecards addressable by a stable public URL so they can later become AP objects.

These are notes for the `auth`, `mandates`, `proposals`, and `scorecard` crate owners; they cost
nothing now and prevent an AP retrofit later.

## Consequences

- `clients/federation` targets AP + the accountability vocabulary; it depends only on the frozen
  `api-contract` (PLAN.md §6.1).
- New Phase-3 work: keypair management, signature verification, instance allow/block lists, federated
  moderation, and the JSON-LD context document.
- Does **not** change the MVP scope or the Phase-1 gate; it only adds reserved seams.
