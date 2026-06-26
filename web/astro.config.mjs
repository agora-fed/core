// @ts-check
import { defineConfig } from 'astro/config';
import svelte from '@astrojs/svelte';
import sitemap from '@astrojs/sitemap';

// Public canonical site URL (used for sitemap, canonical tags, Open Graph).
// Override with the SITE env var in production.
const SITE = process.env.SITE || 'https://democracia.social.br';

// https://astro.build/config
//
// ADR-0009: fully static (SSG). The whole site renders to plain HTML in
// `web/dist/` at build time — no Node runtime in production. SEO pages carry
// full meta / Open Graph / JSON-LD and are indexable WITHOUT JavaScript.
// Live/dynamic data (SLA countdown, fresh tallies) hydrates client-side via the
// Svelte islands against PUBLIC_API_BASE.
export default defineConfig({
  site: SITE,
  output: 'static',
  integrations: [svelte(), sitemap()],
  build: {
    inlineStylesheets: 'auto',
  },
  server: {
    host: '::',
    port: 4321,
  },
});
