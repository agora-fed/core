#!/usr/bin/env python3
"""
Seed real parliamentarians from Câmara dos Deputados + Senado Federal into the
`mandate` table — restricted to a left-bloc party set (PT, PCdoB, PSOL, PV, REDE).
Designed for the DemocraciaBR sovereign VM (ADR-0010 W2 + the W2.4 "real parliament" cut).

Pipeline:
  1. Fetch Câmara/v2/deputados per requested party (server-side filter).
  2. Fetch Senado/dadosabertos/senador/lista/atual once; filter client-side.
  3. Download each official photo to /tmp/dsoc-fotos/<external_id>.jpg.
  4. Emit a SQL UPSERT (ON CONFLICT on the natural key source+source_external_id)
     into /tmp/dsoc-mandates.sql.
  5. The caller (operator) scp's the photos to the VM and runs `mc cp` into MinIO
     under `mandates/<external_id>/avatar.jpg`, then applies the SQL with psql.

Idempotent: re-runs are safe because the SQL is UPSERT and the photo keys are
content-stable (named by external_id).

The org id is fixed to the seeded DemocraciaBR tenant
(`11111111-1111-1111-1111-111111111111`); this matches the seed in production.
"""

from __future__ import annotations

import concurrent.futures
import json
import os
import sys
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Optional
import urllib.request
import urllib.parse

ORG_ID = "11111111-1111-1111-1111-111111111111"
# Cobertura TOTAL: todos os deputados federais + senadores em exercício, de TODOS os partidos.
# (Antes o seed era restrito ao bloco de esquerda; o usuário pediu "todos os partidos".)
# `None` = sem filtro de partido.
PARTIES_WANTED: Optional[set[str]] = None

PHOTOS_DIR = Path("/tmp/dsoc-fotos")
SQL_OUT = Path("/tmp/dsoc-mandates.sql")
USER_AGENT = "DemocraciaBR/0.5 seed (+https://democracia.social.br)"


@dataclass
class Parlamentar:
    source: str  # 'camara' | 'senado'
    external_id: str
    name: str
    party: str
    uf: str
    house: str  # 'camara' | 'senado'
    photo_url: Optional[str]
    public_email: Optional[str]

    @property
    def office(self) -> str:
        if self.house == "camara":
            return f"Deputado(a) Federal — {self.uf}"
        return f"Senador(a) — {self.uf}"


def _get(url: str, timeout: int = 15) -> bytes:
    req = urllib.request.Request(url, headers={
        "User-Agent": USER_AGENT,
        "Accept": "application/json",
    })
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read()


def _get_image(url: str, timeout: int = 15) -> bytes:
    """Same as `_get` but advertises an image Accept — the Câmara host returns 406 otherwise."""
    req = urllib.request.Request(url, headers={
        "User-Agent": USER_AGENT,
        "Accept": "image/jpeg,image/*;q=0.9,*/*;q=0.5",
    })
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read()


def fetch_camara_all() -> list[Parlamentar]:
    """ALL sitting deputados, every party — paginated (the API caps ~100/page)."""
    out: list[Parlamentar] = []
    page = 1
    while True:
        qs = urllib.parse.urlencode(
            {"itens": 100, "pagina": page, "ordem": "ASC", "ordenarPor": "nome"}
        )
        payload = json.loads(_get(f"https://dadosabertos.camara.leg.br/api/v2/deputados?{qs}"))
        dados = payload.get("dados", [])
        if not dados:
            break
        for d in dados:
            out.append(Parlamentar(
                source="camara",
                external_id=str(d["id"]),
                name=d["nome"],
                party=d["siglaPartido"],
                uf=d["siglaUf"],
                house="camara",
                photo_url=d.get("urlFoto"),
                public_email=d.get("email"),
            ))
        # Stop when there is no `next` link (last page).
        has_next = any(l.get("rel") == "next" for l in payload.get("links", []))
        if not has_next:
            break
        page += 1
    return out


def fetch_senado_all() -> list[Parlamentar]:
    """Full list (server ignored ?siglaPartido); filter client-side."""
    payload = json.loads(_get("https://legis.senado.leg.br/dadosabertos/senador/lista/atual"))
    raw = payload.get("ListaParlamentarEmExercicio", {}).get("Parlamentares", {}).get("Parlamentar", [])
    if not isinstance(raw, list):
        raw = [raw]
    out = []
    for p in raw:
        ip = p.get("IdentificacaoParlamentar", {})
        party = ip.get("SiglaPartidoParlamentar")
        if not party:
            continue
        if PARTIES_WANTED is not None and party not in PARTIES_WANTED:
            continue
        # The Senado photo URL is http://... — rewrite to https.
        photo = ip.get("UrlFotoParlamentar")
        if photo and photo.startswith("http://"):
            photo = "https://" + photo[len("http://"):]
        out.append(Parlamentar(
            source="senado",
            external_id=str(ip.get("CodigoParlamentar")),
            name=ip.get("NomeParlamentar"),
            party=party,
            uf=ip.get("UfParlamentar"),
            house="senado",
            photo_url=photo,
            public_email=ip.get("EmailParlamentar"),
        ))
    return out


def _download_one(p: Parlamentar) -> tuple[str, Optional[str]]:
    if not p.photo_url:
        return p.external_id, None
    local = PHOTOS_DIR / f"{p.source}-{p.external_id}.jpg"
    if local.exists() and local.stat().st_size > 0:
        return p.external_id, str(local)
    try:
        data = _get_image(p.photo_url, timeout=20)
        local.write_bytes(data)
        return p.external_id, str(local)
    except Exception as e:
        print(f"  ✗ {p.name}: {e}", flush=True)
        return p.external_id, None


def download_photos(people: list[Parlamentar]) -> dict[str, Optional[str]]:
    """Parallel download (16 workers) — sequential timeouts on slow hosts dominated runtime."""
    PHOTOS_DIR.mkdir(parents=True, exist_ok=True)
    paths: dict[str, Optional[str]] = {}
    done = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=16) as ex:
        futures = {ex.submit(_download_one, p): p for p in people}
        for fut in concurrent.futures.as_completed(futures):
            ext_id, path = fut.result()
            paths[ext_id] = path
            done += 1
            if done % 20 == 0 or done == len(people):
                ok_count = sum(1 for v in paths.values() if v)
                print(f"  ... {done}/{len(people)} concluídas ({ok_count} OK)", flush=True)
    return paths


def emit_sql(people: list[Parlamentar], photo_paths: dict[str, Optional[str]]) -> None:
    """UPSERT on (source, source_external_id). One row per parlamentar."""
    SQL_OUT.parent.mkdir(parents=True, exist_ok=True)
    with SQL_OUT.open("w") as fh:
        fh.write("-- Seed real parliamentarians (Câmara + Senado, left-bloc parties).\n")
        fh.write("-- Idempotent via UNIQUE(source, source_external_id) — re-run safe.\n")
        fh.write("BEGIN;\n")
        for p in people:
            has_photo = photo_paths.get(p.external_id) is not None
            avatar_key = (
                f"'mandates/{p.source}-{p.external_id}/avatar.jpg'" if has_photo else "NULL"
            )
            email = p.public_email or f"{p.source}.{p.external_id}@parlamento.democracia.social.br"
            new_id = str(uuid.uuid4())
            # `display_name` keeps the original casing; `office` is computed.
            fh.write(
                "INSERT INTO mandate ("
                "id, org_id, office, display_name, public_email, is_candidate, "
                "created_at, party, uf, house, avatar_object_key, source, source_external_id"
                ") VALUES ("
                f"'{new_id}', '{ORG_ID}', "
                f"{sql_quote(p.office)}, {sql_quote(p.name)}, {sql_quote(email)}, "
                f"false, now(), "
                f"{sql_quote(p.party)}, {sql_quote(p.uf)}, {sql_quote(p.house)}, "
                f"{avatar_key}, {sql_quote(p.source)}, {sql_quote(p.external_id)}"
                ") "
                "ON CONFLICT (source, source_external_id) "
                "WHERE source IS NOT NULL AND source_external_id IS NOT NULL "
                "DO UPDATE SET "
                "office = EXCLUDED.office, display_name = EXCLUDED.display_name, "
                "party = EXCLUDED.party, uf = EXCLUDED.uf, "
                "avatar_object_key = COALESCE(EXCLUDED.avatar_object_key, mandate.avatar_object_key);"
                "\n"
            )
        fh.write("COMMIT;\n")
    print(f"\nWrote SQL → {SQL_OUT} ({SQL_OUT.stat().st_size//1024} KB)")


def sql_quote(s: Optional[str]) -> str:
    if s is None:
        return "NULL"
    return "'" + s.replace("'", "''") + "'"


def main() -> int:
    print("=== Câmara dos Deputados (todos os partidos) ===", flush=True)
    try:
        camara = fetch_camara_all()
    except Exception as e:
        print(f"  Câmara FALHOU ({e})", flush=True)
        camara = []
    cam_by_party: dict[str, int] = {}
    for p in camara:
        cam_by_party[p.party] = cam_by_party.get(p.party, 0) + 1
    for k, v in sorted(cam_by_party.items()):
        print(f"  {k}: {v}", flush=True)

    print("\n=== Senado Federal ===", flush=True)
    try:
        senado = fetch_senado_all()
    except Exception as e:
        print(f"  Senado FALHOU ({e}) — seguindo só com Câmara", flush=True)
        senado = []
    by_party: dict[str, int] = {}
    for p in senado:
        by_party[p.party] = by_party.get(p.party, 0) + 1
    for k, v in sorted(by_party.items()):
        print(f"  {k}: {v}")

    everyone = camara + senado
    print(f"\nTotal: {len(everyone)} parlamentares\n")

    print("Baixando fotos…")
    paths = download_photos(everyone)
    print(f"Fotos baixadas: {sum(1 for v in paths.values() if v)}/{len(everyone)}")

    emit_sql(everyone, paths)
    print("\nPróximos passos (no operador):")
    print(f"  1. scp -r {PHOTOS_DIR} popsolutions@[2804:710:d0:9::a000]:/tmp/")
    print(f"  2. scp {SQL_OUT} popsolutions@[2804:710:d0:9::a000]:/tmp/")
    print("  3. ssh popsolutions@[...] (na VM):")
    print("       for f in /tmp/dsoc-fotos/*.jpg; do")
    print("         id=$(basename \"$f\" .jpg)  # camara-12345 ou senado-67890")
    print("         mc cp \"$f\" \"dsoc/dsoc-media/mandates/$id/avatar.jpg\"")
    print("       done")
    print("       sudo -u postgres psql democracia_social -f /tmp/dsoc-mandates.sql")
    return 0


if __name__ == "__main__":
    sys.exit(main())
