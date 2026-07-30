#!/usr/bin/env python3
"""Classificar plataforma dos municípios `unknown` — ÁGORA #72 / ADR-0017.

Para uma amostra dos municípios `civic_source.platform='unknown'`, descobre o site da
câmara por convenções de hostname, baixa SÓ a homepage e classifica a plataforma por
assinatura no HTML (vendor legislativo, CMS genérico ou site próprio). Achando SAPL
"escondido" (fora da convenção `sapl.<slug>.<uf>.leg.br`), verifica a API e grava a
base — vira alvo imediato do `extract_sapl`.

Seguro por padrão: **dry-run** (CSV + ranking no stdout). `--apply` grava `civic_source`.

Uso:
    python3 scripts/civic/classify_unknown.py --db <url> --limit 450            # dry-run
    python3 scripts/civic/classify_unknown.py --db <url> --limit 450 --apply    # grava

Nota: um re-run do `fingerprint_sapl` na mesma UF sobrescreve a classificação (upsert
por uf+município) — re-rode este script depois, é barato e idempotente.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import csv
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from sapl_client import is_sapl, normalize_slug  # noqa: E402

UA = "democracia.social.br civic-mapping (contato: /contato)"
TIMEOUT = 6
MAX_BYTES = 300_000
WORKERS = 16

# Assinaturas em ordem de precedência: vendor específico ANTES de CMS genérico
# (um site de vendor pode embutir WordPress; o vendor é a informação útil).
SIGNATURES: list[tuple[str, re.Pattern[str]]] = [
    ("sapl", re.compile(r"sapl\.|/sapl/|sistema de apoio ao processo legislativo", re.I)),
    ("camaraonline", re.compile(r"camaraonline\.org", re.I)),
    ("ipm", re.compile(r"atende\.net", re.I)),
    ("siscam", re.compile(r"siscam", re.I)),
    ("instar", re.compile(r"instar\b|instar\.com\.br|instarnews", re.I)),
    ("cespro", re.compile(r"cespro", re.I)),
    ("betha", re.compile(r"betha\.com\.br|betha sistemas", re.I)),
    ("fiorilli", re.compile(r"fiorilli", re.I)),
    ("elegis", re.compile(r"e-?legis", re.I)),
    ("sem-papel-familia", re.compile(r"camarasempapel|1doc\.com\.br|agapesistemas|nopaper|spl2?/parlamentares\.aspx", re.I)),
    ("vialink", re.compile(r"vialink", re.I)),
    ("wordpress", re.compile(r"wp-content|wp-includes", re.I)),
    ("joomla", re.compile(r"/media/jui/|joomla", re.I)),
    ("drupal", re.compile(r"drupal", re.I)),
]

SAPL_BASE_RE = re.compile(r"https?://sapl\.[a-z0-9.-]+\.leg\.br", re.I)


def candidate_sites(municipio: str, uf: str) -> list[str]:
    """Convenções de hostname de site de câmara (não-SAPL), em ordem de probabilidade."""
    slug = normalize_slug(municipio)
    uf = uf.lower()
    https = [
        f"https://www.camara{slug}.{uf}.gov.br",
        f"https://camara{slug}.{uf}.gov.br",
        f"https://www.cm{slug}.{uf}.gov.br",
        f"https://cm{slug}.{uf}.gov.br",
        f"https://camara{slug}.{uf}.leg.br",
        f"https://www.{slug}.{uf}.leg.br",
        f"https://{slug}.{uf}.leg.br",
    ]
    # Câmara pequena ainda vive em http puro; só as 2 convenções mais comuns.
    http = [
        f"http://www.camara{slug}.{uf}.gov.br",
        f"http://camara{slug}.{uf}.gov.br",
    ]
    return https + http


def fetch_home(url: str) -> tuple[str, str] | None:
    """(html, url_final) da homepage, ou None. Segue redirects; lê no máximo MAX_BYTES."""
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
            if resp.status != 200:
                return None
            html = resp.read(MAX_BYTES).decode("utf-8", "replace")
            return (html, resp.geturl())
    except Exception:
        return None


def classify(html: str) -> str:
    for vendor, pat in SIGNATURES:
        if pat.search(html):
            return vendor
    return "proprio-indefinido"


def probe_municipio(uf: str, ibge: str, nome: str) -> dict:
    for url in candidate_sites(nome, uf):
        got = fetch_home(url)
        if not got:
            continue
        html, final_url = got
        vendor = classify(html)
        row = {"uf": uf, "ibge": ibge, "nome": nome, "vendor": vendor,
               "url": final_url, "sapl_base": None, "sapl_count": None}
        if vendor == "sapl":
            # SAPL fora da convenção: extrai a base do link e VERIFICA a API —
            # só uma base confirmada vira alvo do extract_sapl.
            m = SAPL_BASE_RE.search(html)
            if m:
                base = m.group(0)
                ok, count = is_sapl(base)
                if ok:
                    row["sapl_base"] = base
                    row["sapl_count"] = count
        return row
    return {"uf": uf, "ibge": ibge, "nome": nome, "vendor": "sem-site-descoberto",
            "url": None, "sapl_base": None, "sapl_count": None}


def sample_unknowns(db: str, limit: int) -> list[tuple[str, str, str]]:
    """Amostra determinística (hash do IBGE) de municípios unknown: (uf, municipio, ibge)."""
    sql = ("SELECT uf, municipio, codigo_ibge FROM civic_source "
           f"WHERE platform='unknown' ORDER BY md5(codigo_ibge) LIMIT {int(limit)}")
    out = subprocess.run(["psql", db, "-tA", "-F", "\t", "-c", sql],
                         capture_output=True, text=True, check=True).stdout
    rows = []
    for line in out.splitlines():
        p = line.split("\t")
        if len(p) == 3:
            rows.append((p[0].strip(), p[1].strip(), p[2].strip()))
    return rows


def sql_escape(s: str) -> str:
    return s.replace("'", "''")


def emit_apply_sql(results: list[dict]) -> Path:
    """UPDATEs por código IBGE — só linhas com site descoberto. SAPL confirmado ganha
    a base da API e probe_status='ok' (alvo direto do extract_sapl); o resto registra
    a plataforma informativamente mantendo probe_status='dead' (sem extrator ainda)."""
    out = Path("/tmp/civic-classify-apply.sql")
    lines = ["BEGIN;"]
    for r in results:
        if r["vendor"] == "sem-site-descoberto":
            continue
        if r["vendor"] == "sapl" and r["sapl_base"]:
            cnt = r["sapl_count"] if isinstance(r["sapl_count"], int) else "NULL"
            lines.append(
                "UPDATE civic_source SET platform='sapl', "
                f"base_url='{sql_escape(r['sapl_base'])}', probe_status='ok', "
                f"parlamentares_found={cnt}, last_probed_at=now() "
                f"WHERE codigo_ibge='{r['ibge']}';"
            )
        else:
            url = f"'{sql_escape(r['url'])}'" if r["url"] else "NULL"
            lines.append(
                f"UPDATE civic_source SET platform='{sql_escape(r['vendor'])}', "
                f"base_url={url}, last_probed_at=now() "
                f"WHERE codigo_ibge='{r['ibge']}';"
            )
    lines.append("COMMIT;")
    out.write_text("\n".join(lines) + "\n")
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--db", required=True)
    ap.add_argument("--limit", type=int, default=450)
    ap.add_argument("--apply", action="store_true")
    ap.add_argument("--csv", default="/tmp/civic-classify-sample.csv")
    args = ap.parse_args()

    munis = sample_unknowns(args.db, args.limit)
    print(f"Classificando {len(munis)} municípios unknown (workers={WORKERS})…", flush=True)

    results: list[dict] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as ex:
        futs = [ex.submit(probe_municipio, uf, ibge, nome) for uf, nome, ibge in munis]
        for i, fut in enumerate(concurrent.futures.as_completed(futs), 1):
            results.append(fut.result())
            if i % 50 == 0:
                print(f"  … {i}/{len(munis)}", flush=True)

    ranking: dict[str, int] = {}
    for r in results:
        ranking[r["vendor"]] = ranking.get(r["vendor"], 0) + 1
    print("\n== RANKING DA AMOSTRA ==")
    for vendor, n in sorted(ranking.items(), key=lambda x: -x[1]):
        print(f"  {vendor:<22} {n:>4}  ({100 * n / len(results):.0f}%)")

    with open(args.csv, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["uf", "ibge", "nome", "vendor", "url",
                                          "sapl_base", "sapl_count"])
        w.writeheader()
        w.writerows(results)
    print(f"CSV → {args.csv}", flush=True)

    sql = emit_apply_sql(results)
    print(f"SQL → {sql}", flush=True)
    if args.apply:
        subprocess.run(["psql", args.db, "-f", str(sql)], check=True)
        print("APLICADO em civic_source.", flush=True)
    else:
        print("DRY-RUN (use --apply para gravar).", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
