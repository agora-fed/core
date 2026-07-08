-- Migration 0107 — rate-limit de /auth/login por IP (P5.1).
--
-- Sem contador de tentativas hoje, um atacante pode força-brutar senhas
-- via /auth/login à vontade. Tabela minimalista: registra cada tentativa
-- (sucesso ou falha) com IP e timestamp; o serviço bloqueia se
-- COUNT(*) por IP na última hora ≥ AUTH_LOGIN_RATE_MAX_PER_HOUR (default 10).
--
-- Contamos TOTAL (sucesso + falha) intencionalmente: um cidadão comum
-- fazendo login normalmente 3-4x/dia nunca chega perto do teto; alguém
-- estourando 10/h é sinal de bot ou credential stuffing.
--
-- Cleanup: o worker `signup_cleanup_loop` já periódico também sweep esta
-- tabela (mesmo cutoff_days), pra ela não crescer indefinidamente.

BEGIN;

CREATE TABLE auth_login_attempt (
    id          bigserial PRIMARY KEY,
    request_ip  text NOT NULL,
    at          timestamptz NOT NULL DEFAULT now(),
    -- Marcamos o outcome pra dashboards futuros — 'ok' | 'fail'.
    outcome     text NOT NULL CHECK (outcome IN ('ok','fail'))
);

-- Consulta de rate: WHERE request_ip=$1 AND at > $2. Índice composto acelera.
CREATE INDEX auth_login_attempt_ip_at_idx
    ON auth_login_attempt (request_ip, at DESC);

-- Cleanup by cutoff — precisa varrer eficientemente por at antigo.
CREATE INDEX auth_login_attempt_at_idx
    ON auth_login_attempt (at);

COMMENT ON TABLE auth_login_attempt IS
    '0.25.0-fediverso: tentativas de /auth/login por IP (rate limit + auditoria).';

ALTER TABLE auth_login_attempt OWNER TO dsoc;

COMMIT;
