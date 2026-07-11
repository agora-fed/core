#!/usr/bin/env python3
"""Eleitorado oficial TSE → tabela `electorate` (item 4: threshold dinâmico).

Baixa o `perfil_eleitorado_ATUAL.zip` dos dados abertos do TSE, agrega
`QT_ELEITORES_PERFIL` por (UF, município) e emite SQL idempotente de upsert —
mesmo padrão dos outros seeds (gerar arquivo, aplicar com psql). Além das
linhas por município, grava o total por UF (municipio NULL) e o total
nacional (uf='BR'). Re-executável a cada atualização do TSE.

Uso:
    python3 scripts/seed-eleitorado-tse.py --out /tmp/eleitorado.sql
    psql "$DATABASE_URL" -f /tmp/eleitorado.sql
"""

from __future__ import annotations

import argparse
import csv
import io
import sys
import urllib.request
import uuid
import zipfile
from collections import defaultdict
from pathlib import Path

TSE_URL = (
    "https://cdn.tse.jus.br/estatistica/sead/odsele/perfil_eleitorado/"
    "perfil_eleitorado_ATUAL.zip"
)


def q(s: str) -> str:
    return "'" + s.replace("'", "''") + "'"


def fetch_zip(cache: Path) -> Path:
    if cache.exists() and cache.stat().st_size > 0:
        print(f"usando cache {cache}", file=sys.stderr)
        return cache
    print(f"baixando {TSE_URL} (grande — minutos)", file=sys.stderr)
    tmp = cache.with_suffix(".part")
    urllib.request.urlretrieve(TSE_URL, tmp)
    tmp.rename(cache)
    return cache


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cache-dir", default="/tmp")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    zpath = fetch_zip(Path(args.cache_dir) / "perfil_eleitorado_ATUAL.zip")

    # (uf, municipio) → eleitores. Streaming: o CSV tem milhões de linhas
    # (uma por perfil demográfico por seção/município), só somamos.
    by_mun: dict[tuple[str, str], int] = defaultdict(int)
    with zipfile.ZipFile(zpath) as z:
        names = [n for n in z.namelist() if n.lower().endswith(".csv")]
        for name in names:
            with z.open(name) as raw:
                text = io.TextIOWrapper(raw, encoding="latin-1", newline="")
                for row in csv.DictReader(text, delimiter=";"):
                    uf = (row.get("SG_UF") or "").strip().upper()
                    mun = (row.get("NM_MUNICIPIO") or "").strip()
                    if len(uf) != 2 or uf == "ZZ" or not mun:
                        continue  # exterior/linhas malformadas ficam fora.
                    # ATUAL usa QT_ELEITORES; os arquivos por ano usam
                    # QT_ELEITORES_PERFIL. Aceitar ambos.
                    raw_n = row.get("QT_ELEITORES") or row.get("QT_ELEITORES_PERFIL") or "0"
                    try:
                        n = int(raw_n.strip())
                    except ValueError:
                        continue
                    by_mun[(uf, mun)] += n

    if not by_mun:
        print("nenhuma linha agregada — formato mudou?", file=sys.stderr)
        return 1

    by_uf: dict[str, int] = defaultdict(int)
    for (uf, _), n in by_mun.items():
        by_uf[uf] += n
    total_br = sum(by_uf.values())

    def upsert(uf: str, mun: str | None, voters: int) -> str:
        mun_sql = q(mun) if mun else "NULL"
        return (
            "INSERT INTO electorate (id, uf, municipio, voters, updated_at)"
            f" VALUES ('{uuid.uuid4()}', {q(uf)}, {mun_sql}, {voters}, now())"
            " ON CONFLICT (uf, COALESCE(municipio, ''))"
            " DO UPDATE SET voters = EXCLUDED.voters, updated_at = now();"
        )

    with Path(args.out).open("w") as out:
        print("BEGIN;", file=out)
        print(upsert("BR", None, total_br), file=out)
        for uf, n in sorted(by_uf.items()):
            print(upsert(uf, None, n), file=out)
        for (uf, mun), n in sorted(by_mun.items()):
            print(upsert(uf, mun, n), file=out)
        print("COMMIT;", file=out)

    print(
        f"{len(by_mun)} municípios, {len(by_uf)} UFs, total nacional {total_br}; SQL em {args.out}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
