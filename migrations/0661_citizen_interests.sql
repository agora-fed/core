-- 0661_citizen_interests.sql — citizen interests (topic areas for receiving updates).
--
-- Areas based on the federal MINISTERIAL structure (the citizen ticks in their profile which
-- ones to follow). `interest_area` is the reference table (seeded below); `citizen_interest` is
-- the selection (N:N). In future it will drive topic-targeted updates and consultations.
--
-- Idempotent: rerun-safe.

BEGIN;

CREATE TABLE IF NOT EXISTS interest_area (
    slug     text PRIMARY KEY,          -- e.g. 'saude'
    name     text NOT NULL,             -- e.g. 'Saúde'
    ministry text,                      -- reference ministry
    position integer NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS citizen_interest (
    citizen_id uuid NOT NULL REFERENCES citizen(id),
    area_slug  text NOT NULL REFERENCES interest_area(slug),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (citizen_id, area_slug)
);

CREATE INDEX IF NOT EXISTS citizen_interest_area_idx ON citizen_interest (area_slug);

-- Seed of the areas (2023–2026 ministerial structure). Idempotent.
INSERT INTO interest_area (slug, name, ministry, position) VALUES
    ('saude',              'Saúde',                       'Ministério da Saúde',                                    10),
    ('educacao',           'Educação',                    'Ministério da Educação',                                 20),
    ('seguranca',          'Segurança Pública',           'Ministério da Justiça e Segurança Pública',              30),
    ('justica',            'Justiça',                     'Ministério da Justiça e Segurança Pública',              40),
    ('fazenda',            'Economia e Receita Federal',  'Ministério da Fazenda',                                  50),
    ('trabalho',           'Trabalho e Emprego',          'Ministério do Trabalho e Emprego',                       60),
    ('previdencia',        'Previdência Social',          'Ministério da Previdência Social',                       70),
    ('assistencia_social', 'Assistência Social e Fome',   'Ministério do Desenvolvimento e Assistência Social',     80),
    ('cultura',            'Cultura',                     'Ministério da Cultura',                                  90),
    ('esporte',            'Esporte',                     'Ministério do Esporte',                                 100),
    ('meio_ambiente',      'Meio Ambiente e Clima',       'Ministério do Meio Ambiente e Mudança do Clima',        110),
    ('agricultura',        'Agricultura',                 'Ministério da Agricultura e Pecuária',                  120),
    ('desenvolvimento_agrario', 'Agricultura Familiar',   'Ministério do Desenvolvimento Agrário',                 130),
    ('ciencia_tecnologia', 'Ciência e Tecnologia',        'Ministério da Ciência, Tecnologia e Inovação',          140),
    ('transportes',        'Transportes',                 'Ministério dos Transportes',                            150),
    ('cidades',            'Cidades e Moradia',           'Ministério das Cidades',                                160),
    ('minas_energia',      'Minas e Energia',             'Ministério de Minas e Energia',                         170),
    ('comunicacoes',       'Comunicações',                'Ministério das Comunicações',                           180),
    ('turismo',            'Turismo',                     'Ministério do Turismo',                                 190),
    ('direitos_humanos',   'Direitos Humanos',            'Ministério dos Direitos Humanos e da Cidadania',        200),
    ('povos_indigenas',    'Povos Indígenas',             'Ministério dos Povos Indígenas',                        210),
    ('igualdade_racial',   'Igualdade Racial',            'Ministério da Igualdade Racial',                        220),
    ('mulheres',           'Mulheres',                    'Ministério das Mulheres',                               230)
ON CONFLICT (slug) DO UPDATE SET name = EXCLUDED.name, ministry = EXCLUDED.ministry, position = EXCLUDED.position;

COMMENT ON TABLE interest_area IS
    '0661: áreas de interesse (estrutura ministerial) que o cidadão marca no perfil.';

COMMIT;
