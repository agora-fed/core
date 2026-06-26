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

## HTTPS + domain (Caddy reverse proxy)
`democracia.social.br` resolves **AAAA → the VM** (the VM has no public IPv4; IPv4 `A` records point
elsewhere, so the platform is IPv6-reachable only — consistent with the IPv6-first design). TLS is
terminated by **Caddy** (`apt install caddy`, config in `deploy/caddy/Caddyfile`), which auto-obtains a
Let's Encrypt cert over IPv6 (TLS-ALPN-01) and reverse-proxies `[::1]:8080` (the gateway). Since the
gateway serves the static site AND `/api/v1` at the same origin behind the domain, there is **no CORS**.
HTTP is 308-redirected to HTTPS. Live: **https://democracia.social.br**.
