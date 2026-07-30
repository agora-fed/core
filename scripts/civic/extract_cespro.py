#!/usr/bin/env python3
"""Extrair + casar contatos de vereadores CESPRO → enriquecer `mandate` (ÁGORA #72, ADR-0017).

Lê `civic_source` (platform='cespro', probe_status='ok'), busca o roster de cada câmara
(cespro_client), filtra e-mail INSTITUCIONAL, casa com os mandatos municipais e emite
UPDATEs que só preenchem `public_email` quando hoje é PLACEHOLDER.

Reusa o matcher/filtros do `extract_sapl` (nome fuzzy + partido, is_institutional, etc.) —
a única diferença do SAPL é a FONTE do roster. Seguro por padrão: **dry-run**. `--apply` grava.

Uso:
    python3 scripts/civic/extract_cespro.py --db <url>            # dry-run
    python3 scripts/civic/extract_cespro.py --base https://www.irai.rs.leg.br \
            --municipio Iraí --uf RS                              # 1 câmara (só imprime)
    python3 scripts/civic/extract_cespro.py --db <url> --apply    # aplica
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from cespro_client import fetch_current_vereadores  # noqa: E402
# Reuso do matcher/filtros já provados do SAPL (DRY — ADR-0017).
from extract_sapl import (  # noqa: E402
    DEFAULT_ORG, PLACEHOLDER_SUFFIX, is_institutional, load_mandates, match,
)

_TMP = Path(tempfile.gettempdir())
_PID = os.getpid()


def load_sources(db: str) -> list[tuple[str, str, str]]:
    sql = ("SELECT uf, municipio, base_url FROM civic_source "
           "WHERE platform='cespro' AND probe_status='ok' AND base_url IS NOT NULL "
           "ORDER BY uf, municipio")
    out = subprocess.run(["psql", db, "-tA", "-F", "\t", "-c", sql],
                         capture_output=True, text=True, check=True).stdout
    rows = []
    for line in out.splitlines():
        p = line.split("\t")
        if len(p) == 3:
            rows.append((p[0].strip(), p[1].strip(), p[2].strip()))
    return rows


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db")
    ap.add_argument("--base"); ap.add_argument("--municipio"); ap.add_argument("--uf")
    ap.add_argument("--org", default=DEFAULT_ORG)
    ap.add_argument("--apply", action="store_true")
    args = ap.parse_args()

    # Modo 1 câmara (só extração/impressão, sem banco).
    if args.base:
        pl = fetch_current_vereadores(args.base)
        inst = [p for p in pl if is_institutional(p.email)]
        print(f"{args.base}: {len(pl)} vereadores, {len(inst)} com e-mail institucional")
        for p in pl:
            flag = "INST" if is_institutional(p.email) else ("pessoal" if p.email else "—")
            print(f"  {p.nome:<32} {p.partido or '?':<16} {p.email or ''}  [{flag}]")
        return 0

    if not args.db:
        print("--db obrigatório (ou use --base para testar 1 câmara)", file=sys.stderr)
        return 2

    sources = load_sources(args.db)
    print(f"Fontes CESPRO ok: {len(sources)}", flush=True)

    artifact = []
    updates = []
    tot_ver = tot_inst = tot_match = tot_enrich = 0
    for uf, municipio, base in sources:
        try:
            pl = fetch_current_vereadores(base)
        except Exception as e:
            print(f"  ! {municipio}/{uf}: extração falhou ({e})", flush=True)
            continue
        inst = [p for p in pl if is_institutional(p.email)]
        mandates = load_mandates(args.db, args.org, uf, municipio)
        pairs = match(pl, mandates)
        enrichable = [(v, m) for v, m, _ in pairs
                      if m["placeholder"] and is_institutional(v.email)]
        tot_ver += len(pl); tot_inst += len(inst)
        tot_match += len(pairs); tot_enrich += len(enrichable)
        print(f"  {municipio}/{uf}: {len(pl)} vig · {len(inst)} inst · "
              f"{len(mandates)} mand · {len(pairs)} casados · {len(enrichable)} enriquecíveis", flush=True)
        for v, m in enrichable:
            email = v.email.replace("'", "''")
            updates.append(
                f"UPDATE mandate SET public_email='{email}' "
                f"WHERE id='{m['id']}' AND public_email ILIKE '%{PLACEHOLDER_SUFFIX}';"
            )
            artifact.append({"uf": uf, "municipio": municipio, "mandate_id": m["id"],
                             "nome_cespro": v.nome, "nome_mandato": m["name"],
                             "partido": v.partido, "email": v.email})

    print(f"\nTOTAL: {tot_ver} vereadores · {tot_inst} institucionais · "
          f"{tot_match} casados · {tot_enrich} mandatos enriquecíveis", flush=True)

    art_path = _TMP / f"civic-cespro-extract-{_PID}.json"
    art_path.write_text(json.dumps(artifact, ensure_ascii=False, indent=1))
    sql_path = _TMP / f"civic-cespro-enrich-{_PID}.sql"
    sql_path.write_text("BEGIN;\n" + "\n".join(updates) + "\nCOMMIT;\n" if updates else "-- nada a enriquecer\n")
    print(f"Auditoria → {art_path}\nSQL → {sql_path}", flush=True)

    if args.apply and updates:
        subprocess.run(["psql", args.db, "-v", "ON_ERROR_STOP=1", "-f", str(sql_path)], check=True)
        print(f"APLICADO: {len(updates)} mandatos enriquecidos.", flush=True)
    elif updates:
        print("DRY-RUN (use --apply para gravar).", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
