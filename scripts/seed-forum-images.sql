-- seed-forum-images.sql — logos/brasões oficiais (Wikimedia Commons, 2026-07-26).
-- TODAS as URLs verificadas (HTTP 200, image/png) antes de entrar; licenças:
-- Public domain (símbolos oficiais, Lei 9.610/98 art. 8) exceto Câmara
-- ("Copyrighted free use"). Thumbs de 330px (bucket válido do Commons).
-- Idempotente; re-executável.

BEGIN;

CREATE OR REPLACE FUNCTION pg_temp.set_avatar(p text, u text) RETURNS void LANGUAGE sql AS
$$ UPDATE forum SET avatar_url = u WHERE full_path = p $$;

-- ============ Bandeiras dos estados (avatar do fórum raiz de cada UF)
SELECT pg_temp.set_avatar('ac','https://upload.wikimedia.org/wikipedia/commons/thumb/4/4c/Bandeira_do_Acre.svg/330px-Bandeira_do_Acre.svg.png');
SELECT pg_temp.set_avatar('al','https://upload.wikimedia.org/wikipedia/commons/thumb/8/88/Bandeira_de_Alagoas.svg/330px-Bandeira_de_Alagoas.svg.png');
SELECT pg_temp.set_avatar('ap','https://upload.wikimedia.org/wikipedia/commons/thumb/0/0c/Bandeira_do_Amap%C3%A1.svg/330px-Bandeira_do_Amap%C3%A1.svg.png');
SELECT pg_temp.set_avatar('am','https://upload.wikimedia.org/wikipedia/commons/thumb/6/6b/Bandeira_do_Amazonas.svg/330px-Bandeira_do_Amazonas.svg.png');
SELECT pg_temp.set_avatar('ba','https://upload.wikimedia.org/wikipedia/commons/thumb/2/28/Bandeira_da_Bahia.svg/330px-Bandeira_da_Bahia.svg.png');
SELECT pg_temp.set_avatar('ce','https://upload.wikimedia.org/wikipedia/commons/thumb/2/2e/Bandeira_do_Cear%C3%A1.svg/330px-Bandeira_do_Cear%C3%A1.svg.png');
SELECT pg_temp.set_avatar('df','https://upload.wikimedia.org/wikipedia/commons/thumb/3/3c/Bandeira_do_Distrito_Federal_%28Brasil%29.svg/330px-Bandeira_do_Distrito_Federal_%28Brasil%29.svg.png');
SELECT pg_temp.set_avatar('es','https://upload.wikimedia.org/wikipedia/commons/thumb/4/43/Bandeira_do_Esp%C3%ADrito_Santo.svg/330px-Bandeira_do_Esp%C3%ADrito_Santo.svg.png');
SELECT pg_temp.set_avatar('go','https://upload.wikimedia.org/wikipedia/commons/thumb/b/be/Flag_of_Goi%C3%A1s.svg/330px-Flag_of_Goi%C3%A1s.svg.png');
SELECT pg_temp.set_avatar('ma','https://upload.wikimedia.org/wikipedia/commons/thumb/4/45/Bandeira_do_Maranh%C3%A3o.svg/330px-Bandeira_do_Maranh%C3%A3o.svg.png');
SELECT pg_temp.set_avatar('mt','https://upload.wikimedia.org/wikipedia/commons/thumb/0/0b/Bandeira_de_Mato_Grosso.svg/330px-Bandeira_de_Mato_Grosso.svg.png');
SELECT pg_temp.set_avatar('ms','https://upload.wikimedia.org/wikipedia/commons/thumb/6/64/Bandeira_de_Mato_Grosso_do_Sul.svg/330px-Bandeira_de_Mato_Grosso_do_Sul.svg.png');
SELECT pg_temp.set_avatar('mg','https://upload.wikimedia.org/wikipedia/commons/thumb/6/63/Flag_of_Minas_Gerais.svg/330px-Flag_of_Minas_Gerais.svg.png');
SELECT pg_temp.set_avatar('pa','https://upload.wikimedia.org/wikipedia/commons/thumb/0/02/Bandeira_do_Par%C3%A1.svg/330px-Bandeira_do_Par%C3%A1.svg.png');
SELECT pg_temp.set_avatar('pb','https://upload.wikimedia.org/wikipedia/commons/thumb/b/bb/Bandeira_da_Para%C3%ADba.svg/330px-Bandeira_da_Para%C3%ADba.svg.png');
SELECT pg_temp.set_avatar('pr','https://upload.wikimedia.org/wikipedia/commons/thumb/9/93/Bandeira_do_Paran%C3%A1.svg/330px-Bandeira_do_Paran%C3%A1.svg.png');
SELECT pg_temp.set_avatar('pe','https://upload.wikimedia.org/wikipedia/commons/thumb/5/59/Bandeira_de_Pernambuco.svg/330px-Bandeira_de_Pernambuco.svg.png');
SELECT pg_temp.set_avatar('pi','https://upload.wikimedia.org/wikipedia/commons/thumb/3/33/Bandeira_do_Piau%C3%AD.svg/330px-Bandeira_do_Piau%C3%AD.svg.png');
SELECT pg_temp.set_avatar('rj','https://upload.wikimedia.org/wikipedia/commons/thumb/7/73/Bandeira_do_estado_do_Rio_de_Janeiro.svg/330px-Bandeira_do_estado_do_Rio_de_Janeiro.svg.png');
SELECT pg_temp.set_avatar('rn','https://upload.wikimedia.org/wikipedia/commons/thumb/3/30/Bandeira_do_Rio_Grande_do_Norte.svg/330px-Bandeira_do_Rio_Grande_do_Norte.svg.png');
SELECT pg_temp.set_avatar('rs','https://upload.wikimedia.org/wikipedia/commons/thumb/6/63/Bandeira_do_Rio_Grande_do_Sul.svg/330px-Bandeira_do_Rio_Grande_do_Sul.svg.png');
SELECT pg_temp.set_avatar('ro','https://upload.wikimedia.org/wikipedia/commons/thumb/f/fa/Bandeira_de_Rond%C3%B4nia.svg/330px-Bandeira_de_Rond%C3%B4nia.svg.png');
SELECT pg_temp.set_avatar('rr','https://upload.wikimedia.org/wikipedia/commons/thumb/9/98/Bandeira_de_Roraima.svg/330px-Bandeira_de_Roraima.svg.png');
SELECT pg_temp.set_avatar('sc','https://upload.wikimedia.org/wikipedia/commons/thumb/1/1a/Bandeira_de_Santa_Catarina.svg/330px-Bandeira_de_Santa_Catarina.svg.png');
SELECT pg_temp.set_avatar('sp','https://upload.wikimedia.org/wikipedia/commons/thumb/2/2b/Bandeira_do_estado_de_S%C3%A3o_Paulo.svg/330px-Bandeira_do_estado_de_S%C3%A3o_Paulo.svg.png');
SELECT pg_temp.set_avatar('se','https://upload.wikimedia.org/wikipedia/commons/thumb/b/be/Bandeira_de_Sergipe.svg/330px-Bandeira_de_Sergipe.svg.png');
SELECT pg_temp.set_avatar('to','https://upload.wikimedia.org/wikipedia/commons/thumb/f/ff/Bandeira_do_Tocantins.svg/330px-Bandeira_do_Tocantins.svg.png');

-- ============ Instituições federais
SELECT pg_temp.set_avatar('senado','https://upload.wikimedia.org/wikipedia/commons/thumb/e/ed/Logo_Senado_Federal_Brasil.png/330px-Logo_Senado_Federal_Brasil.png');
SELECT pg_temp.set_avatar('camara','https://upload.wikimedia.org/wikipedia/commons/thumb/5/51/Logo_C%C3%A2mara_dos_Deputados_do_Brasil.png/330px-Logo_C%C3%A2mara_dos_Deputados_do_Brasil.png');
SELECT pg_temp.set_avatar('stf','https://upload.wikimedia.org/wikipedia/commons/thumb/2/20/Logotipo_do_Supremo_Tribunal_Federal.svg/330px-Logotipo_do_Supremo_Tribunal_Federal.svg.png');
SELECT pg_temp.set_avatar('stj','https://upload.wikimedia.org/wikipedia/commons/thumb/4/4c/Superior_Tribunal_de_Justi%C3%A7a_logo.svg/330px-Superior_Tribunal_de_Justi%C3%A7a_logo.svg.png');
SELECT pg_temp.set_avatar('tst','https://upload.wikimedia.org/wikipedia/commons/thumb/0/05/Bandeira_do_Tribunal_Superior_do_Trabalho.svg/330px-Bandeira_do_Tribunal_Superior_do_Trabalho.svg.png');
SELECT pg_temp.set_avatar('tse','https://upload.wikimedia.org/wikipedia/commons/thumb/4/4d/Tribunal_Superior_Eleitoral.png/330px-Tribunal_Superior_Eleitoral.png');
SELECT pg_temp.set_avatar('stm','https://upload.wikimedia.org/wikipedia/commons/thumb/a/a5/Logo_Superior_Tribunal_Militar.svg/330px-Logo_Superior_Tribunal_Militar.svg.png');
SELECT pg_temp.set_avatar('cnj','https://upload.wikimedia.org/wikipedia/commons/2/2a/Logo-cnj-preta-2023-png.png');

-- ============ Herança visual
-- Comissões herdam a logo da casa; ministérios e TRFs usam o Brasão da República.
UPDATE forum SET avatar_url = (SELECT avatar_url FROM forum p WHERE p.full_path = 'senado')
 WHERE full_path LIKE 'senado/%' AND avatar_url IS NULL;
UPDATE forum SET avatar_url = (SELECT avatar_url FROM forum p WHERE p.full_path = 'camara')
 WHERE full_path LIKE 'camara/%' AND avatar_url IS NULL;
UPDATE forum SET avatar_url = 'https://upload.wikimedia.org/wikipedia/commons/thumb/b/bf/Coat_of_arms_of_Brazil.svg/330px-Coat_of_arms_of_Brazil.svg.png'
 WHERE (full_path LIKE 'ministerio-%' OR full_path LIKE 'trf-%' OR full_path = 'governanca')
   AND avatar_url IS NULL;

COMMIT;

SELECT count(*) FILTER (WHERE avatar_url IS NOT NULL) AS com_logo FROM forum;
