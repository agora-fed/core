import { defineConfig, devices } from '@playwright/test';

// Smoke tests against production. The URL is overridable via BASE_URL when
// running against a preview build.
export default defineConfig({
  testDir: './tests/ui',
  timeout: 30_000,
  expect: { timeout: 8_000 },
  reporter: [['list']],
  workers: 4,
  use: {
    baseURL: process.env.BASE_URL ?? 'https://democracia.social.br',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    // Uncaught JS errors + response failures are asserted in the test suite
    // itself; here we only tune viewport + user-agent.
    viewport: { width: 1280, height: 800 },
    userAgent: 'DemocraciaBR-smoke/0.1 (+playwright)',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
});
