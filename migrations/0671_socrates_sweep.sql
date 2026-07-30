-- 0671_socrates_sweep — SOCRATES v2: sweep AUTOMÁTICO das Ideias Legislativas.
--
-- O MVP (0670) era admin-curado: alguém colava a URL da ideia no painel. A v2
-- descobre sozinha as ideias EM ALTA no e-Cidadania a partir de duas fontes
-- públicas do Senado (a API JSON `restcolecaomaisideia` e a página
-- `principalideia`), espelha as novas e RE-SINCRONIZA o contador de apoios das
-- já espelhadas — o número de apoios é o único dado dinâmico que interessa
-- (20.000 apoios = a ideia vira sugestão legislativa formal).
--
-- Por isso `socrates_mirror` ganha:
--   * `apoiamentos`        — o contador COMO O SENADO FORMATA ("20.771"): guardar
--                            o texto evita inventar parsing de milhar e mantém o
--                            corpo do tópico fiel à fonte;
--   * `porcentagem_favor`  — o índice de favorabilidade que a coleção devolve;
--   * `apoios_updated_at`  — quando os dois acima foram lidos pela última vez
--                            (NULL = nunca sincronizado, o caso dos espelhos 0670);
--   * `origin`             — 'manual' (admin colou) × 'sweep' (descoberto pelo
--                            worker). Default 'manual' preserva o histórico 0670.
--
-- `socrates_sweep_run` é o log de cada rodada: quantas ideias a rodada VIU
-- (`found`), quantas virou tópico novo (`mirrored`), quantas ignorou por já
-- existirem/estourarem o teto (`skipped`) e o erro consolidado, quando houve.
-- Sem esse log não há como distinguir "o Senado não publicou nada novo" de "o
-- sweep está quebrado há três dias" — os dois se parecem no fórum.
--
-- OWNER: ALTER TABLE socrates_mirror OWNER TO dsoc
-- OWNER: ALTER TABLE socrates_sweep_run OWNER TO dsoc
--
-- Idempotente: rerun-safe (IF NOT EXISTS / DO block no CHECK).

BEGIN;

ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS apoiamentos       text;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS porcentagem_favor int;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS apoios_updated_at timestamptz;
ALTER TABLE socrates_mirror ADD COLUMN IF NOT EXISTS origin            text NOT NULL DEFAULT 'manual';

-- `ADD CONSTRAINT` não aceita IF NOT EXISTS; o DO block mantém o rerun-safe.
DO $$
BEGIN
    ALTER TABLE socrates_mirror
        ADD CONSTRAINT socrates_mirror_origin_chk CHECK (origin IN ('manual', 'sweep'));
EXCEPTION
    WHEN duplicate_object THEN NULL;
END
$$;

CREATE TABLE IF NOT EXISTS socrates_sweep_run (
    id          uuid PRIMARY KEY,
    -- Gravada no início da rodada: uma rodada em curso já aparece no painel.
    started_at  timestamptz NOT NULL DEFAULT now(),
    -- NULL enquanto a rodada não fechou (ou se o processo morreu no meio).
    finished_at timestamptz,
    found       int NOT NULL DEFAULT 0,
    mirrored    int NOT NULL DEFAULT 0,
    skipped     int NOT NULL DEFAULT 0,
    -- Erros consolidados da rodada (fetch/parse/espelho); NULL = rodada limpa.
    error       text
);

-- O painel lê sempre "as últimas rodadas".
CREATE INDEX IF NOT EXISTS socrates_sweep_run_started_idx
    ON socrates_sweep_run (started_at DESC);

ALTER TABLE socrates_mirror    OWNER TO dsoc;
ALTER TABLE socrates_sweep_run OWNER TO dsoc;

COMMIT;
