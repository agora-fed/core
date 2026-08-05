# GitOps — git is the ONLY path to production

> **The rule (absolute):** nothing reaches production except through a git
> commit on `main`. No manual `helm upgrade`, no manual `kubectl apply`, no
> images pushed by hand, no SSH-and-edit. If it is not in git, it does not run.

## Why

- **Auditability** — every production state is a commit with an author and a diff.
- **Reversibility** — rollback is `git revert`, not archaeology.
- **Reproducibility** — a fresh cluster converges to the same state from the repo alone.
- **Sovereignty** — the repo (mirrored on git.pop.coop and GitHub) *is* the
  deployment record; no proprietary console holds the truth.

## The pipeline

```
developer ──PR──▶ main (CI green: fmt+clippy+tests+coverage+boundaries)
                    │
                tag v* ──▶ release workflow ──▶ ghcr.io/agora-fed/core:vX.Y.Z
                    │                            + chart oci://ghcr.io/agora-fed/charts
                    │
        commit bumping image.tag in
        deploy/helm/agora-core/values-pindorama.yaml
                    │
        agora-gitops timer on the cluster host (pull-based, every 2 min)
                    │
        helm upgrade --atomic  ◀── the ONLY thing that touches the cluster
```

## How to deploy

1. Merge your change to `main` and wait for CI to go green.
2. Tag: `git tag vX.Y.Z && git push --tags` — the release workflow publishes
   the image `ghcr.io/agora-fed/core:vX.Y.Z`.
3. Commit the tag bump to the installation values file:
   ```yaml
   # deploy/helm/agora-core/values-pindorama.yaml
   image:
     tag: "vX.Y.Z"
   ```
4. Push to `main`. Within ~2 minutes the sync agent applies it atomically.

## Post-deploy verification (MANDATORY)

A deploy is NOT done when the rollout succeeds — it is done when the feature
is seen working in production **through a real browser**. Non-negotiable
(2026-08-05 incident: green backend tests + healthy /health, broken panel):

```sh
cd web && npx playwright test tests/ui/   # runs against https://democracia.social.br
```

Every new feature ships with its own spec under `web/tests/ui/` covering the
production surface it added. curl on `/health` is a liveness check, not a
verification.

## How to roll back

```sh
git revert <the tag-bump commit> && git push
```
The next sync tick redeploys the previous tag. `--atomic` already auto-reverts
failed upgrades at the Helm level.

## Emergency procedure

There is no "break glass" bypass. In an incident, the fast path is still git:
revert the offending commit and push — the agent applies it within 2 minutes.
If the cluster host itself is unreachable, fixing the host comes first; state
in git stays correct meanwhile.

## Host installation (once per cluster host)

```sh
sudo cp deploy/gitops/agora-sync.sh /usr/local/bin/agora-sync.sh
sudo chmod +x /usr/local/bin/agora-sync.sh
sudo cp deploy/gitops/agora-gitops.{service,timer} /etc/systemd/system/
# Optional overrides (repo URL, branch, release, values file):
sudo tee /etc/agora-gitops.env >/dev/null <<'EOF'
REPO_URL=https://github.com/agora-fed/core.git
VALUES_FILE=deploy/helm/agora-core/values-pindorama.yaml
RELEASE=agora
NAMESPACE=agora
EOF
sudo systemctl daemon-reload
sudo systemctl enable --now agora-gitops.timer
```

## Enforcement

- `main` is protected on both remotes (Forgejo rule `main`; GitHub branch
  protection) — history rewrites and direct force-pushes are blocked.
- The release workflow is the only producer of production images; the GHCR
  package accepts pushes only from repository workflows.
- The cluster host has no registry credentials for pushing, only pulling.
- Secrets never live in git: the chart consumes a pre-created Kubernetes
  Secret by name (`externalSecrets.name`).
