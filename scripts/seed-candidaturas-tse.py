#!/usr/bin/env python3
"""Pipeline de ingestão TSE DivulgaCand → election/candidacy (item 5 do plano).

Baixa o `consulta_cand_{ANO}.zip` dos dados abertos do TSE, percorre os CSVs
por UF (Latin-1, `;`) e emite um SQL idempotente — mesma mecânica dos seeds
existentes: gerar arquivo, aplicar com psql. Idempotência via
`candidacy.tse_sq` = SQ_CANDIDATO (migration 0520): o TSE republica os CSVs
diariamente na janela de registro (15/08 → eleição), então o mesmo comando
roda todo dia e só insere/atualiza o que mudou.

Uso (ensaio com o dataset 2022; em 15/08/2026 é o mesmo comando com 2026):
    python3 scripts/seed-candidaturas-tse.py --year 2022 --out /tmp/cand-2022.sql
    psql "$DATABASE_URL" -f /tmp/cand-2022.sql
"""

from __future__ import annotations

import argparse
import csv
import io
import sys
import urllib.request
import uuid
import zipfile
from pathlib import Path

DEFAULT_ORG = "11111111-1111-1111-1111-111111111111"
TSE_URL = "https://cdn.tse.jus.br/estatistica/sead/odsele/consulta_cand/consulta_cand_{year}.zip"

# DS_CARGO (upper) → (office do schema, sphere). Vices de chapa majoritária
# federal/estadual e suplentes de senador não disputam voto próprio — fora.
OFFICE_MAP = {
    "PRESIDENTE": ("presidente", "federal"),
    "SENADOR": ("senador", "federal"),
    "DEPUTADO FEDERAL": ("deputado_federal", "federal"),
    "GOVERNADOR": ("governador", "estadual"),
    "DEPUTADO ESTADUAL": ("deputado_estadual", "estadual"),
    "DEPUTADO DISTRITAL": ("deputado_estadual", "estadual"),
    "PREFEITO": ("prefeito", "municipal"),
    "VICE-PREFEITO": ("vice_prefeito", "municipal"),
    "VEREADOR": ("vereador", "municipal"),
}

GENDER_MAP = {"FEMININO": "mulher", "MASCULINO": "homem"}


def q(s: str) -> str:
    """SQL-quote a text value (single-quote doubling)."""
    return "'" + s.replace("'", "''") + "'"


def fetch_zip(year: int, cache: Path) -> Path:
    if cache.exists() and cache.stat().st_size > 0:
        print(f"usando cache {cache}", file=sys.stderr)
        return cache
    url = TSE_URL.format(year=year)
    print(f"baixando {url}", file=sys.stderr)
    tmp = cache.with_suffix(".part")
    urllib.request.urlretrieve(url, tmp)
    tmp.rename(cache)
    return cache


def iter_rows(zpath: Path):
    with zipfile.ZipFile(zpath) as z:
        names = [n for n in z.namelist() if n.lower().endswith(".csv")]
        # O zip traz consulta_cand_{ano}_{UF}.csv e, em alguns anos, um
        # BRASIL consolidado; usar só os por-UF pra não duplicar.
        per_uf = [n for n in names if "BRASIL" not in n.upper()]
        for name in per_uf or names:
            with z.open(name) as raw:
                text = io.TextIOWrapper(raw, encoding="latin-1", newline="")
                yield from csv.DictReader(text, delimiter=";")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--year", type=int, required=True)
    ap.add_argument("--org", default=DEFAULT_ORG)
    ap.add_argument("--cache-dir", default="/tmp")
    ap.add_argument("--out", required=True, help="arquivo SQL de saída")
    args = ap.parse_args()

    zpath = fetch_zip(args.year, Path(args.cache_dir) / f"consulta_cand_{args.year}.zip")

    # Passo 1 — coletar em memória (≈30k rows em ano geral; ≈500k municipal:
    # ainda cabe) pra poder emitir elections ANTES das candidacies (FK).
    elections: dict[tuple[int, str], str] = {}
    election_days: dict[tuple[int, str], str] = {}
    seen_sq: set[str] = set()
    cands: list[str] = []
    counts: dict[str, int] = {}
    skipped = 0

    for row in iter_rows(zpath):
        cargo = (row.get("DS_CARGO") or "").strip().upper()
        mapped = OFFICE_MAP.get(cargo)
        if not mapped:
            skipped += 1
            continue
        office, sphere = mapped
        sq = (row.get("SQ_CANDIDATO") or "").strip()
        if not sq or sq in seen_sq:
            # Sem SQ não há chave de upsert; duplicata entre arquivos idem.
            skipped += 1
            continue
        seen_sq.add(sq)
        try:
            rnd = int((row.get("NR_TURNO") or "1").strip())
        except ValueError:
            rnd = 1
        if rnd not in (1, 2):
            rnd = 1
        key = (rnd, sphere)
        if key not in elections:
            elections[key] = str(uuid.uuid4())
            dt = (row.get("DT_ELEICAO") or "").strip()  # dd/mm/aaaa
            if len(dt) == 10 and dt[2] == "/" and dt[5] == "/":
                election_days[key] = f"{dt[6:10]}-{dt[3:5]}-{dt[0:2]}"
            else:
                election_days[key] = f"{args.year}-10-04"
        number = (row.get("NR_CANDIDATO") or "").strip()
        name = (row.get("NM_URNA_CANDIDATO") or row.get("NM_CANDIDATO") or "").strip()
        party = (row.get("SG_PARTIDO") or "").strip()
        if not (number and name and party):
            skipped += 1
            continue
        uf = (row.get("SG_UF") or "").strip().upper()
        uf_sql = q(uf) if len(uf) == 2 and uf not in ("BR", "ZZ", "VT") else "NULL"
        municipio = (row.get("NM_UE") or "").strip() if sphere == "municipal" else ""
        gender = GENDER_MAP.get((row.get("DS_GENERO") or "").strip().upper())
        status = (row.get("DS_SIT_TOT_TURNO") or "").strip().lower()
        if not status or status.startswith("#"):
            status = (row.get("DS_DETALHE_SITUACAO_CAND") or "").strip().lower()
        counts[office] = counts.get(office, 0) + 1
        cands.append(
            "INSERT INTO candidacy (id, election_id, party_sigla, office, number,"
            " sphere_uf, sphere_municipio, candidate_name, candidate_gender, status, tse_sq)"
            f" VALUES ('{uuid.uuid4()}',"
            f" (SELECT id FROM election WHERE org_id = '{args.org}' AND year = {args.year}"
            f"   AND round = {rnd} AND sphere = {q(sphere)}),"
            f" {q(party)}, {q(office)}, {q(number)}, {uf_sql},"
            f" {q(municipio) if municipio else 'NULL'}, {q(name)},"
            f" {q(gender) if gender else 'NULL'}, {q(status) if status else 'NULL'}, {q(sq)})"
            " ON CONFLICT (tse_sq) WHERE tse_sq IS NOT NULL DO UPDATE SET"
            " party_sigla = EXCLUDED.party_sigla, number = EXCLUDED.number,"
            " candidate_name = EXCLUDED.candidate_name,"
            " candidate_gender = EXCLUDED.candidate_gender,"
            " status = EXCLUDED.status;"
        )

    # Passo 2 — arquivo único, elections antes das candidacies, uma tx.
    # As candidacies resolvem a election por subselect na chave natural
    # (org, year, round, sphere), então re-runs apontam pra row EXISTENTE
    # mesmo que o uuid gerado neste run seja descartado pelo ON CONFLICT.
    with Path(args.out).open("w") as out:
        print("BEGIN;", file=out)
        for (rnd, sphere), eid in sorted(elections.items()):
            day = election_days[(rnd, sphere)]
            print(
                "INSERT INTO election (id, org_id, year, round, sphere, election_day)"
                f" VALUES ('{eid}', '{args.org}', {args.year}, {rnd}, {q(sphere)}, '{day}')"
                " ON CONFLICT (org_id, year, round, sphere) DO NOTHING;",
                file=out,
            )
        for stmt in cands:
            print(stmt, file=out)
        print("COMMIT;", file=out)

    print(
        f"{sum(counts.values())} candidaturas ({counts}); {skipped} linhas puladas; SQL em {args.out}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
