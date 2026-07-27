#!/usr/bin/env python3
"""Extrair atividade legislativa SAPL → civic_proposal/author/vote (ÁGORA #73, ADR-0018).

Coleta, para as fontes SAPL do catálogo `civic_source`, as proposições/atas (matérias) e votações
(ordem do dia) a partir de uma data-piso, liga a autoria ao `mandate` (mesmo casamento nome+partido
do #72) e emite UPSERTs. Fundação da meta-análise cívica — a destilação NLP roda SOBRE estas tabelas.

Bounded por padrão (data-piso + cap de páginas). A ingestão nacional completa é operação custeada
(centenas de milhares de matérias por câmara) — este script prova o mecanismo num recorte.

Uso:
    python3 scripts/civic/sapl_activity.py --base https://sapl.campinas.sp.leg.br \
            --municipio Campinas --uf SP --since 2025-01-01 --max-pages 5   # 1 câmara (imprime)
    python3 scripts/civic/sapl_activity.py --db <url> --since 2025-01-01 [--apply]  # catálogo
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from extract_sapl import DEFAULT_ORG, load_mandates, name_similarity  # noqa: E402
from sapl_client import (  # noqa: E402
    fetch_autoria_de, fetch_materias, fetch_votacoes,
)

NAME_MATCH_THRESHOLD = 0.86


def q(s) -> str:
    return "NULL" if s is None else "'" + str(s).replace("'", "''") + "'"


def qint(s) -> str:
    return str(s) if isinstance(s, int) else "NULL"


def author_name(rec: dict) -> str:
    """Nome do autor a partir do __str__ ('Autoria: NOME - Matéria ...')."""
    s = rec.get("str") or ""
    if s.startswith("Autoria:"):
        s = s[len("Autoria:"):]
    return s.split(" - ")[0].strip()


def match_mandate(nome: str, mandates: list[dict]) -> str | None:
    best, best_id = 0.0, None
    for m in mandates:
        sc = name_similarity(nome, m["name"])
        if sc > best:
            best, best_id = sc, m["id"]
    return best_id if best >= NAME_MATCH_THRESHOLD else None


def collect_one(base: str, uf: str, municipio: str, since: str, max_pages: int, mandates: list[dict]):
    materias = fetch_materias(base, since, max_pages)
    votacoes = fetch_votacoes(base, since, max_pages)
    prop_sql, author_sql, vote_sql = [], [], []
    matched_authors = 0

    for m in materias:
        mid = m.get("id")
        prop_sql.append(
            "INSERT INTO civic_proposal (uf, municipio, source_base_url, external_id, numero, ano, "
            "tipo, ementa, data_apresentacao) VALUES ("
            f"{q(uf)}, {q(municipio)}, {q(base)}, {q(mid)}, {qint(m.get('numero'))}, "
            f"{qint(m.get('ano'))}, {q((m.get('__str__') or '').split(' nº')[0].strip() or None)}, "
            f"{q(m.get('ementa'))}, {q(m.get('data_apresentacao'))}) "
            "ON CONFLICT (source_base_url, external_id) DO UPDATE SET ementa=EXCLUDED.ementa;"
        )
        for a in fetch_autoria_de(base, mid):
            nome = author_name(a)
            if not nome:
                continue
            mand_id = match_mandate(nome, mandates)
            if mand_id:
                matched_authors += 1
            author_sql.append(
                "INSERT INTO civic_proposal_author (proposal_id, mandate_id, autor_external_id, "
                "autor_nome, primeiro_autor) SELECT p.id, "
                f"{q(mand_id)}, {q(a.get('autor_id'))}, {q(nome)}, {str(a['primeiro']).lower()} "
                f"FROM civic_proposal p WHERE p.source_base_url={q(base)} AND p.external_id={q(mid)} "
                "ON CONFLICT (proposal_id, autor_nome) DO UPDATE SET mandate_id=EXCLUDED.mandate_id;"
            )

    for v in votacoes:
        vote_sql.append(
            "INSERT INTO civic_vote (uf, municipio, source_base_url, external_id, data_ordem, "
            "resultado, tipo_votacao, materia_external_id) VALUES ("
            f"{q(uf)}, {q(municipio)}, {q(base)}, {q(v.get('id'))}, {q(v.get('data_ordem'))}, "
            f"{q(v.get('resultado'))}, {q(v.get('tipo_votacao'))}, {q(v.get('materia'))}) "
            "ON CONFLICT (source_base_url, external_id) DO UPDATE SET resultado=EXCLUDED.resultado;"
        )
    return materias, votacoes, matched_authors, prop_sql, author_sql, vote_sql


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db")
    ap.add_argument("--base"); ap.add_argument("--municipio"); ap.add_argument("--uf")
    ap.add_argument("--since", default="2025-01-01")
    ap.add_argument("--max-pages", type=int, default=20)
    ap.add_argument("--org", default=DEFAULT_ORG)
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()

    # Modo 1 câmara (imprime; sem banco → sem casamento de autoria).
    if args.base and not args.db:
        mats = fetch_materias(args.base, args.since, args.max_pages)
        vots = fetch_votacoes(args.base, args.since, args.max_pages)
        print(f"{args.base}: {len(mats)} matérias · {len(vots)} votações (desde {args.since})")
        for m in mats[:8]:
            print(f"  [{m.get('ano')}] {(m.get('ementa') or '')[:90]}")
        return 0

    if not args.db:
        print("--db obrigatório (ou --base para testar 1 câmara)", file=sys.stderr)
        return 2

    sql = ("SELECT uf, municipio, base_url FROM civic_source "
           "WHERE platform='sapl' AND probe_status='ok' AND base_url IS NOT NULL ORDER BY uf, municipio")
    out = subprocess.run(["psql", args.db, "-tA", "-F", "\t", "-c", sql],
                         capture_output=True, text=True, check=True).stdout
    sources = [tuple(l.split("\t")) for l in out.splitlines() if l.count("\t") == 2]

    all_sql, tot_p, tot_v, tot_a, tot_m = [], 0, 0, 0, 0
    for uf, municipio, base in sources:
        mandates = load_mandates(args.db, args.org, uf, municipio)
        try:
            mats, vots, matched, ps, as_, vs = collect_one(
                base, uf, municipio, args.since, args.max_pages, mandates)
        except Exception as e:
            print(f"  ! {municipio}/{uf}: {e}", flush=True)
            continue
        tot_p += len(mats); tot_v += len(vots); tot_a += len(as_); tot_m += matched
        print(f"  {municipio}/{uf}: {len(mats)} matérias · {len(vots)} votações · "
              f"{len(as_)} autorias ({matched} casadas ao mandato)", flush=True)
        all_sql += ps + as_ + vs

    print(f"\nTOTAL: {tot_p} proposições/atas · {tot_v} votações · {tot_a} autorias "
          f"({tot_m} casadas ao mandato)", flush=True)
    sql_path = Path("/tmp/civic-activity.sql")
    sql_path.write_text("BEGIN;\n" + "\n".join(all_sql) + "\nCOMMIT;\n" if all_sql else "-- vazio\n")
    print(f"SQL → {sql_path}", flush=True)
    if args.apply and all_sql:
        subprocess.run(["psql", args.db, "-f", str(sql_path)], check=True)
        print("APLICADO.", flush=True)
    elif all_sql:
        print("DRY-RUN (use --apply).", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
