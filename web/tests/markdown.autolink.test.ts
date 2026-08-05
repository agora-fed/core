// Autolink of bare URLs in forum markdown (issue #30). The SOCRATES mirror
// writes source attribution as a bare URL — it must become a clickable,
// XSS-safe link without double-linking explicit markdown links or code spans.
import { describe, expect, it } from 'vitest';
import { mdToHtml } from '../src/lib/markdown';

const SOCRATES_LINE =
  '📌 Ideia original: https://www12.senado.leg.br/ecidadania/visualizacaoideia?id=213133';

describe('bare-URL autolink (issue #30)', () => {
  it('links the SOCRATES source URL with the safe rel/target', () => {
    const html = mdToHtml(SOCRATES_LINE);
    expect(html).toContain(
      '<a href="https://www12.senado.leg.br/ecidadania/visualizacaoideia?id=213133" ' +
        'target="_blank" rel="noopener nofollow">',
    );
  });

  it('policy: https only — http and javascript schemes are NOT autolinked', () => {
    expect(mdToHtml('veja http://inseguro.example/x')).not.toContain('<a ');
    expect(mdToHtml('veja javascript:alert(1)')).not.toContain('<a ');
  });

  it('does not double-link a URL inside an explicit [text](url) link', () => {
    const html = mdToHtml('[Ideia original](https://senado.leg.br/x)');
    expect(html.match(/<a /g)?.length).toBe(1);
    expect(html).toContain('>Ideia original</a>');
  });

  it('does not link URLs inside inline code', () => {
    const html = mdToHtml('rode `curl https://api.example/x` no terminal');
    expect(html).not.toContain('<a ');
    expect(html).toContain('<code>curl https://api.example/x</code>');
  });

  it('leaves trailing punctuation outside the link', () => {
    const html = mdToHtml('fonte: https://senado.leg.br/x.');
    expect(html).toContain('<a href="https://senado.leg.br/x"');
    expect(html).toContain('</a>.');
  });

  it('security: a hostile URL cannot break out of the href attribute', () => {
    const html = mdToHtml('https://evil.example/"><script>alert(1)</script>');
    expect(html).not.toContain('<script');
    // The quote arrived pre-escaped (&quot;) so the attribute stays closed.
    expect(html).toContain('rel="noopener nofollow"');
  });

  it('a URL in the middle of a sentence keeps surrounding text intact', () => {
    const html = mdToHtml('antes https://a.example/b depois');
    expect(html).toContain('antes <a href="https://a.example/b"');
    expect(html).toContain('</a> depois');
  });
});
