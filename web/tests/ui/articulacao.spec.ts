import { test, expect } from '@playwright/test';

// Post-deploy verification of the DIOGENES add-on (agora-fed/diogenes, ADR-0020).
//
// Incident 2026-11-25: the admin turned the module on and saw nothing change. The link was
// wired into the footer, left rail and command palette, but NOT into the site header, which
// is the menu people look at. This spec reads the module state from the API first, so it
// holds on installations where the module is off, too.

const ORG = '11111111-1111-1111-1111-111111111111';

async function diogenesActive(request: import('@playwright/test').APIRequestContext) {
  const res = await request.get(`/api/v1/orgs/${ORG}/modules`);
  expect(res.status()).toBe(200);
  const body = await res.json();
  const mod = (body.data as Array<{ id: string; active: boolean }>).find((m) => m.id === 'diogenes');
  expect(mod, 'diogenes is listed in the module catalog').toBeTruthy();
  return mod!.active;
}

test('header shows "Articulação" exactly when the module is on', async ({ page, request }) => {
  const active = await diogenesActive(request);
  await page.goto('/');
  const link = page.locator('header nav a[href="/articulacao/"]');
  if (active) {
    await expect(link).toBeVisible();
    await expect(link).toHaveText(/Articulação/);
  } else {
    await expect(link).toBeHidden();
  }
});

test('articulation area loads without JS errors', async ({ page, request }) => {
  test.skip(!(await diogenesActive(request)), 'module off on this installation');
  const jsErrors: string[] = [];
  page.on('pageerror', (err) => jsErrors.push(String(err)));
  await page.goto('/articulacao/#/votacoes');
  await expect(page.getByRole('heading', { name: /Votações nominais/ })).toBeVisible();
  await page.goto('/articulacao/#/grupos');
  await expect(page.getByRole('heading', { name: 'Frentes parlamentares' })).toBeVisible();
  expect(jsErrors, `uncaught JS: ${jsErrors.join(' | ')}`).toHaveLength(0);
});

test('anonymous visitors get the login prompt of the design system', async ({ page, request }) => {
  test.skip(!(await diogenesActive(request)), 'module off on this installation');
  const jsErrors: string[] = [];
  page.on('pageerror', (err) => jsErrors.push(String(err)));
  await page.goto('/articulacao/');
  await expect(page.getByRole('heading', { name: 'Entre na sua conta' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Entrar' }).last()).toHaveAttribute('href', /\/entrar\/\?next=/);
  // The page is the site's own page: same header and sub-navigation, no second product.
  await expect(page.getByRole('navigation', { name: 'Seções da sala de articulação' })).toBeVisible();
  expect(jsErrors, `uncaught JS: ${jsErrors.join(' | ')}`).toHaveLength(0);
});

test('rooms API gates anonymous callers', async ({ request }) => {
  const res = await request.get('/api/v1/diogenes/rooms');
  expect(res.status()).toBe(401);
  const body = await res.json();
  expect(body.success).toBe(false);
});

test('public deputies API answers with the envelope', async ({ request }) => {
  const res = await request.get('/api/v1/diogenes/deputies');
  expect(res.status()).toBe(200);
  const body = await res.json();
  expect(body.success).toBe(true);
  expect(Array.isArray(body.data)).toBe(true);
});

test('campaign finance is open data for anonymous callers (diogenes ADR-0009, ADR-0010)', async ({ request }) => {
  const list = await request.get('/api/v1/diogenes/campaigns');
  expect(list.status()).toBe(200);
  const body = await list.json();
  expect(body.success).toBe(true);
  const rows = body.data.deputies as Array<{ camara_id: number; total_cents: number }>;
  test.skip(rows.length === 0, 'no deputies synced on this installation');
  const res = await request.get(`/api/v1/diogenes/deputies/${rows[0].camara_id}/campaign`);
  expect(res.status()).toBe(200);
  const c = (await res.json()).data;
  expect(c.deputy.camara_id).toBe(rows[0].camara_id);
  expect(c.total_cents).toBe(rows[0].total_cents);
  expect(Array.isArray(c.deputy_companies)).toBe(true);
  // The six CPF digits used for the match never leave the server.
  expect(JSON.stringify(c)).not.toContain('cpf');
});

test('scenario money stays private to the room (diogenes ADR-0009)', async ({ request }) => {
  const res = await request.get('/api/v1/diogenes/scenarios/00000000-0000-0000-0000-000000000000/donors');
  expect(res.status()).toBe(401);
});

test('"Quem financia" page lists deputies and opens a card without sign-in', async ({ page }) => {
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  await page.goto('/politicos/financiamento/');
  await expect(page.getByRole('heading', { level: 1, name: 'Quem financia' })).toBeVisible();
  const first = page.getByRole('button', { name: /^Ver o financiamento de / }).first();
  await expect(first).toBeVisible({ timeout: 15000 });
  await first.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('heading', { name: 'Financiamento de campanha' })).toBeVisible();
  await expect(page).toHaveURL(/deputado=\d+/);
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  expect(errors).toEqual([]);
});
