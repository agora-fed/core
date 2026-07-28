#!/usr/bin/env python3
"""Fingerprint camaraonline — ÁGORA #72 / ADR-0017.

Para os municípios de uma UF (tabela `municipio_ibge`), testa as URLs-base candidatas
(`camara<slug>.<uf>.gov.br` e `camaraonline.org/cm_<slug>`) e descobre quais câmaras rodam o portal
camaraonline — reconhecido pela assinatura `camaraonline.org/cm_<slug>` no HTML. Grava o resultado
em `civic_source` (UPSERT), o catálogo que `extract_camaraonline.py` consome.

Ao contrário do SAPL (convenção `sapl.<slug>.<uf>.leg.br`), o camaraonline não tem convenção de
host garantida — os portais vivem em domínios próprios `.gov.br`. Muitos municípios simplesmente
não usam camaraonline: entram como 'unknown'/'dead' (esperado). Probe concorrente, só stdlib.

Uso:
    python3 scripts/civic/fingerprint_camaraonline.py --uf SP [--limit 50] [--db postgresql://localhost/dsoc_build]
"""

from __future__ import annotations

import argparse
import concurrent.futures
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from camaraonline_client import candidate_bases, is_camaraonline  # noqa: E402

WORKERS = 12


def municipios_da_uf(db: str, uf: str, limit: int | None) -> list[tuple[str, str]]:
    """(codigo_ibge, nome) dos municípios da UF."""
    lim = f"LIMIT {int(limit)}" if limit else ""
    sql = f"SELECT codigo_ibge, nome FROM municipio_ibge WHERE uf = '{uf}' ORDER BY nome {lim}"
    out = subprocess.run(
        ["psql", db, "-tA", "-F", "\t", "-c", sql], capture_output=True, text=True, check=True
    ).stdout
    rows = []
    for line in out.splitlines():
        if "\t" in line:
            ibge, nome = line.split("\t", 1)
            rows.append((ibge.strip(), nome.strip()))
    return rows


def probe_municipio(uf: str, ibge: str, nome: str) -> dict:
    """Testa as bases candidatas; retorna o 1º hit camaraonline, ou marca dead."""
    for base in candidate_bases(nome, uf):
        try:
            ok, count = is_camaraonline(base)
        except Exception:
            ok, count = False, None
        if ok:
            return {"ibge": ibge, "nome": nome, "platform": "camaraonline", "base": base,
                    "status": "ok", "count": count}
    return {"ibge": ibge, "nome": nome, "platform": "unknown", "base": None,
            "status": "dead", "count": None}


def sql_escape(s: str) -> str:
    return s.replace("'", "''")


def emit_sql(uf: str, results: list[dict]) -> Path:
    out = Path(f"/tmp/civic-fingerprint-camaraonline-{uf}.sql")
    lines = ["BEGIN;"]
    for r in results:
        base = f"'{sql_escape(r['base'])}'" if r["base"] else "NULL"
        cnt = r["count"] if isinstance(r["count"], int) else "NULL"
        lines.append(
            "INSERT INTO civic_source (uf, municipio, codigo_ibge, platform, base_url, "
            "probe_status, parlamentares_found, last_probed_at) VALUES "
            f"('{uf}', '{sql_escape(r['nome'])}', '{r['ibge']}', '{r['platform']}', {base}, "
            f"'{r['status']}', {cnt}, now()) "
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

    munis = municipios_da_uf(args.db, uf, args.limit)
    print(f"[{uf}] probando {len(munis)} municípios (camaraonline, workers={WORKERS})…", flush=True)

    results: list[dict] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as ex:
        futs = {ex.submit(probe_municipio, uf, ibge, nome): nome for ibge, nome in munis}
        for fut in concurrent.futures.as_completed(futs):
            r = fut.result()
            results.append(r)
            if r["status"] == "ok":
                print(f"  ✓ camaraonline {r['nome']} → {r['base']} ({r['count']} vereadores)", flush=True)

    hits = [r for r in results if r["status"] == "ok"]
    print(f"[{uf}] camaraonline detectado em {len(hits)}/{len(results)} câmaras", flush=True)

    sql = emit_sql(uf, results)
    print(f"[{uf}] SQL → {sql}", flush=True)
    subprocess.run(["psql", args.db, "-f", str(sql)], check=True)
    print(f"[{uf}] civic_source atualizado.", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
