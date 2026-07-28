#!/usr/bin/env python3
"""Extrair + casar contatos de vereadores camaraonline → enriquecer `mandate` (ÁGORA #72, ADR-0017).

Gêmeo do `extract_sapl.py` para o vendor camaraonline. Lê o catálogo `civic_source`
(platform='camaraonline', probe_status='ok'), raspa o roster VIGENTE do portal público de cada
câmara (via camaraonline_client — HTML, sem API), filtra e-mail INSTITUCIONAL (transparência — nunca
pessoal), casa com os mandatos municipais existentes e emite UPDATEs que só preenchem `public_email`
quando hoje é PLACEHOLDER.

REUSA o matcher e as regras do `extract_sapl.py` (name_similarity/is_institutional/match/
load_mandates) — uma só lógica de casamento para todo o pipeline cívico. Só muda a FONTE do roster.

Seguro por padrão: **dry-run** (só relatório + SQL em /tmp + JSON de auditoria). `--apply` executa.

Uso:
    python3 scripts/civic/extract_camaraonline.py --db postgresql://localhost/dsoc_build          # dry-run
    python3 scripts/civic/extract_camaraonline.py --base https://www.camarasantanadeparnaiba.sp.gov.br \
            --municipio "Santana de Parnaíba" --uf SP                                              # 1 câmara
    python3 scripts/civic/extract_camaraonline.py --db <prod-url> --apply                          # aplica (consentido)
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

# Artefatos por-PID no tempdir do usuário — evita colisão de dono em /tmp (mesmo racional do SAPL).
_TMP = Path(tempfile.gettempdir())
_PID = os.getpid()

sys.path.insert(0, str(Path(__file__).resolve().parent))
from camaraonline_client import fetch_current_parlamentares  # noqa: E402
# REUSO do matcher/regras do extrator SAPL — DRY: uma só lógica de casamento e de e-mail institucional.
from extract_sapl import (  # noqa: E402
    DEFAULT_ORG,
    PLACEHOLDER_SUFFIX,
    is_institutional,
    load_mandates,
    match,
)


def load_sources(db: str) -> list[tuple[str, str, str]]:
    """(uf, municipio, base_url) das fontes camaraonline prontas para extração."""
    sql = ("SELECT uf, municipio, base_url FROM civic_source "
           "WHERE platform='camaraonline' AND probe_status='ok' AND base_url IS NOT NULL "
           "ORDER BY uf, municipio")
    out = subprocess.run(["psql", db, "-tA", "-F", "\t", "-c", sql],
                         capture_output=True, text=True, check=True).stdout
    rows = []
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) == 3:
            rows.append((parts[0].strip(), parts[1].strip(), parts[2].strip()))
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
        pl = fetch_current_parlamentares(args.base)
        inst = [p for p in pl if is_institutional(p.email)]
        print(f"{args.base}: {len(pl)} vigentes, {len(inst)} com e-mail institucional")
        for p in pl:
            flag = "INST" if is_institutional(p.email) else ("pessoal" if p.email else "—")
            print(f"  {p.nome:<34} {p.partido or '?':<14} {p.email or ''}  [{flag}]")
        return 0

    if not args.db:
        print("--db obrigatório (ou use --base para testar 1 câmara)", file=sys.stderr)
        return 2

    sources = load_sources(args.db)
    print(f"Fontes camaraonline ok: {len(sources)}", flush=True)

    artifact = []
    updates = []
    tot_ver = tot_inst = tot_match = tot_enrich = 0
    for uf, municipio, base in sources:
        try:
            pl = fetch_current_parlamentares(base)
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
            # Só preenche o e-mail; NÃO altera `source` (mesmo racional do extract_sapl: a origem TSE
            # do roster é preservada; provar contato institucional não é onboarding).
            updates.append(
                f"UPDATE mandate SET public_email='{email}' "
                f"WHERE id='{m['id']}' AND public_email ILIKE '%{PLACEHOLDER_SUFFIX}';"
            )
            artifact.append({"uf": uf, "municipio": municipio, "mandate_id": m["id"],
                             "nome_portal": v.nome, "nome_mandato": m["name"],
                             "partido": v.partido, "email": v.email, "foto_url": v.foto_url})

    print(f"\nTOTAL: {tot_ver} vigentes · {tot_inst} institucionais · "
          f"{tot_match} casados · {tot_enrich} mandatos enriquecíveis", flush=True)

    art_path = _TMP / f"civic-camaraonline-extract-{_PID}.json"
    art_path.write_text(json.dumps(artifact, ensure_ascii=False, indent=1))
    sql_path = _TMP / f"civic-camaraonline-enrich-{_PID}.sql"
    sql_path.write_text("BEGIN;\n" + "\n".join(updates) + "\nCOMMIT;\n" if updates else "-- nada a enriquecer\n")
    print(f"Auditoria → {art_path}\nSQL → {sql_path}", flush=True)

    if args.apply and updates:
        subprocess.run(["psql", args.db, "-f", str(sql_path)], check=True)
        print(f"APLICADO: {len(updates)} mandatos enriquecidos.", flush=True)
    elif updates:
        print("DRY-RUN (use --apply para gravar).", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
