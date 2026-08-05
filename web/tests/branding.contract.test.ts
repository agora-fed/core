// Branding front-end ↔ backend contract. Guards the exact class of bug that
// broke /admin/aparencia in production (2026-08-05): the island treated the
// ApiResponse envelope ({success, error: {code, message}}) as the Fetched
// shape ({ok, error: string}) — always falsy — and showed the failure banner
// even though the API answered perfectly.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { adminGetBranding, adminPutBranding, type BrandingDto } from '../src/lib/api';
import { brandingLoadState } from '../src/lib/branding';
import type { ApiResponse } from '../src/lib/types';

const DTO: BrandingDto = {
  site_name: 'Pindorama',
  tagline: null,
  logo_url: '/media/logo.png',
  favicon_url: null,
  colors: { accent: '#22c55e' },
};

const envelope = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });

afterEach(() => vi.unstubAllGlobals());

describe('branding envelope handling (the production regression)', () => {
  it('a successful envelope loads the DTO', () => {
    const res: ApiResponse<BrandingDto> = {
      success: true,
      data: DTO,
      error: null,
      meta: null,
    };
    expect(brandingLoadState(res)).toEqual({ loaded: DTO });
  });

  it('a failing envelope surfaces the SERVER message, not a generic one', () => {
    const res: ApiResponse<BrandingDto> = {
      success: false,
      data: null,
      error: { code: 'unauthorized', message: 'Autenticação necessária.' },
      meta: null,
    };
    expect(brandingLoadState(res)).toEqual({ failed: 'Autenticação necessária.' });
  });

  it('success WITHOUT data still fails safely (defensive)', () => {
    const res: ApiResponse<BrandingDto> = {
      success: true,
      data: null,
      error: null,
      meta: null,
    };
    const state = brandingLoadState(res);
    expect('failed' in state).toBe(true);
  });
});

describe('branding API calls (wire contract)', () => {
  it('adminGetBranding parses the ApiResponse envelope', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        envelope({ success: true, data: DTO, error: null, meta: null }),
      ),
    );
    const res = await adminGetBranding();
    expect(res.success).toBe(true);
    expect(res.data?.site_name).toBe('Pindorama');
    expect(res.data?.colors.accent).toBe('#22c55e');
  });

  it('adminPutBranding sends the full DTO as JSON and reads the envelope', async () => {
    let sent: Record<string, unknown> | null = null;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: string, init: RequestInit) => {
        sent = JSON.parse(init.body as string);
        return envelope({ success: true, data: DTO, error: null, meta: null });
      }),
    );
    const res = await adminPutBranding(DTO);
    expect(sent).toMatchObject({ site_name: 'Pindorama', colors: { accent: '#22c55e' } });
    expect(res.success).toBe(true);
  });

  it('a 401 envelope comes back as success=false with the server message', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () =>
        envelope(
          {
            success: false,
            data: null,
            error: { code: 'unauthorized', message: 'Autenticação necessária.' },
            meta: null,
          },
          401,
        ),
      ),
    );
    const res = await adminGetBranding();
    expect(res.success).toBe(false);
    expect(res.error?.message).toBe('Autenticação necessária.');
  });
});
