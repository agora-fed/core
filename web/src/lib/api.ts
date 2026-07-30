// Tiny typed API client wrapping `fetch` with the frozen ApiResponse envelope.
// Base URL comes from PUBLIC_API_BASE (IPv6-first, per platform principle 4).

import type {
  ActivityDto,
  ApiResponse,
  BoostResultDto,
  ConsultationDto,
  ConsultaSummary,
  ConsultaDetail,
  FeedItemDto,
  LikeResultDto,
  MandateCommitmentsDto,
  MandateCrmDto,
  MandateDto,
  MandateInviteSummaryDto,
  MyMandateDto,
  PartyDetailDto,
  PartyDto,
  ProfileDto,
  ProfileUpdateDto,
  ProposalDto,
  PromiseDto,
  ResponsivenessDto,
  ScorecardDto,
  SessionInfoDto,
  SlaDto,
} from './types';

export type {
  ActivityDto,
  BoostResultDto,
  FeedItemDto,
  LikeResultDto,
  MandateDto,
  MandateInviteSummaryDto,
  MyMandateDto,
  PartyDetailDto,
  PartyDto,
  ProfileDto,
  ProfileUpdateDto,
  SessionInfoDto,
};

/** Default to a RELATIVE base (same origin that served the page) so the API call always reaches the
 *  gateway that serves this site — no CORS, no IPv6/host mismatch. Override via PUBLIC_API_BASE only
 *  if the front-end is served from a different origin than the API. */
export const API_BASE: string =
  ((import.meta.env.PUBLIC_API_BASE as string | undefined) ?? '').replace(/\/$/, '');

/** Default organization id used to scope public list endpoints. Override via PUBLIC_ORG_ID. */
export const DEFAULT_ORG_ID: string =
  (import.meta.env.PUBLIC_ORG_ID as string | undefined) ||
  '11111111-1111-1111-1111-111111111111';

/** Result wrapper so callers can render empty/error states without throwing. */
export interface Fetched<T> {
  ok: boolean;
  data: T | null;
  /** Portuguese, user-facing message when something went wrong. */
  error: string | null;
}

/** Adapt the `Fetched<T>` shape from `apiGet` into the `ApiResponse<T>`
 *  envelope used by newer islands. Some islands prefer `.success/.error.code`
 *  because they need to distinguish 401 (`http_401`) from a generic network
 *  failure. This bridges both worlds without doubling the number of clients. */
export function fetchedToApiResponse<T>(f: Fetched<T>): ApiResponse<T> {
  if (f.ok) {
    return { success: true, data: f.data, error: null, meta: null };
  }
  return {
    success: false,
    data: null,
    error: {
      code: 'fetched_error',
      message: f.error ?? 'Não foi possível carregar os dados.',
    },
    meta: null,
  };
}

const DEFAULT_TIMEOUT_MS = 6000;

/** Network-tolerant GET that always resolves (never throws) so SSR pages render
 *  a graceful empty/error state instead of a 500. */
export async function apiGet<T>(
  path: string,
  init?: RequestInit & { timeoutMs?: number },
): Promise<Fetched<T>> {
  const url = `${API_BASE}${path}`;
  const controller = new AbortController();
  const timeout = setTimeout(
    () => controller.abort(),
    init?.timeoutMs ?? DEFAULT_TIMEOUT_MS,
  );
  try {
    const res = await fetch(url, {
      ...init,
      headers: { accept: 'application/json', ...(init?.headers ?? {}) },
      signal: controller.signal,
    });
    // Parse defensively: framework-level 4xx/5xx (e.g. a 400 from the axum
    // Query extractor) often comes back as text/plain, not the ApiResponse
    // envelope. Try JSON first; on failure treat as a plain HTTP error so
    // "response body wasn't JSON" doesn't masquerade as a network outage.
    const text = await res.text();
    let body: ApiResponse<T> | null = null;
    try {
      body = JSON.parse(text) as ApiResponse<T>;
    } catch {
      /* not JSON — fall through */
    }
    if (body && 'success' in body) {
      if (!res.ok || !body.success) {
        return {
          ok: false,
          data: null,
          error: body.error?.message ?? 'Não foi possível carregar os dados.',
        };
      }
      return { ok: true, data: body.data, error: null };
    }
    return {
      ok: false,
      data: null,
      error: res.ok
        ? 'Resposta inesperada do servidor.'
        : 'Não foi possível carregar. Tente novamente em instantes.',
    };
  } catch {
    return {
      ok: false,
      data: null,
      error:
        'Serviço temporariamente indisponível. Tente novamente em instantes.',
    };
  } finally {
    clearTimeout(timeout);
  }
}

/** Client-side POST returning the parsed envelope (used by the Svelte islands). */
export async function apiPost<T>(
  path: string,
  payload: unknown,
  init?: RequestInit,
): Promise<ApiResponse<T>> {
  return apiBody<T>('POST', path, payload, init);
}

/** Client-side PATCH returning the parsed envelope. */
export async function apiPatch<T>(
  path: string,
  payload: unknown,
  init?: RequestInit,
): Promise<ApiResponse<T>> {
  return apiBody<T>('PATCH', path, payload, init);
}

/** Client-side PUT returning the parsed envelope. */
export async function apiPut<T>(
  path: string,
  payload: unknown,
  init?: RequestInit,
): Promise<ApiResponse<T>> {
  return apiBody<T>('PUT', path, payload, init);
}

export async function apiDelete<T>(
  path: string,
  init?: RequestInit,
): Promise<ApiResponse<T>> {
  return apiBody<T>('DELETE', path, undefined, init);
}

/** Client-side GET that uses the same defensive parsing as POST/PATCH and includes the cookie. */
export async function apiGetCredentialed<T>(
  path: string,
  init?: RequestInit,
): Promise<ApiResponse<T>> {
  try {
    const res = await fetch(`${API_BASE}${path}`, {
      credentials: 'include',
      ...init,
      headers: {
        accept: 'application/json',
        ...(init?.headers ?? {}),
      },
    });
    return parseEnvelope<T>(res);
  } catch {
    return networkFailure<T>();
  }
}

async function apiBody<T>(
  method: 'POST' | 'PATCH' | 'PUT' | 'DELETE',
  path: string,
  payload: unknown,
  init?: RequestInit,
): Promise<ApiResponse<T>> {
  try {
    const res = await fetch(`${API_BASE}${path}`, {
      method,
      credentials: 'include',
      ...init,
      headers: {
        'content-type': 'application/json',
        accept: 'application/json',
        ...(init?.headers ?? {}),
      },
      body: JSON.stringify(payload),
    });
    return parseEnvelope<T>(res);
  } catch {
    return networkFailure<T>();
  }
}

async function parseEnvelope<T>(res: Response): Promise<ApiResponse<T>> {
  // Parse defensively: a framework-level 4xx/5xx (e.g. a 422 from the JSON extractor) may come
  // back as text/plain, not the ApiResponse envelope. Don't let JSON.parse throw -> never report
  // a real HTTP error as a connection failure.
  const text = await res.text();
  try {
    const body = JSON.parse(text) as ApiResponse<T>;
    if (body && typeof body === 'object' && 'success' in body) return body;
  } catch {
    /* not JSON — fall through */
  }
  return {
    success: false,
    data: null,
    error: {
      code: `http_${res.status}`,
      message: res.ok
        ? 'Resposta inesperada do servidor.'
        : 'Não foi possível concluir. Verifique os dados e tente novamente.',
    },
    meta: null,
  };
}

function networkFailure<T>(): ApiResponse<T> {
  return {
    success: false,
    data: null,
    error: {
      code: 'network_error',
      message: 'Falha de conexão. Verifique sua internet e tente novamente.',
    },
    meta: null,
  };
}

/** True when the error envelope indicates an expired / missing session.
 *  Recognizes both the framework 401 code and the plain-text body the gateway
 *  returns when the session cookie is stale ("missing authenticated caller"). */
export function isAuthError<T>(res: ApiResponse<T>): boolean {
  if (!res || res.success) return false;
  const code = res.error?.code;
  return code === 'http_401' || code === 'http_403';
}

/** On a stale-session response: nuke the localStorage cache so the header,
 *  LeftRail and every other island stop believing the user is logged in.
 *  Callers show a "please log in" card in place of an error. */
export function clearLocalSession(): void {
  try {
    for (const k of ['dsoc_citizen', 'dsoc_handle', 'dsoc_name', 'dsoc_avatar']) {
      localStorage.removeItem(k);
    }
  } catch {
    /* storage may be blocked; not fatal */
  }
}

// --- Typed convenience readers used by the SSR pages -------------------------

const orgQuery = (orgId: string, extra = '') =>
  `?org_id=${encodeURIComponent(orgId)}${extra}`;

export const getProposals = (orgId = DEFAULT_ORG_ID, limit = 20) =>
  apiGet<ProposalDto[]>(`/api/v1/proposals${orgQuery(orgId, `&limit=${limit}`)}`);

export const getProposal = (id: string) =>
  apiGet<ProposalDto>(`/api/v1/proposals/${encodeURIComponent(id)}`);

export const getScorecards = (orgId = DEFAULT_ORG_ID, limit = 50) =>
  apiGet<ScorecardDto[]>(`/api/v1/scorecards${orgQuery(orgId, `&limit=${limit}`)}`);

export const getScorecard = (mandateId: string, orgId = DEFAULT_ORG_ID) =>
  apiGet<ScorecardDto>(
    `/api/v1/scorecards/${encodeURIComponent(mandateId)}${orgQuery(orgId)}`,
  );

export const getMandate = (mandateId: string, orgId = DEFAULT_ORG_ID) =>
  apiGet<MandateDto>(
    `/api/v1/mandates/${encodeURIComponent(mandateId)}${orgQuery(orgId)}`,
  );

/** Responsividade pública do mandato (Bloco C): selo/tier + streak + comparativo com pares.
 *  Best-effort no front — a página do político nunca quebra se este endpoint falhar. */
export const getResponsiveness = (mandateId: string) =>
  apiGet<ResponsivenessDto>(
    `/api/v1/politicos/${encodeURIComponent(mandateId)}/responsiveness`,
  );

/** Normalized public activity for a mandate (proxy Câmara/Senado). Always OK with empty
 *  sections when the mandate has no linked house profile or an upstream fails. */
export const getMandateActivity = (mandateId: string, orgId = DEFAULT_ORG_ID) =>
  apiGet<ActivityDto>(
    `/api/v1/mandates/${encodeURIComponent(mandateId)}/atividade${orgQuery(orgId)}`,
  );

/** Directory of mandates in an org — drives the "Propor" form's picker so the user does not have
 *  to type a UUID by hand. Public read. `uf`+`municipio` scope to one municipal câmara
 *  (case-insensitive server-side, migration 0504). */
export const getMandates = (
  orgId = DEFAULT_ORG_ID,
  limit = 50,
  offset = 0,
  sphere?: 'federal' | 'estadual' | 'municipal',
  uf?: string,
  municipio?: string,
) =>
  apiGet<MandateDto[]>(
    `/api/v1/mandates${orgQuery(
      orgId,
      `&limit=${limit}&offset=${offset}${sphere ? `&sphere=${sphere}` : ''}${
        uf ? `&uf=${encodeURIComponent(uf)}` : ''
      }${municipio ? `&municipio=${encodeURIComponent(municipio)}` : ''}`,
    )}`,
  );

/** Vereadores de uma câmara municipal (sphere=municipal, escopo uf+municipio) — drives the
 *  "Vereadores desta Câmara" card on a municipal forum. Best-effort; a câmara caps well under
 *  the server's 100-row page, so a single call suffices. */
export const getCamaraVereadores = (
  uf: string,
  municipio: string,
  orgId = DEFAULT_ORG_ID,
) => getMandates(orgId, 100, 0, 'municipal', uf, municipio);

/** Full mandate directory, walking the server's `offset`/`limit` window (server caps a single
 *  page at 100). Optional `sphere` filter is critical now that we have 70k municipal rows —
 *  callers who want federal+estadual (getStaticPaths, /partidos) must skip municipal.
 *  `hardCap` guards against an unbounded loop if the API ever misbehaves. */
export async function getAllMandates(
  orgId = DEFAULT_ORG_ID,
  hardCap = 5000,
  sphere?: 'federal' | 'estadual' | 'municipal',
): Promise<Fetched<MandateDto[]>> {
  const page = 100;
  const all: MandateDto[] = [];
  for (let offset = 0; offset < hardCap; offset += page) {
    const res = await getMandates(orgId, page, offset, sphere);
    if (!res.ok || !res.data) {
      return all.length ? { ok: true, data: all, error: null } : res;
    }
    all.push(...res.data);
    if (res.data.length < page) break;
  }
  return { ok: true, data: all, error: null };
}

/** Party catalogue (Fase 2B) — parties ordered by mandate_count DESC. */
export const getParties = (orgId = DEFAULT_ORG_ID) =>
  apiGet<PartyDto[]>(`/api/v1/parties${orgQuery(orgId)}`);

/** Single party detail: directories + (privacy-safe) administrators. */
export const getParty = (sigla: string, orgId = DEFAULT_ORG_ID) =>
  apiGet<PartyDetailDto>(
    `/api/v1/parties/${encodeURIComponent(sigla)}${orgQuery(orgId)}`,
  );

/** Membro derivado de um diretório partidário (mandato do partido no território). */
export interface DirectoryMemberDto {
  mandate_id: string;
  display_name: string;
  office: string;
  uf: string | null;
  municipio: string | null;
  avatar_object_key: string | null;
}

/** Membros de um diretório: os mandatos do partido naquele território (0.37.0). */
export const getDirectoryMembers = (
  sigla: string,
  dirId: string,
  orgId = DEFAULT_ORG_ID,
) =>
  apiGet<DirectoryMemberDto[]>(
    `/api/v1/parties/${encodeURIComponent(sigla)}/directories/${dirId}/members${orgQuery(orgId)}`,
  );

/** Campos para criar um diretório partidário. */
export interface CreateDirectoryFields {
  esfera: 'federal' | 'estadual' | 'municipal';
  uf?: string;
  municipio?: string;
  name: string;
  parent_directory_id?: string;
}

/** Cria um diretório do partido (admin de plataforma ou do partido). 0.37.0. */
export const createPartyDirectory = (
  sigla: string,
  fields: CreateDirectoryFields,
  orgId = DEFAULT_ORG_ID,
) =>
  apiPost<{ id: string }>(
    `/api/v1/parties/${encodeURIComponent(sigla)}/directories`,
    { org_id: orgId, ...fields },
  );

/** Remove um diretório partidário (admin). 0.37.0. */
export const deletePartyDirectory = async (
  sigla: string,
  dirId: string,
  orgId = DEFAULT_ORG_ID,
): Promise<ApiResponse<null>> => {
  try {
    const res = await fetch(
      `${API_BASE}/api/v1/parties/${encodeURIComponent(sigla)}/directories/${dirId}${orgQuery(orgId)}`,
      { method: 'DELETE', credentials: 'include', headers: { accept: 'application/json' } },
    );
    return parseEnvelope<null>(res);
  } catch {
    return networkFailure<null>();
  }
};

/** Read the authenticated citizen's own profile (cookie required). */
export const getMyProfile = () => apiGetCredentialed<ProfileDto>('/api/v1/me');

/** Read another citizen's public profile by handle (no auth). Gated by `is_public=true` on
 *  the server; hidden citizens 404 the same as unknown ones. Drives `/perfil/<handle>`. */
export const getPublicProfile = (handle: string, orgId = DEFAULT_ORG_ID) =>
  apiGet<ProfileDto>(
    `/api/v1/profiles/${encodeURIComponent(handle)}${orgQuery(orgId)}`,
  );

/** Reverse lookup: does the authenticated citizen operate a mandate? */
export const getMyMandate = () =>
  apiGetCredentialed<MyMandateDto>('/api/v1/me/mandate');

/** CRM de gabinete (C6): quem procurou o mandato e o que pediu. Gated pelo mesmo
 *  vínculo do painel-mandato; só dado público (autoria de proposta dirigida).
 *  Filtros opcionais por status e tema (recontam no servidor). */
export const getMandateCrm = (opts?: { status?: string; theme?: string }) => {
  const qs = new URLSearchParams();
  if (opts?.status) qs.set('status', opts.status);
  if (opts?.theme) qs.set('theme', opts.theme);
  const suffix = qs.toString() ? `?${qs.toString()}` : '';
  return apiGetCredentialed<MandateCrmDto>(`/api/v1/me/mandate/crm${suffix}`);
};

// --- Mandato coletivo: compromisso consultivo declarado (D8.1) ---------------

/** Compromissos consultivos PÚBLICOS de um mandato (perfil do político). Só dado
 *  público: tema, resultado e o agregado da consulta (nunca voto por-cidadão).
 *  Best-effort no front — a página do político nunca quebra se falhar. */
export const getMandateCommitments = (mandateId: string) =>
  apiGet<MandateCommitmentsDto>(
    `/api/v1/politicos/${encodeURIComponent(mandateId)}/commitments`,
  );

/** Operador declara um compromisso (tema + descrição). Gate: vínculo de mandato. */
export const createCommitment = (theme: string, description: string) =>
  apiPost<{ id: string }>('/api/v1/me/mandate/commitments', {
    theme,
    description,
  });

/** Operador abre uma consulta à base ligada ao compromisso (reusa consultations). */
export const openCommitmentConsultation = (
  commitmentId: string,
  question?: string,
) =>
  apiPost<{ consultation_id: string }>(
    `/api/v1/me/mandate/commitments/${encodeURIComponent(commitmentId)}/consult`,
    { question },
  );

/** Operador registra o resultado: `seguiu` ou `nao_seguiu` (+ nota opcional). */
export const recordCommitmentOutcome = (
  commitmentId: string,
  outcome: 'seguiu' | 'nao_seguiu',
  note?: string,
) =>
  apiPost<{ ok: boolean }>(
    `/api/v1/me/mandate/commitments/${encodeURIComponent(commitmentId)}/outcome`,
    { outcome, note },
  );

/** An official records a public response to an SLA. Identity comes from the cookie. */
export const respondToSla = (slaId: string, body: string, committed: boolean) =>
  apiPost<{ outcome: string }>(
    `/api/v1/consequence/slas/${encodeURIComponent(slaId)}/responses`,
    { body, committed },
  );

/** Patch the authenticated citizen's profile. Returns the refreshed profile. */
export const updateMyProfile = (patch: ProfileUpdateDto) =>
  apiPatch<ProfileDto>('/api/v1/me', patch);

/** List my live sessions; the one carrying THIS request is flagged `current`. */
export const getMySessions = () =>
  apiGetCredentialed<SessionInfoDto[]>('/api/v1/me/sessions');

/** OAuth application that currently holds a live bearer token for the caller. */
export interface AuthorizedAppDto {
  application_id: string;
  name: string;
  website: string | null;
  scopes: string;
  token_count: number;
  first_authorized_at: string;
  last_expires_at: string;
}

/** List apps the caller has authorized via OAuth (Ivory/Elk/Ice Cubes/etc). */
export const getAuthorizedApps = () =>
  apiGetCredentialed<AuthorizedAppDto[]>('/api/v1/me/authorized-apps');

/** Revoke every live token this citizen has issued to `applicationId`. */
export const revokeAuthorizedApp = (applicationId: string) =>
  apiPost<{ revoked: number }>(
    `/api/v1/me/authorized-apps/${encodeURIComponent(applicationId)}/revoke`,
    {},
  );

/** Change the caller's password. Kills every OTHER session and every OAuth token. */
export const changePassword = (currentPassword: string, newPassword: string) =>
  apiPost<{ ok: true }>('/api/v1/me/change-password', {
    current_password: currentPassword,
    new_password: newPassword,
  });

/** Admin dashboard aggregate. */
export interface AdminStatsDto {
  citizens: number;
  actors_local: number;
  actors_remote: number;
  notes_total: number;
  notes_last_7d: number;
  mandates: number;
  proposals: number;
  notifications_unread: number;
}
export const getAdminStats = () =>
  apiGetCredentialed<AdminStatsDto>('/api/v1/admin/stats');

/** Row in the admin user list. */
export interface AdminUserRow {
  citizen_id: string;
  handle: string;
  display_name: string;
  email: string;
  is_public: boolean;
  verification_level: string;
  created_at: string;
  role: string | null;
}
export const getAdminUsers = (
  q: string,
  limit = 25,
  offset = 0,
): Promise<ApiResponse<AdminUserRow[]>> => {
  const qs = new URLSearchParams();
  if (q) qs.set('q', q);
  qs.set('limit', String(limit));
  qs.set('offset', String(offset));
  return apiGetCredentialed<AdminUserRow[]>(`/api/v1/admin/users?${qs}`);
};
export const setAdminUserRole = (
  citizenId: string,
  role: 'owner' | 'admin' | 'auditor' | null,
) =>
  apiPost<{ role: string | null }>(
    `/api/v1/admin/users/${encodeURIComponent(citizenId)}/role`,
    { role },
  );

/** Federation peer summary. */
export interface AdminPeerRow {
  host: string;
  actor_count: number;
  last_seen: string;
}
export const getAdminPeers = (limit = 50) =>
  apiGetCredentialed<AdminPeerRow[]>(`/api/v1/admin/federation/peers?limit=${limit}`);

/** Soft-hide a local note by id (moderation). */
export const hideAdminNote = (noteId: string) =>
  apiPost<{ ok: true }>(
    `/api/v1/admin/notes/${encodeURIComponent(noteId)}/hide`,
    {},
  );

// ---------------------------------------------------------------------------
// Amendments (Decidim gap parity)
// ---------------------------------------------------------------------------

export interface AmendmentDto {
  id: string;
  proposal_id: string;
  author_id: string;
  author_handle: string | null;
  author_display_name: string | null;
  body: string;
  rationale: string | null;
  status: 'draft' | 'open' | 'accepted' | 'rejected' | 'withdrawn';
  support_count: number;
  created_at: string;
  published_at: string | null;
  resolved_at: string | null;
}

export const listAmendments = async (
  proposalId: string,
): Promise<ApiResponse<AmendmentDto[]>> =>
  fetchedToApiResponse(
    await apiGet<AmendmentDto[]>(
      `/api/v1/proposals/${encodeURIComponent(proposalId)}/amendments`,
    ),
  );

export const createAmendment = (
  proposalId: string,
  body: string,
  rationale?: string,
) =>
  apiPost<AmendmentDto>(
    `/api/v1/proposals/${encodeURIComponent(proposalId)}/amendments`,
    { body, rationale },
  );

export const publishAmendment = (amendmentId: string) =>
  apiPost<AmendmentDto>(
    `/api/v1/amendments/${encodeURIComponent(amendmentId)}/publish`,
    {},
  );

export const acceptAmendment = (amendmentId: string) =>
  apiPost<AmendmentDto>(
    `/api/v1/amendments/${encodeURIComponent(amendmentId)}/accept`,
    {},
  );

export const rejectAmendment = (amendmentId: string) =>
  apiPost<AmendmentDto>(
    `/api/v1/amendments/${encodeURIComponent(amendmentId)}/reject`,
    {},
  );

// ---------------------------------------------------------------------------
// Elections + candidacies (Fase 4 do roadmap — 2026)
// ---------------------------------------------------------------------------

export interface ElectionDto {
  id: string;
  year: number;
  round: number;
  sphere: 'federal' | 'estadual' | 'municipal';
  election_day: string;
  registration_deadline: string | null;
  candidacy_count: number;
}
export interface CandidacyDto {
  id: string;
  election_id: string;
  mandate_id: string | null;
  candidate_name: string;
  candidate_gender: 'mulher' | 'homem' | 'nao-binarie' | 'prefiro-nao-dizer' | null;
  party_sigla: string;
  office: string;
  number: string;
  sphere_uf: string | null;
  sphere_municipio: string | null;
  result_rank: number | null;
  status: string | null;
  created_at: string;
}

export const listElections = async (): Promise<ApiResponse<ElectionDto[]>> =>
  fetchedToApiResponse(await apiGet<ElectionDto[]>('/api/v1/elections'));

export const listCandidacies = async (
  electionId: string,
  filters: Partial<{
    uf: string;
    office: string;
    party: string;
    gender: string;
    q: string;
    limit: number;
    offset: number;
  }> = {},
): Promise<ApiResponse<CandidacyDto[]>> => {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(filters)) {
    if (v !== undefined && v !== '') qs.set(k, String(v));
  }
  const qstr = qs.toString();
  return fetchedToApiResponse(
    await apiGet<CandidacyDto[]>(
      `/api/v1/elections/${encodeURIComponent(electionId)}/candidacies${qstr ? '?' + qstr : ''}`,
    ),
  );
};

// ---------------------------------------------------------------------------
// Politicos browser — server-side filters (0.23.0-municipais)
// ---------------------------------------------------------------------------

export interface PoliticoRow {
  id: string;
  display_name: string;
  office: string;
  party: string | null;
  uf: string | null;
  municipio: string | null;
  house: string | null;
  sphere: 'federal' | 'estadual' | 'municipal';
  avatar_url: string | null;
  is_candidate: boolean;
  has_verified_operator: boolean;
}
export interface BrowseResponse {
  total: number;
  limit: number;
  offset: number;
  items: PoliticoRow[];
}

export async function browsePoliticos(
  filters: {
    sphere: 'federal' | 'estadual' | 'municipal';
    uf?: string;
    municipio?: string;
    party?: string;
    house?: string;
    q?: string;
    limit?: number;
    offset?: number;
  },
): Promise<ApiResponse<BrowseResponse>> {
  const p = new URLSearchParams();
  p.set('sphere', filters.sphere);
  if (filters.uf) p.set('uf', filters.uf);
  if (filters.municipio) p.set('municipio', filters.municipio);
  if (filters.party) p.set('party', filters.party);
  if (filters.house) p.set('house', filters.house);
  if (filters.q) p.set('q', filters.q);
  if (filters.limit !== undefined) p.set('limit', String(filters.limit));
  if (filters.offset !== undefined) p.set('offset', String(filters.offset));
  return fetchedToApiResponse(
    await apiGet<BrowseResponse>(`/api/v1/politicos/browse?${p}`),
  );
}

export interface MunicipioRow {
  nome: string;
  count: number;
}
export const listMunicipios = async (
  uf: string,
): Promise<ApiResponse<MunicipioRow[]>> =>
  fetchedToApiResponse(
    await apiGet<MunicipioRow[]>(
      `/api/v1/politicos/municipios?uf=${encodeURIComponent(uf)}`,
    ),
  );

/** Município IBGE (código + nome) — referência completa pro selector de domicílio. */
export interface MunicipioIbge {
  codigo_ibge: number;
  nome: string;
}
/**
 * Lista TODOS os municípios de uma UF (referência IBGE, migration 0651) — usada
 * no selector UF→município do cadastro. Diferente de `listMunicipios`, que só
 * traz municípios COM mandato indexado (derivado de `politicos`).
 */
export const getMunicipios = async (
  uf: string,
): Promise<ApiResponse<MunicipioIbge[]>> =>
  fetchedToApiResponse(
    await apiGet<MunicipioIbge[]>(
      `/api/v1/municipios?uf=${encodeURIComponent(uf)}`,
    ),
  );

/** Resumo territorial de um município (Fase 2.2): eleitorado + mandatos por partido. */
export interface TerritorioResponse {
  uf: string;
  municipio: string;
  voters: number | null;
  total: number;
  by_party: Array<{ party: string; count: number }>;
}
export const getTerritorio = async (
  uf: string,
  municipio: string,
): Promise<ApiResponse<TerritorioResponse>> =>
  fetchedToApiResponse(
    await apiGet<TerritorioResponse>(
      `/api/v1/politicos/territorio?uf=${encodeURIComponent(uf)}&municipio=${encodeURIComponent(municipio)}`,
    ),
  );

/** Revoke one of my sessions. Cannot revoke the current one (use logout). */
export async function revokeSession(id: string): Promise<ApiResponse<null>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/me/sessions/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    if (res.status === 204) {
      return { success: true, data: null, error: null, meta: null };
    }
    const text = await res.text();
    try {
      const body = JSON.parse(text) as ApiResponse<null>;
      if (body && typeof body === 'object' && 'success' in body) return body;
    } catch {
      /* not JSON */
    }
    return {
      success: false,
      data: null,
      error: {
        code: `http_${res.status}`,
        message: 'Não foi possível encerrar a sessão.',
      },
      meta: null,
    };
  } catch {
    return {
      success: false,
      data: null,
      error: {
        code: 'network_error',
        message: 'Falha de conexão. Verifique sua internet e tente novamente.',
      },
      meta: null,
    };
  }
}

/** Multipart upload of avatar / cover. The backend ignores the multipart Content-Type and
 *  validates from bytes — so any PNG/JPEG/WebP is accepted; an invalid file gets a 400 with a
 *  Portuguese message. */
export async function uploadProfileImage(
  kind: 'avatar' | 'cover',
  file: File,
): Promise<ApiResponse<ProfileDto>> {
  try {
    const fd = new FormData();
    fd.append('file', file);
    const res = await fetch(`${API_BASE}/api/v1/me/${kind}`, {
      method: 'POST',
      credentials: 'include',
      body: fd,
    });
    const text = await res.text();
    try {
      const body = JSON.parse(text) as ApiResponse<ProfileDto>;
      if (body && typeof body === 'object' && 'success' in body) return body;
    } catch {
      /* not JSON — fall through */
    }
    return {
      success: false,
      data: null,
      error: {
        code: `http_${res.status}`,
        message: res.ok
          ? 'Resposta inesperada do servidor.'
          : 'Não foi possível enviar a imagem.',
      },
      meta: null,
    };
  } catch {
    return {
      success: false,
      data: null,
      error: {
        code: 'network_error',
        message: 'Falha de conexão. Verifique sua internet e tente novamente.',
      },
      meta: null,
    };
  }
}

export const getPromises = (mandateId: string) =>
  apiGet<PromiseDto[]>(
    `/api/v1/scorecards/${encodeURIComponent(mandateId)}/promises`,
  );

// --- Convite pra completar o perfil (0.49.0, admin) ---------------------------

export interface ProfileNudgeOverview {
  total: number;
  incomplete: number;
  incomplete_not_nudged: number;
}
export interface ProfileNudgeCandidate {
  citizen_id: string;
  display_name: string | null;
  handle: string | null;
  email: string;
  created_at: string;
  nudged_at: string | null;
}
export interface ProfileNudgeResult {
  sent: number;
  skipped: number;
  failed: number;
}

export const getProfileNudgeOverview = () =>
  apiGetCredentialed<ProfileNudgeOverview>('/api/v1/admin/profile-nudge/overview');

export const getProfileNudgeCandidates = (limit = 500) =>
  apiGetCredentialed<ProfileNudgeCandidate[]>(
    `/api/v1/admin/profile-nudge/candidates?limit=${limit}`,
  );

export const sendProfileNudge = (citizenIds: string[]) =>
  apiPost<ProfileNudgeResult>('/api/v1/admin/profile-nudge/send', {
    citizen_ids: citizenIds,
  });

// --- Contatos dos políticos (0.51.0, admin) -----------------------------------

export interface PoliticoContactOverviewRow {
  cargo: string;
  total: number;
  com_email: number;
  placeholder: number;
}
export interface PoliticoContact {
  id: string;
  display_name: string;
  office: string;
  party: string | null;
  uf: string | null;
  municipio: string | null;
  public_email: string;
  email_real: boolean;
}
export interface PoliticoContactsResult {
  total: number;
  limit: number;
  offset: number;
  items: PoliticoContact[];
}

export const getPoliticoContactsOverview = () =>
  apiGetCredentialed<PoliticoContactOverviewRow[]>('/api/v1/admin/politico-contacts/overview');

export const getPoliticoContacts = (params: {
  cargo?: string;
  uf?: string;
  status?: string;
  q?: string;
  limit?: number;
  offset?: number;
}) => {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== '' && v !== null) qs.set(k, String(v));
  }
  return apiGetCredentialed<PoliticoContactsResult>(
    `/api/v1/admin/politico-contacts?${qs.toString()}`,
  );
};

// ÁGORA #72 (ADR-0017) — mapa município→plataforma (civic_source).
export interface CivicSourceOverviewRow {
  platform: string;
  probe_status: string;
  total: number;
  parlamentares: number | null;
}
export interface CivicSource {
  id: string;
  uf: string;
  municipio: string;
  platform: string;
  base_url: string | null;
  probe_status: string;
  parlamentares_found: number | null;
  last_probed_at: string | null;
  last_extracted_at: string | null;
}
export interface CivicSourcesResult {
  total: number;
  limit: number;
  offset: number;
  items: CivicSource[];
}
// Completar cadastro obrigatório (0664) — usuários antigos sem nome/sexo/nascimento.
export interface ProfileStatus {
  complete: boolean;
  missing: string[];
  auto_handle: boolean;
}
export const getProfileStatus = () =>
  apiGetCredentialed<ProfileStatus>('/api/v1/me/profile-status');
export const completeProfile = (body: {
  nome: string;
  sexo: string;
  nascimento: string;
  handle?: string;
}) => apiPost<{ complete: boolean }>('/api/v1/me/complete-profile', body);

// Gestão admin de consultas (consultations_consultation) — listar/detalhar/fechar.
export interface AdminConsultation {
  id: string;
  title: string;
  status: string;
  opens_at: string;
  closes_at: string;
  created_at: string;
  question_count: number;
  response_count: number;
}
export interface AdminConsultationsResult {
  total: number;
  limit: number;
  offset: number;
  items: AdminConsultation[];
}
export interface AdminConsultationQuestion {
  id: string;
  prompt: string;
  position: number;
  concordo: number;
  neutro: number;
  discordo: number;
  total: number;
}
export interface AdminConsultationDetail {
  id: string;
  title: string;
  status: string;
  opens_at: string;
  closes_at: string;
  created_at: string;
  questions: AdminConsultationQuestion[];
}

export const getAdminConsultations = (params: {
  status?: string;
  q?: string;
  limit?: number;
  offset?: number;
}) => {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== '' && v !== null) qs.set(k, String(v));
  }
  return apiGetCredentialed<AdminConsultationsResult>(
    `/api/v1/admin/consultations?${qs.toString()}`,
  );
};
export const getAdminConsultation = (id: string) =>
  apiGetCredentialed<AdminConsultationDetail>(
    `/api/v1/admin/consultations/${encodeURIComponent(id)}`,
  );
export const closeAdminConsultation = (id: string) =>
  apiPost<{ id: string; status: string }>(
    `/api/v1/admin/consultations/${encodeURIComponent(id)}/close`,
    {},
  );

export const getCivicSourcesOverview = () =>
  apiGetCredentialed<CivicSourceOverviewRow[]>('/api/v1/admin/civic-sources/overview');
export const getCivicSources = (params: {
  uf?: string;
  platform?: string;
  status?: string;
  q?: string;
  limit?: number;
  offset?: number;
}) => {
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== '' && v !== null) qs.set(k, String(v));
  }
  return apiGetCredentialed<CivicSourcesResult>(`/api/v1/admin/civic-sources?${qs.toString()}`);
};

/** O parlamentar registra uma promessa pública (gate MIN_OFFICIAL_LEVEL no backend). */
export const recordPromise = (mandateId: string, text: string) =>
  apiPost<PromiseDto>(
    `/api/v1/scorecards/${encodeURIComponent(mandateId)}/promises`,
    { text },
  );

/** O parlamentar marca uma promessa como cumprida. */
export const deliverPromise = (promiseId: string) =>
  apiPost<PromiseDto>(
    `/api/v1/scorecards/promises/${encodeURIComponent(promiseId)}/deliver`,
    {},
  );

export const getSlas = (orgId = DEFAULT_ORG_ID, limit = 50) =>
  apiGet<SlaDto[]>(`/api/v1/consequence/slas${orgQuery(orgId, `&limit=${limit}`)}`);

export const getSla = (id: string) =>
  apiGet<SlaDto>(`/api/v1/consequence/slas/${encodeURIComponent(id)}`);

export const getConsultations = (orgId = DEFAULT_ORG_ID, limit = 30) =>
  apiGet<ConsultationDto[]>(
    `/api/v1/surveys${orgQuery(orgId, `&limit=${limit}`)}`,
  );

// --- Consultas participativas (Fase 3.3, migration 0531) ----------------------
// Superfície pública: leitura sem login, resposta com login (concordo/neutro/discordo).

/** Lista pública de consultas (título, status, janela, nº perguntas). */
export const getConsultas = () => apiGet<ConsultaSummary[]>('/api/v1/consultas');

/** Detalhe público de uma consulta. Credenciado: quando logado traz `my_answer`. */
export const getConsulta = (id: string) =>
  apiGetCredentialed<ConsultaDetail>(
    `/api/v1/consultas/${encodeURIComponent(id)}`,
  );

/** O cidadão logado envia/atualiza respostas (concordo/neutro/discordo). */
export const responderConsulta = (
  id: string,
  answers: { question_id: string; answer: string }[],
) =>
  apiPost<{ saved: number }>(
    `/api/v1/consultas/${encodeURIComponent(id)}/responder`,
    { answers },
  );

/** Cria uma consulta (admin ou político). Perguntas são prompts livres. */
export const createConsulta = (input: {
  title: string;
  opens_at: string;
  closes_at: string;
  questions: string[];
}) => apiPost<{ id: string }>('/api/v1/consultas', input);

/** Encerra uma consulta aberta (admin ou político). */
export const closeConsulta = (id: string) =>
  apiPost<{ status: string }>(
    `/api/v1/consultas/${encodeURIComponent(id)}/close`,
    {},
  );

// --- Auth: centralized so the org_id (required by the backend Register/LoginRequest) can NEVER be
//     forgotten by a form. A contract test (web/tests/api.contract.test.ts) guards these shapes. ---

/** Session returned by /auth/login e /auth/register/confirm. */
export interface SessionData {
  id: string;
  citizen_id: string;
  issued_at: string;
  expires_at: string;
  public_handle: string;
}

/**
 * 0.25.0-fediverso: o cadastro passou a exigir verificação de e-mail. O
 * request `POST /auth/register` responde 202 `{status:"verification_sent",email}`
 * e dispara um e-mail com link `/confirmar-conta?token=…`. Só depois do clique
 * a conta é materializada (via `registerConfirm`) e a sessão é emitida.
 */
export interface SignupPendingData {
  status: 'verification_sent';
  email: string;
}

/** Dados de identidade pro cadastro de cidadão (confrontados com a base autorizada). */
export interface SignupIdentity {
  nome_completo?: string;
  /** `YYYY-MM-DD` */
  nascimento?: string;
  /** `M` | `F` */
  sexo?: string;
  /** Título de eleitor (opcional; sem ele = sem poder de voto). */
  titulo_eleitor?: string;
  /** UF de domicílio (sigla 2 letras). Obrigatória pro cidadão. */
  uf?: string;
  /** Município de domicílio (código IBGE). Obrigatório pro cidadão. */
  municipio_ibge?: number;
  /** Nick do fediverso (handle escolhido). Obrigatório pro cidadão. */
  handle?: string;
}

/** Inicia o cadastro de cidadão. Não emite sessão — dispara link por e-mail. */
export const register = (
  email: string,
  password: string,
  cpf: string,
  identity: SignupIdentity = {},
  orgId = DEFAULT_ORG_ID,
) =>
  apiPost<SignupPendingData>('/api/v1/auth/register', {
    org_id: orgId,
    email: email.trim(),
    password,
    cpf,
    nome_completo: identity.nome_completo,
    nascimento: identity.nascimento,
    sexo: identity.sexo,
    titulo_eleitor: identity.titulo_eleitor,
    uf: identity.uf,
    municipio_ibge: identity.municipio_ibge,
    handle: identity.handle,
  });

/**
 * Inicia o cadastro de político(a)/candidata(o) (F1.3/F1.4). Exige que
 * `email` bata com `mandate.public_email` (case-insensitive) — checado no
 * back antes de gravar o pending. Também não emite sessão; o e-mail
 * carrega o link de confirmação.
 */
export const registerPolitician = (
  email: string,
  password: string,
  cpf: string,
  mandateId: string,
  orgId = DEFAULT_ORG_ID,
) =>
  apiPost<SignupPendingData>('/api/v1/auth/register/politician', {
    org_id: orgId,
    email: email.trim(),
    password,
    cpf,
    mandate_id: mandateId,
  });

/** Campos da candidatura auto-declarada (cadastro de candidato sem mandato). */
export interface CandidateSignupFields {
  display_name: string;
  office: string;
  uf?: string;
  municipio?: string;
  party_sigla: string;
  number?: string;
}

/**
 * Inicia o cadastro de CANDIDATO(A) SEM MANDATO (0.36.0). A candidatura é
 * auto-declarada: o confirm cria mandato `source='self'` + vínculo nível
 * `email` (ferramentas destravam já, com selo "não verificada") e a
 * candidatura fica fora do comparador até verificação. Também 202 + e-mail.
 */
export const registerCandidate = (
  email: string,
  password: string,
  cpf: string,
  fields: CandidateSignupFields,
  orgId = DEFAULT_ORG_ID,
) =>
  apiPost<SignupPendingData>('/api/v1/auth/register/candidate', {
    org_id: orgId,
    email: email.trim(),
    password,
    cpf,
    ...fields,
  });

/**
 * Redime o token do e-mail de verificação e finaliza o cadastro. Emite a
 * sessão como se fosse um login — o front seta o cookie e redireciona.
 */
export const registerConfirm = (token: string) =>
  apiPost<SessionData>('/api/v1/auth/register/confirm', { token });

/**
 * Reenvia o link de verificação para uma pending viva. Sempre resolve
 * `success: true` no wire (enumeration-safe, mesmo padrão do password-reset).
 */
export const registerResend = (email: string, orgId = DEFAULT_ORG_ID) =>
  apiPost<null>('/api/v1/auth/register/resend', {
    org_id: orgId,
    email: email.trim(),
  });

/**
 * Status do título de eleitor da cidadã(o) logada. NULL quando não cadastrado.
 * `titulo_last4`: só os 4 últimos dígitos (LGPD-safe). `titulo_status`:
 *   `unverified` (submetido, sem cross-check ainda),
 *   `validated` (dígitos verificadores TSE OK),
 *   `verified` (cross-check com fonte oficial).
 */
export interface TituloEleitorStatus {
  titulo_last4: string | null;
  titulo_status: 'unverified' | 'validated' | 'verified' | null;
  /** Zona eleitoral declarada (até 4 dígitos) — auxiliar, não valida o título. */
  titulo_zona: string | null;
  /** Seção eleitoral declarada (até 4 dígitos) — auxiliar, não valida o título. */
  titulo_secao: string | null;
}

/** GET /me/titulo-eleitor — status da cidadania política. */
export const getTituloEleitor = () =>
  apiGetCredentialed<TituloEleitorStatus>('/api/v1/me/titulo-eleitor');

/** GET público — chave VAPID pra criar subscription no navegador. */
export const getVapidPublicKey = () =>
  apiGetCredentialed<{ public_key: string }>(
    '/api/v1/me/push-subscriptions/vapid-public-key',
  );

/** POST — envia subscription do PushManager pro back persistir. */
export const subscribeWebPush = (subscription: PushSubscriptionJSON, userAgent: string) =>
  apiPost<null>('/api/v1/me/push-subscriptions', {
    endpoint: subscription.endpoint,
    keys: subscription.keys,
    user_agent: userAgent,
  });

/** GET /me/admin-status — leve, usado pelo AuthMenu pra mostrar "Administração". */
export const getMyAdminStatus = () =>
  apiGetCredentialed<{ is_admin: boolean }>('/api/v1/me/admin-status');

/** LGPD art. 18 — direitos do titular. */
export const exportMyData = () =>
  apiGetCredentialed<Record<string, unknown>>('/api/v1/me/lgpd/export');

export const deleteMyAccount = () =>
  apiPost<null>('/api/v1/me/lgpd/delete-account', {});

/** Estatísticas públicas — usadas na landing pra ancorar a tese. */
export interface PublicStats {
  citizens_active: number;
  proposals_published: number;
  mandates_indexed: number;
  responses_public: number;
  silences_public: number;
  response_rate: number | null;
  generated_at: string;
}
export const getPublicStats = () =>
  apiGet<PublicStats>('/api/v1/stats/public');

/** GET /auth/govbr/status — front usa pra decidir se mostra o botão. */
export const getGovbrStatus = () =>
  apiGet<{ enabled: boolean }>('/api/v1/auth/govbr/status');

/** GUI completa de usuários — admin only. */
export interface AdminUserRow {
  citizen_id: string;
  display_name: string | null;
  handle: string | null;
  email: string | null;
  verification_level: string;
  is_public: boolean;
  titulo_status: string | null;
  cpf_status: string | null;
  legal_name: string | null;
  gender: string | null;
  birth_date: string | null;
  uf: string | null;
  municipio: string | null;
  cpf_masked: string | null;
  party_sigla: string | null;
  created_at: string;
  platform_role: 'owner' | 'admin' | 'auditor' | null;
  party_admin_sigla: string | null;
  party_admin_role: 'admin' | 'moderador' | null;
  has_mandate: boolean;
  has_candidacy: boolean;
  suspended_at: string | null;
  silenced_at: string | null;
}
export interface AdminUsersFilter {
  q?: string;
  party?: string;
  platform_role?: 'owner' | 'admin' | 'auditor' | 'none' | 'any';
  party_role?: 'admin' | 'moderador' | 'none' | 'any';
  civic_type?: 'cidadao' | 'politico' | 'candidato' | 'any';
  limit?: number;
  offset?: number;
}
export const listAdminUsers = (f: AdminUsersFilter = {}) => {
  const qs = new URLSearchParams();
  if (f.q) qs.set('q', f.q);
  if (f.party) qs.set('party', f.party);
  if (f.platform_role) qs.set('platform_role', f.platform_role);
  if (f.party_role) qs.set('party_role', f.party_role);
  if (f.civic_type) qs.set('civic_type', f.civic_type);
  if (f.limit != null) qs.set('limit', String(f.limit));
  if (f.offset != null) qs.set('offset', String(f.offset));
  return apiGetCredentialed<AdminUserRow[]>(
    `/api/v1/admin/users-rich${qs.toString() ? `?${qs.toString()}` : ''}`,
  );
};

export const patchAdminUser = (
  citizen_id: string,
  patch: {
    party_sigla?: string | null;
    verification_level?: string;
    is_public?: boolean;
  },
) =>
  fetch(`${API_BASE}/api/v1/admin/users/${citizen_id}`, {
    method: 'PATCH',
    credentials: 'include',
    headers: { 'content-type': 'application/json' },
    // party_sigla: null envia JSON null (limpa); undefined omite.
    body: JSON.stringify(patch),
  }).then((r) => r.json());

export const setPlatformRole = (
  citizen_id: string,
  role: 'owner' | 'admin' | 'auditor' | 'none',
) =>
  apiPut<null>(`/api/v1/admin/users/${citizen_id}/platform-role`, { role });

export const setPartyRole = (
  citizen_id: string,
  role: 'admin' | 'moderador' | 'none',
  party_sigla?: string,
) =>
  apiPut<null>(`/api/v1/admin/users/${citizen_id}/party-role`, {
    role,
    party_sigla: role === 'none' ? undefined : party_sigla,
  });

/** Template de e-mail editável pela UI admin (migration 0151). */
export interface EmailTemplateDto {
  key: string;
  label: string;
  subject: string;
  body: string;
  default_subject: string;
  default_body: string;
  variables: string[];
  updated_at: string;
}

export const listEmailTemplates = () =>
  apiGetCredentialed<EmailTemplateDto[]>('/api/v1/admin/email-templates');

export const updateEmailTemplate = (
  key: string,
  patch: { subject?: string; body?: string; reset?: boolean },
) =>
  fetch(`${API_BASE}/api/v1/admin/email-templates/${encodeURIComponent(key)}`, {
    method: 'PATCH',
    credentials: 'include',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(patch),
  }).then((r) => r.json());

export const previewEmailTemplate = (
  key: string,
  payload: {
    context: Record<string, string>;
    subject?: string;
    body?: string;
  },
) =>
  apiPost<{ subject: string; body: string }>(
    `/api/v1/admin/email-templates/${encodeURIComponent(key)}/preview`,
    payload,
  );

/** Envia o template SALVO pro e-mail informado (caminho real: SMTP +
 *  wrapper HTML da marca). Subject chega prefixado com [TESTE]. */
export const sendTestEmailTemplate = (
  key: string,
  payload: { to: string; context: Record<string, string> },
) =>
  apiPost<null>(
    `/api/v1/admin/email-templates/${encodeURIComponent(key)}/send-test`,
    payload,
  );

// ---------------------------------------------------------------------------
// Base de contatos / audiência (0.35.0).
// ---------------------------------------------------------------------------

/** Captação pública ("receba novidades"): consent LGPD; `website` é honeypot. */
export const subscribeAudience = (payload: {
  email: string;
  name?: string;
  uf?: string;
  website?: string;
}) => apiPost<null>('/api/v1/audience/subscribe', { website: '', ...payload });

export interface AudienceStatsDto {
  total: number;
  active: number;
  unsubscribed: number;
  from_site: number;
  imported: number;
  segments: { segment: string; active: number }[];
}

export interface AudienceContactDto {
  id: string;
  email: string;
  name: string | null;
  uf: string | null;
  segment: string;
  source: string;
  legal_basis: string;
  unsubscribed: boolean;
  created_at: string;
}

export const getAudienceStats = () =>
  apiGetCredentialed<AudienceStatsDto>('/api/v1/admin/audience/stats');

export const listAudience = (opts: {
  segment?: string;
  status?: 'active' | 'unsubscribed' | 'all';
  q?: string;
  limit?: number;
  offset?: number;
}) => {
  const p = new URLSearchParams();
  if (opts.segment) p.set('segment', opts.segment);
  if (opts.status) p.set('status', opts.status);
  if (opts.q) p.set('q', opts.q);
  if (opts.limit) p.set('limit', String(opts.limit));
  if (opts.offset) p.set('offset', String(opts.offset));
  const s = p.toString();
  return apiGetCredentialed<AudienceContactDto[]>(
    `/api/v1/admin/audience${s ? `?${s}` : ''}`,
  );
};

export const importAudience = (payload: {
  source_slug: string;
  legal_basis: 'consent' | 'legitimate_interest';
  segment?: string;
  notes?: string;
  contacts: { email: string; name?: string; uf?: string }[];
}) => apiPost<{ received: number; upserted: number; invalid: number }>(
  '/api/v1/admin/audience/import',
  payload,
);

export const deleteAudienceContact = (id: string) =>
  fetch(`${API_BASE}/api/v1/admin/audience/${id}`, {
    method: 'DELETE',
    credentials: 'include',
  }).then((r) => r.json());

export const AUDIENCE_EXPORT_URL = `${API_BASE}/api/v1/admin/audience/export.csv`;

// ---------------------------------------------------------------------------
// Campanha de convites aos gabinetes (0.34.0) — admin only.
// ---------------------------------------------------------------------------

export interface InviteCampaignFilter {
  sphere?: string;
  house?: string;
  uf?: string;
  party?: string;
}

export interface InviteCampaignOverviewDto {
  total: number;
  with_email: number;
  bound: number;
  invite_pending: number;
  invite_accepted: number;
  invite_expired: number;
  eligible_now: number;
}

export interface InviteBatchItemDto {
  mandate_id: string;
  display_name: string;
  email: string;
  ok: boolean;
  error: string | null;
}

export interface InviteBatchResultDto {
  attempted: number;
  sent: number;
  failed: number;
  items: InviteBatchItemDto[];
}

export interface InviteCampaignRowDto {
  invite_id: string;
  mandate_id: string;
  display_name: string;
  office: string;
  party: string | null;
  uf: string | null;
  email: string;
  sent_at: string;
  expires_at: string;
  accepted_at: string | null;
}

const inviteCampaignQs = (f: InviteCampaignFilter) => {
  const p = new URLSearchParams();
  if (f.sphere) p.set('sphere', f.sphere);
  if (f.house) p.set('house', f.house);
  if (f.uf) p.set('uf', f.uf);
  if (f.party) p.set('party', f.party);
  const s = p.toString();
  return s ? `?${s}` : '';
};

export const getInviteCampaignOverview = (filter: InviteCampaignFilter = {}) =>
  apiGetCredentialed<InviteCampaignOverviewDto>(
    `/api/v1/admin/invite-campaign/overview${inviteCampaignQs(filter)}`,
  );

export const sendInviteCampaignBatch = (
  payload: InviteCampaignFilter & { limit?: number },
) =>
  apiPost<InviteBatchResultDto>(
    '/api/v1/admin/invite-campaign/send-batch',
    payload,
  );

export const listInviteCampaign = (
  status: 'pending' | 'accepted' | 'expired',
  limit = 200,
) =>
  apiGetCredentialed<InviteCampaignRowDto[]>(
    `/api/v1/admin/invite-campaign/invites?status=${status}&limit=${limit}`,
  );

/** POST /me/titulo-eleitor — valida algoritmicamente (12 dígitos) e persiste.
 *  `titulo` vazio + título já vinculado → atualiza só zona/seção. */
export const submitTituloEleitor = (
  titulo: string,
  zona?: string,
  secao?: string,
) =>
  apiPost<{
    titulo_status: string | null;
    titulo_last4: string | null;
    titulo_zona: string | null;
    titulo_secao: string | null;
  }>('/api/v1/me/titulo-eleitor', { titulo, zona, secao });

/** Authenticate (e-mail + senha). Always includes org_id. */
export const login = (email: string, password: string, orgId = DEFAULT_ORG_ID) =>
  apiPost<SessionData>('/api/v1/auth/login', {
    org_id: orgId,
    email: email.trim(),
    password,
  });

/**
 * F1.4 bypass — admin dispatches a mandate invite to any e-mail, independent of the
 * `mandate.public_email` on file. Caller must be a platform admin or an admin of the
 * mandate's party (403 otherwise). Response NEVER carries the token (only id + expiry).
 */
export const sendMandateInvite = (mandateId: string, email: string) =>
  apiPost<{ id: string; expires_at: string }>(
    `/api/v1/mandates/${encodeURIComponent(mandateId)}/invites`,
    { email: email.trim() },
  );

/** Public accept-page summary (nothing personal). 404 collapses expired/revoked/unknown. */
export const getMandateInvite = (token: string) =>
  apiGet<MandateInviteSummaryDto>(
    `/api/v1/mandate-invites/${encodeURIComponent(token)}`,
  );

/** Accept the invite: creates citizen + credential + directory binding + session. */
export const acceptMandateInvite = (
  token: string,
  body: { password: string; cpf: string; display_name: string },
) =>
  apiPost<SessionData>(
    `/api/v1/mandate-invites/${encodeURIComponent(token)}/accept`,
    body,
  );

/** Optional — inviter revokes a pending invite. */
export const revokeMandateInvite = (token: string) =>
  apiPost<null>(
    `/api/v1/mandate-invites/${encodeURIComponent(token)}/revoke`,
    {},
  );

/** Request a password reset link (enumeration-resistant: always returns success). */
export const requestPasswordReset = (email: string, orgId = DEFAULT_ORG_ID) =>
  apiPost<null>('/api/v1/auth/password-reset/request', {
    org_id: orgId,
    email: email.trim(),
  });

/** Redeem a reset token and set a new password. */
export const confirmPasswordReset = (token: string, password: string) =>
  apiPost<null>('/api/v1/auth/password-reset/confirm', { token, password });

/** Resolved remote ActivityPub actor (returned by `/federation/lookup`). */
export interface RemoteActorDto {
  remote_actor_url: string;
  inbox_url: string;
  handle: string;
  name: string | null;
  preferred_username: string | null;
  summary: string | null;
  avatar_url: string | null;
}

/** Look up a fediverse account by `@user@host`. Auth required (citizen cookie). */
export const lookupRemoteActor = (acct: string) =>
  apiGetCredentialed<RemoteActorDto>(
    `/api/v1/federation/lookup?acct=${encodeURIComponent(acct)}`,
  );

/** Send a Follow to a remote actor (uses the URL from `lookupRemoteActor`). */
export const followRemoteActor = (remote_actor_url: string) =>
  apiPost<{ status: string }>('/api/v1/me/follow', { remote_actor_url });

/** A note surfaced by the remote outbox proxy — the shape mirrors the Rust `RemoteNoteDto`. */
export interface RemoteNoteDto {
  id: string;
  url: string | null;
  content_html: string;
  published_at: string | null;
  in_reply_to: string | null;
}

/** Proxy the last ~20 notes from a remote actor's outbox, rendered inside our profile page. */
export const getRemoteActorOutbox = (actor_url: string) =>
  apiGetCredentialed<RemoteNoteDto[]>(
    `/api/v1/federation/actor-outbox?actor_url=${encodeURIComponent(actor_url)}`,
  );

/** True if the current citizen already follows the given remote actor. */
export const getFollowStatus = (actor_url: string) =>
  apiGetCredentialed<{ following: boolean; pending: boolean }>(
    `/api/v1/me/follow/status?actor_url=${encodeURIComponent(actor_url)}`,
  );

/** Bookmark helpers keyed by raw object_uri — covers notes from remote outbox. */
export const bookmarkUri = (object_uri: string) =>
  apiPost<{ ok: true }>('/api/v1/me/bookmarks', { object_uri });

export async function unbookmarkUri(
  object_uri: string,
): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/me/bookmarks`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { 'content-type': 'application/json', accept: 'application/json' },
      body: JSON.stringify({ object_uri }),
    });
    const parsed = (await res.json()) as ApiResponse<{ ok: true }>;
    return parsed;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

export const getBookmarkStatus = (object_uri: string) =>
  apiGetCredentialed<{ bookmarked: boolean }>(
    `/api/v1/me/bookmarks/status?object_uri=${encodeURIComponent(object_uri)}`,
  );

/** Mute/Block por URL do actor (funciona pra locais e remotos). */
export const muteActorUrl = (actor_url: string) =>
  apiPost<{ ok: true }>('/api/v1/me/mutes/url', { actor_url });
export async function unmuteActorUrl(actor_url: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/me/mutes/url`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { 'content-type': 'application/json', accept: 'application/json' },
      body: JSON.stringify({ actor_url }),
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}
export const getMuteUrlStatus = (actor_url: string) =>
  apiGetCredentialed<{ muted: boolean }>(
    `/api/v1/me/mutes/url/status?actor_url=${encodeURIComponent(actor_url)}`,
  );

export const blockActorUrl = (actor_url: string) =>
  apiPost<{ ok: true }>('/api/v1/me/blocks/url', { actor_url });
export async function unblockActorUrl(actor_url: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/me/blocks/url`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { 'content-type': 'application/json', accept: 'application/json' },
      body: JSON.stringify({ actor_url }),
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}
export const getBlockUrlStatus = (actor_url: string) =>
  apiGetCredentialed<{ blocked: boolean }>(
    `/api/v1/me/blocks/url/status?actor_url=${encodeURIComponent(actor_url)}`,
  );

/** Categorias: spam | violation | other (vocabulário Mastodon). */
export const reportNote = (body: {
  object_uri: string;
  author_actor_url: string;
  category: 'spam' | 'violation' | 'other';
  reason?: string;
}) => apiPost<{ ok: true }>('/api/v1/me/reports', body);

export const blockDomain = (domain: string) =>
  apiPost<{ ok: true; domain: string }>('/api/v1/me/domain_blocks', { domain });
export async function unblockDomain(domain: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/me/domain_blocks`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { 'content-type': 'application/json', accept: 'application/json' },
      body: JSON.stringify({ domain }),
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}
export const getDomainBlockStatus = (domain: string) =>
  apiGetCredentialed<{ blocked: boolean; domain: string }>(
    `/api/v1/me/domain_blocks/status?domain=${encodeURIComponent(domain)}`,
  );

/** Admin: fila de denúncias. */
export interface AdminReportDto {
  id: string;
  object_uri: string;
  author_actor_url: string;
  category: 'spam' | 'violation' | 'other';
  reason: string | null;
  created_at: string;
  resolved_at: string | null;
  resolution_notes: string | null;
  reporter_handle: string | null;
  reporter_display_name: string | null;
  total_for_note: number;
}

export const adminListReports = (
  status: 'pending' | 'resolved' | 'all' = 'pending',
  limit = 30,
  offset = 0,
) =>
  apiGetCredentialed<AdminReportDto[]>(
    `/api/v1/admin/reports?status=${status}&limit=${limit}&offset=${offset}`,
  );

export const adminResolveReport = (id: string, notes?: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/reports/${encodeURIComponent(id)}/resolve`, {
    notes,
  });

export const adminReopenReport = (id: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/reports/${encodeURIComponent(id)}/reopen`, {});

/** Admin: ações em contas (suspender/silenciar). */
export const adminSuspendAccount = (id: string, reason?: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/accounts/${encodeURIComponent(id)}/suspend`, { reason });
export const adminUnsuspendAccount = (id: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/accounts/${encodeURIComponent(id)}/unsuspend`, {});
export const adminSilenceAccount = (id: string, reason?: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/accounts/${encodeURIComponent(id)}/silence`, { reason });
export const adminUnsilenceAccount = (id: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/accounts/${encodeURIComponent(id)}/unsilence`, {});

/** Admin: audit log. */
export interface AdminAuditRowDto {
  id: string;
  admin_id: string;
  admin_handle: string | null;
  action: string;
  target_citizen_id: string | null;
  target_citizen_handle: string | null;
  target_domain: string | null;
  target_id: string | null;
  detail: unknown;
  created_at: string;
}
export const adminListAudit = (limit = 100, offset = 0) =>
  apiGetCredentialed<AdminAuditRowDto[]>(
    `/api/v1/admin/audit?limit=${limit}&offset=${offset}`,
  );

/** Convites (0.26.15). */
export interface InvitationDto {
  id: string;
  token: string;
  target_email: string | null;
  notes: string | null;
  uses_left: number;
  max_uses: number;
  created_at: string;
  expires_at: string | null;
  first_used_at: string | null;
  last_used_at: string | null;
}
export const listMyInvitations = () =>
  apiGetCredentialed<InvitationDto[]>('/api/v1/invitations');
export const createInvitation = (body: {
  target_email?: string;
  notes?: string;
  max_uses?: number;
  expires_in_hours?: number;
}) => apiPost<InvitationDto>('/api/v1/invitations', body);
export async function deleteInvitation(id: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/invitations/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

export interface InvitationPreviewDto {
  valid: boolean;
  reason: string | null;
  invited_by_handle: string | null;
  invited_by_display_name: string | null;
  target_email: string | null;
}
export const previewInvitation = (token: string) =>
  apiGet<InvitationPreviewDto>(
    `/api/v1/invitations/${encodeURIComponent(token)}/preview`,
  );

/** Seguindo/Seguidores do cidadão autenticado (0.26.16). */
export interface SocialLinkDto {
  actor_url: string;
  handle_hint: string | null;
  since: string;
  accepted: boolean;
}
export const listMyFollowing = () =>
  apiGetCredentialed<SocialLinkDto[]>('/api/v1/me/social/following');
export const listMyFollowers = () =>
  apiGetCredentialed<SocialLinkDto[]>('/api/v1/me/social/followers');

/** Anúncios servidor-wide (0.26.17). */
export interface AnnouncementDto {
  id: string;
  body: string;
  severity: 'info' | 'warning' | 'critical';
  starts_at: string | null;
  ends_at: string | null;
  published_at: string | null;
  created_at: string;
}
export const listActiveAnnouncements = () =>
  apiGetCredentialed<AnnouncementDto[]>('/api/v1/announcements/active');
export const dismissAnnouncement = (id: string) =>
  apiPost<{ ok: true }>(`/api/v1/announcements/${encodeURIComponent(id)}/dismiss`, {});

/** Admin: CRUD de anúncios. */
export const adminListAnnouncements = () =>
  apiGetCredentialed<AnnouncementDto[]>('/api/v1/admin/announcements');
export const adminCreateAnnouncement = (body: {
  body: string;
  severity?: 'info' | 'warning' | 'critical';
  starts_at?: string | null;
  ends_at?: string | null;
  publish_now?: boolean;
}) => apiPost<AnnouncementDto>('/api/v1/admin/announcements', body);
export const adminUpdateAnnouncement = (id: string, patch: Record<string, unknown>) =>
  apiPatch<{ ok: true }>(`/api/v1/admin/announcements/${encodeURIComponent(id)}`, patch);
export const adminPublishAnnouncement = (id: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/announcements/${encodeURIComponent(id)}/publish`, {});
export const adminUnpublishAnnouncement = (id: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/announcements/${encodeURIComponent(id)}/unpublish`, {});
/** Preferências pessoais (0.26.18). */
export interface EmailPrefs {
  mention?: boolean;
  reply?: boolean;
  favorite?: boolean;
  reblog?: boolean;
  follow?: boolean;
  admin_action?: boolean;
}
export interface MyPreferencesDto {
  email_prefs: EmailPrefs;
  default_visibility: 'public' | 'unlisted' | 'followers' | 'direct';
  default_sensitive: boolean;
  /** 0.26.24: Note pública automática quando minha proposta cruza o gatilho. */
  auto_federate_threshold: boolean;
}
export const getMyPreferences = () =>
  apiGetCredentialed<MyPreferencesDto>('/api/v1/me/preferences');
export const patchMyPreferences = (patch: Partial<MyPreferencesDto>) =>
  apiPatch<{ ok: true }>('/api/v1/me/preferences', patch);

/** Regras do servidor (0.26.18). */
export interface ServerRuleDto {
  id: string;
  ordinal: number;
  text: string;
  created_at: string;
  updated_at: string;
}
export const getServerRules = () =>
  apiGet<ServerRuleDto[]>('/api/v1/server/rules');
export const adminListRules = () =>
  apiGetCredentialed<ServerRuleDto[]>('/api/v1/admin/rules');
export const adminCreateRule = (text: string, ordinal = 0) =>
  apiPost<ServerRuleDto>('/api/v1/admin/rules', { text, ordinal });
export const adminUpdateRule = (id: string, patch: { text?: string; ordinal?: number }) =>
  apiPatch<{ ok: true }>(`/api/v1/admin/rules/${encodeURIComponent(id)}`, patch);
/** Emojis personalizados (0.26.19). */
export interface CustomEmojiDto {
  id: string;
  shortcode: string;
  url: string;
  enabled: boolean;
  created_at: string;
}
export const getServerEmojis = () =>
  apiGet<CustomEmojiDto[]>('/api/v1/server/emojis');
export const adminListEmojis = () =>
  apiGetCredentialed<CustomEmojiDto[]>('/api/v1/admin/emojis');
export async function adminUploadEmoji(file: File, shortcode: string): Promise<ApiResponse<CustomEmojiDto>> {
  const form = new FormData();
  form.append('file', file);
  form.append('shortcode', shortcode);
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/emojis`, {
      method: 'POST',
      credentials: 'include',
      body: form,
    });
    return (await res.json()) as ApiResponse<CustomEmojiDto>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}
export const adminToggleEmoji = (id: string, enabled: boolean) =>
  apiPatch<{ ok: true }>(`/api/v1/admin/emojis/${encodeURIComponent(id)}`, { enabled });
export async function adminDeleteEmoji(id: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/emojis/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** Moderação de hashtag (0.26.19). */
export interface HashtagModDto {
  tag: string;
  state: 'banned' | 'promoted';
  reason: string | null;
  created_at: string;
}
export const adminListHashtags = () =>
  apiGetCredentialed<HashtagModDto[]>('/api/v1/admin/hashtags/moderation');
export const adminUpsertHashtag = (tag: string, state: 'banned' | 'promoted', reason?: string) =>
  apiPost<{ ok: true }>('/api/v1/admin/hashtags/moderation', { tag, state, reason });
export async function adminDeleteHashtag(tag: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/hashtags/moderation/${encodeURIComponent(tag)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** Auto-delete de publicações antigas (0.26.19). */
export const getAutoDelete = () =>
  apiGetCredentialed<{ days: number | null }>('/api/v1/me/preferences/auto_delete');
/** Termos editáveis (0.26.20). */
export interface ServerTermsDto { body: string | null; updated_at: string | null; }
export const getServerTerms = () =>
  apiGet<ServerTermsDto>('/api/v1/server/terms');
export const adminGetTerms = () =>
  apiGetCredentialed<ServerTermsDto>('/api/v1/admin/server/terms');
export const adminPatchTerms = (body: string) =>
  apiPatch<{ ok: true }>('/api/v1/admin/server/terms', { body });

/** CW presets (0.26.20). */
export interface CwPresetDto {
  id: string;
  phrase: string;
  spoiler_text: string | null;
  created_at: string;
}
export const getCwPresets = () =>
  apiGet<CwPresetDto[]>('/api/v1/server/cw_presets');
export const adminListCwPresets = () =>
  apiGetCredentialed<CwPresetDto[]>('/api/v1/admin/cw_presets');
export const adminCreateCwPreset = (phrase: string, spoiler_text?: string) =>
  apiPost<CwPresetDto>('/api/v1/admin/cw_presets', { phrase, spoiler_text });
export async function adminDeleteCwPreset(id: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/cw_presets/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** Bulk follow via CSV (0.26.21). */
export interface BulkFollowResultDto {
  total: number;
  followed: number;
  already: number;
  failed: number;
  errors: string[];
}
export const bulkFollow = (entries: string[]) =>
  apiPost<BulkFollowResultDto>('/api/v1/me/bulk_follow', { entries });

/** Admin: domínios de e-mail bloqueados. */
export interface EmailDomainDto {
  domain: string;
  reason: string | null;
  created_at: string;
}
export const adminListEmailDomains = () =>
  apiGetCredentialed<EmailDomainDto[]>('/api/v1/admin/email_domain_blocks');
export const adminAddEmailDomain = (domain: string, reason?: string) =>
  apiPost<{ ok: true }>('/api/v1/admin/email_domain_blocks', { domain, reason });
export async function adminRemoveEmailDomain(domain: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/email_domain_blocks/${encodeURIComponent(domain)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** Admin: regras de IP. */
export interface IpRuleDto {
  id: string;
  cidr: string;
  scope: 'signup' | 'login' | 'all';
  state: 'allow' | 'deny';
  reason: string | null;
  created_at: string;
}
export const adminListIpRules = () =>
  apiGetCredentialed<IpRuleDto[]>('/api/v1/admin/ip_rules');
export const adminAddIpRule = (body: { cidr: string; scope: IpRuleDto['scope']; state: IpRuleDto['state']; reason?: string }) =>
  apiPost<{ ok: true }>('/api/v1/admin/ip_rules', body);
export async function adminRemoveIpRule(id: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/ip_rules/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** Admin: pending signups (revisão manual). */
export interface PendingSignupDto {
  citizen_id: string;
  email: string | null;
  handle: string | null;
  display_name: string | null;
  created_at: string;
}
export const adminListPending = () =>
  apiGetCredentialed<PendingSignupDto[]>('/api/v1/admin/pending_signups');
export const adminApprovePending = (id: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/pending_signups/${encodeURIComponent(id)}/approve`, {});
export const adminRejectPending = (id: string) =>
  apiPost<{ ok: true }>(`/api/v1/admin/pending_signups/${encodeURIComponent(id)}/reject`, {});

/** Webhooks (0.26.22). */
export interface WebhookDto {
  id: string;
  url: string;
  events: string[];
  enabled: boolean;
  last_status: number | null;
  last_delivery_at: string | null;
  created_at: string;
}
export interface WebhookWithSecretDto extends WebhookDto { secret: string; }
export const adminListWebhooks = () =>
  apiGetCredentialed<WebhookDto[]>('/api/v1/admin/webhooks');
export const adminCreateWebhook = (url: string, events: string[]) =>
  apiPost<WebhookWithSecretDto>('/api/v1/admin/webhooks', { url, events });
export const adminUpdateWebhook = (id: string, enabled: boolean) =>
  apiPatch<{ ok: true }>(`/api/v1/admin/webhooks/${encodeURIComponent(id)}`, { enabled });
export async function adminDeleteWebhook(id: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/webhooks/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** Associa o cidadão logado a um convite (pós-cadastro). */
export const associateInvitation = (token: string) =>
  apiPost<{ ok: boolean; already?: boolean; reason?: string }>(
    '/api/v1/me/associate-invitation',
    { token },
  );

export async function putAutoDelete(days: number | null): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/me/preferences/auto_delete`, {
      method: 'PUT',
      credentials: 'include',
      headers: { 'content-type': 'application/json', accept: 'application/json' },
      body: JSON.stringify({ days }),
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

export async function adminDeleteRule(id: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/rules/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

export async function adminDeleteAnnouncement(id: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/admin/announcements/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** Admin: bloqueios de domínio a nível de instância (Fatia 3). */
export interface AdminDomainBlockDto {
  domain: string;
  severity: 'silence' | 'suspend';
  reason: string | null;
  created_at: string;
  created_by_handle: string | null;
}
export const adminListDomainBlocks = () =>
  apiGetCredentialed<AdminDomainBlockDto[]>(
    '/api/v1/admin/federation/domain_blocks',
  );
export const adminAddDomainBlock = (domain: string, severity: 'silence' | 'suspend', reason?: string) =>
  apiPost<{ ok: true }>('/api/v1/admin/federation/domain_blocks', {
    domain,
    severity,
    reason,
  });
/** Filtros pessoais do cidadão (esconde publicações por termo). */
export interface ContentFilterDto {
  id: string;
  phrase: string;
  context: string[];
  expires_at: string | null;
  created_at: string;
}
export const listMyFilters = () =>
  apiGetCredentialed<ContentFilterDto[]>('/api/v1/filters');
export const createMyFilter = (phrase: string, context: string[] = ['home'], expires_in?: number) =>
  apiPost<ContentFilterDto>('/api/v1/filters', { phrase, context, expires_in });
export async function deleteMyFilter(id: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/filters/${encodeURIComponent(id)}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

export async function adminRemoveDomainBlock(domain: string): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(
      `${API_BASE}/api/v1/admin/federation/domain_blocks/${encodeURIComponent(domain)}`,
      {
        method: 'DELETE',
        credentials: 'include',
        headers: { accept: 'application/json' },
      },
    );
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** List de bookmarks (retorna object_uris; usa junto com getMyFeed pra hidratar). */
export const listBookmarks = (limit = 20, offset = 0) =>
  apiGetCredentialed<{ object_uri: string; created_at: string }[]>(
    `/api/v1/bookmarks?limit=${limit}&offset=${offset}`,
  );

/** Options passed to `postNote` for 0.18.0's Mastodon-parity fields. */
export interface PostNoteOptions {
  in_reply_to_uri?: string;
  sensitive?: boolean;
  spoiler_text?: string;
  media_ids?: string[];
  /** Per-media alt_text, parallel to media_ids. Empty strings are ignored. */
  media_alts?: string[];
  /** 0.18.0-rc1: poll input — flips AP object to Question. */
  poll?: {
    options: string[];
    multiple?: boolean;
    expires_in_minutes: number;
  };
}

/** Publish a public Note. Fan-out happens in the background; the response is fast. */
export const postNote = (content: string, options: PostNoteOptions = {}) =>
  apiPost<{ activity_id: string; fanout_count: number; status: string }>(
    '/api/v1/me/notes',
    { content, ...options },
  );

/** Upload a single image attachment. Returns the row id the composer plumbs
 *  into `media_ids` on `postNote`. */
export async function uploadNoteMedia(
  file: File,
  altText?: string,
): Promise<{
  success: boolean;
  data?: {
    id: string;
    url: string;
    kind: string;
    content_type: string;
    alt_text: string | null;
    width: number | null;
    height: number | null;
  };
  error?: { message?: string };
}> {
  const form = new FormData();
  form.append('file', file);
  if (altText) form.append('alt_text', altText);
  try {
    const res = await fetch(`${API_BASE}/api/v1/me/media`, {
      method: 'POST',
      credentials: 'include',
      body: form,
    });
    const body = await res.json();
    return body;
  } catch {
    return { success: false, error: { message: 'Falha ao enviar o arquivo.' } };
  }
}

/** Descendants of a Note by its ActivityPub object URI. */
export const getThreadContext = (uri: string) =>
  apiGetCredentialed<FeedItemDto[]>(
    `/api/v1/notes/context?uri=${encodeURIComponent(uri)}`,
  );

/**
 * One notification item as returned by `/api/v1/me/notifications`.
 *
 * Kinds fediverso (migration 0406): mention | reply | favourite | reblog | follow.
 * Kinds cívicas (migration 0411, 0.25.0-fediverso Feed): proposal_threshold |
 * sla_started | sla_response | sla_expired. Front unknown kinds usam fallback.
 */
export interface NotificationDto {
  id: string;
  kind:
    | 'mention'
    | 'reply'
    | 'favourite'
    | 'reblog'
    | 'follow'
    | 'proposal_threshold'
    | 'sla_started'
    | 'sla_response'
    | 'sla_expired'
    | string;
  source_actor_url: string | null;
  source_handle: string;
  source_display_name: string | null;
  source_avatar_url: string | null;
  object_uri: string | null;
  object_preview: string | null;
  created_at: string;
  read: boolean;
}

/** In-app notifications feed for the authenticated citizen. */
export const getMyNotifications = (limit = 30, offset = 0) =>
  apiGetCredentialed<{ items: NotificationDto[]; unread_count: number }>(
    `/api/v1/me/notifications?limit=${limit}&offset=${offset}`,
  );

/** Mark every unread notification as read. */
export const clearMyNotifications = () =>
  apiPost<{ cleared: number }>('/api/v1/me/notifications/clear', {});

/** Public hashtag timeline. Returns the items indexed under `#name`. */
export const getHashtagTimeline = (name: string, limit = 30, offset = 0) =>
  apiGetCredentialed<{ tag: string; items: FeedItemDto[] }>(
    `/api/v1/timelines/tag/${encodeURIComponent(name)}?limit=${limit}&offset=${offset}`,
  );

/** Emit Update(Person) to every ACK'd inbound follower so remote instances
 *  (Mastodon et al.) drop their cached Actor doc and pick up avatar / cover /
 *  bio / name changes. Returns `{ delivered_to, targets }`. */
export const refreshMyActor = () =>
  apiPost<{ delivered_to: number; targets: number }>(
    '/api/v1/me/actor/refresh',
    {},
  );

/** Soft-delete a Note the caller owns + emit Delete(Note) to followers. */
export async function deleteNote(
  uri: string,
): Promise<ApiResponse<{ deleted: boolean; delivered_to: number }>> {
  try {
    const res = await fetch(
      `${API_BASE}/api/v1/me/notes?uri=${encodeURIComponent(uri)}`,
      { method: 'DELETE', credentials: 'include' },
    );
    const text = await res.text();
    try {
      const body = JSON.parse(text);
      if (body && typeof body === 'object' && 'success' in body) return body;
    } catch {}
    return {
      success: false,
      data: null,
      error: { code: `http_${res.status}`, message: 'Falha ao excluir.' },
      meta: null,
    };
  } catch {
    return {
      success: false,
      data: null,
      error: { code: 'network_error', message: 'Falha de conexão.' },
      meta: null,
    };
  }
}

/** Edit a Note the caller owns + emit Update(Note) to followers. */
export const editNote = (
  uri: string,
  content: string,
  options: { sensitive?: boolean; spoiler_text?: string } = {},
) =>
  apiPatch<{ updated: boolean; delivered_to: number }>(
    `/api/v1/me/notes?uri=${encodeURIComponent(uri)}`,
    { content, ...options },
  );

import type { PollDto } from './types';

/** Cast a ballot on a Note's poll. Returns the refreshed poll DTO. */
export const votePoll = (uri: string, option_ids: string[]) =>
  apiPost<PollDto>(
    `/api/v1/me/notes/vote?uri=${encodeURIComponent(uri)}`,
    { option_ids },
  );

/** One hit from `/api/v1/search/hashtags`. */
export interface HashtagHit {
  tag_normalized: string;
  tag_original: string;
  note_count: number;
}

/** One hit from `/api/v1/search/mentions` and `/directory`. */
export interface MentionHit {
  handle: string;
  display_name: string | null;
  bio: string | null;
  avatar_url: string | null;
  actor_url: string;
}

export interface NoteHit {
  object_uri: string;
  author_handle: string;
  author_display_name: string | null;
  author_avatar_url: string | null;
  content_html: string;
  published_at: string;
  is_remote: boolean;
}

/** Autocomplete hashtags by prefix. */
export const searchHashtags = (q: string, limit = 8) =>
  apiGetCredentialed<{ items: HashtagHit[] }>(
    `/api/v1/search/hashtags?q=${encodeURIComponent(q)}&limit=${limit}`,
  );

/** Autocomplete local citizen handles. */
export const searchMentions = (q: string, limit = 8) =>
  apiGetCredentialed<{ items: MentionHit[] }>(
    `/api/v1/search/mentions?q=${encodeURIComponent(q)}&limit=${limit}`,
  );

/** Unified search — accounts + hashtags + notes. */
export const searchAll = (q: string, per = 10) =>
  apiGetCredentialed<{
    accounts: MentionHit[];
    hashtags: HashtagHit[];
    notes: NoteHit[];
  }>(`/api/v1/search?q=${encodeURIComponent(q)}&limit=${per}`);

/** Trending hashtags in the past 24h. */
export const getTrendingHashtags = (limit = 10) =>
  apiGetCredentialed<{ items: HashtagHit[] }>(
    `/api/v1/trends/hashtags?limit=${limit}`,
  );

/** Public profile directory. */
export const getDirectory = (limit = 24, offset = 0) =>
  apiGetCredentialed<{ items: MentionHit[] }>(
    `/api/v1/directory?limit=${limit}&offset=${offset}`,
  );

/** People the caller doesn't follow yet, ordered by recent activity. */
export const getFollowSuggestions = (limit = 12) =>
  apiGetCredentialed<{ items: MentionHit[] }>(
    `/api/v1/suggestions/follow?limit=${limit}`,
  );

// --- Aggregated political dashboards (0.19.0-dashboards) -----------------

export interface ReportFilters {
  group_by?: 'partido' | 'politico' | 'casa' | 'esfera' | 'uf' | 'office';
  uf?: string;
  house?: 'camara' | 'senado' | '';
  party?: string;
  sphere?: 'federal' | 'estadual' | 'municipal' | '';
  status?: 'draft' | 'published' | 'clustered' | '';
}

export interface GastoGroup {
  key: string;
  label: string;
  amount_cents: number;
  mandate_count: number;
}

export interface GastoReport {
  total_cents: number;
  mandate_count: number;
  groups: GastoGroup[];
  pending: number;
  cached_at: string;
}

export interface PropostasGroup {
  key: string;
  label: string;
  count: number;
  published: number;
  clustered: number;
  answered: number;
  ignored: number;
  pending: number;
}

export interface PropostasReport {
  total: number;
  groups: PropostasGroup[];
}

function filtersToQuery(f: ReportFilters): string {
  const p = new URLSearchParams();
  if (f.group_by) p.set('group_by', f.group_by);
  if (f.uf) p.set('uf', f.uf);
  if (f.house) p.set('house', f.house);
  if (f.party) p.set('party', f.party);
  if (f.sphere) p.set('sphere', f.sphere);
  if (f.status) p.set('status', f.status);
  return p.toString() ? `?${p.toString()}` : '';
}

// The gasto report aggregates CEAP (Câmara) + CEAPS (Senado) for 594
// parliamentarians in a single call. Cold cache the call is ~60s; served
// warm it's <100ms. Give the client 90s so a first-in-window user does not
// bounce with a bogus "serviço indisponível".
const REPORTS_TIMEOUT_MS = 90_000;
export const getGastoParlamentar = (f: ReportFilters = {}) =>
  apiGet<GastoReport>(
    `/api/v1/reports/gasto-parlamentar${filtersToQuery(f)}`,
    { timeoutMs: REPORTS_TIMEOUT_MS },
  );

export const getPropostasSummary = (f: ReportFilters = {}) =>
  apiGet<PropostasReport>(
    `/api/v1/reports/proposals-summary${filtersToQuery(f)}`,
    { timeoutMs: REPORTS_TIMEOUT_MS },
  );

/** Federated feed of the authenticated citizen (own notes + followed actors). */
export const getMyFeed = (limit = 30, offset = 0) =>
  apiGetCredentialed<FeedItemDto[]>(
    `/api/v1/me/feed?limit=${limit}&offset=${offset}`,
  );

/** Toggle a Like (favoritar) on a note by its ActivityPub object URI. */
export const toggleLike = (object_uri: string) =>
  apiPost<LikeResultDto>('/api/v1/me/like', { object_uri });

/** Toggle an Announce (republicar) on a note by its ActivityPub object URI. */
export const toggleBoost = (object_uri: string) =>
  apiPost<BoostResultDto>('/api/v1/me/boost', { object_uri });

/** Atestado de cidadania por operador verificado (0.28.3, web-of-trust). */
export interface AttestationItemDto {
  attester_citizen_id: string;
  display_name: string | null;
  handle: string | null;
  kind: 'mandato' | 'partido';
  note: string | null;
  created_at: string;
}
export interface AttestationsDto {
  count: number;
  viewer_can_attest: boolean;
  viewer_attested: boolean;
  items: AttestationItemDto[];
}
export const getAttestations = (citizenId: string) =>
  apiGetCredentialed<AttestationsDto>(
    `/api/v1/citizens/${encodeURIComponent(citizenId)}/attestations`,
  );
export const attestCitizen = (citizenId: string, note?: string) =>
  apiPost<{ ok: true; kind: string }>(
    `/api/v1/citizens/${encodeURIComponent(citizenId)}/attestations`,
    { note },
  );
export async function revokeAttestation(
  citizenId: string,
): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(
      `${API_BASE}/api/v1/citizens/${encodeURIComponent(citizenId)}/attestations`,
      {
        method: 'DELETE',
        credentials: 'include',
        headers: { accept: 'application/json' },
      },
    );
    return (await res.json()) as ApiResponse<{ ok: true }>;
  } catch (err) {
    return { success: false, error: { code: 'network', message: String(err) } };
  }
}

/** Prova de notificação (0.29) — recibos hash-encadeados dos avisos ao gabinete. */
export interface DeliveryReceiptDto {
  attempt: number;
  recipient: string;
  subject: string;
  outcome: string;
  sent_at: string;
  prev_hash: string;
  hash: string;
}
export const getDeliveryReceipts = (proposalId: string) =>
  apiGetCredentialed<DeliveryReceiptDto[]>(
    `/api/v1/proposals/${encodeURIComponent(proposalId)}/delivery-receipts`,
  );

/** Preview do gatilho dinâmico (0.30.3) — o form mostra a regra do território. */
export interface ThresholdPreviewDto {
  threshold: number;
  voters: number | null;
  fraction: number;
}
export const getThresholdPreview = (mandateId: string) =>
  apiGetCredentialed<ThresholdPreviewDto>(
    `/api/v1/threshold-preview?mandate_id=${encodeURIComponent(mandateId)}`,
  );

/** Reply-to-respond (0.30) — gabinete responde via link assinado, sem conta. */
export interface RespondContextDto {
  proposal_title: string;
  mandate_display_name: string | null;
  due_at: string;
  status: string;
}
export const getRespondContext = (sla: string, t: string) =>
  apiGetCredentialed<RespondContextDto>(
    `/api/v1/respond/context?sla=${encodeURIComponent(sla)}&t=${encodeURIComponent(t)}`,
  );
export const submitRespond = (body: {
  sla_id: string;
  token: string;
  body: string;
  committed: boolean;
}) => apiPost<{ ok: true }>('/api/v1/respond', body);

// ---------------------------------------------------------------------------
// Super-admin (SOCRATES) — editar/ocultar/apagar conteúdo (0.40.0)
// ---------------------------------------------------------------------------

export interface AdminMandateEdit {
  display_name?: string;
  party?: string;
  office?: string;
  uf?: string;
  municipio?: string;
  house?: string;
  sphere?: string;
  public_email?: string;
}
export const adminEditMandate = (id: string, fields: AdminMandateEdit) =>
  apiPatch<null>(`/api/v1/admin/mandates/${encodeURIComponent(id)}`, fields);

const adminDelete = async (path: string): Promise<ApiResponse<null>> => {
  try {
    const res = await fetch(`${API_BASE}${path}`, {
      method: 'DELETE',
      credentials: 'include',
      headers: { accept: 'application/json' },
    });
    return parseEnvelope<null>(res);
  } catch {
    return networkFailure<null>();
  }
};

export const adminHideMandate = (id: string, on = true) =>
  apiPost<null>(`/api/v1/admin/mandates/${encodeURIComponent(id)}/hide?on=${on}`, {});
export const adminDeleteMandate = (id: string) =>
  adminDelete(`/api/v1/admin/mandates/${encodeURIComponent(id)}?force=true`);

export const adminHideProposal = (id: string, on = true) =>
  apiPost<null>(`/api/v1/admin/proposals/${encodeURIComponent(id)}/hide?on=${on}`, {});
export const adminDeleteProposal = (id: string) =>
  adminDelete(`/api/v1/admin/proposals/${encodeURIComponent(id)}?force=true`);

export interface AdminPartyEdit {
  name?: string;
  tse_number?: number;
  logo_url?: string;
  website?: string;
  founded_year?: number;
}
export const adminEditParty = (sigla: string, fields: AdminPartyEdit) =>
  apiPatch<null>(`/api/v1/admin/parties/${encodeURIComponent(sigla)}`, fields);
export const adminDeleteParty = (sigla: string) =>
  adminDelete(`/api/v1/admin/parties/${encodeURIComponent(sigla)}?force=true`);

// ---------------------------------------------------------------------------
// Grupos de campanha (Fase 2.3) — canal proativo campanha→eleitor
// ---------------------------------------------------------------------------

export interface CampaignGroupPost {
  id: string;
  body: string;
  created_at: string;
}
/** Enquete dirigida do grupo de campanha (Fase 3.4, migration 0532). */
export interface CampaignGroupPoll {
  id: string;
  question: string;
  status: 'open' | 'closed';
  created_at: string;
  tally: { concordo: number; neutro: number; discordo: number; total: number };
  my_answer: 'concordo' | 'neutro' | 'discordo' | null;
}
/** Painel do dono: GET /me/campaign-group. */
export interface MyCampaignGroup {
  is_politico: boolean;
  group: { id: string; name: string; description: string | null; created_at: string } | null;
  member_count: number;
  posts: CampaignGroupPost[];
  polls: CampaignGroupPoll[];
}
/** Página pública: GET /campaign-groups/{id}. */
export interface PublicCampaignGroup {
  id: string;
  name: string;
  description: string | null;
  owner_display_name: string | null;
  owner_handle: string | null;
  mandate_id: string;
  member_count: number;
  sou_membro: boolean;
  posts: CampaignGroupPost[];
  polls: CampaignGroupPoll[];
}

export const getMyCampaignGroup = () =>
  apiGetCredentialed<MyCampaignGroup>('/api/v1/me/campaign-group');

export const upsertCampaignGroup = (name: string, description?: string) =>
  apiPost<{ id: string }>('/api/v1/me/campaign-group', { name, description });

export const postCampaignGroupUpdate = (body: string) =>
  apiPost<{ id: string }>('/api/v1/me/campaign-group/posts', { body });

/** O dono abre uma enquete rápida dirigida à base. */
export const createCampaignPoll = (question: string) =>
  apiPost<{ id: string }>('/api/v1/me/campaign-group/polls', { question });

/** O dono encerra uma enquete. */
export const closeCampaignPoll = (pollId: string) =>
  apiPost<{ status: string }>(
    `/api/v1/me/campaign-group/polls/${encodeURIComponent(pollId)}/close`,
    {},
  );

/** O cidadão logado responde a uma enquete do grupo (concordo/neutro/discordo). */
export const respondCampaignPoll = (groupId: string, pollId: string, answer: string) =>
  apiPost<{ saved: boolean }>(
    `/api/v1/campaign-groups/${encodeURIComponent(groupId)}/polls/${encodeURIComponent(pollId)}/respond`,
    { answer },
  );

export const getCampaignGroup = (id: string) =>
  apiGetCredentialed<PublicCampaignGroup>(
    `/api/v1/campaign-groups/${encodeURIComponent(id)}`,
  );

export const joinCampaignGroup = (id: string) =>
  apiPost<{ joined: boolean }>(`/api/v1/campaign-groups/${encodeURIComponent(id)}/join`, {});

export const leaveCampaignGroup = async (id: string): Promise<ApiResponse<{ joined: boolean }>> => {
  try {
    const res = await fetch(
      `${API_BASE}/api/v1/campaign-groups/${encodeURIComponent(id)}/join`,
      { method: 'DELETE', credentials: 'include', headers: { accept: 'application/json' } },
    );
    return parseEnvelope<{ joined: boolean }>(res);
  } catch {
    return networkFailure<{ joined: boolean }>();
  }
};

/** Formulário público de contato (0.28.1) — setores fechados no backend. */
export type ContactSector = 'contato' | 'lgpd' | 'moderacao' | 'seguranca' | 'imprensa';
export const sendContactMessage = (body: {
  sector: ContactSector;
  name: string;
  email: string;
  subject: string;
  message: string;
  /** Honeypot anti-bot — sempre vazio em envio humano. */
  website?: string;
}) => apiPost<null>('/api/v1/contact', body);

// ---------------------------------------------------------------------------
// Doações/financiamento de campanha (0.31) — gated por vínculo de mandato.
// ---------------------------------------------------------------------------

/** Um lançamento da declaração pública de financiamento. */
export interface CampanhaEntryDto {
  id: string;
  kind: 'entrada' | 'saida';
  descricao: string;
  valor_centavos: number;
  /** ISO `YYYY-MM-DD`. */
  occurred_on: string;
  /** Recibo eleitoral — presente ⇒ o lançamento é uma doação. */
  receipt_ref: string | null;
  donor_name: string | null;
  created_at: string;
}

/** Configuração da página de arrecadação. */
export interface CampanhaConfigDto {
  meta_centavos: number | null;
  bank_account: string | null;
  crowdfunding_url: string | null;
  is_published: boolean;
}

/** `GET /me/campanha` — `is_politico=false` ⇒ conta sem vínculo de mandato. */
export interface CampanhaDto {
  is_politico: boolean;
  /** false = candidatura autodeclarada ainda não verificada (selo no painel). */
  verificado: boolean;
  config: CampanhaConfigDto | null;
  lancamentos: CampanhaEntryDto[];
}

export const getMinhaCampanha = () =>
  apiGetCredentialed<CampanhaDto>('/api/v1/me/campanha');

export const addCampanhaLancamento = (body: {
  kind: 'entrada' | 'saida';
  descricao: string;
  valor_centavos: number;
  occurred_on: string;
  receipt_ref?: string;
  donor_name?: string;
}) => apiPost<{ id: string }>('/api/v1/me/campanha/lancamentos', body);

export async function revokeCampanhaLancamento(
  id: string,
): Promise<ApiResponse<{ ok: true }>> {
  try {
    const res = await fetch(
      `${API_BASE}/api/v1/me/campanha/lancamentos/${encodeURIComponent(id)}`,
      {
        method: 'DELETE',
        credentials: 'include',
        headers: { accept: 'application/json' },
      },
    );
    return parseEnvelope<{ ok: true }>(res);
  } catch {
    return networkFailure<{ ok: true }>();
  }
}

export const saveCampanhaConfig = (body: {
  meta_centavos?: number | null;
  bank_account?: string | null;
  crowdfunding_url?: string | null;
  is_published: boolean;
}) => apiBody<{ ok: true }>('PUT', '/api/v1/me/campanha/config', body);

/** Página pública da declaração — 404 quando despublicada/inexistente. */
export interface CampanhaPublicaDto {
  handle: string;
  display_name: string | null;
  avatar_url: string | null;
  /** false = candidatura autodeclarada, ainda sem verificação (selo público). */
  verificado: boolean;
  meta_centavos: number | null;
  bank_account: string | null;
  crowdfunding_url: string | null;
  total_entradas_centavos: number;
  total_saidas_centavos: number;
  doacoes_count: number;
  lancamentos: CampanhaEntryDto[];
}

export const getCampanhaPublica = (handle: string) =>
  apiGetCredentialed<CampanhaPublicaDto>(
    `/api/v1/campanha/${encodeURIComponent(handle)}`,
  );

// ---------------------------------------------------------------------------
// Fóruns institucionais (/f/<caminho>, crate dsoc-forums, 0540)
// ---------------------------------------------------------------------------

/** Um fórum da malha institucional. */
export interface ForumDto {
  id: string;
  full_path: string;
  slug: string;
  name: string;
  description: string;
  kind: 'institucional' | 'governanca' | 'comunitario';
  esfera: 'federal' | 'estadual' | 'municipal' | null;
  uf: string | null;
  municipio: string | null;
  has_contact_email: boolean;
  avatar_url: string | null;
  banner_url: string | null;
  thresholds: number[];
}

/** Filho na árvore — real ou seção padrão ainda não materializada. */
export interface ForumChildDto {
  slug: string;
  full_path: string;
  name: string;
  virtual_section: boolean;
}

export interface ForumTreeDto {
  forum: ForumDto | null;
  children: ForumChildDto[];
}

/** Posição num tópico (fusão debates→fóruns, 0544). */
export type ForumStance = 'favor' | 'contra';

/** Tópico de fórum: interações contáveis × federadas + contadores por posição. */
export interface ForumTopicDto {
  id: string;
  forum_id: string;
  title: string;
  body: string;
  author_public_handle: string;
  interactions: number;
  federated_interactions: number;
  score: number;
  favor: number;
  contra: number;
  comment_count: number;
  created_at: string;
}

/**
 * Transparência da câmara municipal (catálogo civic_source, 0662/0669).
 * Presente só em fóruns municipais; usa a ausência de dados abertos como
 * cobrança pública e aponta o site oficial quando existe.
 */
export interface TransparencyDto {
  /** `plena` | `parcial` | `ausente`. */
  status: 'plena' | 'parcial' | 'ausente';
  /** Site oficial da câmara (base_url), quando conhecido. */
  official_url: string | null;
}

export interface ForumTopicListDto {
  forum: ForumDto;
  topics: ForumTopicDto[];
  /** Banner de transparência da câmara — só em fóruns municipais. */
  transparency: TransparencyDto | null;
}

export interface ForumCommentItemDto {
  id: string;
  author: string;
  /** Karma (reputação SO) do autor local; null p/ federado (ADR-0019). */
  author_karma: number | null;
  federated: boolean;
  stance: ForumStance | null;
  favor: number;
  contra: number;
  body: string;
  created_at: string;
}

/** Recibo público do envio institucional por patamar. */
export interface ForumDispatchDto {
  threshold: number;
  sent_at: string | null;
  crossed_at: string;
}

export interface ForumTopicDetailDto {
  topic: ForumTopicDto;
  comments: ForumCommentItemDto[];
  dispatches: ForumDispatchDto[];
  /**
   * Patamar de encaminhamento PROPORCIONAL efetivo (D3): o score que o placar
   * precisa cruzar para acionar o gabinete, proporcional ao eleitorado do
   * território (piso 10). A UI usa isto no "faltam N" em vez do 10 fixo.
   */
  escalation_threshold: number;
  /**
   * A QUEM o placar encaminha ao cruzar o patamar: nomes dos mandatos-alvo
   * alcançáveis (B1) ou o nome da seção com contato institucional curado
   * (ex.: "Ministério dos Transportes"). null = nenhum canal alcançável —
   * a UI mostra "encaminhamento pendente" em vez de prometer entrega.
   */
  escalation_destination?: string | null;
  /**
   * Privacidade graduada (D5/D6): true quando o fórum é de um município
   * pequeno. Nesse caso a atribuição individual de posição foi omitida dos
   * comentários (autor = "participante", stance/karma nulos); só o agregado
   * do tópico é público. A UI deve sinalizar "apoio agregado por privacidade".
   */
  aggregate_only: boolean;
}

/**
 * Uma afirmação-ponte (D8.2 — síntese estilo Polis/vTaiwan): um argumento que
 * reúne endosso ATRAVESSANDO a divisão favor×contra do tópico. `favor_side` e
 * `contra_side` = endossos vindos de cada lado; `bridge_score` = média harmônica
 * dos dois (quanto maior, mais o argumento UNE quem discorda).
 */
export interface ForumBridgeCommentDto {
  comment: ForumCommentItemDto;
  favor_side: number;
  contra_side: number;
  bridge_score: number;
}

/** Consenso de um tópico (D8.2): as afirmações-ponte do topo. Camada ADITIVA sobre o placar. */
export interface ForumTopicConsensusDto {
  topic_id: string;
  bridges: ForumBridgeCommentDto[];
  aggregate_only: boolean;
}

export const getForumTree = (path?: string) =>
  apiGet<ForumTreeDto>(
    `/api/v1/f/tree${path ? `?path=${encodeURIComponent(path)}` : ''}`,
  );

/** Top N afirmações-ponte de um tópico (D8.2) — "argumentos que uniram quem discorda". */
export const getForumConsensus = (id: string, limit = 5) =>
  apiGet<ForumTopicConsensusDto>(
    `/api/v1/f/topics/${encodeURIComponent(id)}/consensus?limit=${limit}`,
  );

export const getForumTopics = (path: string, sort: 'hot' | 'new' = 'hot') =>
  apiGet<ForumTopicListDto>(
    `/api/v1/f/topics?path=${encodeURIComponent(path)}&sort=${sort}`,
  );

export const getForumTopic = (id: string) =>
  apiGet<ForumTopicDetailDto>(`/api/v1/f/topics/${encodeURIComponent(id)}`);

/** Cria um tópico de fórum. `targets` (B1) = mandate_ids para direcionar a
 *  demanda a gabinete(s) específico(s); ausente/vazio = tópico sem alvo (cai no
 *  contato curado da seção). O mesmo placar/patamar do fórum — uma régua só. */
export const createForumTopic = (
  path: string,
  title: string,
  body: string,
  targets: string[] = [],
) =>
  apiPost<ForumTopicDto>('/api/v1/f/topics', {
    path,
    title,
    body,
    ...(targets.length > 0 ? { targets } : {}),
  });

export const voteForumTopic = (id: string, stance: ForumStance) =>
  apiPost<ForumTopicDto>(`/api/v1/f/topics/${encodeURIComponent(id)}/vote`, {
    stance,
  });

export const commentForumTopic = (
  id: string,
  body: string,
  stance?: ForumStance,
) =>
  apiPost<ForumTopicDto>(
    `/api/v1/f/topics/${encodeURIComponent(id)}/comments`,
    { body, stance: stance ?? null },
  );

/** Voto num argumento (estilo StackOverflow) — devolve argumento + tópico atualizados. */
export const voteForumComment = (id: string, stance: ForumStance) =>
  apiPost<{ comment: ForumCommentItemDto; topic: ForumTopicDto }>(
    `/api/v1/f/comments/${encodeURIComponent(id)}/vote`,
    { stance },
  );

/** Linha do painel admin de fóruns (F3). */
export interface AdminForumDto {
  id: string;
  full_path: string;
  name: string;
  kind: string;
  esfera: string | null;
  contact_email: string | null;
  avatar_url: string | null;
  banner_url: string | null;
  thresholds: number[];
  moderator_count: number;
  pending_dispatches: number;
  topic_count: number;
}

export interface ForumModeratorDto {
  citizen_id: string;
  handle: string | null;
  display_name: string | null;
}

export const adminListForums = (q = '', offset = 0, limit = 50) =>
  apiGetCredentialed<AdminForumDto[]>(
    `/api/v1/admin/forums?q=${encodeURIComponent(q)}&offset=${offset}&limit=${limit}`,
  );

export const adminUpdateForum = (
  id: string,
  payload: {
    contact_email?: string;
    thresholds?: number[];
    avatar_url?: string;
    banner_url?: string;
  },
) => apiPatch<{ ok: true }>(`/api/v1/admin/forums/${encodeURIComponent(id)}`, payload);

export const adminForumModerators = (id: string) =>
  apiGetCredentialed<ForumModeratorDto[]>(
    `/api/v1/admin/forums/${encodeURIComponent(id)}/moderators`,
  );

export const adminForumAddModerator = (id: string, handle: string) =>
  apiPost<{ citizen_id: string }>(
    `/api/v1/admin/forums/${encodeURIComponent(id)}/moderators`,
    { handle },
  );

export async function adminForumRemoveModerator(
  id: string,
  citizenId: string,
): Promise<boolean> {
  try {
    const res = await fetch(
      `${API_BASE}/api/v1/admin/forums/${encodeURIComponent(id)}/moderators/${encodeURIComponent(citizenId)}`,
      { method: 'DELETE', credentials: 'include' },
    );
    return res.ok;
  } catch {
    return false;
  }
}

/** Item do feed de últimas postagens dos fóruns (home /f). */
export interface RecentForumTopicDto {
  id: string;
  title: string;
  score: number;
  favor: number;
  contra: number;
  ponderacao: number;
  interactions: number;
  comment_count: number;
  created_at: string;
  forum_path: string;
  forum_name: string;
}

export const getRecentForumTopics = (limit = 25) =>
  apiGet<RecentForumTopicDto[]>(`/api/v1/f/recent?limit=${limit}`);

/** Permissões efetivas do caller (chaves modulo.acao) — a UI decide o que mostrar. */
export interface MyPermissions {
  keys: string[];
  is_administrator: boolean;
}
export const getMyPermissions = () =>
  apiGetCredentialed<MyPermissions>('/api/v1/me/permissions');

/** Remove (moderação) um tópico de fórum. Backend exige content.moderate/forums.moderate
 *  ou ser moderador do fórum. */
export const moderateRemoveTopic = (id: string, reason?: string) =>
  apiPost<{ removed: true }>(
    `/api/v1/f/topics/${encodeURIComponent(id)}/remove`,
    { reason },
  );

/** Remove (moderação) um argumento/comentário de fórum. */
export const moderateRemoveComment = (id: string, reason?: string) =>
  apiPost<{ removed: true }>(
    `/api/v1/f/comments/${encodeURIComponent(id)}/remove`,
    { reason },
  );

// --- Papéis & permissões (R4 /admin/papeis) --------------------------------
export interface PermissionCatalogItem {
  key: string;
  label: string;
  category: string;
  category_label: string;
}
export interface RoleDto {
  id: string;
  name: string;
  color: string | null;
  position: number;
  permissions: string[];
  highlighted: boolean;
}
export interface RoleMemberDto {
  citizen_id: string;
  handle: string | null;
  display_name: string | null;
}
export interface RoleInput {
  name: string;
  color?: string | null;
  position: number;
  permissions: string[];
  highlighted: boolean;
}

export const getPermissionCatalog = () =>
  apiGetCredentialed<PermissionCatalogItem[]>('/api/v1/admin/permission-catalog');
export const listRoles = () => apiGetCredentialed<RoleDto[]>('/api/v1/admin/roles');

// ÁGORA campaign layer (F1, #58) — party directories & administrators (/admin/parties).
export interface PartyDto {
  sigla: string;
  name: string;
  directory_count: number;
  administrator_count: number;
}
export interface DirectoryDto {
  id: string;
  esfera: 'federal' | 'estadual' | 'municipal';
  uf: string | null;
  municipio: string | null;
  name: string;
  parent_directory_id: string | null;
  created_at: string;
}
export interface PartyAdministratorDto {
  id: string;
  directory_id: string | null;
  citizen_id: string;
  handle: string | null;
  role: 'admin' | 'moderador';
  created_at: string;
}

export const listParties = () => apiGetCredentialed<PartyDto[]>('/api/v1/admin/parties');
export const listDirectories = (sigla: string) =>
  apiGetCredentialed<DirectoryDto[]>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories`,
  );
export const createDirectory = (
  sigla: string,
  body: { esfera: string; uf?: string; municipio?: string; name: string; parent_directory_id?: string },
) => apiPost<string>(`/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories`, body);
export const listPartyAdministrators = (sigla: string) =>
  apiGetCredentialed<PartyAdministratorDto[]>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/administrators`,
  );
export const assignPartyAdministrator = (
  sigla: string,
  body: { citizen_id?: string; handle?: string; role: string; directory_id?: string },
) => apiPost<string>(`/api/v1/admin/parties/${encodeURIComponent(sigla)}/administrators`, body);
export const removePartyAdministrator = (sigla: string, id: string) =>
  apiDelete<null>(`/api/v1/admin/parties/${encodeURIComponent(sigla)}/administrators/${id}`);

// INTERCOMS #69 — SMSGateway por diretório (credenciais cifradas no backend).
export const getSmsGateway = (sigla: string, dirId: string) =>
  apiGetCredentialed<{ configured: boolean; url: string | null }>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories/${dirId}/sms-gateway`,
  );
export const setSmsGateway = (
  sigla: string,
  dirId: string,
  body: { url: string; user?: string; pass?: string },
) =>
  apiPut<{ configured: boolean }>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories/${dirId}/sms-gateway`,
    body,
  );
export const deleteSmsGateway = (sigla: string, dirId: string) =>
  apiDelete<{ deleted: number }>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories/${dirId}/sms-gateway`,
  );

// ÁGORA F7 (#64) — painel de campanha do partido.
export interface PartyDashboard {
  directories_count: number;
  administrators_count: number;
  consent: {
    all_parties: number;
    this_party: number;
    directory_this_party: number;
    municipality_any: number;
  };
  own_contacts_total: number;
  own_contacts_matched: number;
  broadcasts_count: number;
  broadcasts: {
    subject: string;
    recipients: number;
    created_at: string;
    consultation_id: string | null;
    uf: string | null;
    municipio: string | null;
  }[];
}
export const getPartyDashboard = (sigla: string) =>
  apiGetCredentialed<PartyDashboard>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/dashboard`,
  );

// ÁGORA F4 (#61) — base própria de contatos por diretório (verificada contra a base central).
export interface ContactImportResult {
  received: number;
  inserted: number;
  duplicates: number;
  matched: number;
  invalid: number;
}
export const importContacts = (
  sigla: string,
  directoryId: string,
  body: { legal_basis: string; contacts: { email: string; name?: string; phone?: string }[] },
) =>
  apiPost<ContactImportResult>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories/${directoryId}/contacts/import`,
    body,
  );
export const contactStats = (sigla: string, directoryId: string) =>
  apiGetCredentialed<{ total: number; matched: number }>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories/${directoryId}/contacts`,
  );
export const clearContacts = (sigla: string, directoryId: string) =>
  apiDelete<{ deleted: number }>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories/${directoryId}/contacts`,
  );

// ÁGORA F3 (#60) — broadcast consentido por diretório municipal.
export const sendBroadcast = (
  sigla: string,
  directoryId: string,
  body: { subject: string; body: string; questions?: string[] },
) =>
  apiPost<{ recipients: number; broadcast_id: string; consultation_id: string | null }>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories/${directoryId}/broadcast`,
    body,
  );

// ÁGORA #69b — broadcast SMS consentido (usa o SMSGateway do diretório; 1/semana, owner ilimitado).
export const sendBroadcastSms = (sigla: string, directoryId: string, body: { body: string }) =>
  apiPost<{ recipients: number; broadcast_id: string }>(
    `/api/v1/admin/parties/${encodeURIComponent(sigla)}/directories/${directoryId}/broadcast-sms`,
    body,
  );

// ÁGORA F2 (#59) — consentimento de campanha do cidadão (/me/campaign-consent).
export interface CampaignConsentDto {
  id: string;
  scope: 'all_parties' | 'party' | 'municipality' | 'directory';
  party_sigla: string | null;
  uf: string | null;
  municipio: string | null;
  granted_at: string;
}
export const listCampaignConsent = () =>
  apiGetCredentialed<CampaignConsentDto[]>('/api/v1/me/campaign-consent');
export const grantCampaignConsent = (body: {
  scope: string;
  party_sigla?: string;
  uf?: string;
  municipio?: string;
}) => apiPost<string>('/api/v1/me/campaign-consent', body);
export const revokeCampaignConsent = (id: string) =>
  apiDelete<null>(`/api/v1/me/campaign-consent/${id}`);

// ÁGORA F5 (#62) — telefone + verificação por OTP SMS (opt-in).
export const getPhoneStatus = () =>
  apiGetCredentialed<{ phone: string | null; verified: boolean }>('/api/v1/me/phone');
export const setPhone = (phone: string) =>
  apiPost<{ sent: boolean }>('/api/v1/me/phone', { phone });
export const verifyPhone = (code: string) =>
  apiPost<{ verified: boolean }>('/api/v1/me/phone/verify', { code });

// Interesses do cidadão (áreas ministeriais) — perfil.
export interface InterestArea {
  slug: string;
  name: string;
  ministry: string | null;
}
export const getInterestAreas = () => apiGetCredentialed<InterestArea[]>('/api/v1/interest-areas');
export const getMyInterests = () => apiGetCredentialed<string[]>('/api/v1/me/interests');
export const setMyInterests = (areas: string[]) =>
  apiPut<{ saved: boolean }>('/api/v1/me/interests', { areas });

// Gestão admin das áreas de interesse (interest_area) — CRUD + contagem de uso.
export interface AdminInterestArea {
  slug: string;
  name: string;
  ministry: string | null;
  position: number;
  citizen_count: number;
}
export const getAdminInterestAreas = () =>
  apiGetCredentialed<AdminInterestArea[]>('/api/v1/admin/interest-areas');
export const createInterestArea = (body: {
  slug: string;
  name: string;
  ministry?: string | null;
  position?: number;
}) => apiPost<AdminInterestArea>('/api/v1/admin/interest-areas', body);
export const updateInterestArea = (
  slug: string,
  body: { name: string; ministry?: string | null; position?: number },
) => apiPut<{ updated: boolean }>(`/api/v1/admin/interest-areas/${encodeURIComponent(slug)}`, body);
export const deleteInterestArea = (slug: string) =>
  apiDelete<{ deleted: boolean }>(`/api/v1/admin/interest-areas/${encodeURIComponent(slug)}`);

// ÁGORA F6 (#63) — 2FA por TOTP (app autenticador).
export const getTotpStatus = () =>
  apiGetCredentialed<{ enabled: boolean }>('/api/v1/me/2fa/totp');
export const totpSetup = () =>
  apiPost<{ secret: string; uri: string }>('/api/v1/me/2fa/totp/setup', {});
export const totpEnable = (code: string) =>
  apiPost<{ enabled: boolean; recovery_codes: string[] }>('/api/v1/me/2fa/totp/enable', { code });
export const totpDisable = (code: string) =>
  apiPost<{ enabled: boolean }>('/api/v1/me/2fa/totp/disable', { code });
export const createRole = (body: RoleInput) =>
  apiPost<{ ok: true }>('/api/v1/admin/roles', body);
export const updateRole = (id: string, body: RoleInput) =>
  apiPut<{ ok: true }>(`/api/v1/admin/roles/${encodeURIComponent(id)}`, body);
export const deleteRole = (id: string) =>
  apiDelete<{ ok: true }>(`/api/v1/admin/roles/${encodeURIComponent(id)}`);
export const listRoleMembers = (id: string) =>
  apiGetCredentialed<RoleMemberDto[]>(
    `/api/v1/admin/roles/${encodeURIComponent(id)}/members`,
  );
export const addRoleMember = (id: string, handle: string) =>
  apiPost<{ ok: true }>(
    `/api/v1/admin/roles/${encodeURIComponent(id)}/members`,
    { handle },
  );
export const removeRoleMember = (id: string, citizenId: string) =>
  apiDelete<{ ok: true }>(
    `/api/v1/admin/roles/${encodeURIComponent(id)}/members/${encodeURIComponent(citizenId)}`,
  );

// ---------------------------------------------------------------------------
// Orçamento participativo — piloto de mandato (D8.3)
// ---------------------------------------------------------------------------
import type { OpRoundDto, OpRoundSummaryDto } from './types';

/** Operador cria uma rodada (verba de emenda). Gate: vínculo de mandato. */
export const createOpRound = (body: {
  title: string;
  budget_cents: number;
  uf?: string | null;
  municipio_ibge?: number | null;
}) => apiPost<{ id: string }>('/api/v1/me/mandate/op/rounds', body);

/** Operador avança a fase da rodada (propostas → votacao → resultado → execucao). */
export const advanceOpPhase = (roundId: string, phase: string) =>
  apiPost<{ phase: string }>(
    `/api/v1/me/mandate/op/rounds/${encodeURIComponent(roundId)}/phase`,
    { phase },
  );

/** Operador marca o status de execução de um item (prestação de contas). */
export const markOpExecution = (
  roundId: string,
  itemId: string,
  execution_status: string,
) =>
  apiPost<{ execution_status: string }>(
    `/api/v1/me/mandate/op/rounds/${encodeURIComponent(roundId)}/items/${encodeURIComponent(itemId)}/execution`,
    { execution_status },
  );

/** Cidadão logado submete um item (só na fase 'propostas'). */
export const submitOpItem = (
  roundId: string,
  body: { title: string; description?: string; estimated_cents?: number | null },
) => apiPost<{ id: string }>(`/api/v1/op/rounds/${encodeURIComponent(roundId)}/items`, body);

/** Cidadão logado vota num item (só na fase 'votacao'). Upsert: 1 voto por rodada. */
export const castOpVote = (roundId: string, itemId: string) =>
  apiPost<{ voted: boolean; item_id: string }>(
    `/api/v1/op/rounds/${encodeURIComponent(roundId)}/vote`,
    { item_id: itemId },
  );

/** Superfície pública de uma rodada (rodada + itens + ranking). */
export const getOpRound = (roundId: string) =>
  apiGetCredentialed<OpRoundDto>(`/api/v1/op/rounds/${encodeURIComponent(roundId)}`);

/** Rodadas de OP de um mandato (perfil do político). */
export const getMandateOpRounds = (mandateId: string) =>
  apiGetCredentialed<{ mandate_id: string; rounds: OpRoundSummaryDto[] }>(
    `/api/v1/politicos/${encodeURIComponent(mandateId)}/op`,
  );

/** Rodadas recentes (descoberta + build-time SSG). Usa o GET simples (Fetched). */
export const getRecentOpRounds = () =>
  apiGet<{ rounds: OpRoundSummaryDto[] }>('/api/v1/op/rounds');

// SOCRATES — espelho de Ideias Legislativas do e-Cidadania (Senado) como
// tópicos do fórum `senado` (admin-curado, migration 0670).
export interface SocratesMirrorEntry {
  ideia_id: string;
  source_url: string;
  topic_id: string;
  topic_title: string;
  /** Caminho navegável do tópico no front (`/f/topico/<id>`). */
  path: string;
  created_at: string;
}
export interface SocratesMirrorCreated {
  topic_id: string;
  path: string;
}
export const getSocratesMirrors = () =>
  apiGetCredentialed<SocratesMirrorEntry[]>('/api/v1/admin/socrates/mirrors');
/** Espelha uma ideia (URL do e-Cidadania ou id numérico). 409 = já espelhada
 *  (`error.code === 'already_mirrored'`, com o tópico existente em `data`). */
export const socratesMirrorIdea = (url_or_id: string) =>
  apiPost<SocratesMirrorCreated>('/api/v1/admin/socrates/mirror', { url_or_id });
