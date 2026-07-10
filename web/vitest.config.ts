import { defineConfig } from 'vitest/config';

// `tests/ui/**` is the Playwright suite (`npm run test:ui`) — it must never be
// collected by vitest (`npm test`), which only owns the API contract tests.
export default defineConfig({
  test: {
    exclude: ['tests/ui/**', 'node_modules/**', 'dist/**'],
  },
});
