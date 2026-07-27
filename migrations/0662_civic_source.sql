-- 0662_civic_source.sql — catálogo de fontes cívicas por município (ÁGORA #72, ADR-0017).
--
-- A espinha dorsal da estratégia "extrair POR PLATAFORMA, não por município": para cada câmara
-- municipal registramos QUAL software ela roda (Interlegis/SAPL, camaraonline, IPM…) e a URL-base
-- da sua API/portal. Um extrator por plataforma consome este catálogo — resumível e auditável.
--
-- O *fingerprint* (probe) grava aqui; a extração lê daqui. Chaveado por (uf, municipio) — o mesmo
-- par que a tabela `mandate` usa para os mandatos municipais (0504). NÃO guarda contatos: só a
-- origem. Os contatos extraídos enriquecem `mandate.public_email/avatar` (dado institucional público).
--
-- Idempotente: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS civic_source (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    uf           text NOT NULL,                 -- 'SP', 'PR'…
    municipio    text NOT NULL,                 -- nome do município (como em mandate.municipio)
    codigo_ibge  text,                          -- IBGE quando conhecido (join com municipio_ibge)
    -- Plataforma detectada. 'unknown' = probado e não identificado; 'none' = sem portal on-line.
    platform     text NOT NULL DEFAULT 'unknown'
                 CHECK (platform IN ('sapl', 'camaraonline', 'ipm', 'camarasempapel', 'other', 'unknown', 'none')),
    base_url     text,                          -- URL-base da API/portal (ex.: https://sapl.agudo.rs.leg.br)
    -- Status do probe: 'pending' (a probar), 'ok' (respondeu), 'dead' (não respondeu), 'blocked' (ToS/robots).
    probe_status text NOT NULL DEFAULT 'pending'
                 CHECK (probe_status IN ('pending', 'ok', 'dead', 'blocked')),
    parlamentares_found integer,                -- quantos a fonte reportou no último fingerprint
    last_probed_at      timestamptz,
    last_extracted_at   timestamptz,
    notes        text,
    created_at   timestamptz NOT NULL DEFAULT now()
);

-- Uma fonte por município (o par que casa com mandate). Case-insensitive no município.
CREATE UNIQUE INDEX IF NOT EXISTS civic_source_uf_municipio_uidx
    ON civic_source (uf, upper(municipio));

-- Fila de trabalho do extrator: "quais fontes SAPL estão OK e precisam extrair?"
CREATE INDEX IF NOT EXISTS civic_source_platform_status_idx
    ON civic_source (platform, probe_status);

COMMENT ON TABLE civic_source IS
    '0662 (#72/ADR-0017): catálogo município→plataforma+URL para extração de contatos por plataforma.';

COMMIT;
