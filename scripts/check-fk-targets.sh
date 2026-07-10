#!/usr/bin/env bash
# Cross-crate FKs may target ONLY core identity tables (org, citizen, mandate), a table created in
# the SAME migration file (self/intra reference), or a target declared in scripts/fk-allow.txt
# (legitimate intra-crate references across migration files — e.g. amendment → proposal inside the
# proposals crate). Enforces crate isolation at the schema level (ARCHITECTURE.md section 3).
#
# fk-allow.txt format, one rule per line:  <migration-file-basename> <table> [<table>...]
# Kept OUTSIDE the migrations because applied migrations are immutable (sqlx checksums).
set -euo pipefail
cd "$(dirname "$0")/.."
core='org|citizen|mandate'
allowfile=scripts/fk-allow.txt
violations=0
for f in migrations/*.sql; do
  [[ "$f" == *0001_baseline.sql ]] && continue
  base=$(basename "$f")
  # tables this migration defines (allowed self-reference targets). `|| true`:
  # an ALTER-only migration has no CREATE TABLE and grep exits 1, which under
  # `set -e` killed the whole script silently before any check ran.
  own=$(grep -ioE 'CREATE TABLE (IF NOT EXISTS )?[a-z_]+' "$f" | awk '{print $NF}' | tr '\n' '|' | sed 's/|$//' || true)
  # extra targets declared for this file in the allowlist
  extra=$( [[ -f "$allowfile" ]] && awk -v b="$base" '$1==b {for (i=2;i<=NF;i++) print $i}' "$allowfile" | tr '\n' '|' | sed 's/|$//' || true)
  allowed="$core${own:+|$own}${extra:+|$extra}"
  while read -r tgt; do
    [[ -z "$tgt" ]] && continue
    if ! [[ "$tgt" =~ ^($allowed)$ ]]; then
      echo "FK VIOLATION in $f: REFERENCES $tgt (allowed: $core, a table created in this file, or $allowfile)"
      violations=$((violations+1))
    fi
    # scan only real SQL: strip `-- …` comments so prose like "references the
    # RSA keypair" never false-flags.
  done < <(sed 's/--.*$//' "$f" | grep -ioE 'REFERENCES[[:space:]]+[a-z_]+' | awk '{print $2}' || true)
done
if [[ "$violations" -gt 0 ]]; then echo "FAILED: $violations cross-crate FK violation(s)."; exit 1; fi
echo "OK: cross-crate FK targets valid."
