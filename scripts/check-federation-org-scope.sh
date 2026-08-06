#!/usr/bin/env bash
# Tenant-scope guard for the federation/notes tables (issue #14, phase 1).
#
# WHY THIS EXISTS: these four tables shipped with NO org column at all, so tenant
# isolation could not be enforced even in principle — there was nothing to filter on.
# Migration 0681 added `org_id NOT NULL`; this guard keeps every INSERT honest.
#
# It checks INSERTs only, deliberately. A SELECT that forgets the filter over-reads,
# which is bad; an INSERT that forgets the column now fails at the database (NOT NULL)
# and would be caught at runtime — but caught in CI is cheaper, and an INSERT is where
# a row becomes permanently unattributable.
#
# Reads are the job of phase 2 (the connection-scope decision), NOT of grep: with 380
# pool call sites, a read-side guard here would be theatre.
set -euo pipefail
cd "$(dirname "$0")/.."

readonly TABLES=(
  federation_follow
  federation_outbox_entry
  federation_timeline_entry
  note_hashtag
)

# Lines after the INSERT that still count as the same statement's column list.
readonly WINDOW=4

violations=0
for table in "${TABLES[@]}"; do
  while IFS=: read -r file line _; do
    statement=$(sed -n "${line},$((line + WINDOW))p" "$file")
    if ! grep -q 'org_id' <<<"$statement"; then
      echo "UNSCOPED INSERT: $file:$line — into $table without org_id"
      sed -n "${line},$((line + 2))p" "$file" | sed 's/^/    /'
      violations=$((violations + 1))
    fi
  done < <(
    git ls-files '*.rs' \
      | grep -v '/tests/' \
      | xargs grep -nE "INSERT INTO ${table}\b" \
      || true
  )
done

if ((violations > 0)); then
  cat <<'EOF'

FAILED: a federation/notes INSERT omits org_id (issue #14).

Where the row has a citizen owner, derive it — `(SELECT org_id FROM citizen WHERE
id = $n)` — so the value is correct by construction rather than passed in and
possibly wrong.

`federation_timeline_entry` and `note_hashtag` have no local owner: a remote note
arrives because somebody follows its author, and with several orgs it could belong
to more than one at once. They use the install's own org as a PLACEHOLDER until
per-follower fan-out is modelled. See migration 0681's header.
EOF
  exit 1
fi

echo "OK: every federation/notes INSERT carries org_id."
