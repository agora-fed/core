-- 0662_civic_source.sql — catalogue of civic sources per municipality (AGORA #72, ADR-0017).
--
-- The backbone of the "extract PER PLATFORM, not per municipality" strategy: for each city
-- council we record WHICH software it runs (Interlegis/SAPL, camaraonline, IPM…) and the base URL
-- of its API/portal. One extractor per platform consumes this catalogue — resumable and auditable.
--
-- The *fingerprint* (probe) writes here; extraction reads from here. Keyed by (uf, municipio) — the
-- same pair the `mandate` table uses for municipal mandates (0504). Stores NO contacts: only the
-- origin. Extracted contacts enrich `mandate.public_email/avatar` (public institutional data).
--
-- Idempotent: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS civic_source (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    uf           text NOT NULL,                 -- 'SP', 'PR'…
    municipio    text NOT NULL,                 -- municipality name (as in mandate.municipio)
    codigo_ibge  text,                          -- IBGE code when known (joins municipio_ibge)
    -- Detected platform. 'unknown' = probed and unidentified; 'none' = no online portal.
    platform     text NOT NULL DEFAULT 'unknown'
                 CHECK (platform IN ('sapl', 'camaraonline', 'ipm', 'camarasempapel', 'other', 'unknown', 'none')),
    base_url     text,                          -- API/portal base URL (e.g. https://sapl.agudo.rs.leg.br)
    -- Probe status: 'pending' (to probe), 'ok' (answered), 'dead' (no answer), 'blocked' (ToS/robots).
    probe_status text NOT NULL DEFAULT 'pending'
                 CHECK (probe_status IN ('pending', 'ok', 'dead', 'blocked')),
    parlamentares_found integer,                -- how many the source reported on the last fingerprint
    last_probed_at      timestamptz,
    last_extracted_at   timestamptz,
    notes        text,
    created_at   timestamptz NOT NULL DEFAULT now()
);

-- One source per municipality (the pair that matches mandate). Case-insensitive on the municipality.
CREATE UNIQUE INDEX IF NOT EXISTS civic_source_uf_municipio_uidx
    ON civic_source (uf, upper(municipio));

-- Extractor work queue: "which SAPL sources are OK and need extracting?"
CREATE INDEX IF NOT EXISTS civic_source_platform_status_idx
    ON civic_source (platform, probe_status);

COMMENT ON TABLE civic_source IS
    '0662 (#72/ADR-0017): catálogo município→plataforma+URL para extração de contatos por plataforma.';

COMMIT;
