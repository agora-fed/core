# ADR-0020 — Federate consultas as AP `Question` via forum `Group` actor

- **Status:** Implemented (2026-11-03, branch `add/consulta-federation`).
- **Context:** FEDIVERSE-STRATEGY §3.3 (Decidim layer has zero AP surface), §6.3
  (`Ballot` primitive), §5.3 (8 voting subsystems). ADR-0005 (ActivityPub federation).
  This is the **first Decidim-layer object to gain an AP surface** — a concrete step
  toward Wave 4 (Ballot), done incrementally without the full `fed_object` spine (Wave 5).

## Decision

### 1. Consultation kinds: `consulta` vs `plebiscito`

Split `consultations_consultation` into two behavioral kinds via a `kind` column
(migration 0689):

| Kind | Voting model | Federation | Access |
|------|-------------|------------|--------|
| `consulta` | Single-choice (radio) | ✅ AP `Question` with `oneOf` | Open to all (including remote actors) |
| `plebiscito` | Concordo / neutro / discordo | ❌ Not federated | SSO-only (`citizen.oidc_subject IS NOT NULL`) |

The plebiscito keeps the original `consultation_response` table (concordo/neutro/discordo).
The consulta uses a new `consultation_vote` table (migration 0690) that supports both
local citizens (`citizen_id`) and remote actors (`actor_url`), enforced by a CHECK
constraint (`num_nonnulls(citizen_id, actor_url) = 1`).

### 2. Group actor attribution (not Person)

A consulta is attributed to a forum's `Group` actor, not the creating citizen's `Person`
actor. Rationale:

- A consulta is an **institutional act** (the community asks a question), not a personal
  opinion. It should come from `@propostas@agora.organica.social`, not `@joao@…`.
- The `Group` actor already exists (FEP-1b12, `forum_federation.rs`) with WebFinger,
  HTTP Signatures keypair, inbox, and followers — all infrastructure a federated poll
  needs.
- Votes arrive at the Group's inbox (`/actors/propostas/inbox`), which is where Mastodon
  sends poll votes (to the poll owner's inbox).
- This aligns with FEDIVERSE-STRATEGY §6.2 (unified Group registry) — when the full
  `fed_group` table lands, consultas already use the right actor type.

Implementation: `forum_id` FK on `consultations_consultation` (migration 0691).
The AP object is served at `/actors/{forum_slug}/objects/consulta-{uuid}`.

### 3. Vote routing

Mastodon poll votes arrive as `Create{Note{name: "<option>", inReplyTo: "<question URL>"}}`.
The vote handler is wired into **both** inbox paths:

- Person inbox (`federation.rs:~2886`) — catches votes sent to a citizen's inbox (fallback)
- Forum inbox (`federation.rs:~683`) — catches votes sent to the Group's inbox (primary path)

The forum inbox vote check runs **before** the general inbound federation gate, so consulta
votes work even when full forum inbound federation (`config.enabled`) is off.

### 4. AP object shape

```json
{
  "@context": "https://www.w3.org/ns/activitystreams",
  "id": "https://agora.organica.social/actors/propostas/objects/consulta-{uuid}",
  "type": "Question",
  "attributedTo": "https://agora.organica.social/actors/propostas",
  "audience": "https://agora.organica.social/actors/propostas",
  "content": "<p>Qual nome…?</p>",
  "endTime": "2026-11-24T…Z",
  "url": "https://agora.organica.social/consulta/?id={uuid}",
  "to": ["…/actors/propostas", "as:Public"],
  "cc": ["…/actors/propostas/followers"],
  "oneOf": [{"type": "Note", "name": "Opção A", "replies": {"type": "Collection", "totalItems": 3}}],
  "votersCount": 7
}
```

Follows the same conventions as `topic_object_json` (forum topics): `audience` points to
the owning Group, `to` includes both the Group and Public.

## Rationale

### Why not use the existing `polls.rs` (feed polls)?

`polls.rs` serves polls attached to a `Note` (Mastodon-style microblog polls). A consulta
is a **standalone deliberation instrument** — it has a title, a time window, admin-only
creation, and belongs to a Decidim space (consultations), not to the feed. Reusing
`polls.rs` would conflate two distinct domain concepts. However, the AP wire format is
identical (`Question` + `oneOf`), which is intentional — maximum Mastodon compatibility.

### Why a new `consultation_vote` table instead of reusing `consultation_response`?

`consultation_response` stores concordo/neutro/discordo per question — a 3-way stance.
A consulta is single-choice across the question set (pick one option). The data model is
fundamentally different. A shared table would require NULLable stance columns and
ambiguous semantics. Separate tables make the domain boundary explicit.

### Why `forum_id` FK instead of a generic `group_id`?

The full unified Group registry (FEDIVERSE-STRATEGY §6.2, `fed_group` table) does not exist
yet. Using `forum_id` avoids building speculative infrastructure while remaining forward-
compatible: when `fed_group` lands, `forum_id` becomes a view or a simple migration to
`group_id`. The column is nullable — plebiscitos have no forum association.

## Consequences

- **First AP surface for the Decidim layer.** Consultas can be discovered by searching
  their AP URL from any Mastodon/Pleroma/Misskey instance.
- **Pull-only federation.** The consulta appears when a remote user searches the URL.
  It does NOT automatically push to followers (no `Create{Question}` in the Group outbox).
  Push (Announce to forum followers) is a follow-up.
- **No outbox entry.** The Question is not in the Group's `outbox` collection, so it
  won't appear on the @propostas profile page in Mastodon. URL search works.
- **Forward path to Ballot (Wave 4).** The consulta is `Ballot{method: SingleChoice,
  scope: Everyone, privacy: PublicTally, federation: Question}` in the Ballot model.
  When the Ballot primitive lands, consulta becomes a Ballot view.
- **Constraint fix deployed.** The `NULLS NOT DISTINCT` bug in `consultation_vote`
  (which silently limited votes to one remote actor per consultation) was fixed on prod
  with standard UNIQUE + CHECK. The migration file (0690) already has the correct schema.
