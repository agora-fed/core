#!/usr/bin/env bash
# English-only RATCHET for code documentation (ADR-0013 + the 2026-08-05 rule:
# "documentação do código TUDO EM INGLÊS; só na l10n-brazil pode ter português").
#
# Scope — CODE DOCUMENTATION only:
#   * Rust doc comments and inline comments (`//`, `///`, `//!`)
#   * SQL comments in migrations (`--`)
# NOT in scope (deliberately):
#   * user-facing UI copy and API/e-mail messages — those are localized per
#     installation (Pindorama = pt-BR) and are NOT code documentation;
#   * crates/platform/l10n-br — the Brazilian localization module, destined for
#     github.com/agora-fed/l10n-brazil, the one repo where pt-BR is allowed.
#
# RATCHET: MAX_PT is the measured floor and may only go DOWN. Lower it in the
# same PR that translates lines; raising it is a regression.
set -euo pipefail
cd "$(dirname "$0")/.."

MAX_PT=585   # measured 2026-08-05: Rust at ZERO; migration SQL comments remain — only goes DOWN

# Portuguese markers: diacritics English never uses, plus frequent stopwords
# that survive accent-free writing ("nao", "pra", "que", ...).
PT_RE='[çãõáéíóúâêôàÇÃÕÁÉÍÓÚÂÊÔÀ]|\b([Nn]ão|nao|pra|pro|[Qq]ue|[Pp]ara|[Cc]om|[Uu]ma|[Dd]os|[Dd]as|[Pp]elo|[Pp]ela|[Qq]uando|[Oo]nde|[Pp]orque|[Tt]ambém|[Jj]á|[Ss]ó|[Ss]er|[Tt]em|[Ff]az|[Uu]sa|[Cc]ria|[Ee]ntão|[Aa]inda|[Cc]ada|[Ss]em|[Mm]as|[Ss]obre|[Dd]epois|[Aa]ntes)\b'

# Proper nouns and identifiers quoted as DATA are not prose: `Saúde` as a
# section name or "São Paulo" in an example is a value, not documentation
# language. Strip inline-code spans and quoted strings before judging.
strip_data() { sed -E 's/`[^`]*`//g; s/"[^"]*"//g'; }

count_rust() {
  git ls-files '*.rs' \
    | grep -v '^crates/platform/l10n-br/' \
    | xargs grep -hE '^\s*//' 2>/dev/null \
    | strip_data \
    | grep -cE "$PT_RE" || true
}

count_sql() {
  git ls-files 'migrations/*.sql' \
    | xargs grep -hE '^\s*--' 2>/dev/null \
    | strip_data \
    | grep -cE "$PT_RE" || true
}

rust=$(count_rust)
sql=$(count_sql)
total=$((rust + sql))

echo "Portuguese comment lines — rust: $rust, sql: $sql, total: $total (ratchet floor: $MAX_PT)"
if (( total > MAX_PT )); then
  echo "FAILED: code documentation must be English (ADR-0013)."
  echo "Translate the lines you touched — or, if the count DROPPED, lower MAX_PT."
  exit 1
fi
if (( total < MAX_PT )); then
  echo "NOTE: count dropped below the floor — lower MAX_PT to $total in this PR."
fi
echo "OK: English-docs ratchet holds."
