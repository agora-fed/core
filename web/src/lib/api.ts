// Tiny typed API client wrapping `fetch` with the frozen ApiResponse envelope.
// Base URL comes from PUBLIC_API_BASE (IPv6-first, per platform principle 4).

import type {
  ApiResponse,
  ConsultationDto,
  DebateDto,
  MandateDto,
  MyMandateDto,
  ProfileDto,
  ProfileUpdateDto,
  ProposalDto,
  PromiseDto,
  ScorecardDto,
  SessionInfoDto,
  SlaDto,
} from './types';

export type {
  MandateDto,
  MyMandateDto,
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
    const body = (await res.json()) as ApiResponse<T>;
    if (!res.ok || !body.success) {
      return {
        ok: false,
        data: null,
        error: body.error?.message ?? 'Não foi possível carregar os dados.',
      };
    }
    return { ok: true, data: body.data, error: null };
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

// --- Typed convenience readers used by the SSR pages -------------------------

const orgQuery = (orgId: string, extra = '') =>
  `?org_id=${encodeURIComponent(orgId)}${extra}`;

export const getProposals = (orgId = DEFAULT_ORG_ID, limit = 20) =>
  apiGet<ProposalDto[]>(`/api/v1/proposals${orgQuery(orgId, `&limit=${limit}`)}`);

export const getProposal = (id: string) =>
  apiGet<ProposalDto>(`/api/v1/proposals/${encodeURIComponent(id)}`);

export const getScorecards = (orgId = DEFAULT_ORG_ID, limit = 50) =>
  apiGet<ScorecardDto[]>(`/api/v1/scorecards${orgQuery(orgId, `&limit=${limit}`)}`);

export const getScorecard = (mandateId: string) =>
  apiGet<ScorecardDto>(`/api/v1/scorecards/${encodeURIComponent(mandateId)}`);

export const getMandate = (mandateId: string) =>
  apiGet<MandateDto>(`/api/v1/mandates/${encodeURIComponent(mandateId)}`);

/** Directory of mandates in an org — drives the "Propor" form's picker so the user does not have
 *  to type a UUID by hand. Public read. */
export const getMandates = (orgId = DEFAULT_ORG_ID, limit = 50) =>
  apiGet<MandateDto[]>(`/api/v1/mandates${orgQuery(orgId, `&limit=${limit}`)}`);

/** Read the authenticated citizen's own profile (cookie required). */
export const getMyProfile = () => apiGetCredentialed<ProfileDto>('/api/v1/me');

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

/** Session returned by /auth/register and /auth/login. */
export interface SessionData {
  id: string;
  citizen_id: string;
  issued_at: string;
  expires_at: string;
  public_handle: string;
}

/** Register a citizen (e-mail + senha + CPF). Always includes org_id. */
export const register = (email: string, password: string, cpf: string, orgId = DEFAULT_ORG_ID) =>
  apiPost<SessionData>('/api/v1/auth/register', {
    org_id: orgId,
    email: email.trim(),
    password,
    cpf,
  });

/**
 * Register as a politician/candidate (F1.3/F1.4). Requires `email` to match the mandate's
 * `public_email` (case-insensitive) on the server side — the only automatic proof of
 * mandate control at registration time. On success the citizen is created at `directory`
 * verification level with `is_public=true` and a mandate_identity_binding.
 */
export const registerPolitician = (
  email: string,
  password: string,
  cpf: string,
  mandateId: string,
  orgId = DEFAULT_ORG_ID,
) =>
  apiPost<SessionData>('/api/v1/auth/register/politician', {
    org_id: orgId,
    email: email.trim(),
    password,
    cpf,
    mandate_id: mandateId,
  });

/** Authenticate (e-mail + senha). Always includes org_id. */
export const login = (email: string, password: string, orgId = DEFAULT_ORG_ID) =>
  apiPost<SessionData>('/api/v1/auth/login', {
    org_id: orgId,
    email: email.trim(),
    password,
  });

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

/** Publish a public Note. Fan-out happens in the background; the response is fast. */
export const postNote = (content: string) =>
  apiPost<{ activity_id: string; fanout_count: number; status: string }>(
    '/api/v1/me/notes',
    { content },
  );
