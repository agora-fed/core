// Suíte de smoke UI (issue #36 / R6.2). Formaliza os repro-scripts do dia num
// gate pós-deploy: abre as telas-chave num navegador real e falha se uma ilha
// morre (pageerror) ou se um marcador esperado some. Pega bugs que os testes de
// contrato/API NÃO pegam — os 3 de 2026-07-26 (each_key_duplicate no feed,
// unobserve(null) ao apagar, .ok/.success no painel) teriam falhado aqui.
//
// Uso:  BASE_URL=https://democracia.social.br DSOC_SESSION=<cookie> node web/tests/smoke.mjs
// Sem DSOC_SESSION, roda só as checagens públicas. Sai != 0 em qualquer falha.
//
// Roda a partir de web/ (playwright resolve de web/node_modules).
import { chromium } from 'playwright';

const BASE = process.env.BASE_URL || 'https://democracia.social.br';
const SESSION = process.env.DSOC_SESSION || null;
const results = [];

function record(name, ok, detail = '') {
  results.push({ name, ok, detail });
  const tag = ok ? '✓' : '✗';
  console.log(`${tag} ${name}${detail ? ' — ' + detail : ''}`);
}

/** Abre `path`, coleta pageerror/console.error, e roda `check(page)` -> string|null (erro). */
async function checkPage(ctx, name, path, check) {
  const page = await ctx.newPage();
  const errs = [];
  // pageerror = exceção de JS não capturada (ilha morta) → sempre falha; foi a
  // assinatura de 2 dos 3 bugs de 26/07. Erros de console só contam quando são
  // FALTA de JS real (TypeError etc.); 404 de recurso opcional (ex.: /campanha
  // de quem não tem campanha) é ruído esperado, não regressão.
  page.on('pageerror', (e) => errs.push(`pageerror: ${e.message}`));
  page.on('console', (m) => {
    if (m.type() === 'error' && /TypeError|ReferenceError|is not a function|Cannot read|undefined is not/.test(m.text()))
      errs.push(`console: ${m.text()}`);
  });
  try {
    const resp = await page.goto(BASE + path, { waitUntil: 'networkidle', timeout: 45000 });
    if (!resp || resp.status() >= 400) {
      record(name, false, `HTTP ${resp ? resp.status() : 'sem resposta'}`);
      await page.close();
      return;
    }
    await page.waitForTimeout(2500);
    const problem = await check(page);
    if (problem) {
      record(name, false, problem);
    } else if (errs.length) {
      record(name, false, errs[0]);
    } else {
      record(name, true);
    }
  } catch (e) {
    record(name, false, e.message.split('\n')[0]);
  } finally {
    await page.close();
  }
}

const hasText = (page, t) => page.evaluate((s) => document.body.innerText.includes(s), t);

const browser = await chromium.launch();

// --- Checagens públicas (sem sessão) ------------------------------------------------
const pub = await browser.newContext();
await checkPage(pub, 'home /', '/', (p) => hasText(p, 'DemocraciaBR').then((v) => (v ? null : 'sem marca')));
await checkPage(pub, 'feed (gate anon)', '/feed/', (p) =>
  hasText(p, 'Entre para ver').then((v) => (v ? null : 'gate de login nao apareceu')),
);
await checkPage(pub, 'foruns /f/', '/f/', (p) => hasText(p, 'Fóruns').then((v) => (v ? null : 'sem titulo')));
await checkPage(pub, 'perfil publico', '/perfil/?u=socrates', (p) =>
  hasText(p, 'Socrates').then((v) => (v ? null : 'perfil nao carregou')),
);
await checkPage(pub, 'propostas', '/propostas', () => null);
await checkPage(pub, 'consultas', '/consultas', () => null);
await pub.close();

// --- Checagens autenticadas (com sessao) --------------------------------------------
if (SESSION) {
  const ctx = await browser.newContext();
  await ctx.addCookies([
    { name: 'dsoc_session', value: SESSION, domain: new URL(BASE).hostname, path: '/', httpOnly: true, secure: true, sameSite: 'Lax' },
  ]);
  await ctx.addInitScript(() => {
    localStorage.setItem('dsoc_citizen', '1');
    localStorage.setItem('dsoc_handle', 'socrates');
  });

  await checkPage(ctx, 'feed autenticado (cards, sem gate)', '/feed/', async (p) => {
    if (await hasText(p, 'Entre para ver')) return 'caiu no gate mesmo logado';
    const cards = await p.evaluate(() => document.querySelectorAll('article').length);
    return cards > 0 ? null : 'zero cards no feed';
  });

  await checkPage(ctx, 'perfil proprio (notas)', '/perfil/?u=socrates', (p) =>
    hasText(p, 'nao consegui carregar').then((v) => (v ? 'erro ao carregar notas' : null)),
  );

  await checkPage(ctx, 'composer autocomplete @', '/feed/', async (p) => {
    const ta = p.locator('textarea').first();
    await ta.click();
    await ta.pressSequentially('@so', { delay: 100 });
    await p.waitForTimeout(1200);
    const open = await p.evaluate(() => !!document.querySelector('ul.ac'));
    return open ? null : 'dropdown de mencao nao abriu';
  });

  await checkPage(ctx, '/admin/papeis (matriz carrega)', '/admin/papeis/', async (p) => {
    if (await hasText(p, 'precisa da permissão')) return 'erro de permissao indevido';
    const roles = await p.evaluate(() => document.querySelectorAll('.rname').length);
    return roles > 0 ? null : 'nenhum papel listado';
  });

  await ctx.close();
} else {
  record('checagens autenticadas', true, 'PULADAS (sem DSOC_SESSION)');
}

await browser.close();

const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} ok`);
if (failed.length) {
  console.log('FALHOU:', failed.map((f) => f.name).join(', '));
  process.exit(1);
}
console.log('smoke UI: tudo verde');
