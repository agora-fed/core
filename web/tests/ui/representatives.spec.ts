import { test, expect } from '@playwright/test';

// Post-deploy verification of tag-a-representative (issue #3). Anchors on a
// known public deliberation in production (the cause the feature was asked
// for); skips gracefully in environments where that topic does not exist.
const TOPIC = '019fb451-1fd0-7892-8e82-49e8b973fcf4'; // isenção de IR p/ militares

async function topicExists(request: import('@playwright/test').APIRequestContext) {
  const res = await request.get(`/api/v1/f/topics/${TOPIC}`);
  return res.status() === 200;
}

test('representatives aggregate API answers with the LGPD-safe shape', async ({
  request,
}) => {
  test.skip(!(await topicExists(request)), 'anchor topic absent in this environment');

  const res = await request.get(`/api/v1/topics/${TOPIC}/representatives`);
  expect(res.status()).toBe(200);
  const body = await res.json();
  expect(body.success).toBe(true);
  expect(body.data).toHaveProperty('representatives');
  expect(body.data).toHaveProperty('total_tags');
  // LGPD: aggregate only — no citizen identifiers in the payload.
  expect(JSON.stringify(body.data)).not.toContain('citizen');
});

test('anonymous tag attempt is refused with a proper envelope', async ({ request }) => {
  test.skip(!(await topicExists(request)), 'anchor topic absent in this environment');

  const res = await request.post(`/api/v1/topics/${TOPIC}/representatives`, {
    data: { mandate_id: '00000000-0000-7000-8000-000000000000' },
  });
  expect(res.status()).toBe(401);
  const body = await res.json();
  expect(body.success).toBe(false);
});

test('topic page renders the representative widget', async ({ page, request }) => {
  test.skip(!(await topicExists(request)), 'anchor topic absent in this environment');

  const jsErrors: string[] = [];
  page.on('pageerror', (err) => jsErrors.push(String(err)));
  await page.goto(`/f/topico/${TOPIC}`);
  await expect(
    page.getByText('Quem deve te representar nesta causa?'),
  ).toBeVisible({ timeout: 10_000 });
  await expect(
    page.getByRole('button', { name: /marcar representante/i }),
  ).toBeVisible();
  expect(jsErrors, `uncaught JS: ${jsErrors.join(' | ')}`).toHaveLength(0);
});
