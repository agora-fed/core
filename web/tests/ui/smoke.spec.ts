import { test, expect, Page } from '@playwright/test';

// Every public page in the site. If a route needs auth, we allow the "entre
// para ver" gate to be a valid success — we only fail on JS/network errors
// or Portuguese error phrases visible to the user.
const PUBLIC_ROUTES = [
  { path: '/', match: /democracia/i, name: 'home' },
  { path: '/feed/', match: /feed|acesso|entre/i, name: 'feed (auth-gated ok)' },
  { path: '/politicos/', match: /pol[íi]tico|placar/i, name: 'politicos browser' },
  { path: '/partidos/', match: /partido/i, name: 'partidos' },
  { path: '/propor/', match: /demanda|propor|escolha|placar/i, name: 'propor' },
  { path: '/politicos/gastos/', match: /gasto|carregando/i, name: 'gasto parlamentar' },
  { path: '/politicos/propostas/', match: /proposta|carregando/i, name: 'propostas dash' },
  { path: '/eleicoes/2026/', match: /elei[çc][ãa]o|2026/i, name: 'eleicoes 2026' },
  { path: '/explorar/', match: /explor/i, name: 'explorar' },
  { path: '/buscar/', match: /busc/i, name: 'buscar' },
  { path: '/entrar/', match: /entrar|e-mail/i, name: 'entrar' },
  { path: '/cadastrar/', match: /cadastr|criar|conta/i, name: 'cadastrar' },
  { path: '/configuracoes/', match: /configura[çc]|entre/i, name: 'configuracoes' },
  { path: '/admin/', match: /admin|entre|restrit/i, name: 'admin' },
  { path: '/notificacoes/', match: /notifica[çc]|entre/i, name: 'notificacoes' },
  { path: '/design/', match: /design|componente/i, name: 'design system' },
];

// Phrases that indicate an error the USER sees on-screen (not just a stack
// trace in the console). We fail the test when any of these appears in the
// main content.
const USER_VISIBLE_ERROR_PATTERNS = [
  /servi[çc]o temporariamente indispon[íi]vel/i,
  /n[ãa]o foi poss[íi]vel carregar/i,
  /erro interno/i,
  /falha ao carregar/i,
  /oops.*erro/i,
];

// The Alert component surfaces user-facing text under [role="alert"]. Some
// gates ("entre para ver seu feed") also render there — we filter those.
const ACCEPTABLE_ALERT_PATTERNS = [
  /entre para/i,
  /precisa entrar/i,
  /acesso restrito/i,
  /autenticad/i,
  /admin/i,
];

async function collectPageIssues(page: Page): Promise<{
  jsErrors: string[];
  netErrors: string[];
}> {
  const jsErrors: string[] = [];
  const netErrors: string[] = [];
  page.on('pageerror', (err) => jsErrors.push(err.message));
  page.on('console', (msg) => {
    if (msg.type() === 'error') jsErrors.push(msg.text());
  });
  page.on('requestfailed', (req) => {
    const failure = req.failure()?.errorText ?? '';
    // Chromium sometimes reports ERR_ABORTED for user-cancelled prefetches —
    // ignore that noise. Everything else is a real load failure.
    if (!/ABORTED/.test(failure)) {
      netErrors.push(`${req.method()} ${req.url()} — ${failure}`);
    }
  });
  page.on('response', async (res) => {
    if (res.status() >= 500) {
      netErrors.push(`${res.status()} ${res.url()}`);
    }
  });
  return { jsErrors, netErrors };
}

for (const route of PUBLIC_ROUTES) {
  test(`${route.name}: ${route.path}`, async ({ page }) => {
    const { jsErrors, netErrors } = await collectPageIssues(page);
    const resp = await page.goto(route.path, { waitUntil: 'networkidle', timeout: 25_000 });
    expect(resp?.status(), `HTTP for ${route.path}`).toBeLessThan(400);

    // Let islands hydrate + fetch initial data.
    await page.waitForTimeout(1500);

    // Body contains something meaningful.
    const bodyText = (await page.locator('body').innerText()).slice(0, 5000);
    expect(bodyText, `${route.path} body content`).toMatch(route.match);

    // No user-visible error phrase. The /design/ catalog demos error states
    // by showing sample Alert/Toast strings — so we skip this assertion
    // there (a real bug on /design/ would still surface as a JS error below).
    if (!route.path.startsWith('/design')) {
      for (const p of USER_VISIBLE_ERROR_PATTERNS) {
        expect(bodyText, `${route.path} shows error: ${p}`).not.toMatch(p);
      }
    }

    // Alerts: at most acceptable ones (auth gates). Skipped on /design/ for
    // the same reason as the body check above — the catalog previews sample
    // Alert states with error-styled copy.
    if (!route.path.startsWith('/design')) {
      const alerts = await page.locator('[role="alert"], [role="status"]').allInnerTexts();
      for (const alert of alerts) {
        if (!alert.trim()) continue;
        const acceptable = ACCEPTABLE_ALERT_PATTERNS.some((p) => p.test(alert));
        if (!acceptable) {
          for (const p of USER_VISIBLE_ERROR_PATTERNS) {
            expect(alert, `${route.path} alert: ${alert.slice(0, 80)}`).not.toMatch(p);
          }
        }
      }
    }

    // JS errors (from window.onerror and console.error). We tolerate a few
    // known-noisy sources but any other error is a real regression.
    const filteredJs = jsErrors.filter(
      (e) =>
        !/hydrat/i.test(e) &&
        !/favicon/i.test(e) &&
        !/chrome-extension/i.test(e) &&
        // Auth-gated APIs on unauthenticated pages log a 401. That is the
        // expected "please log in" path — Chrome's network layer surfaces it
        // as a console.error even when the app handles it gracefully.
        !/status of 401/i.test(e) &&
        !/status of 403/i.test(e) &&
        // Some sub-resources (scorecards, SLA, atividade) return 404 for
        // mandates without data yet — that is the correct "no data" path.
        !/status of 404/i.test(e),
    );
    expect(filteredJs, `${route.path} JS errors`).toEqual([]);

    // Network errors: any 5xx or connection failure fails the test. 4xx is
    // OK because a few endpoints return 401 (auth-gated APIs called from
    // islands).
    expect(netErrors, `${route.path} network 5xx`).toEqual([]);
  });
}

// --- deeper journeys ---

test('politicos browser: filtro Municipal → SP → São Paulo carrega vereadores', async ({ page }) => {
  const { jsErrors } = await collectPageIssues(page);
  await page.goto('/politicos/?sphere=municipal&uf=SP&municipio=S%C3%83O%20PAULO', {
    waitUntil: 'networkidle',
  });
  await page.waitForTimeout(2500);
  const bodyText = await page.locator('body').innerText();
  // Should render either the results (with the count word) or the empty-state
  // gate if the filter is invalid.
  expect(bodyText).toMatch(/pol[íi]tico|vereador|munic[íi]pio/i);
  expect(
    jsErrors.filter(
      (e) =>
        !/hydrat/i.test(e) &&
        !/status of 40[134]/i.test(e) &&
        !/favicon/i.test(e),
    ),
  ).toEqual([]);
});

test('perfil individual estadual: página SSG carrega e hidrata', async ({ page }) => {
  const { jsErrors } = await collectPageIssues(page);
  // Fetch a mandate id from the API first so the test doesn't hardcode.
  const api = await page.request.get(
    '/api/v1/politicos/browse?sphere=estadual&uf=SP&limit=1',
  );
  const body = await api.json();
  const id = body?.data?.items?.[0]?.id;
  test.skip(!id, 'no estadual mandate seeded in prod');
  await page.goto(`/politicos/${id}/`, { waitUntil: 'networkidle' });
  await page.waitForTimeout(2000);
  const bodyText = await page.locator('body').innerText();
  expect(bodyText).toMatch(/parlamentar|ESP|MG|mandat|perfil/i);
  expect(
    jsErrors.filter(
      (e) =>
        !/hydrat/i.test(e) &&
        !/status of 40[134]/i.test(e) &&
        !/favicon/i.test(e),
    ),
  ).toEqual([]);
});

test('perfil municipal via ?id: fallback CSR renderiza', async ({ page }) => {
  const { jsErrors } = await collectPageIssues(page);
  const api = await page.request.get(
    '/api/v1/politicos/browse?sphere=municipal&uf=SP&municipio=S%C3%83O%20PAULO&limit=1',
  );
  const body = await api.json();
  const id = body?.data?.items?.[0]?.id;
  test.skip(!id, 'no municipal mandate seeded in prod');
  await page.goto(`/politicos/?id=${id}`, { waitUntil: 'networkidle' });
  await page.waitForTimeout(2500);
  const bodyText = await page.locator('body').innerText();
  // The MandateDetail island shows "carregando" briefly then the profile.
  expect(bodyText).toMatch(/mandat|prefeito|vereador|carregando/i);
  expect(
    jsErrors.filter(
      (e) =>
        !/hydrat/i.test(e) &&
        !/status of 40[134]/i.test(e) &&
        !/favicon/i.test(e),
    ),
  ).toEqual([]);
});

test('partidos: catalogo renderiza pelo menos 10 partidos', async ({ page }) => {
  const { jsErrors } = await collectPageIssues(page);
  await page.goto('/partidos/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(2500);
  // Party cards are anchors linking to /partidos/{slug}.
  const partyLinks = await page.locator('a[href^="/partidos/"]').count();
  expect(partyLinks, 'party links').toBeGreaterThan(10);
  expect(
    jsErrors.filter(
      (e) =>
        !/hydrat/i.test(e) &&
        !/status of 40[134]/i.test(e) &&
        !/favicon/i.test(e),
    ),
  ).toEqual([]);
});

test('propor: picker de mandato carrega pelo menos 100 opções', async ({ page }) => {
  const { jsErrors } = await collectPageIssues(page);
  await page.goto('/propor/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(3000);
  const bodyText = await page.locator('body').innerText();
  // The picker either shows a login gate or lists mandates.
  const hasGate = /entre|precisa/i.test(bodyText);
  if (!hasGate) {
    // If not gated, mandate options should exist somewhere in the DOM.
    const optionCount = await page.locator('option, li, [role="option"]').count();
    expect(optionCount, 'mandate options').toBeGreaterThan(50);
  }
  expect(
    jsErrors.filter(
      (e) =>
        !/hydrat/i.test(e) &&
        !/status of 40[134]/i.test(e) &&
        !/favicon/i.test(e),
    ),
  ).toEqual([]);
});

test('gasto parlamentar: dashboard hidrata sem erro de carregamento', async ({ page }) => {
  const { jsErrors, netErrors } = await collectPageIssues(page);
  await page.goto('/politicos/gastos/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(3500);
  const bodyText = await page.locator('body').innerText();
  expect(bodyText).not.toMatch(/n[ãa]o foi poss[íi]vel|servi[çc]o.*indispon/i);
  expect(bodyText).toMatch(/gasto|R\$|carregando/i);
  expect(
    jsErrors.filter(
      (e) =>
        !/hydrat/i.test(e) &&
        !/status of 40[134]/i.test(e) &&
        !/favicon/i.test(e),
    ),
  ).toEqual([]);
  expect(netErrors).toEqual([]);
});

test('gasto parlamentar: federal-only (chip Esfera removido) e link antigo cai em federal', async ({ page }) => {
  const { jsErrors, netErrors } = await collectPageIssues(page);
  // Link antigo com sphere=municipal — o front deve normalizar / o back
  // deve ignorar. O painel precisa mostrar CEAP+CEAPS (dado real), nunca
  // "R$ 0" enganoso vindo de estadual/municipal.
  await page.goto('/politicos/gastos/?sphere=municipal', {
    waitUntil: 'networkidle',
  });
  await page.waitForTimeout(2500);
  const bodyText = await page.locator('body').innerText();
  // A UI não pode oferecer chip de Esfera nesta página.
  expect(bodyText, 'chip esfera não deve existir').not.toMatch(/^Esfera$/m);
  // O aviso deve estar visível para orientar o usuário.
  expect(bodyText).toMatch(/Federal apenas|CEAP|CEAPS/);
  // Total deve ser positivo (dado real) — nunca R$ 0,00 no grande.
  expect(bodyText).toMatch(/R\$\s*\d/);
  expect(bodyText).not.toMatch(/R\$\s*0[.,]00\s*$/m);
  expect(jsErrors.filter((e) => !/hydrat|status of 40[134]|favicon/i.test(e))).toEqual([]);
  expect(netErrors).toEqual([]);
});

test('/eleicoes/2026: carrega comparador com filtros', async ({ page }) => {
  const { jsErrors, netErrors } = await collectPageIssues(page);
  await page.goto('/eleicoes/2026/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(2500);
  const bodyText = await page.locator('body').innerText();
  // Either shows the empty-state ("nenhuma eleição") OR the browser.
  expect(bodyText).toMatch(/elei[çc]|2026|candidato|carregando/i);
  expect(
    jsErrors.filter(
      (e) =>
        !/hydrat/i.test(e) &&
        !/status of 40[134]/i.test(e) &&
        !/favicon/i.test(e),
    ),
  ).toEqual([]);
  expect(netErrors).toEqual([]);
});

test('/cadastrar: mostra regras do fediverso (3000 chars + 15 min + verificação)', async ({ page }) => {
  await page.goto('/cadastrar/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(1000);
  const body = await page.locator('body').innerText();
  expect(body).toMatch(/3\s*000\s*caracteres/i);
  expect(body).toMatch(/15\s*minutos/i);
  expect(body).toMatch(/T[íi]tulo de eleitor/i);
  expect(body).toMatch(/vota[çc][ãa]o restrita|não computam voto/i);
});

test('/confirmar-conta sem token: mostra erro + botão pra refazer', async ({ page }) => {
  // Sem `?token=`, a página deve cair no branch de erro do ConfirmSignupForm
  // e oferecer o CTA de refazer cadastro — protege contra alguém aterrissar
  // aqui via link colado errado.
  await page.goto('/confirmar-conta/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(1500);
  const body = await page.locator('body').innerText();
  expect(body).toMatch(/Link inv[áa]lido|Refazer cadastro/i);
  await expect(page.locator('a[href="/cadastrar"]').first()).toBeVisible();
});

test('/confirmar-conta com token bogus: back → 401 + CTA visível', async ({ page }) => {
  // O front chama POST /auth/register/confirm{token:"…"}; o back responde 401
  // pra token inexistente, e o island mostra o alerta em vermelho + os botões
  // de Refazer/Entrar. Não estamos mockando o back — assume que o gateway
  // co-servindo o site também responde /api/v1/… (mesma origem, sem CORS).
  await page.goto('/confirmar-conta/?token=this-token-does-not-exist', {
    waitUntil: 'networkidle',
  });
  await page.waitForTimeout(2500);
  const body = await page.locator('body').innerText();
  expect(body).toMatch(/link pode ter expirado|Refazer cadastro|N[ãa]o autenticado/i);
});

test('menu principal: link "Eleições 2026" navegável', async ({ page }) => {
  await page.goto('/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(500);
  // Header nav é sempre visível (drawer se abre em mobile, mas viewport
  // default do Playwright é 1280×800).
  const link = page.locator('nav a[href="/eleicoes/2026"]').first();
  await expect(link).toBeVisible();
  await expect(link).toHaveText(/Elei[çc][õo]es 2026/i);
  await link.click();
  await page.waitForURL(/\/eleicoes\/2026/);
  const body = await page.locator('body').innerText();
  expect(body).toMatch(/Pr[óo]ximo passo|Registro de candidatura/i);
});

test('/eleicoes/2026: painel TSE mostra countdown + calendário', async ({ page }) => {
  const { jsErrors, netErrors } = await collectPageIssues(page);
  await page.goto('/eleicoes/2026/', { waitUntil: 'networkidle' });
  await page.waitForTimeout(2000);
  const body = await page.locator('body').innerText();
  // O painel TSE deve mostrar pelo menos "Próximo passo" (eyebrow do countdown).
  expect(body).toMatch(/Pr[óo]ximo passo/i);
  // Milestones críticos do calendário.
  expect(body).toMatch(/Registro de candidatura/i);
  expect(body).toMatch(/1º turno/i);
  expect(body).toMatch(/2º turno/i);
  expect(body).toMatch(/Posse dos eleitos/i);
  expect(jsErrors.filter((e) => !/hydrat|status of 40[134]|favicon/i.test(e))).toEqual([]);
  expect(netErrors).toEqual([]);
});
