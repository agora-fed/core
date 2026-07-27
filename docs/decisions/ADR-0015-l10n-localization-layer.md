# ADR-0015 — Camada de localização (l10n): documentos/território plugáveis por país

- **Status:** Accepted
- **Context:** Diretriz do Marcos (2026-07-27); framework ÁGORA por [[ADR-0013]]. Estilo Odoo (`l10n_br`).

## Decision

O **core do ÁGORA é agnóstico de país**. Tudo que é específico de um país — documentos de
identidade, unidades territoriais e conceito de registro eleitoral — é fornecido por um
**módulo de localização** `l10n_<cc>`, resolvido da configuração da instalação.

- **`l10n_br`** (localização brasileira, ativa no Pindorama) empacota:
  - **CPF** — documento de identidade (dígitos verificadores + base autorizada via SaaS cpf-verify).
  - **Título de Eleitor** — registro eleitoral (dígitos verificadores; opcional; âncora fraca — ver
    [[project-production-deploy]]).
  - **IBGE** — unidades territoriais (municípios/UFs; tabela `municipio_ibge`).
- **Abstrações do core (inglês, [[ADR-0013]]):**
  - `IdentityVerifier` — verifica um documento de identidade do país.
  - `TerritorialProvider` — hierarquia país → estado → município (o eixo de escopo de
    sorteio/federação/campanha).
  - `VoterRegistration` — conceito opcional de registro eleitoral.
- **Configuração:** a instalação declara qual `l10n_<cc>` está ativa. Pindorama → `l10n_br`.

## Rationale

Um framework internacionalista não pode ter CPF/Título/IBGE cravados no núcleo. Outros países
plugam seus próprios métodos de documento (SSN, DNI, etc.) e suas próprias unidades territoriais
sem tocar o core. É o mesmo padrão do Odoo (`l10n_br`, `l10n_fr`, ...). O trabalho de identidade
que já existe (CPF/Título/IBGE) é hoje embutido no core — migra para trás da fronteira `l10n_br`
incrementalmente.

## Consequences

- Refactor: mover `titulo_eleitor`, `municipios`, `identity_verify` (CPF) para trás das
  abstrações `IdentityVerifier`/`TerritorialProvider`/`VoterRegistration`, com `l10n_br` como 1ª impl.
- O `CpfVerifier` já plugável é o embrião do `IdentityVerifier`.
- Contratos de API que hoje expõem `cpf`/`municipio_ibge`/`titulo_eleitor` precisam de uma forma
  neutra no core + específica na localização (decidir na varredura de i18n, #56).
- Não bloqueia a camada de campanha (ADR-0014): o eixo território é consumido via `TerritorialProvider`.
