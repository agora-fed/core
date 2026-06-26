#!/usr/bin/env bash
# Bootstrap a registered citizen as a platform admin (privileged, manual — see docs/ops/ADMIN.md).
# Raises verification_level to 'strong' (>= Directory, the admin gate) and records an 'owner' role
# binding. Run on the VM (or with DATABASE_URL pointing at the prod DB).
#
# Usage: ./scripts/bootstrap-admin.sh <email> [org_id]
set -euo pipefail
EMAIL="${1:?usage: bootstrap-admin.sh <email> [org_id]}"
ORG="${2:-11111111-1111-1111-1111-111111111111}"
PSQL="${PSQL:-psql -h 127.0.0.1 -U dsoc -d democracia_social}"
export PGPASSWORD="${PGPASSWORD:-dsoc}"

CID=$($PSQL -tAc "SELECT citizen_id FROM auth_credential WHERE org_id='$ORG' AND email='$EMAIL'")
if [[ -z "${CID:-}" ]]; then
  echo "Nenhum cidadão com e-mail '$EMAIL' no org '$ORG'. Cadastre-se primeiro em /cadastrar." >&2
  exit 1
fi
echo "Cidadão: $CID"
$PSQL -v ON_ERROR_STOP=1 <<SQL
UPDATE citizen SET verification_level='strong' WHERE id='$CID';
INSERT INTO admin_role_binding (id, org_id, citizen_id, role, created_at)
VALUES (gen_random_uuid(), '$ORG', '$CID', 'owner', now())
ON CONFLICT (org_id, citizen_id, role) DO NOTHING;
SQL
echo "OK — '$EMAIL' agora é admin (nível strong + role owner) no org $ORG."
echo "Use os endpoints /api/v1/admin/* com o header:  x-citizen-id: $CID"
