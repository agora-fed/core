import { test, expect } from '@playwright/test';

// Post-deploy verification of issue #30: the SOCRATES "Ideia original" bare
// URL on a mirrored topic must render as a clickable link.
const TOPIC = '019fb451-1fd0-7892-8e82-49e8b973fcf4'; // isenção de IR p/ militares

test('SOCRATES source URL is a clickable link on the topic page', async ({
  page,
  request,
}) => {
  const probe = await request.get(`/api/v1/f/topics/${TOPIC}`);
  test.skip(probe.status() !== 200, 'anchor topic absent in this environment');

  await page.goto(`/f/topico/${TOPIC}`);
  const source = page.locator('a[href*="senado.leg.br/ecidadania"]').first();
  await expect(source).toBeVisible({ timeout: 10_000 });
  await expect(source).toHaveAttribute('rel', /noopener/);
  await expect(source).toHaveAttribute('target', '_blank');
});
