-- 0537_proposal_targets — multiple recipients per proposal (same sphere).
--
-- A proposal may be directed at SEVERAL offices at once (e.g. a group of
-- federal deputies), provided they all belong to the SAME federative sphere
-- (federal | estadual | municipal — validated in the service layer, the
-- mandate.sphere column from 0203). Support stays single: the proposal is one; what
-- multiplies is the delivery and the receipt per office.
--
-- `proposal.mandate_id` remains the PRIMARY recipient (it drives the consequence/SLA
-- loop and keeps compat with events, federation and old clients).
-- This table holds the COMPLETE set of recipients, the primary included.
--
-- FKs: `proposal` is intra-crate (proposals); `mandate` is a core identity
-- table — both allowed by the REGISTRY.md rule.

BEGIN;

CREATE TABLE IF NOT EXISTS proposal_target (
    proposal_id  uuid NOT NULL REFERENCES proposal(id),
    mandate_id   uuid NOT NULL REFERENCES mandate(id),
    -- Delivery receipt per office (the e-mail left the relay) — the same
    -- "delivered" pattern as 0303, now per recipient.
    notified_at  timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (proposal_id, mandate_id)
);

-- The official's panel lists "proposals directed at me" — a query by mandate.
CREATE INDEX IF NOT EXISTS proposal_target_mandate_idx
    ON proposal_target (mandate_id, proposal_id);

-- Backfill: toda proposta existente vira 1 linha (seu alvo principal),
-- preserving the legacy receipt. Idempotent for re-runs.
INSERT INTO proposal_target (proposal_id, mandate_id, notified_at, created_at)
SELECT id, mandate_id, notified_mandate_at, created_at
  FROM proposal
ON CONFLICT (proposal_id, mandate_id) DO NOTHING;

COMMENT ON TABLE proposal_target IS
    'Destinatários da proposta (0537): conjunto completo de gabinetes, principal incluído; recibo de entrega por gabinete.';

-- In production the migrations run as `postgres`, but the gateway connects as `dsoc`
-- (gotcha documentado em deployment-workflow: 0106/0107/0111/0151).
ALTER TABLE proposal_target OWNER TO dsoc;

COMMIT;
