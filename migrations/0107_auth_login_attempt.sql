-- Migration 0107 — per-IP rate limit on /auth/login (P5.1).
--
-- With no attempt counter today, an attacker can brute-force passwords
-- via /auth/login freely. A minimalist table: it records every attempt
-- (success or failure) with the IP and a timestamp; the service blocks when
-- COUNT(*) per IP in the last hour ≥ AUTH_LOGIN_RATE_MAX_PER_HOUR (default 10).
--
-- We count the TOTAL (success + failure) intentionally: an ordinary citizen
-- logging in normally 3-4x/day never comes close to the cap; someone
-- exceeding 10/h is a bot or credential-stuffing signal.
--
-- Cleanup: the already-periodic `signup_cleanup_loop` worker also sweeps this
-- table (the same cutoff_days), so it does not grow indefinitely.

BEGIN;

CREATE TABLE auth_login_attempt (
    id          bigserial PRIMARY KEY,
    request_ip  text NOT NULL,
    at          timestamptz NOT NULL DEFAULT now(),
    -- We record the outcome for future dashboards — 'ok' | 'fail'.
    outcome     text NOT NULL CHECK (outcome IN ('ok','fail'))
);

-- Rate query: WHERE request_ip=$1 AND at > $2. The composite index speeds it up.
CREATE INDEX auth_login_attempt_ip_at_idx
    ON auth_login_attempt (request_ip, at DESC);

-- Cleanup by cutoff — must sweep efficiently by old `at`.
CREATE INDEX auth_login_attempt_at_idx
    ON auth_login_attempt (at);

COMMENT ON TABLE auth_login_attempt IS
    '0.25.0-fediverso: tentativas de /auth/login por IP (rate limit + auditoria).';

ALTER TABLE auth_login_attempt OWNER TO dsoc;

COMMIT;
