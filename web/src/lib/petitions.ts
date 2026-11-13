// Client for public petitions ("abaixo-assinados"). The server enforces every rule;
// the helpers here only decide what to show and solve the proof-of-work check.
import { API_BASE, apiGetCredentialed, apiPatch, apiPost } from './api';

export type PetitionStatus = 'open' | 'closed' | 'hidden';

export interface PetitionListItem {
  id: string;
  title: string;
  summary: string;
  threshold: number;
  signature_count: number;
  reached: boolean;
  status: PetitionStatus;
  author_name: string | null;
  created_at: string;
}

export interface PublicSigner {
  name: string;
  city: string | null;
  signed_at: string;
}

export interface PetitionDetail extends Omit<PetitionListItem, 'summary'> {
  body: string;
  closed_at: string | null;
  signers: PublicSigner[];
  signed_by_me: boolean;
  can_close: boolean;
  is_admin: boolean;
}

export interface PetitionSettings {
  anonymous_signing: boolean;
  verification: 'local' | 'sso' | null;
  creates_sso_account: boolean;
  pending_ttl_hours: number | null;
}

export interface Tally {
  petition_id: string;
  signature_count: number;
  threshold: number;
  reached: boolean;
  newly_signed: boolean;
}

export interface Confirmed extends Tally {
  account_created: boolean;
}

const base = (id: string) => `/api/v1/petitions/${encodeURIComponent(id)}`;

export const listPetitions = (status: 'open' | 'closed' | 'hidden' | 'all' = 'open') =>
  apiGetCredentialed<PetitionListItem[]>(`/api/v1/petitions?status=${status}`);
export const getPetition = (id: string) => apiGetCredentialed<PetitionDetail>(base(id));
export const petitionSettings = () => apiGetCredentialed<PetitionSettings>('/api/v1/petitions/settings');
export const createPetition = (payload: { title: string; body: string; threshold: number }) =>
  apiPost<{ id: string }>('/api/v1/petitions', payload);
export const setPetitionStatus = (id: string, status: PetitionStatus) =>
  apiPatch<{ ok: true }>(base(id), { status });
export const signAsMember = (id: string, payload: { show_name: boolean; city?: string }) =>
  apiPost<Tally>(`${base(id)}/sign`, payload);
export const signAsVisitor = (
  id: string,
  payload: { name: string; email: string; city?: string; show_name: boolean; consent: boolean; altcha: string },
) => apiPost<{ status: 'check_email' }>(`${base(id)}/public-signatures`, payload);
export const confirmSignature = (
  proof: { token: string } | { pending: string; user: string; code: string },
) => apiPost<Confirmed>('/api/v1/petitions/confirm', proof);

/** Share of the goal reached, 0–100, for the progress bar. */
export function progress(count: number, threshold: number): number {
  if (threshold <= 0) return 100;
  return Math.min(100, Math.round((count / threshold) * 100));
}

/** Read the confirmation link: `?t=` (local mode) or `?p=&u=&c=` (sso mode). */
export function proofFromQuery(
  search: string,
): { token: string } | { pending: string; user: string; code: string } | null {
  const q = new URLSearchParams(search);
  const token = q.get('t');
  if (token) return { token };
  const pending = q.get('p');
  const user = q.get('u');
  const code = q.get('c');
  if (pending && user && code) return { pending, user, code };
  return null;
}

// ---------------------------------------------------------------------------
// ALTCHA proof of work (self-hosted, no third party). Same format as the official
// widget: find `number` so that SHA-256(salt + number) equals `challenge`.
// ---------------------------------------------------------------------------

export interface AltchaChallenge {
  algorithm: string;
  challenge: string;
  maxnumber: number;
  salt: string;
  signature: string;
}

const encoder = new TextEncoder();

async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', encoder.encode(text));
  return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('');
}

/** Solve a challenge. `onProgress` gets 0–1. Returns the base64 payload, or null. */
export async function solveAltcha(
  c: AltchaChallenge,
  onProgress?: (share: number) => void,
): Promise<string | null> {
  if (c.algorithm !== 'SHA-256') return null;
  for (let n = 0; n <= c.maxnumber; n += 1) {
    if ((await sha256Hex(`${c.salt}${n}`)) === c.challenge) {
      const payload = { algorithm: c.algorithm, challenge: c.challenge, number: n, salt: c.salt, signature: c.signature };
      return btoa(JSON.stringify(payload));
    }
    if (onProgress && n % 2000 === 0) onProgress(n / c.maxnumber);
  }
  return null;
}

/** Fetch a fresh challenge and solve it. */
export async function humanCheck(onProgress?: (share: number) => void): Promise<string | null> {
  try {
    const res = await fetch(`${API_BASE}/api/v1/petitions/altcha`, {
      headers: { accept: 'application/json' },
      cache: 'no-store',
    });
    if (!res.ok) return null;
    return solveAltcha((await res.json()) as AltchaChallenge, onProgress);
  } catch {
    return null;
  }
}
