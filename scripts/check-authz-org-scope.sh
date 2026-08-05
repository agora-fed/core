#!/usr/bin/env bash
# Tenant-isolation guard (issue #8): every `admin_role_binding` query must filter
# by `org_id`.
#
# WHY THIS EXISTS: the table has been per-org since migration 0150
# (`UNIQUE (org_id, citizen_id, role)`), but sixteen modules each grew a private
# copy of the admin check and fifteen omitted the org filter — so an owner of ANY
# org passed the gate EVERYWHERE and the multi-tenant boundary was a naming
# convention. The copies are gone (all delegate to
# `crate::authz_ext::require_org_admin`); this guard is what stops the seventeenth.
#
# The check is intentionally crude and syntactic: it reads the SQL statement
# containing the table reference and demands the token `org_id` appear in it. A
# reviewer cannot be relied on to notice an absent WHERE clause; grep can.
set -euo pipefail
cd "$(dirname "$0")/.."

# How many lines after the table reference still count as "the same statement".
# Long enough to reach the WHERE of the multi-line CTEs in admin_users.rs.
readonly STATEMENT_WINDOW=6

violations=0
while IFS=: read -r file line _; do
  # The statement text: the matching line plus the window that follows it.
  statement=$(sed -n "${line},$((line + STATEMENT_WINDOW))p" "$file")
  if ! grep -qi 'org_id' <<<"$statement"; then
    echo "UNSCOPED: $file:$line — admin_role_binding query without an org_id filter"
    sed -n "${line},$((line + 3))p" "$file" | sed 's/^/    /'
    violations=$((violations + 1))
  fi
done < <(
  git ls-files '*.rs' \
    | xargs grep -nE 'FROM admin_role_binding|INTO admin_role_binding|UPDATE admin_role_binding' \
    | grep -v '^crates/gateway/tests/' \
    || true
)

if ((violations > 0)); then
  cat <<'EOF'

FAILED: an admin_role_binding query is not scoped to an org (issue #8).

An admin of one org must have no authority over another. Do not write the check
inline — call `crate::authz_ext::require_org_admin(db, headers)`, which proves
the binding in the CALLER'S org and hands back the org to act on. Never take the
target org from the request body (see crates/app/src/caller.rs).
EOF
  exit 1
fi

echo "OK: every admin_role_binding query is org-scoped."
