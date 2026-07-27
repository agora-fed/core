# ADR-0013 — ÁGORA é o framework; Pindorama é a instalação brasileira; API em inglês

- **Status:** Accepted
- **Context:** Diretriz do Marcos (2026-07-27); épico de i18n em git.pop.coop/brasil/democracia-social#56.

## Decision

1. **ÁGORA** é o nome do **framework** de infraestrutura democrática — internacionalista,
   reusável por qualquer país/instalação.
2. **Pindorama** é a **instalação brasileira** do ÁGORA (a que roda em `democracia.social.br`).
3. **Toda a API e todo identificador de código são em inglês** — rotas (`/admin/roles`, não
   `/admin/papeis`), campos de DTO/JSON, nomes de módulo/arquivo, tabelas novas e slugs de
   permissão. Exceção: **cópia de UI visível ao usuário** é localizada por instalação
   (Pindorama = pt-BR).
4. Toda **feature nova** já nasce em inglês. O código legado em português é "des-portuguesado"
   incrementalmente (issue #56), com alias/redirect temporário onde o front está acoplado.

## Rationale

Um framework que se pretende reusável fora do Brasil não pode ter o contrato de API em
português. Separar **framework (ÁGORA, inglês)** de **instalação (Pindorama, pt-BR na UI)**
deixa o núcleo internacionalizável e a experiência local. O split "código em inglês / UI
localizada" é o padrão de qualquer projeto i18n sério e evita reescrever a UI para
internacionalizar o motor.

## Consequences

- Renomeações de endpoint precisam de janela de transição (alias EN canônico + pt como alias
  temporário) para não quebrar o front acoplado — rastreado em #56.
- Campos de DTO em pt já persistidos (`nome_completo`, `municipio_ibge`, `residencia_*`,
  `sigla`, `esfera`, `titulo_eleitor`) → decidir por tabela entre tradução na borda do DTO ou
  migration de rename.
- Endpoints criados nesta semana em pt (`/api/v1/municipios`) entram na fila de rename
  (`/api/v1/municipalities`).
- A camada de campanha (ADR-0014) é o primeiro grande subsistema a nascer 100% em inglês.
