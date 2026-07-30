#!/usr/bin/env python3
"""Fingerprint CESPRO — ÁGORA #72 / ADR-0017.

Para os municípios de uma UF (de `municipio_ibge`), testa as URLs-base candidatas e
descobre quais câmaras rodam CESPRO (assinatura `cespro.com.br` + listagem
`/vereadores/`). Grava em `civic_source` (UPSERT). Probe concorrente, só stdlib.

Prioriza quem AINDA é `unknown`: por padrão só reclassifica municípios cujo
`platform` está em ('unknown', NULL) — não pisa num SAPL já confirmado.

Uso:
    python3 scripts/civic/fingerprint_cespro.py --uf RS [--limit 50] --db <url>
"""

from __future__ import annotations

import argparse
import concurrent.futures
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from cespro_client import candidate_bases, is_cespro  # noqa: E402

WORKERS = 12


def municipios_unknown_da_uf(db: str, uf: str, limit: int | None) -> list[tuple[str, str]]:
    """(codigo_ibge, municipio) dos municípios da UF ainda não classificados como plataforma
    conhecida — lê da `civic_source` (que o fingerprint_sapl já populou por UF)."""
    lim = f"LIMIT {int(limit)}" if limit else ""
    sql = ("SELECT codigo_ibge, municipio FROM civic_source "
           f"WHERE uf = '{uf}' AND (platform = 'unknown' OR platform IS NULL) "
           f"ORDER BY municipio {lim}")
    out = subprocess.run(["psql", db, "-tA", "-F", "\t", "-c", sql],
                         capture_output=True, text=True, check=True).stdout
    rows = []
    for line in out.splitlines():
        if "\t" in line:
            ibge, nome = line.split("\t", 1)
            rows.append((ibge.strip(), nome.strip()))
    return rows


def probe_municipio(uf: str, ibge: str, nome: str) -> dict:
    for base in candidate_bases(nome, uf):
        ok, count = is_cespro(base)
        if ok:
            return {"ibge": ibge, "nome": nome, "base": base, "count": count, "ok": True}
    return {"ibge": ibge, "nome": nome, "base": None, "count": None, "ok": False}


def sql_escape(s: str) -> str:
    return s.replace("'", "''")


def emit_sql(uf: str, hits: list[dict]) -> Path:
    """Só grava os HITS (não rebaixa unknowns que continuam unknown — outros extratores
    podem pegá-los). UPSERT por (uf, upper(municipio))."""
    out = Path(f"/tmp/civic-fingerprint-cespro-{uf}.sql")
    lines = ["BEGIN;"]
    for r in hits:
        cnt = r["count"] if isinstance(r["count"], int) else "NULL"
        lines.append(
            "INSERT INTO civic_source (uf, municipio, codigo_ibge, platform, base_url, "
            "probe_status, parlamentares_found, last_probed_at) VALUES "
            f"('{uf}', '{sql_escape(r['nome'])}', '{r['ibge']}', 'cespro', "
            f"'{sql_escape(r['base'])}', 'ok', {cnt}, now()) "
            "ON CONFLICT (uf, upper(municipio)) DO UPDATE SET "
            "platform = EXCLUDED.platform, base_url = EXCLUDED.base_url, "
            "probe_status = EXCLUDED.probe_status, parlamentares_found = EXCLUDED.parlamentares_found, "
            "codigo_ibge = EXCLUDED.codigo_ibge, last_probed_at = now();"
        )
    lines.append("COMMIT;")
    out.write_text("\n".join(lines) + "\n")
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--uf", required=True)
    ap.add_argument("--limit", type=int, default=None)
    ap.add_argument("--db", default="postgresql://localhost/dsoc_build")
    args = ap.parse_args()
    uf = args.uf.upper()

    munis = municipios_unknown_da_uf(args.db, uf, args.limit)
    print(f"[{uf}] probando {len(munis)} municípios unknown (workers={WORKERS})…", flush=True)

    hits: list[dict] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as ex:
        futs = {ex.submit(probe_municipio, uf, ibge, nome): nome for ibge, nome in munis}
        for fut in concurrent.futures.as_completed(futs):
            r = fut.result()
            if r["ok"]:
                hits.append(r)
                print(f"  ✓ CESPRO {r['nome']} → {r['base']} ({r['count']} vereadores)", flush=True)

    print(f"[{uf}] CESPRO detectado em {len(hits)}/{len(munis)} câmaras", flush=True)
    if hits:
        sql = emit_sql(uf, hits)
        subprocess.run(["psql", args.db, "-v", "ON_ERROR_STOP=1", "-f", str(sql)], check=True)
        print(f"[{uf}] civic_source atualizado ({sql}).", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
