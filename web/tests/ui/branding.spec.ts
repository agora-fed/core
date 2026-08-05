import { test, expect } from '@playwright/test';

// Post-deploy verification of the branding feature (production incident
// 2026-08-05: the panel showed the generic failure banner on every load
// because it read the wrong envelope shape — and no browser-level check
// existed to catch it before a human did).
//
// Anonymous run (CI/post-deploy): we cannot log in as an admin here, but the
// broken build was distinguishable WITHOUT auth — it rendered the generic
// "Falha ao carregar identidade visual." string, while a correct build
// surfaces the SERVER's message ("Autenticação necessária.") coming through
// the properly-parsed envelope.

test('branding public API answers with the envelope', async ({ request }) => {
  const res = await request.get('/api/v1/branding');
  expect(res.status()).toBe(200);
  const body = await res.json();
  expect(body.success).toBe(true);
  expect(body.data).toHaveProperty('colors');
});

test('admin branding API gates anonymous callers with a proper envelope', async ({
  request,
}) => {
  const res = await request.get('/api/v1/admin/branding');
  expect(res.status()).toBe(401);
  const body = await res.json();
  expect(body.success).toBe(false);
  expect(body.error?.message).toBeTruthy();
});

test('aparencia panel parses the envelope (no generic failure banner)', async ({
  page,
}) => {
  const jsErrors: string[] = [];
  page.on('pageerror', (err) => jsErrors.push(String(err)));

  await page.goto('/admin/aparencia/');
  // The island always resolves its load: either the form (admin session) or
  // the SERVER's auth message. The generic banner means envelope parsing broke.
  await expect(
    page.getByText(/Autenticação necessária|Requer administrador|Nome do site/i).first(),
  ).toBeVisible();
  await expect(
    page.getByText('Falha ao carregar identidade visual.'),
  ).toHaveCount(0);
  expect(jsErrors, `uncaught JS: ${jsErrors.join(' | ')}`).toHaveLength(0);
});
