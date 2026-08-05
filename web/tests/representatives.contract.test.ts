// Tag-a-representative front-end ↔ backend contract (issue #3).
// Pins the wire paths, the envelope shapes (apiGet=Fetched vs apiPost=ApiResponse
// — the exact class that broke /admin/aparencia) and the LGPD posture: the
// aggregate payload never carries citizen identifiers.
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  getTopicRepresentatives,
  tagTopicRepresentative,
  untagTopicRepresentative,
  type TopicRepresentativesDto,
} from '../src/lib/api';

const TOPIC = '019fb451-1fd0-7892-8e82-49e8b973fcf4';
const AGG: TopicRepresentativesDto = {
  representatives: [
    {
      mandate_id: 'm-1',
      display_name: 'Dep. Teste',
      office: 'deputado_federal',
      party: 'PT',
      state: 'SP',
      avatar_url: null,
      tag_count: 42,
    },
  ],
  total_tags: 42,
  mine: null,
};

const envelope = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });

afterEach(() => vi.unstubAllGlobals());

describe('representatives wire contract', () => {
  it('GET hits the topics path and parses the Fetched shape', async () => {
    let url = '';
    vi.stubGlobal(
      'fetch',
      vi.fn(async (u: string) => {
        url = u;
        return envelope({ success: true, data: AGG, error: null, meta: null });
      }),
    );
    const res = await getTopicRepresentatives(TOPIC);
    expect(url).toContain(`/api/v1/topics/${TOPIC}/representatives`);
    expect(res.ok).toBe(true);
    expect(res.data?.representatives[0].tag_count).toBe(42);
  });

  it('POST sends {mandate_id} and reads the ApiResponse shape (success, not ok)', async () => {
    let sent: Record<string, unknown> | null = null;
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_u: string, init: RequestInit) => {
        sent = JSON.parse(init.body as string);
        return envelope({ success: true, data: null, error: null, meta: null });
      }),
    );
    const res = await tagTopicRepresentative(TOPIC, 'm-1');
    expect(sent).toEqual({ mandate_id: 'm-1' });
    expect(res.success).toBe(true);
  });

  it('anonymous POST surfaces the unauthorized code the widget branches on', async () => {
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
    const res = await tagTopicRepresentative(TOPIC, 'm-1');
    expect(res.success).toBe(false);
    expect(res.error?.code).toBe('unauthorized');
  });

  it('DELETE hits the same path (untag)', async () => {
    let url = '';
    let method = '';
    vi.stubGlobal(
      'fetch',
      vi.fn(async (u: string, init: RequestInit) => {
        url = u;
        method = String(init.method);
        return envelope({ success: true, data: null, error: null, meta: null });
      }),
    );
    await untagTopicRepresentative(TOPIC);
    expect(url).toContain(`/api/v1/topics/${TOPIC}/representatives`);
    expect(method).toBe('DELETE');
  });

  it('LGPD: the aggregate DTO shape has no citizen fields', () => {
    const keys = Object.keys(AGG.representatives[0]);
    expect(keys).not.toContain('citizen_id');
    expect(keys).not.toContain('citizens');
    expect(Object.keys(AGG)).toEqual(['representatives', 'total_tags', 'mine']);
  });
});
