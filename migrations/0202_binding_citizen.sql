-- 0202_binding_citizen — link a citizen to the mandate they OPERATE. Until now
-- `mandate_identity_binding` recorded *that* a mandate had been verified at some level, but not
-- *which* citizen was the authorized operator on the platform. That gap meant the parlamentarian
-- panel had nothing to look up: "given the logged-in citizen, which mandate's SLAs is she
-- responsible for?" — the natural answer needed this column. Range 0200-0209 = mandates per
-- REGISTRY.md.
--
-- Nullable so the historical binding rows (the seed didn't have a citizen attached) keep
-- loading without backfill. New bindings created through the upcoming invitation-acceptance
-- flow always set this column.

ALTER TABLE mandate_identity_binding
    ADD COLUMN citizen_id uuid REFERENCES citizen(id);

-- Reverse lookup: "given a citizen, what mandate do they operate?" Partial so the index stays
-- compact (only the small subset of citizens that ARE parlamentarians).
CREATE INDEX mandate_identity_binding_citizen_idx
    ON mandate_identity_binding (citizen_id, verified_at DESC)
 WHERE citizen_id IS NOT NULL;

COMMENT ON COLUMN mandate_identity_binding.citizen_id IS
    'The citizen authorized to act on behalf of the mandate. NULL = legacy / unbound.';
