"""Cliente camaraonline — ÁGORA #72 / ADR-0017.

camaraonline (camaraonline.org) é um vendor privado que hospeda portais de câmaras municipais.
Ao contrário do SAPL, NÃO expõe API: os dados de vereadores vivem no HTML público das páginas de
transparência. Cada portal roda no domínio próprio da câmara (convenção `camara<slug>.<uf>.gov.br`)
ou hospedado em `camaraonline.org/cm_<slug>/`. A assinatura de fingerprint é o link
`camaraonline.org/cm_<slug>` presente no HTML de toda página do portal.

Existem duas gerações de template:
  - "moderno" (ex.: Santana de Parnaíba): e-mail INSTITUCIONAL em texto plano no perfil.
  - "legado"  (ex.: Caieiras): e-mail ofuscado pelo Cloudflare email-protection.

Só extraímos e-mail em TEXTO PLANO. NÃO decodificamos a ofuscação do Cloudflare — é um sinal
anti-scraping explícito e o ADR-0017 manda respeitar ToS/robots. Perfis ofuscados entram no roster
(nome/partido/foto) mas sem e-mail — logo, simplesmente não são enriquecíveis.

Só stdlib (urllib), mesmo padrão do sapl_client. Este módulo NÃO toca o banco. Só contato
INSTITUCIONAL público (transparência), nunca pessoal.
"""

from __future__ import annotations

import html
import re
import unicodedata
import urllib.request
from typing import Optional

# Reusa a dataclass de roster do cliente SAPL — mesma forma para todo o pipeline cívico.
from sapl_client import Parlamentar

TIMEOUT = 12
UA = "democracia.social.br civic-extractor (ADR-0017; contato institucional público)"

# Assinatura de fingerprint: link do vendor no HTML. Captura também o slug `cm_<slug>`.
SIGNATURE_RE = re.compile(r"camaraonline\.org/(cm_[a-z0-9_]+)", re.IGNORECASE)

# Templates de rota do detalhe de vereador vistos ao vivo:
#   moderno: /vereador/<id>/<slug>          (Santana de Parnaíba)
#   legado:  /vereadores/<id>/biografia     (Caieiras)
DETAIL_HREF_RE = re.compile(
    r'href="([^"]*?/vereador(?:es)?/\d+/[^"?#]+)"', re.IGNORECASE
)

EMAIL_RE = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}")


def normalize_slug(municipio: str) -> str:
    """'Santana de Parnaíba' → 'santanadeparnaiba' (sem acento/espaço)."""
    s = unicodedata.normalize("NFKD", municipio)
    s = "".join(c for c in s if not unicodedata.combining(c)).lower()
    return "".join(c for c in s if c.isalnum())


def candidate_bases(municipio: str, uf: str) -> list[str]:
    """URLs-base candidatas para uma câmara camaraonline, em ordem de probabilidade.

    Convenção comprovada ao vivo: `camara<slug>.<uf>.gov.br` (Santana de Parnaíba, Caieiras).
    """
    slug = normalize_slug(municipio)
    uf = uf.lower()
    return [
        f"https://www.camara{slug}.{uf}.gov.br",
        f"https://camara{slug}.{uf}.gov.br",
        f"https://camaraonline.org/cm_{slug}",
    ]


def _get_html(url: str) -> Optional[str]:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        if resp.status != 200:
            return None
        body = resp.read().decode("utf-8", "replace")
        # A página placeholder do vendor tem ~64 bytes ("Teste") — não é portal real.
        if len(body) < 512:
            return None
        return body


def _signature_slug(body: str) -> Optional[str]:
    m = SIGNATURE_RE.search(body or "")
    return m.group(1).lower() if m else None


def is_camaraonline(base_url: str) -> tuple[bool, Optional[int]]:
    """Fingerprint: `base` é um portal camaraonline? Retorna (é_camaraonline, n_vereadores_ou_None).

    Confirma pela assinatura `camaraonline.org/cm_<slug>` no HTML da listagem `/vereadores`
    (com fallback para a home). A contagem vem dos links de detalhe encontrados na listagem.
    """
    for path in ("/vereadores", "/", ""):
        try:
            body = _get_html(f"{base_url}{path}")
        except Exception:
            continue
        if not body:
            continue
        if _signature_slug(body):
            n = len(_detail_urls(base_url, body)) or None
            return (True, n)
    return (False, None)


def _detail_urls(base_url: str, listing_html: str) -> list[str]:
    """Extrai URLs de detalhe de vereador (dedupe, absolutas) da página de listagem."""
    out: list[str] = []
    seen: set[str] = set()
    for href in DETAIL_HREF_RE.findall(listing_html):
        href = html.unescape(href.strip())
        if href.startswith("http"):
            url = href
        elif href.startswith("/"):
            url = base_url + href
        else:
            url = f"{base_url}/{href}"
        # Normaliza o ':443' redundante que alguns templates emitem nos links.
        url = url.replace(":443/", "/")
        if url not in seen:
            seen.add(url)
            out.append(url)
    return out


def _strip_tags(s: str) -> str:
    return re.sub(r"<[^>]+>", " ", s or "")


def _clean(s: str) -> str:
    return re.sub(r"\s+", " ", html.unescape(_strip_tags(s))).strip()


def _field_after_label(body: str, label: str) -> Optional[str]:
    """Valor após `<span>Label:</span> valor` ou `<h6>Label: valor</h6>` (até <br>/</...>)."""
    # Template moderno: <span>Nome:</span> valor<br>
    m = re.search(
        rf"<span>\s*{label}\s*:?\s*</span>(.*?)(?:<br|</p>|<span>)",
        body, re.IGNORECASE | re.DOTALL,
    )
    if not m:
        # Template legado: <h6>Label: valor</h6>
        m = re.search(rf"<h6>\s*{label}\s*:?\s*(.*?)</h6>", body, re.IGNORECASE | re.DOTALL)
    if not m:
        return None
    val = _clean(m.group(1))
    return val or None


def _parse_detail(base_url: str, url: str, body: str) -> Optional[Parlamentar]:
    """Extrai um Parlamentar de uma página de detalhe. E-mail: só texto plano institucional-elegível."""
    # Nome: prefere o nome legal do campo "Nome:" (casa melhor com o roster TSE); cai no nome de
    # tratamento do título (`title_vereador`) quando não houver campo "Nome:".
    nome = _field_after_label(body, "Nome")
    if not nome:
        m = re.search(r'class="title_vereador"[^>]*>(.*?)</', body, re.IGNORECASE | re.DOTALL)
        if m:
            nome = _clean(m.group(1))
    if not nome:
        # Template legado (Caieiras): nome no <h4> do card (o <h1> é só o título "Vereadores").
        for m in re.finditer(r"<h[34][^>]*>(.*?)</h[34]>", body, re.IGNORECASE | re.DOTALL):
            cand = _clean(m.group(1))
            if cand and not _is_generic_heading(cand):
                nome = cand
                break
    if not nome or _is_generic_heading(nome):
        return None

    partido = _field_after_label(body, "Partido")

    # E-mail: só os em TEXTO PLANO (ofuscação Cloudflare fica intacta e some no regex → sem e-mail).
    # Pode haver 2 (pessoal + institucional); guardamos o 1º institucional-elegível, senão o 1º.
    emails = EMAIL_RE.findall(body)
    email = None
    if emails:
        inst = [e for e in emails if _looks_institutional(e)]
        email = (inst[0] if inst else emails[0]).strip()

    # Foto: <img ... class="img-fluid"> no bloco de perfil, ou 1ª imagem com 'vereador' no caminho.
    foto = None
    m = re.search(r'<img[^>]+src="([^"]+)"[^>]*class="img-fluid"', body, re.IGNORECASE)
    if not m:
        m = re.search(r'<img[^>]+src="([^"]*vereador[^"]*)"', body, re.IGNORECASE)
    if m:
        foto = html.unescape(m.group(1)).replace(":443/", "/")

    ext_id = url.rstrip("/").rsplit("/vereador", 1)[-1]  # sufixo estável (…/<id>/<slug>)
    return Parlamentar(
        external_id=f"camaraonline:{ext_id}",
        nome=nome,
        email=email,
        telefone=None,
        foto_url=foto,
        sexo=None,
        ativo=True,
        partido=partido,
        raw={"detail_url": url},
    )


_GENERIC_HEADINGS = {"vereador", "vereadores", "vereadora", "vereadoras", "camara", "camara municipal"}


def _is_generic_heading(s: str) -> bool:
    """True se `s` é um rótulo de seção genérico (ex.: 'Vereadores'), não um nome de pessoa."""
    n = unicodedata.normalize("NFKD", s or "")
    n = "".join(c for c in n if not unicodedata.combining(c)).lower().strip()
    return n in _GENERIC_HEADINGS


def _looks_institutional(email: str) -> bool:
    """Heurística local (o extrator revalida com is_institutional): domínio de câmara/gov/leg."""
    dom = email.rsplit("@", 1)[-1].lower()
    return dom.endswith(".gov.br") or dom.endswith(".leg.br") or "camara" in dom or dom.startswith("cm")


def fetch_current_parlamentares(base_url: str) -> list[Parlamentar]:
    """Roster VIGENTE de uma câmara camaraonline: listagem `/vereadores` → páginas de detalhe.

    A listagem mostra a legislatura atual (o portal expõe os vereadores em exercício). Cada link
    de detalhe é buscado e parseado. Falhas por perfil são ignoradas (não abortam o lote).
    """
    listing = None
    for path in ("/vereadores", "/vereadores/", "/"):
        try:
            listing = _get_html(f"{base_url}{path}")
        except Exception:
            listing = None
        if listing and _detail_urls(base_url, listing):
            break
    if not listing:
        return []

    out: list[Parlamentar] = []
    for url in _detail_urls(base_url, listing):
        try:
            body = _get_html(url)
        except Exception:
            continue
        if not body:
            continue
        p = _parse_detail(base_url, url, body)
        if p:
            out.append(p)
    return out
