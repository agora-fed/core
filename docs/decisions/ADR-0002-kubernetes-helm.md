# ADR-0002 — Deployment on Kubernetes + Helm (reversal of the original LXC/systemd stance)

- **Status:** Accepted
- **Supersedes:** PLAN.md §2 principle 5 (originally `[FROZEN]` as "No Docker / Proxmox LXC +
  TurnKey + native systemd").
- **Required by:** PLAN.md principle 12 — every reversal of technical direction must be justified.

## The mandated three-part justification (principle 12)

### (a) Why the previous approach fails for this platform's goals

The original stance — hand-placed Rust binaries as systemd units on Proxmox LXC/TurnKey appliances —
optimizes for operational simplicity and host-level auditability. But the North Star (PLAN.md §1)
demands properties LXC+systemd does not give cheaply:

- **"No single point of takedown" (§3, principle in DO-NOT) and survive coordinated pressure (§1.6).**
  Multi-region failover, rolling redeploys, and self-healing of crashed services are manual,
  bespoke scripting on LXC. They are first-class primitives on Kubernetes (Deployments,
  liveness/readiness probes, multi-zone scheduling).
- **Horizontal scale of stateless services.** The gateway and component crates must scale out under
  national load. systemd scaling is manual host provisioning; Kubernetes gives declarative HPA.
- **Reproducible, signed provenance.** A politically targeted platform must prove which exact
  artifact runs where. Immutable, signed container images + a versioned Helm release give a stronger
  audit trail than mutable host state.

### (b) Whether the old approach can be salvaged

Partially, and we keep what is good: the **stateful sovereign tier remains operationally simple and
host-pinned** — PostgreSQL + pgvector, Zitadel, and the local embedding model run as managed
StatefulSets/pinned nodes (or stay on dedicated LXC and are consumed as external services), so the
data-sovereignty and explicit-SQL guarantees (ADR-0001) are untouched. We are **not** containerizing
away auditability; we are containerizing the **stateless** request tier where elasticity and
self-healing matter.

### (c) Why Kubernetes + Helm is better here

- Declarative desired-state, rolling updates, automatic restart, and multi-region topology directly
  serve "must-not-be-taken-down."
- **Helm** gives versioned, reviewable, diff-able deployment manifests — the same auditability
  principle applied to ops. A deploy is a chart version tied to a tested commit.
- The sovereign park already runs `helm`/`kubectl` tooling, so this is not new foreign dependency.

## Decision

- **Stateless tier** (gateway, platform/spaces/components services, web) → containerized, deployed
  via the umbrella Helm chart in `deploy/helm/`, IPv6-first.
- **Stateful sovereign tier** (PostgreSQL+pgvector, Redis, Zitadel, embeddings) → StatefulSets with
  pinned storage, or external managed services; never casually rescheduled.
- IPv6-first networking throughout (principle 4) — Services, Ingress, and probes default to IPv6.

## Consequences

- We now maintain container images and Helm charts; CI gains `helm lint`/`kubeconform` and image
  signing (`.forgejo/workflows/helm.yml`, `release.yml`).
- `PLAN.md` principle 5 is annotated as superseded by this ADR; the "no Docker anywhere" rule no
  longer holds. All other frozen principles stand.
