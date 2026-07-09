// Tiny typed API client wrapping `fetch` with the frozen ApiResponse envelope.
// Base URL comes from PUBLIC_API_BASE (IPv6-first, per platform principle 4).

import type {
  ActivityDto,
  ApiResponse,
  BoostResultDto,
  ConsultationDto,
  DebateDto,
  ApiResponse,
  FeedItemDto,
  LikeResultDto,
  MandateDto,
  MandateInviteSummaryDto,
  MyMandateDto,
  PartyDetailDto,
  PartyDto,
  ProfileDto,
  ProfileUpdateDto,
  ProposalDto,
  PromiseDto,
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
  method: 'POST' | 'PATCH',
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

/** Normalized public activity for a mandate (proxy Câmara/Senado). Always OK with empty
 *  sections when the mandate has no linked house profile or an upstream fails. */
export const getMandateActivity = (mandateId: string, orgId = DEFAULT_ORG_ID) =>
  apiGet<ActivityDto>(
    `/api/v1/mandates/${encodeURIComponent(mandateId)}/atividade${orgQuery(orgId)}`,
  );

/** Directory of mandates in an org — drives the "Propor" form's picker so the user does not have
 *  to type a UUID by hand. Public read. */
export const getMandates = (
  orgId = DEFAULT_ORG_ID,
  limit = 50,
  offset = 0,
  sphere?: 'federal' | 'estadual' | 'municipal',
) =>
  apiGet<MandateDto[]>(
    `/api/v1/mandates${orgQuery(orgId, `&limit=${limit}&offset=${offset}${sphere ? `&sphere=${sphere}` : ''}`)}`,
  );

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

export const getSlas = (orgId = DEFAULT_ORG_ID, limit = 50) =>
  apiGet<SlaDto[]>(`/api/v1/consequence/slas${orgQuery(orgId, `&limit=${limit}`)}`);

export const getSla = (id: string) =>
  apiGet<SlaDto>(`/api/v1/consequence/slas/${encodeURIComponent(id)}`);

export const getDebates = (orgId = DEFAULT_ORG_ID, limit = 30) =>
  apiGet<DebateDto[]>(`/api/v1/debates${orgQuery(orgId, `&limit=${limit}`)}`);

export const getConsultations = (orgId = DEFAULT_ORG_ID, limit = 30) =>
  apiGet<ConsultationDto[]>(
    `/api/v1/surveys${orgQuery(orgId, `&limit=${limit}`)}`,
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

/** Inicia o cadastro de cidadão. Não emite sessão — dispara link por e-mail. */
export const register = (email: string, password: string, cpf: string, orgId = DEFAULT_ORG_ID) =>
  apiPost<SignupPendingData>('/api/v1/auth/register', {
    org_id: orgId,
    email: email.trim(),
    password,
    cpf,
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
  apiPost<null>(`/api/v1/admin/users/${citizen_id}/platform-role`, { role });

export const setPartyRole = (
  citizen_id: string,
  role: 'admin' | 'moderador' | 'none',
  party_sigla?: string,
) =>
  apiPost<null>(`/api/v1/admin/users/${citizen_id}/party-role`, {
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

/** POST /me/titulo-eleitor — valida algoritmicamente (12 dígitos) e persiste. */
export const submitTituloEleitor = (titulo: string) =>
  apiPost<{ titulo_status: string; titulo_last4: string }>(
    '/api/v1/me/titulo-eleitor',
    { titulo },
  );

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
