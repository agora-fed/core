#!/usr/bin/env python3
"""Extrair contatos de vereadores via WordPress wp-json → enriquecer `mandate` (ADR-0017).

Lê `civic_source` (platform='wordpress', probe_status='ok'), busca o roster de cada câmara
(wpjson_client, CPT de vereador na REST), filtra e-mail INSTITUCIONAL, casa com os mandatos
municipais e enriquece `public_email` só onde é PLACEHOLDER. Reusa o matcher do `extract_sapl`.

Realidade: WordPress raramente publica e-mail (biografia sim, contato não). Este extrator
colhe os poucos que publicam (ex.: Mucuri/BA); o resto fica pro aviso de transparência no fórum.

Seguro por padrão: **dry-run**. `--apply` grava.

Uso:
    python3 scripts/civic/extract_wpjson.py --db <url>            # dry-run
    python3 scripts/civic/extract_wpjson.py --base https://camaramucuri.ba.gov.br \
            --municipio Mucuri --uf BA                            # 1 câmara (só imprime)
    python3 scripts/civic/extract_wpjson.py --db <url> --apply    # aplica
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
from wpjson_client import fetch_current_vereadores  # noqa: E402
from extract_sapl import (  # noqa: E402
    DEFAULT_ORG, PLACEHOLDER_SUFFIX, is_institutional, load_mandates, match,
)

_TMP = Path(tempfile.gettempdir())
_PID = os.getpid()


def load_sources(db: str) -> list[tuple[str, str, str]]:
    sql = ("SELECT uf, municipio, base_url FROM civic_source "
           "WHERE platform='wordpress' AND probe_status='ok' AND base_url IS NOT NULL "
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

    if args.base:
        pl = fetch_current_vereadores(args.base)
        inst = [p for p in pl if is_institutional(p.email)]
        print(f"{args.base}: {len(pl)} vereadores, {len(inst)} com e-mail institucional")
        for p in pl:
            flag = "INST" if is_institutional(p.email) else ("pessoal" if p.email else "—")
            print(f"  {p.nome:<34} {p.partido or '?':<14} {p.email or ''}  [{flag}]")
        return 0

    if not args.db:
        print("--db obrigatório (ou use --base para testar 1 câmara)", file=sys.stderr)
        return 2

    sources = load_sources(args.db)
    print(f"Fontes WordPress ok: {len(sources)}", flush=True)

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
        if pl:
            print(f"  {municipio}/{uf}: {len(pl)} vig · {len(inst)} inst · "
                  f"{len(mandates)} mand · {len(pairs)} casados · {len(enrichable)} enriquecíveis", flush=True)
        for v, m in enrichable:
            email = v.email.replace("'", "''")
            updates.append(
                f"UPDATE mandate SET public_email='{email}' "
                f"WHERE id='{m['id']}' AND public_email ILIKE '%{PLACEHOLDER_SUFFIX}';"
            )
            artifact.append({"uf": uf, "municipio": municipio, "mandate_id": m["id"],
                             "nome_wp": v.nome, "nome_mandato": m["name"], "email": v.email})

    print(f"\nTOTAL: {tot_ver} vereadores · {tot_inst} institucionais · "
          f"{tot_match} casados · {tot_enrich} mandatos enriquecíveis", flush=True)

    art_path = _TMP / f"civic-wpjson-extract-{_PID}.json"
    art_path.write_text(json.dumps(artifact, ensure_ascii=False, indent=1))
    sql_path = _TMP / f"civic-wpjson-enrich-{_PID}.sql"
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
