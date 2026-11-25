// Client and pure logic of the DIOGENES articulation area (agora-fed/diogenes, ADR-0020).
// The server enforces every rule (membership, roles, org, module gate); these helpers only
// decide what to show. All pure functions return new values and never mutate their input.
import { API_BASE, apiDelete, apiGetCredentialed, apiPatch, apiPost, apiPut } from './api';
import type { ApiResponse } from './types';

export type Role = 'owner' | 'editor' | 'viewer';

export interface RoomSummary {
  id: string;
  name: string;
  role: Role;
  created_at: string;
}
export interface MyRooms {
  rooms: RoomSummary[];
  can_create_rooms: boolean;
  display_name: string;
}
export interface Member {
  user_id: string;
  display_name: string;
  role: Role;
}
/** How a project is approved (diogenes ADR-0008). */
export type Rule = 'fixed' | 'simple';
export interface ProjectSummary {
  id: string;
  name: string;
  /** For the rule `simple`: the quorum. */
  threshold: number;
  rule: Rule;
  bill_id: number | null;
}
export interface RoomDetail extends RoomSummary {
  members: Member[];
  projects: ProjectSummary[];
  can_create_rooms: boolean;
}
export interface AuditRow {
  id: number;
  actor_id: string;
  actor_name: string | null;
  action: string;
  payload: Record<string, unknown>;
  at: string;
}
export interface Column {
  id: string;
  key: string | null;
  name: string;
  color: string;
  position: number;
}
export interface ScenarioSummary {
  id: string;
  name: string;
  source_roll_call: string | null;
  yes: number;
}
export interface Note {
  id: string;
  camara_id: number;
  author_id: string;
  author_name: string | null;
  body: string;
  visibility: 'room' | 'author';
  updated_at: string;
}
export interface Project {
  id: string;
  room_id: string;
  name: string;
  threshold: number;
  rule: Rule;
  bill_id: number | null;
  role: Role;
  columns: Column[];
  scenarios: ScenarioSummary[];
  priorities: number[];
  notes: Note[];
}
export interface Position {
  camara_id: number;
  column_id: string;
}
export interface Counts {
  yes: number;
  no: number;
  disputed: number;
  absent: number;
  chair: number;
  undecided: number;
}
export type Outcome =
  | { state: 'approved'; margin: number }
  | { state: 'rejected' }
  | { state: 'open'; missing: number; reachable: boolean; quorum_missing?: number };
export interface Scenario {
  id: string;
  project_id: string;
  name: string;
  source_roll_call: string | null;
  columns: Column[];
  positions: Position[];
  column_counts: Record<string, number>;
  counts: Counts;
  total: number;
  threshold: number;
  rule: Rule;
  outcome: Outcome;
  orientations: [string, string | null][];
  dissidents: number[];
}
export interface Deputy {
  camara_id: number;
  name: string;
  party: string;
  uf: string;
  photo_url: string | null;
  groups: string[];
  board_role: string | null;
  leader_role: string | null;
}
export interface Group {
  id: string;
  kind: 'caucus' | 'front';
  title: string;
  source: 'camara' | 'tse';
  official_total: number | null;
  member_count: number;
  in_office_count: number;
}
export interface GroupDetail extends Group {
  details: { signatures_net?: number; document_url?: string } | null;
  members: { camara_id: number; name: string | null; role: string | null; in_office: boolean }[];
}
export interface RollCall {
  id: string;
  voted_on: string;
  description: string;
  approved: boolean | null;
  bill_id: number | null;
  bill_label: string | null;
  vote_count: number;
  totals_match: boolean | null;
}
export interface RollCallDetail extends RollCall {
  bill_summary: string | null;
  orientations: [string, string | null][];
  counts: Record<string, number>;
  votes: { camara_id: number; name: string; party: string; uf: string; vote: string }[];
  dissidents: number[];
}
export interface SyncRun {
  job: string;
  started_at: string;
  finished_at: string | null;
  status: string;
  items: number;
}
export interface ListMatch {
  found: [number, string][];
  missed: string[];
}

// ---------------------------------------------------------------- API ----------
const D = '/api/v1/diogenes';
const enc = encodeURIComponent;

// ------------------------------------------------------- campaign finance (TSE) ------
// diogenes ADR-0009. Declared facts with their source; the site never labels a person.
export type MoneySource = 'individuals' | 'party' | 'own' | 'candidates' | 'crowdfunding' | 'other';
export interface Provenance {
  year: number;
  source_url: string;
  imported_at: string | null;
}
export interface SourceTotal {
  source: MoneySource;
  cents: number;
  receipts: number;
}
export interface DonorView {
  key: string;
  name: string;
  kind: 'person' | 'candidate';
  activity: string | null;
  uf: string | null;
  cents: number;
  via_party: boolean;
}
export interface Campaign extends Provenance {
  total_cents: number;
  totals: SourceTotal[];
  donors: DonorView[];
}
export interface ColumnMoney {
  column_id: string;
  deputies: number;
  with_accounts: number;
  cents_by_source: Partial<Record<MoneySource, number>>;
}
export interface SharedDonor {
  key: string;
  name: string;
  kind: 'person' | 'candidate';
  activity: string | null;
  deputies: number;
  cents: number;
  by_column: Record<string, number>;
}
export interface ScenarioMoney extends Provenance {
  columns: ColumnMoney[];
  donors: SharedDonor[];
}

const SOURCE_LABEL: Record<MoneySource, string> = {
  party: 'Partido (inclui fundo eleitoral)',
  individuals: 'Pessoas físicas',
  own: 'Recursos próprios',
  candidates: 'Outros candidatos',
  crowdfunding: 'Financiamento coletivo',
  other: 'Outras origens',
};
export const sourceLabel = (s: string) => SOURCE_LABEL[s as MoneySource] ?? s;

const BRL = new Intl.NumberFormat('pt-BR', { style: 'currency', currency: 'BRL' });
/** Cents → "R$ 1.234,56". */
export const brl = (cents: number) => BRL.format((Number(cents) || 0) / 100);

/** Each source with its share of the total, in percent (one decimal). */
export function sourceShares(totals: SourceTotal[]): { source: MoneySource; label: string; cents: number; pct: number }[] {
  const all = totals.reduce((n, t) => n + t.cents, 0);
  return totals.map((t) => ({
    source: t.source,
    label: sourceLabel(t.source),
    cents: t.cents,
    pct: all > 0 ? Math.round((1000 * t.cents) / all) / 10 : 0,
  }));
}

export function donorKindText(d: Pick<DonorView, 'kind' | 'via_party'>): string {
  if (d.kind === 'candidate') return 'campanha de outro candidato';
  return d.via_party ? 'doou ao partido, que repassou' : 'doação direta';
}

/** "Sim 12 · Não 3": deputies per column, in the order of the columns, without zeros. */
export function byColumnText(byColumn: Record<string, number>, columns: Column[]): string {
  return columns
    .filter((c) => (byColumn[c.id] ?? 0) > 0)
    .map((c) => `${c.name} ${byColumn[c.id]}`)
    .join(' · ');
}

/** "importado em 25/11/2026" or a notice that there is no import yet. */
export function provenanceText(p: Provenance): string {
  if (!p.imported_at) return 'Os dados de campanha ainda não foram importados.';
  return `Fonte: TSE, prestação de contas ${p.year} · importado em ${new Date(p.imported_at).toLocaleDateString('pt-BR')}`;
}

export const api = {
  campaign: (camaraId: number) => apiGetCredentialed<Campaign>(`${D}/deputies/${Number(camaraId)}/campaign`),
  scenarioMoney: (id: string) => apiGetCredentialed<ScenarioMoney>(`${D}/scenarios/${enc(id)}/donors`),
  rooms: () => apiGetCredentialed<MyRooms>(`${D}/rooms`),
  createRoom: (name: string) => apiPost<RoomSummary>(`${D}/rooms`, { name }),
  room: (id: string) => apiGetCredentialed<RoomDetail>(`${D}/rooms/${enc(id)}`),
  renameRoom: (id: string, name: string) => apiPatch<string>(`${D}/rooms/${enc(id)}`, { name }),
  deleteRoom: (id: string) => apiDelete<string>(`${D}/rooms/${enc(id)}`),
  addMember: (id: string, handle: string, role: Role) =>
    apiPost<Member>(`${D}/rooms/${enc(id)}/members`, { handle, role }),
  removeMember: (id: string, userId: string) =>
    apiDelete<string>(`${D}/rooms/${enc(id)}/members/${enc(userId)}`),
  audit: (id: string) => apiGetCredentialed<AuditRow[]>(`${D}/rooms/${enc(id)}/audit?limit=200`),
  createProject: (roomId: string, body: ProjectBody) =>
    apiPost<string>(`${D}/rooms/${enc(roomId)}/projects`, body),
  project: (id: string) => apiGetCredentialed<Project>(`${D}/projects/${enc(id)}`),
  patchProject: (id: string, body: ProjectBody) =>
    apiPatch<string>(`${D}/projects/${enc(id)}`, body),
  deleteProject: (id: string) => apiDelete<string>(`${D}/projects/${enc(id)}`),
  addColumn: (projectId: string, name: string) =>
    apiPost<string>(`${D}/projects/${enc(projectId)}/columns`, { name }),
  renameColumn: (id: string, name: string) => apiPatch<string>(`${D}/columns/${enc(id)}`, { name }),
  deleteColumn: (id: string) => apiDelete<string>(`${D}/columns/${enc(id)}`),
  setPriorities: (projectId: string, ids: number[], value: boolean) =>
    apiPut<string>(`${D}/projects/${enc(projectId)}/priorities`, { camara_ids: ids, value }),
  addNote: (projectId: string, camaraId: number, body: string, isPrivate: boolean) =>
    apiPost<string>(`${D}/projects/${enc(projectId)}/notes`, {
      camara_id: camaraId,
      body,
      private: isPrivate,
    }),
  deleteNote: (id: string) => apiDelete<string>(`${D}/notes/${enc(id)}`),
  matchList: (projectId: string, text: string) =>
    apiPost<ListMatch>(`${D}/projects/${enc(projectId)}/match-list`, { text }),
  createScenario: (
    projectId: string,
    body: { name: string; copy_from?: string; roll_call?: string; absent_if_missing?: boolean },
  ) => apiPost<string>(`${D}/projects/${enc(projectId)}/scenarios`, body),
  scenario: (id: string) => apiGetCredentialed<Scenario>(`${D}/scenarios/${enc(id)}`),
  renameScenario: (id: string, name: string) => apiPatch<string>(`${D}/scenarios/${enc(id)}`, { name }),
  deleteScenario: (id: string) => apiDelete<string>(`${D}/scenarios/${enc(id)}`),
  move: (scenarioId: string, moves: { camara_id: number; column_id: string | null }[]) =>
    apiPut<string>(`${D}/scenarios/${enc(scenarioId)}/positions`, { moves }),
  deputies: () => apiGetCredentialed<Deputy[]>(`${D}/deputies`),
  groups: () => apiGetCredentialed<Group[]>(`${D}/groups`),
  group: (id: string) => apiGetCredentialed<GroupDetail>(`${D}/groups/${enc(id)}`),
  rollCalls: (bill?: number | null) =>
    apiGetCredentialed<RollCall[]>(`${D}/roll-calls?limit=${bill ? 100 : 200}${bill ? `&bill=${bill}` : ''}`),
  rollCall: (id: string) => apiGetCredentialed<RollCallDetail>(`${D}/roll-calls/${enc(id)}`),
  syncStatus: () => apiGetCredentialed<SyncRun[]>(`${D}/sync/status`),
};

export const eventsUrl = (projectId: string) => `${API_BASE}${D}/projects/${enc(projectId)}/events`;

/** What the page must show for a failed call: a login prompt, "module off", or a message. */
export type Failure = { kind: 'login' } | { kind: 'module_off' } | { kind: 'message'; text: string };

const LOGIN_CODES = new Set(['unauthenticated', 'unauthorized', 'http_401']);

export function failureOf(res: ApiResponse<unknown>): Failure {
  const code = res.error?.code ?? '';
  if (LOGIN_CODES.has(code)) return { kind: 'login' };
  if (code === 'module_disabled') return { kind: 'module_off' };
  return { kind: 'message', text: res.error?.message ?? 'Não foi possível carregar.' };
}

// ------------------------------------------------------------- routing ---------
export type View =
  | { name: 'rooms' }
  | { name: 'room'; id: string }
  | { name: 'audit'; id: string }
  | { name: 'project'; id: string; scenario: string | null }
  | { name: 'roll-calls' }
  | { name: 'roll-call'; id: string }
  | { name: 'groups' }
  | { name: 'group'; id: string }
  | { name: 'not-found' };

/** Parse the hash routes kept from the first version (`#/sala/…`, `#/projeto/…?cenario=…`). */
export function parseRoute(hash: string): View {
  const raw = hash.replace(/^#/, '') || '/';
  const [path, query] = raw.split('?');
  const params = new URLSearchParams(query ?? '');
  const parts = path.split('/').filter(Boolean).map((p) => {
    try {
      return decodeURIComponent(p);
    } catch {
      return p;
    }
  });
  const [head, id, tail] = parts;
  if (!head) return { name: 'rooms' };
  if (head === 'sala' && id && tail === 'historico') return { name: 'audit', id };
  if (head === 'sala' && id) return { name: 'room', id };
  if (head === 'projeto' && id) return { name: 'project', id, scenario: params.get('cenario') };
  if (head === 'votacoes') return { name: 'roll-calls' };
  if (head === 'votacao' && id) return { name: 'roll-call', id };
  if (head === 'grupos') return { name: 'groups' };
  if (head === 'grupo' && id) return { name: 'group', id };
  return { name: 'not-found' };
}

export const href = {
  rooms: () => '#/',
  room: (id: string) => `#/sala/${enc(id)}`,
  audit: (id: string) => `#/sala/${enc(id)}/historico`,
  project: (id: string, scenario?: string | null) =>
    `#/projeto/${enc(id)}${scenario ? `?cenario=${enc(scenario)}` : ''}`,
  rollCalls: () => '#/votacoes',
  rollCall: (id: string) => `#/votacao/${enc(id)}`,
  groups: () => '#/grupos',
  group: (id: string) => `#/grupo/${enc(id)}`,
};

// --------------------------------------------------------------- labels --------
export const ROLE_LABEL: Record<Role, string> = {
  owner: 'Responsável',
  editor: 'Editor(a)',
  viewer: 'Leitor(a)',
};
export const roleLabel = (r: string) => ROLE_LABEL[r as Role] ?? r;

const AUDIT_LABEL: Record<string, string> = {
  'room.create': 'criou a sala',
  'room.rename': 'renomeou a sala',
  'member.set': 'definiu papel de membro',
  'member.remove': 'removeu membro',
  'project.create': 'criou projeto',
  'project.update': 'alterou projeto',
  'project.delete': 'excluiu projeto',
  'column.create': 'criou coluna',
  'column.update': 'alterou coluna',
  'column.delete': 'excluiu coluna',
  'scenario.create': 'criou cenário',
  'scenario.rename': 'renomeou cenário',
  'scenario.delete': 'excluiu cenário',
  'position.move': 'moveu deputados',
  'priority.set': 'alterou prioritários',
  'note.create': 'escreveu anotação',
  'note.update': 'editou anotação',
  'note.delete': 'excluiu anotação',
};
export const auditLabel = (action: string) => AUDIT_LABEL[action] ?? action;

export function auditDetail(row: AuditRow, names: Map<number, string>): string {
  if (row.action === 'position.move') {
    const moves = Array.isArray(row.payload.moves) ? (row.payload.moves as [number, unknown][]) : [];
    const shown = moves.slice(0, 4).map((m) => names.get(Number(m[0])) ?? String(m[0]));
    return `${moves.length} — ${shown.join(', ')}${moves.length > 4 ? '…' : ''}`;
  }
  const name = row.payload.name;
  return typeof name === 'string' ? name : '';
}

export function outcomeText(sc: Pick<Scenario, 'outcome' | 'counts' | 'threshold'> & { rule?: Rule }): string {
  const { outcome: o, counts: c, threshold } = sc;
  if (sc.rule === 'simple') return simpleOutcomeText(o, c, threshold);
  if (o.state === 'approved') {
    return `Aprovado — ${c.yes} votos Sim, ${o.margin} acima do mínimo de ${threshold}`;
  }
  if (o.state === 'rejected') {
    return `Rejeitado — com ${c.no} votos Não, o Sim não chega mais a ${threshold}`;
  }
  const reach = o.reachable ? 'alcança' : 'não alcança';
  return `Faltam ${o.missing} votos Sim · Sim + Em disputa = ${c.yes + c.disputed} (${reach} o mínimo)`;
}

function simpleOutcomeText(o: Outcome, c: Counts, quorum: number): string {
  if (o.state === 'approved') {
    return `Aprovado por maioria simples — ${c.yes} Sim × ${c.no} Não, vantagem de ${o.margin}`;
  }
  if (o.state === 'rejected') {
    return `Rejeitado — o Sim (${c.yes}) não supera mais o Não (${c.no}), ou o quórum de ${quorum} não é possível`;
  }
  const parts: string[] = [];
  if (o.missing > 0) {
    parts.push(`Faltam ${o.missing} votos Sim entre os indecisos para vencer`);
  } else {
    parts.push(`O Sim vence o Não (${c.yes} × ${c.no})`);
  }
  parts.push(`Sim + Em disputa ${o.reachable ? 'vence' : 'não vence'} o Não`);
  const q = o.quorum_missing ?? 0;
  if (q > 0) parts.push(`faltam ${q} presentes para o quórum de ${quorum}`);
  return parts.join(' · ');
}

/** The short label of a rule, for lists and tiles. */
export function ruleText(rule: Rule | undefined, threshold: number): string {
  return rule === 'simple' ? `maioria simples · quórum ${threshold}` : `mínimo ${threshold}`;
}

/** The choices of the rule in the project forms. `auto` lets the kind of the bill decide. */
export const RULE_CHOICES = [
  { value: 'auto', label: 'Automática pelo tipo da proposição' },
  { value: 'simple', label: 'Maioria simples (PL, MPV, PDL)' },
  { value: 'fixed', label: 'Número fixo de votos (PLP 257, PEC 308)' },
] as const;
export type RuleChoice = (typeof RULE_CHOICES)[number]['value'];

export interface ProjectBody {
  name: string;
  bill_id: number | null;
  rule?: Rule;
  threshold?: number;
}

/**
 * The body of a project form. With `auto` and no number, the server picks the rule and the
 * number from the kind of the bill (PEC 308, PLP 257, other kinds simple majority).
 */
export function projectBody(form: { name: string; rule: RuleChoice; threshold: string; bill: string }): ProjectBody {
  const body: ProjectBody = { name: form.name.trim(), bill_id: form.bill.trim() ? Number(form.bill) : null };
  if (form.rule !== 'auto') body.rule = form.rule;
  const n = Number(form.threshold);
  if (form.threshold.trim() && Number.isInteger(n) && n > 0) body.threshold = n;
  return body;
}

export function thresholdHint(rule: RuleChoice): string {
  if (rule === 'simple') return 'Quórum de presença. Padrão: 257.';
  if (rule === 'fixed') return 'Votos Sim necessários. 257 = maioria absoluta · 308 = 3/5 (PEC).';
  return 'Deixe em branco: PEC 308, PLP 257, demais tipos por maioria simples.';
}

// ------------------------------------------------------------- the board -------
export interface Filter {
  q: string;
  party: string;
  uf: string;
  group: string;
  onlyPriorities: boolean;
  onlyDissidents: boolean;
}
export const EMPTY_FILTER: Filter = {
  q: '',
  party: '',
  uf: '',
  group: '',
  onlyPriorities: false,
  onlyDissidents: false,
};
export type GroupBy = 'none' | 'party' | 'uf';

/** Lower case, no accents — for search. */
export const normalize = (s: string) =>
  (s ?? '').normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();

export function filterDeputies(
  deputies: Deputy[],
  f: Filter,
  priorities: ReadonlySet<number>,
  dissidents: ReadonlySet<number>,
): Deputy[] {
  const q = normalize(f.q.trim());
  return deputies.filter(
    (d) =>
      (!q || normalize(`${d.name} ${d.party} ${d.uf}`).includes(q)) &&
      (!f.party || d.party === f.party) &&
      (!f.uf || d.uf === f.uf) &&
      (!f.group || d.groups.includes(f.group)) &&
      (!f.onlyPriorities || priorities.has(d.camara_id)) &&
      (!f.onlyDissidents || dissidents.has(d.camara_id)),
  );
}

export const undecidedId = (columns: Column[]) =>
  columns.find((c) => c.key === 'undecided')?.id ?? columns[0]?.id ?? '';

export function positionsMap(positions: Position[]): Map<number, string> {
  return new Map(positions.map((p) => [p.camara_id, p.column_id]));
}

/** Deputies per column; a deputy with no position is "undecided". */
export function byColumn(columns: Column[], positions: Position[], deputies: Deputy[]) {
  const pos = positionsMap(positions);
  const und = undecidedId(columns);
  const out = new Map<string, Deputy[]>(columns.map((c) => [c.id, []]));
  for (const d of deputies) {
    const col = pos.get(d.camara_id);
    const bucket = (col && out.get(col)) || out.get(und);
    bucket?.push(d);
  }
  return out;
}

/** Sections of one column, sorted by key; one unnamed section when not grouped. */
export function sections(deputies: Deputy[], by: GroupBy): { key: string; items: Deputy[] }[] {
  if (by === 'none') return [{ key: '', items: deputies }];
  const map = new Map<string, Deputy[]>();
  for (const d of deputies) {
    const k = by === 'party' ? d.party : d.uf;
    map.set(k, [...(map.get(k) ?? []), d]);
  }
  return [...map.entries()]
    .sort((a, b) => a[0].localeCompare(b[0], 'pt-BR'))
    .map(([key, items]) => ({ key, items }));
}

/** Toggle one id, or add the range from `last` to `id` (shift-click). */
export function selectIds(
  selected: ReadonlySet<number>,
  order: number[],
  id: number,
  last: number | null,
  range: boolean,
): Set<number> {
  const next = new Set(selected);
  if (range && last !== null) {
    const a = order.indexOf(last);
    const b = order.indexOf(id);
    if (a >= 0 && b >= 0) {
      for (const x of order.slice(Math.min(a, b), Math.max(a, b) + 1)) next.add(x);
      return next;
    }
  }
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/** The positions after a move, for an optimistic update. */
export function movedPositions(
  positions: Position[],
  ids: number[],
  columnId: string,
  undecided: string,
): Position[] {
  const moving = new Set(ids);
  const kept = positions.filter((p) => !moving.has(p.camara_id));
  return columnId === undecided ? kept : [...kept, ...ids.map((camara_id) => ({ camara_id, column_id: columnId }))];
}

/** The request body of a move: undecided is "no row". */
export const moveBody = (ids: number[], columnId: string, undecided: string) =>
  ids.map((camara_id) => ({ camara_id, column_id: columnId === undecided ? null : columnId }));

export function prioritiesIn(
  columnId: string,
  priorities: number[],
  positions: Position[],
  undecided: string,
): number {
  const pos = positionsMap(positions);
  return priorities.filter((id) => (pos.get(id) ?? undecided) === columnId).length;
}

/** Bar segments in percent of the total, in voting order. */
export function barSegments(c: Counts, total: number) {
  const t = total > 0 ? total : 513;
  const pct = (n: number) => (100 * n) / t;
  return {
    yes: pct(c.yes),
    absent: pct(c.absent),
    chair: pct(c.chair),
    no: pct(c.no),
  };
}

// ------------------------------------------------------------- formatting ------
export const fmtDate = (s: string | null) =>
  s ? new Date(s.length === 10 ? `${s}T12:00:00` : s).toLocaleDateString('pt-BR') : '';
export const fmtTime = (s: string | null) => (s ? new Date(s).toLocaleString('pt-BR') : '');
export const camaraUrl = (id: number) => `https://www.camara.leg.br/deputados/${Number(id)}`;
/** Links and images from the upstream API: https only. */
export const safeUrl = (u: unknown): string | null =>
  typeof u === 'string' && /^https:\/\/[^\s"'<>]+$/i.test(u) ? u : null;
export const orientationsText = (o: [string, string | null][]) =>
  o
    .filter((x) => x[1])
    .map((x) => `${x[0]} ${x[1]}`)
    .join(' · ');
export const loginHref = () => `/entrar/?next=${encodeURIComponent('/articulacao/')}`;

// ------------------------------------------------------- reference cache -------
export interface Reference {
  deputies: Deputy[];
  byId: Map<number, Deputy>;
  groups: Group[];
  groupById: Map<string, Group>;
}
let referenceCache: Promise<Reference | Failure> | null = null;

/** Deputies and groups, loaded once per page. A failure is not cached. */
export function loadReference(): Promise<Reference | Failure> {
  referenceCache ??= (async () => {
    const [d, g] = await Promise.all([api.deputies(), api.groups()]);
    if (!d.success || !d.data) return failureOf(d);
    if (!g.success || !g.data) return failureOf(g);
    return {
      deputies: d.data,
      byId: new Map(d.data.map((x) => [x.camara_id, x])),
      groups: g.data,
      groupById: new Map(g.data.map((x) => [x.id, x])),
    };
  })().then((r) => {
    if ('kind' in r) referenceCache = null;
    return r;
  });
  return referenceCache;
}

export const isFailure = (x: unknown): x is Failure =>
  typeof x === 'object' && x !== null && 'kind' in x && !('deputies' in x);
