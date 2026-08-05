// Chapter page front-end ↔ backend contract (EN contract, ADR-0013).
// Same discipline as branding.contract.test.ts: pin the envelope, the URL
// the site links to, and the request path the helper hits.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { getChapter, type ChapterDto } from '../src/lib/api';
import { chapterUrl, partySlug } from '../src/lib/parties';

const CHAPTER: ChapterDto = {
  id: '0198b000-0000-7000-8000-000000000001',
  party_short_name: 'PT',
  party_name: 'Partido dos Trabalhadores',
  party_logo_url: null,
  level: 'municipal',
  state: 'SP',
  municipality: 'Ubatuba',
  name: 'Diretório Municipal — Ubatuba',
  parent_id: null,
  administrators: [
    { public_handle: 'dir-admin', display_name: 'Fulana', role: 'admin', directory_id: null },
  ],
};

afterEach(() => vi.unstubAllGlobals());

describe('chapter URL (what PartyDetail links to)', () => {
  it('builds the SSG runtime-entity URL from sigla + id', () => {
    expect(chapterUrl('PT', CHAPTER.id)).toBe(
      `/partidos/pt/diretorio/?id=${CHAPTER.id}`,
    );
  });

  it('slugifies accented/case siglas the same way the party page does', () => {
    expect(chapterUrl('PCdoB', 'x')).toBe('/partidos/pcdob/diretorio/?id=x');
    expect(partySlug('PCdoB')).toBe('pcdob');
  });
});

describe('getChapter wire contract', () => {
  it('hits the English chapters path and parses the envelope', async () => {
    let url = '';
    vi.stubGlobal(
      'fetch',
      vi.fn(async (u: string) => {
        url = u;
        return new Response(
          JSON.stringify({ success: true, data: CHAPTER, error: null, meta: null }),
          { status: 200, headers: { 'content-type': 'application/json' } },
        );
      }),
    );
    const res = await getChapter('PT', CHAPTER.id);
    expect(url).toContain(`/api/v1/parties/PT/chapters/${CHAPTER.id}`);
    expect(res.ok).toBe(true);
    expect(res.data?.level).toBe('municipal');
    expect(res.data?.administrators[0].public_handle).toBe('dir-admin');
  });

  it('a null data (unknown chapter) stays a clean miss, not an exception', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(
        async () =>
          new Response(
            JSON.stringify({ success: true, data: null, error: null, meta: null }),
            { status: 200, headers: { 'content-type': 'application/json' } },
          ),
      ),
    );
    const res = await getChapter('PT', 'does-not-exist');
    expect(res.ok).toBe(true);
    expect(res.data).toBeNull();
  });
});
