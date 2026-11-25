import { describe, expect, it } from 'vitest';
import {
  EMPTY_FILTER,
  auditDetail,
  auditLabel,
  barSegments,
  byColumn,
  failureOf,
  filterDeputies,
  href,
  movedPositions,
  moveBody,
  normalize,
  orientationsText,
  outcomeText,
  parseRoute,
  prioritiesIn,
  roleLabel,
  safeUrl,
  sections,
  selectIds,
  undecidedId,
  type AuditRow,
  type Column,
  type Deputy,
} from '../src/lib/articulacao';

const dep = (camara_id: number, name: string, party: string, uf: string, groups: string[] = []): Deputy => ({
  camara_id,
  name,
  party,
  uf,
  photo_url: null,
  groups,
  board_role: null,
  leader_role: null,
});
const deputies = [
  dep(1, 'Ana Lúcia', 'PT', 'SP', ['caucus:women']),
  dep(2, 'Bruno', 'PL', 'RJ'),
  dep(3, 'Célia', 'PT', 'BA', ['caucus:women']),
  dep(4, 'Davi', 'PSD', 'SP'),
];
const col = (id: string, key: string | null, name = id): Column => ({ id, key, name, color: '#000', position: 0 });
const columns = [col('u', 'undecided'), col('y', 'yes'), col('n', 'no'), col('c', null, 'Indecisos PSD')];

describe('parseRoute keeps the first-version links', () => {
  it('maps every hash route', () => {
    expect(parseRoute('')).toEqual({ name: 'rooms' });
    expect(parseRoute('#/')).toEqual({ name: 'rooms' });
    expect(parseRoute('#/sala/r1')).toEqual({ name: 'room', id: 'r1' });
    expect(parseRoute('#/sala/r1/historico')).toEqual({ name: 'audit', id: 'r1' });
    expect(parseRoute('#/projeto/p1?cenario=s1')).toEqual({ name: 'project', id: 'p1', scenario: 's1' });
    expect(parseRoute('#/projeto/p1')).toEqual({ name: 'project', id: 'p1', scenario: null });
    expect(parseRoute('#/votacoes')).toEqual({ name: 'roll-calls' });
    expect(parseRoute('#/votacao/2645346-18')).toEqual({ name: 'roll-call', id: '2645346-18' });
    expect(parseRoute('#/grupos')).toEqual({ name: 'groups' });
    expect(parseRoute('#/grupo/caucus%3Awomen')).toEqual({ name: 'group', id: 'caucus:women' });
    expect(parseRoute('#/nada')).toEqual({ name: 'not-found' });
    expect(parseRoute('#/sala')).toEqual({ name: 'not-found' });
  });
  it('builds links that parse back', () => {
    expect(parseRoute(href.project('p 1', 's/2'))).toEqual({ name: 'project', id: 'p 1', scenario: 's/2' });
    expect(parseRoute(href.group('caucus:women'))).toEqual({ name: 'group', id: 'caucus:women' });
    expect(parseRoute(href.audit('r1'))).toEqual({ name: 'audit', id: 'r1' });
  });
});

describe('failureOf', () => {
  const res = (code: string, message = 'm') => ({ success: false, data: null, error: { code, message }, meta: null });
  it('asks to log in on 401 codes', () => {
    for (const c of ['unauthenticated', 'unauthorized', 'http_401']) expect(failureOf(res(c))).toEqual({ kind: 'login' });
  });
  it('says the module is off', () => {
    expect(failureOf(res('module_disabled'))).toEqual({ kind: 'module_off' });
  });
  it('shows the server message otherwise', () => {
    expect(failureOf(res('invalid', 'Nome vazio.'))).toEqual({ kind: 'message', text: 'Nome vazio.' });
  });
});

describe('labels', () => {
  it('names roles and audit actions in Portuguese', () => {
    expect(roleLabel('owner')).toBe('Responsável');
    expect(roleLabel('viewer')).toBe('Leitor(a)');
    expect(roleLabel('x')).toBe('x');
    expect(auditLabel('position.move')).toBe('moveu deputados');
    expect(auditLabel('other')).toBe('other');
  });
  it('details a move with up to four names', () => {
    const names = new Map([[1, 'Ana'], [2, 'Bruno']]);
    const row = (payload: Record<string, unknown>, action = 'position.move'): AuditRow => ({
      id: 1, actor_id: 'a', actor_name: 'Ana', action, payload, at: '',
    });
    expect(auditDetail(row({ moves: [[1, 'y'], [2, 'y']] }), names)).toBe('2 — Ana, Bruno');
    expect(auditDetail(row({ moves: [[1], [2], [9], [1], [2]] }), names)).toBe('5 — Ana, Bruno, 9, Ana…');
    expect(auditDetail(row({ name: 'Cenário 2' }, 'scenario.create'), names)).toBe('Cenário 2');
    expect(auditDetail(row({}, 'note.create'), names)).toBe('');
  });
  it('states the outcome', () => {
    const counts = { yes: 300, no: 100, disputed: 20, absent: 10, chair: 1, undecided: 82 };
    expect(outcomeText({ outcome: { state: 'approved', margin: 43 }, counts, threshold: 257 })).toBe(
      'Aprovado — 300 votos Sim, 43 acima do mínimo de 257',
    );
    expect(outcomeText({ outcome: { state: 'rejected' }, counts, threshold: 308 })).toBe(
      'Rejeitado — com 100 votos Não, o Sim não chega mais a 308',
    );
    expect(outcomeText({ outcome: { state: 'open', missing: 8, reachable: true }, counts, threshold: 308 })).toBe(
      'Faltam 8 votos Sim · Sim + Em disputa = 320 (alcança o mínimo)',
    );
    expect(outcomeText({ outcome: { state: 'open', missing: 8, reachable: false }, counts, threshold: 308 })).toContain(
      'não alcança',
    );
  });
  it('lists only orientations that exist', () => {
    expect(orientationsText([['PT', 'Sim'], ['PL', null], ['Gov.', 'Não']])).toBe('PT Sim · Gov. Não');
  });
});

describe('filterDeputies', () => {
  const none = new Set<number>();
  it('searches without accents or case', () => {
    expect(filterDeputies(deputies, { ...EMPTY_FILTER, q: 'celia' }, none, none).map((d) => d.camara_id)).toEqual([3]);
    expect(filterDeputies(deputies, { ...EMPTY_FILTER, q: 'sp' }, none, none).map((d) => d.camara_id)).toEqual([1, 4]);
    expect(normalize('Ação')).toBe('acao');
  });
  it('combines party, UF, group, priorities and dissidents', () => {
    expect(filterDeputies(deputies, { ...EMPTY_FILTER, party: 'PT', uf: 'BA' }, none, none)).toHaveLength(1);
    expect(filterDeputies(deputies, { ...EMPTY_FILTER, group: 'caucus:women' }, none, none)).toHaveLength(2);
    expect(filterDeputies(deputies, { ...EMPTY_FILTER, onlyPriorities: true }, new Set([2]), none)[0].name).toBe('Bruno');
    expect(filterDeputies(deputies, { ...EMPTY_FILTER, onlyDissidents: true }, none, new Set([4]))[0].name).toBe('Davi');
  });
  it('does not change the input list', () => {
    const copy = [...deputies];
    filterDeputies(deputies, { ...EMPTY_FILTER, q: 'x' }, none, none);
    expect(deputies).toEqual(copy);
  });
});

describe('the board', () => {
  const positions = [
    { camara_id: 2, column_id: 'y' },
    { camara_id: 4, column_id: 'c' },
  ];
  it('puts a deputy with no position in "undecided"', () => {
    expect(undecidedId(columns)).toBe('u');
    const map = byColumn(columns, positions, deputies);
    expect(map.get('u')!.map((d) => d.camara_id)).toEqual([1, 3]);
    expect(map.get('y')!.map((d) => d.camara_id)).toEqual([2]);
    expect(map.get('c')!.map((d) => d.camara_id)).toEqual([4]);
    expect(map.get('n')).toEqual([]);
  });
  it('sends a position to a deleted column back to "undecided"', () => {
    const map = byColumn(columns, [{ camara_id: 1, column_id: 'gone' }], deputies);
    expect(map.get('u')!.map((d) => d.camara_id)).toContain(1);
  });
  it('groups a column by party or UF, sorted', () => {
    expect(sections(deputies, 'none')).toEqual([{ key: '', items: deputies }]);
    expect(sections(deputies, 'party').map((s) => [s.key, s.items.length])).toEqual([
      ['PL', 1],
      ['PSD', 1],
      ['PT', 2],
    ]);
    expect(sections(deputies, 'uf').map((s) => s.key)).toEqual(['BA', 'RJ', 'SP']);
  });
  it('selects one, toggles it, or a shift range', () => {
    const order = [1, 2, 3, 4];
    const one = selectIds(new Set(), order, 2, null, false);
    expect([...one]).toEqual([2]);
    expect([...selectIds(one, order, 2, 2, false)]).toEqual([]);
    expect([...selectIds(one, order, 4, 2, true)].sort()).toEqual([2, 3, 4]);
    expect([...selectIds(one, order, 9, 2, true)].sort()).toEqual([2, 9]);
    expect([...one]).toEqual([2]);
  });
  it('moves optimistically and builds the request', () => {
    expect(movedPositions(positions, [1, 2], 'n', 'u')).toEqual([
      { camara_id: 4, column_id: 'c' },
      { camara_id: 1, column_id: 'n' },
      { camara_id: 2, column_id: 'n' },
    ]);
    expect(movedPositions(positions, [2], 'u', 'u')).toEqual([{ camara_id: 4, column_id: 'c' }]);
    expect(moveBody([1, 2], 'u', 'u')).toEqual([
      { camara_id: 1, column_id: null },
      { camara_id: 2, column_id: null },
    ]);
    expect(moveBody([1], 'y', 'u')).toEqual([{ camara_id: 1, column_id: 'y' }]);
  });
  it('counts priorities per column', () => {
    expect(prioritiesIn('u', [1, 2, 3], positions, 'u')).toBe(2);
    expect(prioritiesIn('y', [1, 2, 3], positions, 'u')).toBe(1);
  });
  it('sizes the bar in percent of the total', () => {
    const s = barSegments({ yes: 257, no: 0, disputed: 0, absent: 0, chair: 1, undecided: 255 }, 513);
    expect(s.yes).toBeCloseTo(50.1, 1);
    expect(barSegments({ yes: 1, no: 0, disputed: 0, absent: 0, chair: 0, undecided: 0 }, 0).yes).toBeCloseTo(
      100 / 513,
    );
  });
});

describe('safeUrl', () => {
  it('accepts https only', () => {
    expect(safeUrl('https://www.camara.leg.br/x.jpg')).toBe('https://www.camara.leg.br/x.jpg');
    expect(safeUrl('http://x')).toBeNull();
    expect(safeUrl('javascript:alert(1)')).toBeNull();
    expect(safeUrl('https://x" onerror="y')).toBeNull();
    expect(safeUrl(null)).toBeNull();
  });
});
