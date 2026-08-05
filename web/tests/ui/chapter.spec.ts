import { test, expect } from '@playwright/test';

// Post-deploy verification of the chapter (directory) page. Self-sufficient:
// discovers a real chapter through the public API, then drives the page a
// citizen would reach by clicking a directory on /partidos/<sigla>.
const ORG = '11111111-1111-1111-1111-111111111111';

test('chapter API + page render end to end', async ({ page, request }) => {
  // 1) Find a party that has at least one directory.
  const parties = await request.get(`/api/v1/parties?org_id=${ORG}`);
  expect(parties.status()).toBe(200);
  const list = (await parties.json()).data ?? [];
  let sigla: string | null = null;
  let chapterId: string | null = null;
  for (const p of list.slice(0, 12)) {
    const detail = await request.get(
      `/api/v1/parties/${encodeURIComponent(p.sigla)}?org_id=${ORG}`,
    );
    const dirs = (await detail.json()).data?.directories ?? [];
    if (dirs.length > 0) {
      sigla = p.sigla;
      chapterId = dirs[0].id;
      break;
    }
  }
  test.skip(!sigla || !chapterId, 'no party with directories in this environment');

  // 2) The English chapters endpoint answers with the EN contract.
  const chapter = await request.get(
    `/api/v1/parties/${encodeURIComponent(sigla!)}/chapters/${chapterId}?org_id=${ORG}`,
  );
  expect(chapter.status()).toBe(200);
  const dto = (await chapter.json()).data;
  expect(dto).not.toBeNull();
  expect(['national', 'state', 'municipal']).toContain(dto.level);
  // Privacy wall: no citizen UUIDs besides the chapter id itself.
  const raw = JSON.stringify(dto.administrators ?? []);
  expect(raw).not.toMatch(/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-/); // uuidv7 of citizens

  // 3) The page renders the chapter for a real browser.
  const slug = sigla!
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '');
  const jsErrors: string[] = [];
  page.on('pageerror', (err) => jsErrors.push(String(err)));
  await page.goto(`/partidos/${slug}/diretorio/?id=${chapterId}`);
  await expect(page.getByRole('heading', { level: 1 })).toContainText(dto.name, {
    timeout: 10_000,
  });
  await expect(page.getByText('Diretório não encontrado.')).toHaveCount(0);
  expect(jsErrors, `uncaught JS: ${jsErrors.join(' | ')}`).toHaveLength(0);
});

test('unknown chapter id shows the graceful miss, never a crash', async ({ page }) => {
  await page.goto('/partidos/pt/diretorio/?id=00000000-0000-7000-8000-000000000000');
  await expect(page.getByText('Diretório não encontrado.')).toBeVisible();
});
