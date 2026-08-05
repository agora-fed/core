/**
 * Markdown seguro dos fóruns — subconjunto útil, XSS-safe POR CONSTRUÇÃO:
 * todo o texto é HTML-escapado ANTES de qualquer regra; as regras só inserem
 * tags de uma lista fixa (strong/em/code/pre/a/ul/ol/li/h3/h4/blockquote/p).
 * Nenhum HTML do usuário passa; links só http(s), sempre rel="noopener".
 *
 * Suporta: **negrito**, *itálico*, `código`, ```bloco```, [texto](url),
 * # títulos (viram h3/h4), - listas, 1. listas numeradas, > citação.
 */

function escapeHtml(s: string): string {
  return s
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

/**
 * Autolink bare `https://` URLs (issue #30 — the SOCRATES "Ideia original"
 * attribution is a bare URL). Runs AFTER the other inline rules and SKIPS
 * segments already inside `<a>…</a>` or `<code>…</code>` so nothing is
 * double-linked. Policy: https only (stricter than the explicit-markdown
 * rule); trailing punctuation/escaped quotes stay outside the link. XSS-safe
 * by construction: the text was HTML-escaped before any rule, so the URL can
 * never break out of the href attribute.
 */
function autolink(html: string): string {
  return html
    .split(/(<a [^>]*>.*?<\/a>|<code>.*?<\/code>)/g)
    .map((segment, i) => {
      if (i % 2 === 1) return segment; // already-rendered link/code span
      return segment.replace(/(^|[\s(])(https:\/\/[^\s<]+)/g, (_m, pre, url) => {
        const clean = url.replace(/(?:[.,;:!?)\]]|&quot;|&#39;)+$/, '');
        const rest = url.slice(clean.length);
        return `${pre}<a href="${clean}" target="_blank" rel="noopener nofollow">${clean}</a>${rest}`;
      });
    })
    .join('');
}

function inline(s: string): string {
  return autolink(
    s
      // código inline primeiro (protege o conteúdo das demais regras)
      .replace(/`([^`]+)`/g, '<code>$1</code>')
      // [texto](url) — só http(s)
      .replace(
        /\[([^\]]+)\]\((https?:\/\/[^\s)]+)\)/g,
        '<a href="$2" target="_blank" rel="noopener nofollow">$1</a>',
      )
      .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
      .replace(/(^|\W)\*([^*\n]+)\*(?=\W|$)/g, '$1<em>$2</em>'),
  );
}

/** Converte Markdown (subconjunto) em HTML seguro para {@html}. */
export function mdToHtml(md: string): string {
  const escaped = escapeHtml(md.trim());
  const lines = escaped.split('\n');
  const out: string[] = [];
  let list: 'ul' | 'ol' | null = null;
  let inCode = false;

  const closeList = () => {
    if (list) {
      out.push(`</${list}>`);
      list = null;
    }
  };

  for (const raw of lines) {
    const line = raw.trimEnd();
    if (line.trim().startsWith('```')) {
      closeList();
      out.push(inCode ? '</code></pre>' : '<pre><code>');
      inCode = !inCode;
      continue;
    }
    if (inCode) {
      out.push(`${line}\n`);
      continue;
    }
    if (!line.trim()) {
      closeList();
      continue;
    }
    const h = /^(#{1,4})\s+(.*)$/.exec(line.trim());
    if (h) {
      closeList();
      const tag = h[1].length <= 2 ? 'h3' : 'h4';
      out.push(`<${tag}>${inline(h[2])}</${tag}>`);
      continue;
    }
    if (line.trim().startsWith('&gt;')) {
      closeList();
      out.push(`<blockquote>${inline(line.trim().slice(4).trim())}</blockquote>`);
      continue;
    }
    const li = /^[-*]\s+(.*)$/.exec(line.trim());
    if (li) {
      if (list !== 'ul') {
        closeList();
        out.push('<ul>');
        list = 'ul';
      }
      out.push(`<li>${inline(li[1])}</li>`);
      continue;
    }
    const oli = /^\d+[.)]\s+(.*)$/.exec(line.trim());
    if (oli) {
      if (list !== 'ol') {
        closeList();
        out.push('<ol>');
        list = 'ol';
      }
      out.push(`<li>${inline(oli[1])}</li>`);
      continue;
    }
    closeList();
    out.push(`<p>${inline(line.trim())}</p>`);
  }
  if (inCode) out.push('</code></pre>');
  closeList();
  return out.join('');
}

/** Slug de URL a partir do título (cosmético/SEO — o roteador ignora). */
export function titleSlug(title: string): string {
  return title
    .toLowerCase()
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '')
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 80);
}
