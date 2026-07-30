#!/usr/bin/env python3
"""Cliente CESPRO — ÁGORA #72 / ADR-0017.

CESPRO (cespro.com.br) hospeda portais de câmaras municipais (forte no RS/Sul) com HTML
server-rendered — sem API, mas estrutura uniforme:
  - listagem: `{base}/vereadores/` com links `…/vereadores/<id>/` (id numérico OU slug);
  - detalhe:  `{base}/vereadores/<id>/` com rótulos `Nome:</span>`, `Partido:</span>`,
              `Email:</span>` (e-mail institucional POR vereador — o alvo).

Só stdlib. Devolve `Parlamentar` (mesmo dataclass do sapl_client) pra reusar o matcher do
extract. Fingerprint: homepage cita `cespro.com.br` E existe listagem `/vereadores/`.
"""

from __future__ import annotations

import re
import urllib.request
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from sapl_client import Parlamentar, normalize_slug  # noqa: E402

UA = "democracia.social.br civic-mapping (contato: /contato)"
TIMEOUT = 8
MAX_BYTES = 400_000

# Link do vereador: `…/vereadores/<id>[/<slug>]…` — id numérico (Iraí, seguido de
# `/nome-slug/`) OU slug direto (Ibiraiaras). O `/` final após o id exclui as fotos
# `vereadores/<hash>.jpg` (que têm `.` no lugar do `/`). Capturamos o 1º segmento.
_LINK_RE = re.compile(r'/vereadores/([0-9]+|[a-z][a-z0-9-]{2,})/', re.I)
# Detalhe: rótulo seguido de </span> e o valor até a próxima tag.
_NOME_RE = re.compile(r"Nome:\s*</span>\s*([^<]+)", re.I)
_PARTIDO_RE = re.compile(r"Partido:\s*</span>\s*([^<]+)", re.I)
_EMAIL_RE = re.compile(r"Email:\s*</span>\s*([a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,})", re.I)
_CESPRO_RE = re.compile(r"cespro\.com\.br", re.I)


def candidate_bases(municipio: str, uf: str) -> list[str]:
    """URLs-base candidatas (convenções de câmara), em ordem de probabilidade."""
    slug = normalize_slug(municipio)
    uf = uf.lower()
    return [
        f"https://www.camara{slug}.{uf}.gov.br",
        f"https://camara{slug}.{uf}.gov.br",
        f"https://www.{slug}.{uf}.leg.br",
        f"https://{slug}.{uf}.leg.br",
        f"https://www.cm{slug}.{uf}.gov.br",
        f"https://cm{slug}.{uf}.gov.br",
    ]


def _fetch(url: str) -> str | None:
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            if resp.status != 200:
                return None
            return resp.read(MAX_BYTES).decode("utf-8", "replace")
    except Exception:
        return None


def is_cespro(base_url: str) -> tuple[bool, int | None]:
    """(é_cespro, nº de vereadores na listagem ou None).

    Muitas câmaras citam `cespro.com.br` só pelo repositório de LEGISLAÇÃO enquanto
    rodam WordPress no resto (falso-positivo: `/vereadores/feed/`, "Vereadores
    Archive", slugs `ver-nome/`). O discriminador REAL é o TEMPLATE de detalhe do
    CESPRO — rótulos `Nome:</span>` e (quando há) `Email:</span>`. Só confirma se um
    detalhe de vereador tem essa estrutura."""
    base = base_url.rstrip("/")
    listing = _fetch(f"{base}/vereadores/")
    if not listing:
        return (False, None)
    ids = []
    seen = set()
    for m in _LINK_RE.finditer(listing):
        vid = m.group(1)
        if vid not in seen and vid.lower() != "feed":
            seen.add(vid)
            ids.append(vid)
    if not ids:
        return (False, None)
    # Confirma no 1º detalhe: template CESPRO tem `Nome:</span>`. WordPress não.
    detail = _fetch(f"{base}/vereadores/{ids[0]}/")
    if not detail or not _NOME_RE.search(detail):
        return (False, None)
    return (True, len(ids))


def fetch_current_vereadores(base_url: str) -> list[Parlamentar]:
    """Roster da câmara: 1 fetch da listagem + 1 por vereador. E-mail institucional
    fica no próprio detalhe. Sem paginação (câmaras pequenas cabem numa página)."""
    base = base_url.rstrip("/")
    listing = _fetch(f"{base}/vereadores/")
    if not listing:
        return []
    ids: list[str] = []
    seen = set()
    for m in _LINK_RE.finditer(listing):
        vid = m.group(1)
        if vid not in seen and vid.lower() != "feed":
            seen.add(vid)
            ids.append(vid)
    out: list[Parlamentar] = []
    for vid in ids:
        html = _fetch(f"{base}/vereadores/{vid}/")
        if not html:
            continue
        nome = _NOME_RE.search(html)
        if not nome:
            continue
        partido = _PARTIDO_RE.search(html)
        email = _EMAIL_RE.search(html)
        out.append(Parlamentar(
            external_id=vid,
            nome=nome.group(1).strip(),
            email=email.group(1).strip() if email else None,
            telefone=None,
            foto_url=None,
            sexo=None,
            ativo=True,
            partido=partido.group(1).strip() if partido else None,
            raw={},
        ))
    return out


if __name__ == "__main__":
    # Teste rápido: python3 cespro_client.py https://www.irai.rs.leg.br
    base = sys.argv[1] if len(sys.argv) > 1 else "https://www.irai.rs.leg.br"
    ok, n = is_cespro(base)
    print(f"is_cespro({base}) = {ok} ({n} vereadores)")
    for p in fetch_current_vereadores(base):
        print(f"  {p.nome:<30} {p.partido or '?':<16} {p.email or '—'}")
