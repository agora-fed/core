// Minimal, dependency-free HTML sanitizer for fediverse note content.
//
// `content_html` comes from REMOTE instances and must be treated as hostile even if the
// backend claims to sanitize — defense in depth before any `{@html}` render. Strategy:
// parse with DOMParser (inert document, scripts never execute) and REBUILD a clean tree
// keeping only a small whitelist of tags. No attribute is ever copied — `href` on <a> is
// re-emitted only after validating the protocol, alongside forced rel/target.

const ALLOWED = new Set(['P', 'BR', 'A', 'SPAN', 'STRONG', 'EM']);

/** Tags whose CONTENT must be dropped too (not just unwrapped). */
const DROP = new Set([
  'SCRIPT',
  'STYLE',
  'IFRAME',
  'OBJECT',
  'EMBED',
  'SVG',
  'MATH',
  'TEMPLATE',
  'LINK',
  'META',
  'TITLE',
  'HEAD',
  'BASE',
  'NOSCRIPT',
]);

const SAFE_HREF = /^https?:\/\//i;

function sanitizeNode(node: Node, doc: Document): Node | null {
  if (node.nodeType === Node.TEXT_NODE) {
    return doc.createTextNode(node.textContent ?? '');
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return null; // comments, CDATA, PIs…

  const el = node as Element;
  const tag = el.tagName.toUpperCase();
  if (DROP.has(tag)) return null;

  if (!ALLOWED.has(tag)) {
    // Unknown tag: unwrap — keep the (sanitized) children, lose the wrapper.
    const frag = doc.createDocumentFragment();
    for (const child of Array.from(el.childNodes)) {
      const clean = sanitizeNode(child, doc);
      if (clean) frag.appendChild(clean);
    }
    return frag;
  }

  // Allowed tag: fresh element, zero attributes copied.
  const out = doc.createElement(tag.toLowerCase());
  if (tag === 'A') {
    const href = el.getAttribute('href')?.trim() ?? '';
    if (SAFE_HREF.test(href)) {
      out.setAttribute('href', href);
      out.setAttribute('rel', 'nofollow noopener noreferrer');
      out.setAttribute('target', '_blank');
    }
  }
  for (const child of Array.from(el.childNodes)) {
    const clean = sanitizeNode(child, doc);
    if (clean) out.appendChild(clean);
  }
  return out;
}

/** Sanitize untrusted note HTML down to `p, br, a, span, strong, em` with vetted hrefs.
 *  Outside the browser (SSR/SSG pass of an island) falls back to stripping every tag. */
export function sanitizeNoteHtml(html: string): string {
  if (!html) return '';
  if (typeof DOMParser === 'undefined') {
    return html.replace(/<[^>]*>/g, ' ').replace(/\s+/g, ' ').trim();
  }
  const doc = new DOMParser().parseFromString(html, 'text/html');
  const clean = doc.createElement('div');
  for (const child of Array.from(doc.body.childNodes)) {
    const s = sanitizeNode(child, doc);
    if (s) clean.appendChild(s);
  }
  return clean.innerHTML;
}
