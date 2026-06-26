# ADR-0009 — Web front-end: Astro + Svelte islands; SSG (static) now, SSR pod later

- **Status:** Accepted · **Context:** PLAN.md §9 (design) and §10 (open: "Web SPA framework").
  Priorities (user): lightweight, performant, **SEO**.

## Decision

### Framework — Astro + Svelte islands
Astro ships ~0 KB of JS by default (islands architecture): public, SEO-critical pages (scorecards,
proposals, debates — the shareable accountability artifacts) are server-/build-rendered and
indexable, while only the interactive bits (vote, SLA countdown, register/login forms) hydrate as
small Svelte islands. Best fit for leveza + Core Web Vitals + SEO. Lives in `web/`. UI brand =
**DemocraciaBR** (logo + manual in `brand/`, Poppins, green/blue/red/amber).

### Rendering — **SSG (static output) now; SSR as a separate pod later**
- **Now:** `output: 'static'` — the site builds to static HTML/CSS/JS, served as files (by the gateway
  or a tiny static server / CDN). Cheapest, fastest, best SEO, and fits the constrained sovereign VM
  with no extra Node runtime. Fresh/dynamic data (live scorecard numbers, SLA clocks) is fetched
  **client-side** by the Svelte islands against `/api/v1`, while the indexable static shell carries the
  SEO content (meta, Open Graph, JSON-LD).
- **Future:** when per-request server rendering is needed (e.g. fully fresh SSR of every scorecard for
  crawlers, or auth-gated SSR), move the web to **Astro SSR running as its own pod** in k3s (Node
  adapter), separate from the Rust gateway. This is a clean, non-breaking upgrade — same codebase, swap
  the Astro adapter and add a Deployment.

## Consequences
- The front-end agent targets `output: 'static'`; no Node runtime is required in production now.
- Deployment now: publish the `web/dist/` static bundle (served by the gateway/static host). Later: an
  SSR Deployment + Service is added under `deploy/k8s/` when capacity and SSR requirements justify it.
