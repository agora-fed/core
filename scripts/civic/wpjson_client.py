#!/usr/bin/env python3
"""Cliente WordPress wp-json — ÁGORA #72 / ADR-0017.

Muitas câmaras rodam WordPress com um custom post type `vereadores`/`vereador` exposto
pela REST API padrão do WordPress (`/wp-json/wp/v2/<cpt>`). O tema varia (Astra, OceanWP,
Elementor…), mas a REST é UNIFORME — um cliente serve pra todas. Nome vem em
`title.rendered`; partido/e-mail (quando existem) vêm no `content.rendered` (HTML de bio).

Realidade (amostra de 65, 2026-07-30): ~29% têm o CPT, mas só ~1,5% publicam e-mail
institucional — o WordPress guarda BIOGRAFIA, raramente CONTATO. Por isso este cliente é
best-effort: enriquece e-mail onde houver; o resto vira aviso de transparência no fórum.

Só stdlib. Devolve `Parlamentar` (dataclass do sapl_client) pra reusar o matcher do extract.
"""

from __future__ import annotations

import json
import re
import urllib.request
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from sapl_client import Parlamentar  # noqa: E402

UA = "democracia.social.br civic-mapping (contato: /contato)"
TIMEOUT = 8
MAX_BYTES = 800_000

_CPT_RE = re.compile(r"vere|parlam", re.I)
_EMAIL_RE = re.compile(r"[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}", re.I)
_PARTIDO_RE = re.compile(
    r"PARTIDO\s*:?\s*</?[^>]*>?\s*([A-Za-zÀ-ÿ.\s]{2,40})", re.I)


def _get(url: str) -> str | None:
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            if resp.status != 200:
                return None
            return resp.read(MAX_BYTES).decode("utf-8", "replace")
    except Exception:
        return None


def find_vereador_cpt(base_url: str) -> str | None:
    """Nome do custom post type de vereador exposto na REST, ou None."""
    raw = _get(f"{base_url.rstrip('/')}/wp-json/wp/v2/types")
    if not raw:
        return None
    try:
        types = json.loads(raw)
    except Exception:
        return None
    if not isinstance(types, dict):
        return None
    for key in types:
        if _CPT_RE.search(key):
            return key
    return None


def is_wpjson_vereador(base_url: str) -> tuple[bool, int | None]:
    """(tem CPT de vereador na REST, nº de registros ou None)."""
    cpt = find_vereador_cpt(base_url)
    if not cpt:
        return (False, None)
    raw = _get(f"{base_url.rstrip('/')}/wp-json/wp/v2/{cpt}?per_page=100")
    if not raw:
        return (False, None)
    try:
        items = json.loads(raw)
    except Exception:
        return (False, None)
    if not isinstance(items, list) or not items:
        return (False, None)
    return (True, len(items))


def _clean(s: str) -> str:
    return re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", s)).strip()


def fetch_current_vereadores(base_url: str) -> list[Parlamentar]:
    """Roster via REST do CPT. Nome = title; partido/e-mail extraídos do content HTML.
    Prefixos de tratamento ('Ver.', 'Vereador(a)') são removidos do nome."""
    cpt = find_vereador_cpt(base_url)
    if not cpt:
        return []
    raw = _get(f"{base_url.rstrip('/')}/wp-json/wp/v2/{cpt}?per_page=100")
    if not raw:
        return []
    try:
        items = json.loads(raw)
    except Exception:
        return []
    out: list[Parlamentar] = []
    for it in items if isinstance(items, list) else []:
        nome = _clean((it.get("title") or {}).get("rendered", ""))
        nome = re.sub(r"^(Ver\.?|Vereador[a]?|Ver[ºª])\s+", "", nome, flags=re.I).strip()
        if not nome:
            continue
        content = (it.get("content") or {}).get("rendered", "")
        em = _EMAIL_RE.search(content)
        pt = _PARTIDO_RE.search(content)
        partido = _clean(pt.group(1)) if pt else None
        if partido and len(partido) > 30:
            partido = None
        out.append(Parlamentar(
            external_id=str(it.get("id", "")),
            nome=nome,
            email=em.group(0).strip() if em else None,
            telefone=None,
            foto_url=None,
            sexo=None,
            ativo=True,
            partido=partido,
            raw={},
        ))
    return out


if __name__ == "__main__":
    base = sys.argv[1] if len(sys.argv) > 1 else "https://camaramucuri.ba.gov.br"
    ok, n = is_wpjson_vereador(base)
    print(f"is_wpjson_vereador({base}) = {ok} ({n} registros)")
    for p in fetch_current_vereadores(base):
        print(f"  {p.nome:<34} {p.partido or '?':<14} {p.email or '—'}")
