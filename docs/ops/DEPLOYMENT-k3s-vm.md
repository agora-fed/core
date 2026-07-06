# Production deployment runbook — k3s on the sovereign VM

The live instance runs on the sovereign VM (`[2804:710:d0:9::a000]`, Debian 13) under **k3s**,
serving the 21-crate API over **public IPv6** at `:8080/api/v1`. This documents exactly how it was
stood up, including the constraints hit (so it's reproducible).

## Topology (lean, single-node, IPv6-first)
- **Stateful tier on the host:** PostgreSQL 17 + `pgvector` via `apt` (external to k8s — aligned with
  ADR-0002). DB `democracia_social`, role `dsoc`, all 23 migrations applied (57 tables).
- **k3s** (lean: `--disable traefik,servicelb,metrics-server`) runs the **gateway** pod with
  `hostNetwork: true`, so it reaches Postgres on `[::1]` and binds the host's IPv6 `:8080` directly —
  no Ingress/Service needed on a single IPv6 node.

## Constraints encountered (and fixes)
1. **VM has IPv6-only egress** — can't reach `github.com`/`update.k3s.io` (IPv4-only). → **airgap k3s**:
   download `k3s` binary + `k3s-airgap-images-amd64.tar.zst` on an IPv4-capable host, transfer, install
   with `INSTALL_K3S_SKIP_DOWNLOAD=true`.
2. **Tiny disk initially (3 GB)** → resized the virtual disk to 20 GB, then `growpart /dev/sda 1` +
   `resize2fs /dev/sda1`. Removed unused k3s system images (traefik/metrics-server/klipper) from
   containerd to reclaim space.
3. **`ErrImageNeverPull`** — the gateway image must be imported into the **`k8s.io`** containerd
   namespace: `gunzip -c gw.tar.gz | k3s ctr -n k8s.io images import -`.
4. **`Tls(InvalidDnsNameError)`** — sqlx tries TLS by default; an IP literal `[::1]` fails cert
   validation. → append `?sslmode=disable` (local Postgres, no TLS).

## Build & deploy
```sh
# 1. Build the gateway image (on an x86_64 Debian-13 host with docker):
cargo build -p dsoc-gateway --release
DOCKER_BUILDKIT=0 docker build -t dsoc-gateway:0.1.0 deploy/docker   # Dockerfile: debian:trixie-slim + binary
docker save dsoc-gateway:0.1.0 | gzip > gw.tar.gz

# 2. On the VM: import into k3s and apply:
gunzip -c gw.tar.gz | sudo k3s ctr -n k8s.io images import -
sudo k3s kubectl apply -f deploy/k8s/gateway.yaml   # set the real DATABASE_URL secret first
```

## Verify
```sh
curl http://[2804:710:d0:9::a000]:8080/health         # {"status":"ok"}
curl 'http://[2804:710:d0:9::a000]:8080/api/v1/proposals?org_id=<uuid>'   # paginated ApiResponse
```

## Known follow-ups
- OIDC not configured yet (`AUTH_OIDC_ISSUER`/`AUTH_OIDC_JWKS_URL` unset) → `/auth` endpoints reject
  until Zitadel is wired. Verification-level checks (other crates) work against the DB regardless.
- Single replica (hostNetwork). For HA/multi-region, move to the Helm chart (`deploy/helm/`) once the
  cluster has capacity, with an Ingress and a real Secret manager.

## SMTP (sovereign relay)

Password reset, mandate invitations, and SLA notifications go through the sovereign relay
`mail.autonomia.lat` as `sistema@democracia.social.br`. The code (`crates/platform/auth/src/
password_reset.rs`) reads five discrete env vars: `SMTP_HOST`, `SMTP_PORT`, `SMTP_USER`,
`SMTP_PASS`, `SMTP_FROM`. Port 587 → STARTTLS; port 465 → implicit TLS. Auth is enabled only
when both USER and PASS are set. Missing `SMTP_HOST` or `SMTP_FROM` drops the service into
DEV mode — the reset URL is logged instead of sent, and no e-mail leaves the pod.

Bootstrap or update the values on the VM (never commit real credentials):

```sh
# Read the current secret to confirm shape:
sudo k3s kubectl get secret dsoc-gateway-secrets -o yaml

# Apply the SMTP block (merge; leaves DATABASE_URL etc. untouched):
sudo k3s kubectl patch secret dsoc-gateway-secrets --type=merge -p "$(cat <<'JSON'
{
  "stringData": {
    "SMTP_HOST": "mail.autonomia.lat",
    "SMTP_PORT": "587",
    "SMTP_USER": "sistema@democracia.social.br",
    "SMTP_PASS": "<paste-from-.config/settings.env-on-workstation>",
    "SMTP_FROM": "DemocraciaBR <sistema@democracia.social.br>"
  }
}
JSON
)"

# envFrom pulls at pod start — restart to load:
sudo k3s kubectl rollout restart deploy/dsoc-gateway
sudo k3s kubectl rollout status deploy/dsoc-gateway
```

Smoke-test via the live enumeration-resistant reset endpoint (returns 200 either way — check
the pod logs for a real send vs the DEV-mode warn):

```sh
curl -sS -X POST https://democracia.social.br/api/v1/auth/password-reset/request \
    -H 'content-type: application/json' \
    -d '{"org_id":"<uuid>","email":"seu-endereco@dominio.tld"}'
sudo k3s kubectl logs deploy/dsoc-gateway | grep -i 'password-reset\|smtp\|password_reset'
```

The VM is IPv6-only — `mail.autonomia.lat` needs an AAAA record (or a reachable IPv4 via
your egress path). If lookup/connect fails, the pod logs `password-reset e-mail send failed`
with the transport error; the wire response stays 200 (enumeration-resistance guarantee).

## HTTPS + domain (Caddy reverse proxy)
`democracia.social.br` resolves **AAAA → the VM** (the VM has no public IPv4; IPv4 `A` records point
elsewhere, so the platform is IPv6-reachable only — consistent with the IPv6-first design). TLS is
terminated by **Caddy** (`apt install caddy`, config in `deploy/caddy/Caddyfile`), which auto-obtains a
Let's Encrypt cert over IPv6 (TLS-ALPN-01) and reverse-proxies `[::1]:8080` (the gateway). Since the
gateway serves the static site AND `/api/v1` at the same origin behind the domain, there is **no CORS**.
HTTP is 308-redirected to HTTPS. Live: **https://democracia.social.br**.
