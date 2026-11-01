// @ts-check
import { defineConfig } from 'astro/config';
import svelte from '@astrojs/svelte';
import sitemap from '@astrojs/sitemap';

// Public canonical site URL (used for sitemap, canonical tags, Open Graph).
// Each instance MUST set SITE in production (ActivityPub IDs are permanent).
const SITE = process.env.SITE || 'http://localhost:4321';

// Served by the gateway depending on the active modules, so not every instance has them.
const INSTANCE_ONLY_PAGES = [
  '/inicio-comunidade',
  '/sobre-comunidade',
  '/modulo-indisponivel',
  '/processos',
  '/orcamento',
  '/enquetes',
  '/reunioes',
];

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
  integrations: [
    svelte(),
    sitemap({
      filter: (page) => !INSTANCE_ONLY_PAGES.some((path) => new URL(page).pathname.startsWith(path)),
    }),
  ],
  build: {
    inlineStylesheets: 'auto',
  },
  server: {
    host: '::',
    port: 4321,
  },
});
