-- 0680_link_preview.sql — cached link preview cards for posted URLs (AGORA).
--
-- A citizen pastes a YouTube link and the post renders as bare text: no title, no
-- thumbnail. Mastodon calls this a `preview_card` and the API field has been
-- hardcoded to `null` since it was written. This table is the cache behind it.
--
-- Keyed by the URL itself, NOT by the note: the same link posted by a thousand people
-- is fetched once. That is also the rate-limiting story — an attacker cannot make us
-- hammer a third party by reposting the same URL.
--
-- `fetched_at` drives refresh, and a row is written even for a FAILURE (`ok = false`).
-- Without that, every render of a post whose link is dead would re-attempt the fetch
-- forever; negative caching is what stops one broken URL from becoming permanent load.
--
-- Idempotent: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS link_preview (
    -- SHA-256 of the normalized URL. A URL can exceed what btree will index, and a
    -- fixed-width key sidesteps that entirely.
    url_hash      bytea PRIMARY KEY,
    url           text NOT NULL,
    -- false = the fetch failed or the page carried nothing usable. Cached on purpose.
    ok            boolean NOT NULL DEFAULT false,
    title         text,
    description   text,
    image_url     text,
    site_name     text,
    -- 'link' | 'video' | 'photo' — mirrors Mastodon's card `type`.
    kind          text NOT NULL DEFAULT 'link'
                  CHECK (kind IN ('link', 'video', 'photo')),
    fetched_at    timestamptz NOT NULL DEFAULT now()
);

-- The refresh sweep: oldest first.
CREATE INDEX IF NOT EXISTS link_preview_fetched_idx ON link_preview (fetched_at);

-- Which note carries which link. Text (not FK) for the same reason as note_hashtag:
-- the object may live on the local outbox side or the remote timeline side.
CREATE TABLE IF NOT EXISTS note_link_preview (
    object_uri  text NOT NULL,
    url_hash    bytea NOT NULL REFERENCES link_preview(url_hash) ON DELETE CASCADE,
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (object_uri)
);

COMMENT ON TABLE link_preview IS
    '0680: cached OpenGraph/oEmbed card per URL (negative results cached too).';
COMMENT ON TABLE note_link_preview IS
    '0680: the one preview card a note carries (its first eligible link).';

DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'dsoc') THEN
        ALTER TABLE link_preview OWNER TO dsoc;
        ALTER TABLE note_link_preview OWNER TO dsoc;
    END IF;
END $$;

COMMIT;
