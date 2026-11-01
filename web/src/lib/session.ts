// A session can expire while the browser still holds the cached identity. A failed request
// is not proof of that (a wrong current password also answers 401), so ask GET /me first.

export const IDENTITY_KEYS = [
  'dsoc_citizen',
  'dsoc_handle',
  'dsoc_name',
  'dsoc_avatar',
  'dsoc_is_admin',
  'dsoc_is_politico',
];

const SIGNED_IN_ONLY = [
  '/admin',
  '/configuracoes',
  '/convites',
  '/meu-grupo',
  '/notificacoes',
  '/painel-mandato',
  '/rede',
  '/salvos',
];

export const SESSION_EXPIRED_EVENT = 'dsoc:session-expired';

export function requiresSignIn(path: string): boolean {
  return SIGNED_IN_ONLY.some((p) => path === p || path.startsWith(`${p}/`));
}

export function loginUrl(path: string, search = ''): string {
  return `/entrar/?next=${encodeURIComponent(path + search)}`;
}

let checking = false;

/** Call after a 401. `acting` means the person tried to do something that needs a session. */
export async function onUnauthorized(acting: boolean, apiBase: string): Promise<void> {
  if (typeof window === 'undefined' || checking) return;
  try {
    if (!localStorage.getItem('dsoc_citizen')) return;
  } catch {
    return;
  }
  checking = true;
  try {
    const me = await fetch(`${apiBase}/api/v1/me`, { credentials: 'include' });
    if (me.status !== 401) return;
    for (const key of IDENTITY_KEYS) localStorage.removeItem(key);
    const { pathname, search } = window.location;
    if (acting || requiresSignIn(pathname)) {
      window.location.assign(loginUrl(pathname, search));
    } else {
      window.dispatchEvent(new Event(SESSION_EXPIRED_EVENT));
    }
  } catch {
    // Offline or storage blocked: keep the current state and let the next request retry.
  } finally {
    checking = false;
  }
}
