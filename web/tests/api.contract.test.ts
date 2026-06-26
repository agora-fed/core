// Front-end ↔ backend CONTRACT tests. These guard the request shapes the front-end sends against
// what the backend DTOs require — exactly the class of bug that broke production registration:
// the form omitted `org_id`, so Axum's Json<RegisterRequest> returned a 422 text/plain that the
// client surfaced as a "connection failure". A test like this would have caught it before deploy.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { register, login } from '../src/lib/api';

let lastBody: Record<string, unknown> | null = null;

beforeEach(() => {
  lastBody = null;
  vi.stubGlobal(
    'fetch',
    vi.fn(async (_url: string, init: RequestInit) => {
      lastBody = JSON.parse(init.body as string);
      return new Response(
        JSON.stringify({
          success: true,
          data: { id: 'x', citizen_id: 'y', issued_at: '', expires_at: '', public_handle: '' },
          error: null,
          meta: null,
        }),
        { status: 200, headers: { 'content-type': 'application/json' } },
      );
    }),
  );
});
afterEach(() => vi.unstubAllGlobals());

describe('auth contract (front-end ↔ backend)', () => {
  it('register sends ALL RegisterRequest fields (org_id, email, password, cpf)', async () => {
    const res = await register('  Maria@Pop.Coop ', 'senha-bem-forte-2026', '52998224725');
    expect(res.success).toBe(true);
    // The field whose absence broke production:
    expect(lastBody).toHaveProperty('org_id');
    expect(String(lastBody!.org_id)).toMatch(/^[0-9a-f-]{36}$/);
    expect(lastBody).toHaveProperty('email');
    expect(lastBody).toHaveProperty('password');
    expect(lastBody).toHaveProperty('cpf');
    expect(String(lastBody!.email)).not.toMatch(/\s/); // trimmed
  });

  it('login sends ALL LoginRequest fields (org_id, email, password)', async () => {
    await login('maria@pop.coop', 'senha-bem-forte-2026');
    expect(lastBody).toHaveProperty('org_id');
    expect(lastBody).toHaveProperty('email');
    expect(lastBody).toHaveProperty('password');
  });

  it('a non-JSON 4xx (e.g. 422 text/plain) is surfaced as an error, NOT a connection failure', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response('missing field `org_id`', { status: 422, headers: { 'content-type': 'text/plain' } })),
    );
    const res = await register('a@b.co', 'senha-bem-forte-2026', '52998224725');
    expect(res.success).toBe(false);
    expect(res.error?.code).toBe('http_422'); // must NOT be 'network_error'
  });
});
