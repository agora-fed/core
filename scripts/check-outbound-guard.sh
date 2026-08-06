#!/usr/bin/env bash
# SSRF guard (issue #9): a raw HTTP client may only be built inside outbound.rs.
#
# WHY THIS EXISTS: there was no shared outbound policy, so each of ~10 call sites
# validated (at most) the URL scheme prefix — and `starts_with("https://")` accepts
# `https://10.0.0.1/`. The surfaces fed by a stranger's string (the federation inbox
# takes an actor URL from an UNAUTHENTICATED `Signature` header) now go through
# `crate::outbound`. This guard is what stops the eleventh client from being built by
# hand next to the tenth.
#
# The allowlist below is the set of call sites NOT yet migrated. It may only SHRINK.
# Adding to it means shipping a fetch that no one checked; if a surface genuinely
# cannot use the guard, say why here, in this file, where the next person will read it.
set -euo pipefail
cd "$(dirname "$0")/.."

# Not yet migrated — each fetches a FIXED, compiled-in government URL, never a
# user-supplied one, so none is an SSRF sink today. Migrating them is follow-up
# work, tracked with issue #27 (federation client reuse).
readonly ALLOWED=(
  "crates/gateway/src/outbound.rs"              # the guard itself
  "crates/gateway/src/parlamentar_activity.rs"  # dadosabertos.camara.leg.br / legis.senado.leg.br
  "crates/gateway/src/reports.rs"               # Senate CEAPS open-data CSV
  "crates/gateway/src/socrates_mirror.rs"       # www12.senado.leg.br e-Cidadania
  "crates/gateway/src/govbr_oidc.rs"            # gov.br OIDC token/JWKS endpoints
  "crates/gateway/src/federation.rs"            # signed ActivityPub DELIVERY (POST to peer inboxes)
  "crates/platform/auth/src/http.rs"            # gov.br JWKS
  "crates/platform/l10n-br/src/saas.rs"         # the CPF-verify SaaS, a configured private service
)

is_allowed() {
  local file="$1"
  for a in "${ALLOWED[@]}"; do [[ "$file" == "$a" ]] && return 0; done
  return 1
}

violations=0
while IFS=: read -r file line _; do
  is_allowed "$file" && continue
  echo "UNGUARDED: $file:$line — a raw HTTP client outside crate::outbound"
  sed -n "${line},$((line + 2))p" "$file" | sed 's/^/    /'
  violations=$((violations + 1))
done < <(
  git ls-files '*.rs' \
    | grep -v '/tests/' \
    | xargs grep -nE 'reqwest::Client::(builder|new)\(' \
    || true
)

if ((violations > 0)); then
  cat <<'EOF'

FAILED: a server-side HTTP client is being built outside the SSRF guard (issue #9).

If the URL can come from a user, a peer or admin-entered config, call
`crate::outbound::guarded_get` / `guarded_post`, which require HTTPS, validate the
resolved addresses against the non-routable ranges with PINNED resolution, refuse
redirects and cap the body.

If the URL is a fixed compiled-in constant, add the file to ALLOWED in this script
WITH the reason — the list is the record of what was consciously left unguarded.
EOF
  exit 1
fi

echo "OK: every ad-hoc HTTP client is accounted for."
