// Tiny typed API client wrapping `fetch` with the frozen ApiResponse envelope.
// Base URL comes from PUBLIC_API_BASE (IPv6-first, per platform principle 4).

import type {
  ActivityDto,
  ApiResponse,
  BoostResultDto,
  ConsultationDto,
  DebateDto,
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
export const getMandates = (orgId = DEFAULT_ORG_ID, limit = 50, offset = 0) =>
  apiGet<MandateDto[]>(
    `/api/v1/mandates${orgQuery(orgId, `&limit=${limit}&offset=${offset}`)}`,
  );

/** Full mandate directory, walking the server's `offset`/`limit` window (server caps a single
 *  page at 100). We need every parlamentar for /politicos and /partidos, so page until a short
 *  page comes back. `hardCap` guards against an unbounded loop if the API ever misbehaves. */
export async function getAllMandates(
  orgId = DEFAULT_ORG_ID,
  hardCap = 5000,
): Promise<Fetched<MandateDto[]>> {
  const page = 100;
  const all: MandateDto[] = [];
  for (let offset = 0; offset < hardCap; offset += page) {
    const res = await getMandates(orgId, page, offset);
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

/** One notification item as returned by `/api/v1/me/notifications`. */
export interface NotificationDto {
  id: string;
  kind: 'mention' | 'reply' | 'favourite' | 'reblog' | 'follow';
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
