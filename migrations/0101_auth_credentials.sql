-- 0101_auth_credentials — sovereign credential auth (e-mail + senha + CPF). Replaces OIDC as the
-- citizen identity path (auth verified by CPF, not Zitadel). Owned by dsoc-auth (range 0100-0109).

-- Citizens may now exist without an external OIDC subject (they authenticate by e-mail+senha+CPF).
ALTER TABLE citizen ALTER COLUMN oidc_subject DROP NOT NULL;

CREATE TABLE auth_credential (
    id              uuid PRIMARY KEY,
    citizen_id      uuid NOT NULL UNIQUE REFERENCES citizen(id),
    org_id          uuid NOT NULL REFERENCES org(id),
    email           text NOT NULL,
    -- Argon2id PHC string. Never the plaintext password (PLAN.md principle 8 / SECURITY.md).
    password_hash   text NOT NULL,
    -- Normalized CPF (11 digits, no punctuation). Unique per org.
    cpf             char(11) NOT NULL,
    -- 'validated' = check-digits valid (algorithmic); 'verified' = confirmed against an official
    -- source (Serpro/KYC) — see CpfVerifier. 'unverified' reserved.
    cpf_status      text NOT NULL DEFAULT 'validated'
                    CHECK (cpf_status IN ('unverified','validated','verified')),
    created_at      timestamptz NOT NULL,
    UNIQUE (org_id, email),
    UNIQUE (org_id, cpf)
);
CREATE INDEX auth_credential_email_idx ON auth_credential (org_id, email);
