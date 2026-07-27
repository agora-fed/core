"""Cliente SAPL (Interlegis) — ÁGORA #72 / ADR-0017.

SAPL é o software legislativo gratuito do Interlegis/Senado, rodado por 1.200+ câmaras municipais
em domínios `.leg.br`. Todas expõem a MESMA API REST (`/api/parlamentares/parlamentar/`), então
UM cliente cobre todas — o coração da estratégia "extrair por plataforma, não por município".

Só stdlib (urllib) — mesmo padrão dos outros `scripts/seed-*.py`, sem dependências novas. Este
módulo NÃO toca o banco: descobre/normaliza hosts e busca dados públicos. A ingestão (SQL) vive
nos scripts que o importam. Só contato INSTITUCIONAL público (transparência), nunca pessoal.
"""

from __future__ import annotations

import json
import unicodedata
import urllib.request
from dataclasses import dataclass, field
from typing import Optional

TIMEOUT = 10
UA = "democracia.social.br civic-extractor (ADR-0017; contato institucional público)"


def normalize_slug(municipio: str) -> str:
    """'Santana de Parnaíba' → 'santanadeparnaiba' (convenção Interlegis: sem acento/espaço)."""
    s = unicodedata.normalize("NFKD", municipio)
    s = "".join(c for c in s if not unicodedata.combining(c))
    s = s.lower()
    return "".join(c for c in s if c.isalnum())


def candidate_bases(municipio: str, uf: str) -> list[str]:
    """URLs-base candidatas a testar para uma câmara, em ordem de probabilidade (Interlegis)."""
    slug = normalize_slug(municipio)
    uf = uf.lower()
    return [
        f"https://sapl.{slug}.{uf}.leg.br",
        f"https://www.{slug}.{uf}.leg.br",
        f"https://{slug}.{uf}.leg.br",
        f"https://sapl.camara{slug}.{uf}.leg.br",
    ]


def _get_json(url: str) -> Optional[dict]:
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        if resp.status != 200:
            return None
        ctype = resp.headers.get("Content-Type", "")
        if "json" not in ctype:
            return None
        return json.loads(resp.read().decode("utf-8", "replace"))


def is_sapl(base_url: str) -> tuple[bool, Optional[int]]:
    """Fingerprint: `base` responde a API SAPL? Retorna (é_sapl, total_parlamentares_ou_None)."""
    try:
        data = _get_json(f"{base_url}/api/parlamentares/parlamentar/?format=json&limit=1")
    except Exception:
        return (False, None)
    if not isinstance(data, dict):
        return (False, None)
    # A API DRF do SAPL sempre traz 'results'; 'count' pode vir null em algumas versões.
    if "results" not in data:
        return (False, None)
    count = data.get("count")
    if not isinstance(count, int):
        count = None
    return (True, count)


@dataclass
class Parlamentar:
    external_id: str          # id no SAPL (estável dentro daquela câmara)
    nome: str                 # nome_parlamentar (preferido) ou nome_completo
    email: Optional[str]
    telefone: Optional[str]
    foto_url: Optional[str]
    sexo: Optional[str]       # 'M' | 'F' | None (mapeado a mandate.gender pelo chamador)
    ativo: Optional[bool]
    partido: Optional[str] = None  # preenchido via filiação/mandato pelo extrator
    raw: dict = field(default_factory=dict)


def _next_url(base_url: str, data: dict) -> Optional[str]:
    """Resolve a próxima página cobrindo os DOIS formatos SAPL: 'pagination' (antigo) e DRF."""
    pag = data.get("pagination") or {}
    links = pag.get("links") or {}
    nxt = links.get("next") or data.get("next")  # pagination.links.next (rel/abs) ou DRF next (abs)
    if not isinstance(nxt, str) or not nxt:
        return None
    return nxt if nxt.startswith("http") else base_url + nxt


def _iter(base_url: str, path: str):
    """Itera resultados de um endpoint SAPL seguindo a paginação (ambos os formatos)."""
    url = f"{base_url}{path}"
    seen_urls = set()
    while url and url not in seen_urls:
        seen_urls.add(url)
        data = _get_json(url)
        if not isinstance(data, dict):
            break
        for row in data.get("results") or []:
            yield row
        url = _next_url(base_url, data)


def _sigla_from_str(s: str) -> Optional[str]:
    """'Fulano - PDT - Partido...' → 'PDT' (formato estável do __str__ de filiação)."""
    parts = [p.strip() for p in (s or "").split(" - ")]
    return parts[1] if len(parts) >= 2 and parts[1] else None


def current_legislatura_id(base_url: str) -> Optional[int]:
    """Legislatura vigente: a marcada '(Atual)' no __str__ (fallback: intervalo de datas cobre hoje)."""
    best = None
    for row in _iter(base_url, "/api/parlamentares/legislatura/?format=json"):
        s = row.get("__str__") or ""
        if "(Atual)" in s or "(atual)" in s:
            return row.get("id")
        # fallback: guarda a de maior data_inicio (a mais recente).
        di = row.get("data_inicio") or ""
        if best is None or di > best[0]:
            best = (di, row.get("id"))
    return best[1] if best else None


def _filiacao_atual(base_url: str) -> dict[int, str]:
    """{parlamentar_id: sigla} da filiação vigente (data_desfiliacao IS NULL)."""
    out: dict[int, str] = {}
    for row in _iter(base_url, "/api/parlamentares/filiacao/?format=json"):
        if row.get("data_desfiliacao"):
            continue
        pid = row.get("parlamentar")
        sig = _sigla_from_str(row.get("__str__") or "")
        if pid is not None and sig:
            out[pid] = sig
    return out


def _row_to_parlamentar(row: dict, partido: Optional[str]) -> Optional[Parlamentar]:
    nome = (row.get("nome_parlamentar") or row.get("nome_completo") or "").strip()
    if not nome:
        return None
    return Parlamentar(
        external_id=str(row.get("id")),
        nome=nome,
        email=(row.get("email") or "").strip() or None,
        telefone=(row.get("telefone_celular") or row.get("telefone") or "").strip() or None,
        foto_url=(row.get("fotografia") or "").strip() or None,
        sexo=(row.get("sexo") or None),
        ativo=row.get("ativo"),
        partido=partido,
        raw=row,
    )


def fetch_current_parlamentares(base_url: str) -> list[Parlamentar]:
    """Roster VIGENTE de uma câmara SAPL, com partido.

    Autoritativo: cruza os `parlamentar` ids dos mandatos da legislatura atual com a lista de
    parlamentares. Fallback (câmaras que não mantêm mandato/legislatura): usa `ativo=True`.
    Partido vem da filiação vigente.
    """
    parl_by_id: dict[int, dict] = {}
    for row in _iter(base_url, "/api/parlamentares/parlamentar/?format=json"):
        if row.get("id") is not None:
            parl_by_id[row["id"]] = row

    filiacao = _filiacao_atual(base_url)

    current_ids: list[int] = []
    leg = current_legislatura_id(base_url)
    if leg is not None:
        for m in _iter(base_url, f"/api/parlamentares/mandato/?format=json&legislatura={leg}"):
            pid = m.get("parlamentar")
            if pid is not None:
                current_ids.append(pid)

    if not current_ids:
        # Fallback: quem está marcado ativo.
        current_ids = [pid for pid, r in parl_by_id.items() if r.get("ativo")]

    out: list[Parlamentar] = []
    seen = set()
    for pid in current_ids:
        if pid in seen or pid not in parl_by_id:
            continue
        seen.add(pid)
        p = _row_to_parlamentar(parl_by_id[pid], filiacao.get(pid))
        if p:
            out.append(p)
    return out


# --- Atividade legislativa (#73, ADR-0018) ---------------------------------------------------------


def _iter_capped(base_url: str, path: str, max_pages: int):
    """Como _iter, mas para após `max_pages` páginas (proteção — matérias têm centenas de milhares)."""
    url = f"{base_url}{path}"
    seen_urls = set()
    pages = 0
    while url and url not in seen_urls and pages < max_pages:
        seen_urls.add(url)
        pages += 1
        data = _get_json(url)
        if not isinstance(data, dict):
            break
        for row in data.get("results") or []:
            yield row
        url = _next_url(base_url, data)


def fetch_materias(base_url: str, gte_date: str, max_pages: int = 50) -> list[dict]:
    """Proposições/atas com data_apresentacao >= gte_date ('YYYY-MM-DD'). Filtro SERVER-SIDE + cap."""
    path = (f"/api/materia/materialegislativa/?format=json"
            f"&data_apresentacao__gte={gte_date}&o=-data_apresentacao")
    return list(_iter_capped(base_url, path, max_pages))


def fetch_autoria_de(base_url: str, materia_id) -> list[dict]:
    """Autoria de UMA matéria (filtro ?materia=). [] quando a matéria não tem autor (ex.: atas)."""
    out = []
    for row in _iter(base_url, f"/api/materia/autoria/?format=json&materia={materia_id}"):
        out.append({"autor_id": row.get("autor"), "primeiro": bool(row.get("primeiro_autor")),
                    "str": row.get("__str__") or ""})
    return out


def fetch_votacoes(base_url: str, gte_date: str, max_pages: int = 50) -> list[dict]:
    """Votações (ordemdia) com data_ordem >= gte_date ('YYYY-MM-DD'). Filtro SERVER-SIDE + cap."""
    path = f"/api/sessao/ordemdia/?format=json&data_ordem__gte={gte_date}&o=-data_ordem"
    return list(_iter_capped(base_url, path, max_pages))
