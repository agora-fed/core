-- 0543_forum_identity — identidade visual do fórum (perfil de fediverso).
--
-- Logo (avatar) e capa (banner) por fórum: aparecem na página /f/<caminho> e
-- viajam no ator Group (`icon`/`image`) — o fórum se apresenta no Mastodon
-- como um perfil completo. URLs absolutas ou caminhos /media/… (absolutizados
-- na borda, mesma convenção dos perfis de cidadão). Curadoria via admin.

BEGIN;

ALTER TABLE forum
    ADD COLUMN IF NOT EXISTS avatar_url text,
    ADD COLUMN IF NOT EXISTS banner_url text;

COMMENT ON COLUMN forum.avatar_url IS 'Logo do fórum (0543) — icon do ator Group e página /f.';
COMMENT ON COLUMN forum.banner_url IS 'Capa do fórum (0543) — image do ator Group e página /f.';

COMMIT;
