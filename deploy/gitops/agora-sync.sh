#!/usr/bin/env bash
# AGORA GitOps sync agent — the ONLY path to production (docs/GITOPS.md).
#
# Pull-based reconciler, Flux-style but sovereign-park-sized: it runs ON the
# cluster host (systemd timer), fetches the deploy branch from git, and applies
# whatever the repo says with `helm upgrade`. Nobody pushes to production;
# production pulls from git. To deploy: commit a new image.tag to the
# installation values file and merge to the deploy branch. To roll back:
# `git revert` and let the next tick apply it.
set -euo pipefail

# --- Configuration (override via /etc/agora-gitops.env) -------------------------
REPO_URL="${REPO_URL:-https://github.com/agora-fed/core.git}"
BRANCH="${BRANCH:-main}"
CHART_PATH="${CHART_PATH:-deploy/helm/agora-core}"
VALUES_FILE="${VALUES_FILE:-deploy/helm/agora-core/values-pindorama.yaml}"
RELEASE="${RELEASE:-agora}"
NAMESPACE="${NAMESPACE:-agora}"
WORKDIR="${WORKDIR:-/var/lib/agora-gitops}"
STATE_FILE="$WORKDIR/deployed-commit"

[[ -f /etc/agora-gitops.env ]] && source /etc/agora-gitops.env

mkdir -p "$WORKDIR"

# --- Fetch desired state from git ------------------------------------------------
if [[ ! -d "$WORKDIR/repo/.git" ]]; then
  git clone --depth 1 --branch "$BRANCH" "$REPO_URL" "$WORKDIR/repo"
fi
cd "$WORKDIR/repo"
git fetch --depth 1 origin "$BRANCH"
git reset --hard "origin/$BRANCH"

HEAD_COMMIT="$(git rev-parse HEAD)"
LAST_DEPLOYED="$(cat "$STATE_FILE" 2>/dev/null || echo none)"

# Only reconcile when the deploy inputs changed (chart, values, or first run).
if [[ "$HEAD_COMMIT" == "$LAST_DEPLOYED" ]]; then
  echo "agora-gitops: up to date at $HEAD_COMMIT"
  exit 0
fi
if [[ "$LAST_DEPLOYED" != "none" ]] && \
   git diff --quiet "$LAST_DEPLOYED" "$HEAD_COMMIT" -- "$CHART_PATH" "$VALUES_FILE"; then
  echo "agora-gitops: $HEAD_COMMIT touches no deploy input; recording and skipping"
  echo "$HEAD_COMMIT" > "$STATE_FILE"
  exit 0
fi

# --- Apply -----------------------------------------------------------------------
echo "agora-gitops: reconciling $LAST_DEPLOYED -> $HEAD_COMMIT"
helm upgrade --install "$RELEASE" "$CHART_PATH" \
  --namespace "$NAMESPACE" --create-namespace \
  -f "$VALUES_FILE" \
  --atomic --timeout 5m

echo "$HEAD_COMMIT" > "$STATE_FILE"
echo "agora-gitops: deployed $HEAD_COMMIT"
