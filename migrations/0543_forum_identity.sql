-- 0543_forum_identity — the forum's visual identity (fediverse profile).
--
-- Logo (avatar) and cover (banner) per forum: they appear on the /f/<path> page
-- and travel in the Group actor (`icon`/`image`) — the forum presents itself on
-- Mastodon as a full profile. Absolute URLs or /media/… paths (absolutised at
-- the edge, same convention as citizen profiles). Curated from the admin panel.

BEGIN;

ALTER TABLE forum
    ADD COLUMN IF NOT EXISTS avatar_url text,
    ADD COLUMN IF NOT EXISTS banner_url text;

COMMENT ON COLUMN forum.avatar_url IS 'Logo do fórum (0543) — icon do ator Group e página /f.';
COMMENT ON COLUMN forum.banner_url IS 'Capa do fórum (0543) — image do ator Group e página /f.';

COMMIT;
