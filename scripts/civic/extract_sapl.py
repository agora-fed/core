#!/usr/bin/env python3
"""Extrair + casar contatos de vereadores SAPL → enriquecer `mandate` (ÁGORA #72, ADR-0017).

Lê o catálogo `civic_source` (platform='sapl', probe_status='ok'), busca o roster VIGENTE de cada
câmara (via sapl_client), filtra e-mail INSTITUCIONAL (transparência — nunca pessoal), casa com os
mandatos municipais existentes (sphere='municipal', house='camara_municipal', uf+município, nome
fuzzy + partido) e emite UPDATEs que só preenchem `public_email` quando hoje é PLACEHOLDER.

Seguro por padrão: **dry-run** (só relatório + SQL em /tmp + JSON de auditoria). `--apply` executa.

Uso:
    python3 scripts/civic/extract_sapl.py --db postgresql://localhost/dsoc_build           # dry-run
    python3 scripts/civic/extract_sapl.py --base https://sapl.campinas.sp.leg.br \
            --municipio Campinas --uf SP                                                    # 1 câmara (só imprime)
    python3 scripts/civic/extract_sapl.py --db <prod-url> --apply                           # aplica (consentido)
"""

from __future__ import annotations

import argparse
import difflib
import json
import subprocess
import sys
import unicodedata
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from sapl_client import Parlamentar, fetch_current_parlamentares  # noqa: E402

DEFAULT_ORG = "11111111-1111-1111-1111-111111111111"
PLACEHOLDER_SUFFIX = "@parlamento.democracia.social.br"
PERSONAL_DOMAINS = {
    "gmail.com", "hotmail.com", "outlook.com", "yahoo.com", "yahoo.com.br",
    "hotmail.com.br", "bol.com.br", "uol.com.br", "live.com", "icloud.com", "terra.com.br",
}
NAME_MATCH_THRESHOLD = 0.86


def norm_name(s: str) -> str:
    s = unicodedata.normalize("NFKD", s or "")
    s = "".join(c for c in s if not unicodedata.combining(c)).lower()
    return " ".join("".join(c for c in tok if c.isalnum()) for tok in s.split()).strip()


def is_institutional(email: str | None) -> bool:
    """Institucional = domínio de câmara/legislativo público; nunca provedor pessoal."""
    if not email or "@" not in email:
        return False
    domain = email.rsplit("@", 1)[1].lower()
    if domain in PERSONAL_DOMAINS:
        return False
    return (
        domain.endswith(".leg.br")
        or domain.endswith(".gov.br")
        or "camara" in domain
        or domain.startswith("cm")
    )


def name_similarity(a: str, b: str) -> float:
    na, nb = norm_name(a), norm_name(b)
    if not na or not nb:
        return 0.0
    if na == nb:
        return 1.0
    # subconjunto de tokens (apelido curto ⊂ nome completo) conta como forte.
    ta, tb = set(na.split()), set(nb.split())
    if ta and (ta <= tb or tb <= ta):
        return 0.95
    return difflib.SequenceMatcher(None, na, nb).ratio()


def load_sources(db: str) -> list[tuple[str, str, str]]:
    sql = ("SELECT uf, municipio, base_url FROM civic_source "
           "WHERE platform='sapl' AND probe_status='ok' AND base_url IS NOT NULL "
           "ORDER BY uf, municipio")
    out = subprocess.run(["psql", db, "-tA", "-F", "\t", "-c", sql],
                         capture_output=True, text=True, check=True).stdout
    rows = []
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) == 3:
            rows.append((parts[0].strip(), parts[1].strip(), parts[2].strip()))
    return rows


def load_mandates(db: str, org: str, uf: str, municipio: str) -> list[dict]:
    m = municipio.replace("'", "''")
    sql = (f"SELECT id, display_name, coalesce(party,''), "
           f"(public_email ILIKE '%{PLACEHOLDER_SUFFIX}') AS is_placeholder "
           f"FROM mandate WHERE org_id='{org}' AND sphere='municipal' "
           f"AND house='camara_municipal' AND uf='{uf}' AND upper(municipio)=upper('{m}')")
    out = subprocess.run(["psql", db, "-tA", "-F", "\t", "-c", sql],
                         capture_output=True, text=True, check=True).stdout
    rows = []
    for line in out.splitlines():
        p = line.split("\t")
        if len(p) == 4:
            rows.append({"id": p[0], "name": p[1], "party": p[2],
                         "placeholder": p[3] == "t"})
    return rows


def match(vereadores: list[Parlamentar], mandates: list[dict]) -> list[tuple[Parlamentar, dict, float]]:
    """Casa cada vereador ao melhor mandato (nome fuzzy; partido desempata). Match único e confiante."""
    pairs = []
    used = set()
    cands = []
    for v in vereadores:
        for m in mandates:
            score = name_similarity(v.nome, m["name"])
            if v.partido and m["party"] and norm_name(v.partido) == norm_name(m["party"]):
                score = min(1.0, score + 0.05)
            if score >= NAME_MATCH_THRESHOLD:
                cands.append((score, v, m))
    for score, v, m in sorted(cands, key=lambda x: -x[0]):
        if id(v) in used or m["id"] in used:
            continue
        used.add(id(v))
        used.add(m["id"])
        pairs.append((v, m, score))
    return pairs


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
            print(f"  {p.nome:<32} {p.partido or '?':<10} {p.email or ''}  [{flag}]")
        return 0

    if not args.db:
        print("--db obrigatório (ou use --base para testar 1 câmara)", file=sys.stderr)
        return 2

    sources = load_sources(args.db)
    print(f"Fontes SAPL ok: {len(sources)}", flush=True)

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
            # Só preenche o e-mail; NÃO altera `source` (a origem TSE do roster é preservada — marcar
            # 'self' mentiria que o vereador se auto-cadastrou; não há enum 'sapl' e provar contato
            # institucional não é o mesmo que onboarding).
            updates.append(
                f"UPDATE mandate SET public_email='{email}' "
                f"WHERE id='{m['id']}' AND public_email ILIKE '%{PLACEHOLDER_SUFFIX}';"
            )
            artifact.append({"uf": uf, "municipio": municipio, "mandate_id": m["id"],
                             "nome_sapl": v.nome, "nome_mandato": m["name"],
                             "partido": v.partido, "email": v.email, "foto_url": v.foto_url})

    print(f"\nTOTAL: {tot_ver} vigentes · {tot_inst} institucionais · "
          f"{tot_match} casados · {tot_enrich} mandatos enriquecíveis", flush=True)

    art_path = Path("/tmp/civic-sapl-extract.json")
    art_path.write_text(json.dumps(artifact, ensure_ascii=False, indent=1))
    sql_path = Path("/tmp/civic-sapl-enrich.sql")
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
