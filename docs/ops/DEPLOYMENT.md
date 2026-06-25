# Deployment — Kubernetes + Helm (IPv6-first)

Authority: [ADR-0002](../decisions/ADR-0002-kubernetes-helm.md). Production target VM (sovereign
park): `popsolutions@[2804:710:d0:9::a000]` (IPv6-only).

## Topology

```
                         ┌──────────────── Ingress (IPv6, HAProxy/ingress-nginx) ───────────────┐
                         │                                                                       │
                   ┌─────▼─────┐   internal Services (IPv6)                                       │
 Internet ───────▶ │  gateway  │ ───▶ platform/* + spaces/* + components/* (stateless Deployments)│
   (push, web)     └─────┬─────┘                                                                  │
                         │                                                                         │
        ┌────────────────┼─────────────────────────────────────────────────────────────┐        │
        │  Sovereign stateful tier (StatefulSets / pinned nodes — NOT casually rescheduled)│        │
        │  PostgreSQL 16 + pgvector · Redis · Zitadel (OIDC) · local embeddings model      │        │
        └─────────────────────────────────────────────────────────────────────────────────┘        │
                         └─────────────────────────────────────────────────────────────────────────┘
```

- **Stateless tier** → containerized, scaled by Helm `replicaCount` / HPA, rolling updates,
  liveness/readiness probes. Self-healing serves "no single point of takedown."
- **Stateful sovereign tier** → StatefulSets with persistent volumes (or consumed as external
  managed services). Data sovereignty + explicit SQL (ADR-0001) are untouched.

## Install

```sh
# Render and validate first (also enforced in CI by helm.yml)
helm lint deploy/helm/democracia-social
helm template democracia-social deploy/helm/democracia-social -f deploy/helm/democracia-social/values-prod.yaml | kubeconform -strict

# Deploy (secrets come from a sealed/external secret, NEVER from values in git — principle 8)
helm upgrade --install democracia-social deploy/helm/democracia-social \
  -n democracia-social --create-namespace \
  -f deploy/helm/democracia-social/values-prod.yaml
```

## Secrets

`.config/settings.env` is local-dev only. In-cluster secrets are provided by an external secret
manager / sealed-secret; **no secret values are ever committed** (PLAN.md principle 8). Helm values
reference secret *names*, not secret *values*.

## IPv6-first

All Services, the Ingress, and probes default to IPv6. The `values.yaml` `network.ipFamily` is
`IPv6`; IPv4 is an explicit opt-in fallback only.

## Resilience roadmap (Phase 3)

Multi-region clusters, redundant DNS/ingress, append-only hashed audit log, and an external security
audit (PLAN.md §7 Phase 3). CockroachDB is evaluated **only then**, **only** for the transactional
core, justified by the threat model.
