#!/usr/bin/env python3
"""
Seed state legislators (deputados estaduais + distritais) into the `mandate` table
from three sources, in priority order:

  1. ALESP (São Paulo)     — official XML + JSON detail API (photos included).
  2. ALMG (Minas Gerais)   — dados abertos JSON API + deterministic photo URL.
  3. TSE 2022 (rest, 25 UFs) — CSV of eleitos (`consulta_cand_2022`). No photos;
     covers every UF *except* SP and MG, which are already handled above.

Pipeline:
  1. Fetch each source into a common dict shape (see `Person` dataclass).
  2. Download photos for SP+MG rows into /tmp/dsoc-fotos-estaduais/<source>-<id>.jpg.
  3. Emit a single BEGIN/COMMIT SQL UPSERT into /tmp/dsoc-mandates-estaduais.sql.
  4. Operator later `mc cp`'s the photos to MinIO under
     `mandates/<source>-<external_id>/avatar.jpg` and applies the SQL with psql.

Idempotent: SQL is UPSERT on the natural key (source, source_external_id);
photo filenames are content-stable so re-runs skip already-downloaded files.
"""

from __future__ import annotations

import concurrent.futures
import csv
import io
import json
import sys
import uuid
import xml.etree.ElementTree as ET
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional
import urllib.request
import urllib.parse

ORG_ID = "11111111-1111-1111-1111-111111111111"

PHOTOS_DIR = Path("/tmp/dsoc-fotos-estaduais")
SQL_OUT = Path("/tmp/dsoc-mandates-estaduais.sql")
TSE_ZIP = Path("/tmp/consulta_cand_2022.zip")
TSE_EXTRACT_DIR = Path("/tmp/consulta_cand_2022")
USER_AGENT = "DemocraciaBR/0.5 seed (+https://democracia.social.br)"


@dataclass
class Person:
    source: str  # 'assembleia' | 'tse'
    external_id: str
    name: str
    party: str
    uf: str
    office: str
    house: str  # always 'assembleia' here
    sphere: str  # always 'estadual' here
    photo_url: Optional[str] = None
    public_email: Optional[str] = None
    gender: Optional[str] = None  # 'homem' | 'mulher' | None

    @property
    def key(self) -> str:
        """Uniquely identifies this row within photo_paths dict."""
        return f"{self.source}-{self.external_id}"


# ---------------------------------------------------------------------------
# HTTP helpers (stdlib only)
# ---------------------------------------------------------------------------

def _get(url: str, timeout: int = 20, accept: str = "application/json") -> bytes:
    req = urllib.request.Request(url, headers={
        "User-Agent": USER_AGENT,
        "Accept": accept,
    })
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read()


def _get_image(url: str, timeout: int = 20) -> bytes:
    req = urllib.request.Request(url, headers={
        "User-Agent": USER_AGENT,
        "Accept": "image/jpeg,image/*;q=0.9,*/*;q=0.5",
    })
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read()


def sql_quote(s: Optional[str]) -> str:
    if s is None:
        return "NULL"
    return "'" + s.replace("'", "''") + "'"


# ---------------------------------------------------------------------------
# Source 1: ALESP (São Paulo)
# ---------------------------------------------------------------------------

def fetch_alesp() -> list[Person]:
    """São Paulo state deputies in exercise (Situacao='EXE')."""
    out: list[Person] = []
    try:
        xml_bytes = _get(
            "https://www.al.sp.gov.br/repositorioDados/deputados/deputados.xml",
            accept="application/xml,text/xml,*/*;q=0.5",
        )
    except Exception as e:
        print(f"  ALESP list FALHOU ({e})", flush=True)
        return out

    try:
        root = ET.fromstring(xml_bytes)
    except ET.ParseError as e:
        print(f"  ALESP XML parse FALHOU ({e})", flush=True)
        return out

    for dep in root.findall(".//Deputado"):
        situacao = (dep.findtext("Situacao") or "").strip()
        if situacao != "EXE":
            continue
        id_dep = (dep.findtext("IdDeputado") or "").strip()
        nome = (dep.findtext("NomeParlamentar") or "").strip()
        partido = (dep.findtext("Partido") or "").strip()
        email = (dep.findtext("Email") or "").strip() or None
        matricula = (dep.findtext("Matricula") or "").strip()
        if not id_dep or not nome:
            continue

        # Try to fetch photo path via the portal detail endpoint. Non-fatal.
        photo_url: Optional[str] = None
        if matricula:
            try:
                detail = json.loads(_get(
                    f"https://legis-api-portal.pub.al.sp.gov.br/parlamentar-portal/detalhes/{matricula}",
                    timeout=15,
                ))
                bio = (detail or {}).get("biografia") or {}
                foto = bio.get("txFotoGrande")
                if foto:
                    # foto starts with a leading slash — concat straight onto the base.
                    photo_url = "https://www3.al.sp.gov.br/legis" + foto
            except Exception as e:
                # Photo is optional; keep the row.
                print(f"  ALESP detalhe {matricula} sem foto ({e})", flush=True)

        out.append(Person(
            source="assembleia",
            external_id=id_dep,
            name=nome,
            party=partido,
            uf="SP",
            office="Deputado(a) Estadual — SP",
            house="assembleia",
            sphere="estadual",
            photo_url=photo_url,
            public_email=email,
            gender=None,  # ALESP feed doesn't expose gender uniformly.
        ))
    return out


# ---------------------------------------------------------------------------
# Source 2: ALMG (Minas Gerais)
# ---------------------------------------------------------------------------

def fetch_almg() -> list[Person]:
    """Minas Gerais state deputies in exercise."""
    out: list[Person] = []
    try:
        payload = json.loads(_get(
            "https://dadosabertos.almg.gov.br/api/v2/deputados/em_exercicio?formato=json"
        ))
    except Exception as e:
        print(f"  ALMG list FALHOU ({e})", flush=True)
        return out

    lista = payload.get("list") or payload.get("resultado", {}).get("listaDeputado", []) or []
    if isinstance(lista, dict):
        # Some ALMG endpoints wrap under "listaDeputado" — normalize.
        lista = lista.get("deputado", [])

    for row in lista:
        dep_id = row.get("id") or row.get("idDeputado") or row.get("codigo")
        if dep_id is None:
            continue
        dep_id = str(dep_id)
        nome = (row.get("nome") or row.get("nomeParlamentar") or "").strip()
        partido = (row.get("partido") or row.get("siglaPartido") or "").strip()
        if not nome:
            continue

        email: Optional[str] = None
        gender: Optional[str] = None

        # Detail fetch is optional — gives us email + sexo. Non-fatal per row.
        try:
            detail = json.loads(_get(
                f"https://dadosabertos.almg.gov.br/api/v2/deputados/{dep_id}?formato=json",
                timeout=15,
            ))
            dep = detail.get("deputado") or detail.get("list", [{}])[0] or {}
            situacao = (dep.get("situacao") or "").strip()
            if situacao and situacao.lower() != "em exercício" and situacao.lower() != "em exercicio":
                # If the list said em_exercicio, trust it; but log the mismatch.
                pass
            sexo = (dep.get("sexo") or "").strip().upper()
            if sexo == "M":
                gender = "homem"
            elif sexo == "F":
                gender = "mulher"

            emails = dep.get("emails") or []
            if isinstance(emails, list) and emails:
                first = emails[0]
                if isinstance(first, dict):
                    endereco = (first.get("endereco") or "").strip()
                else:
                    endereco = str(first).strip()
                if endereco:
                    if "@" not in endereco:
                        endereco = endereco + "@almg.gov.br"
                    email = endereco
        except Exception as e:
            print(f"  ALMG detalhe {dep_id} sem detalhes ({e})", flush=True)

        photo_url = (
            f"https://www.almg.gov.br/export/sites/portal/a-assembleia/deputados/"
            f"fotos/{dep_id}.jpg_1098812190.jpg"
        )

        out.append(Person(
            source="assembleia",
            external_id=dep_id,
            name=nome,
            party=partido,
            uf="MG",
            office="Deputado(a) Estadual — MG",
            house="assembleia",
            sphere="estadual",
            photo_url=photo_url,
            public_email=email,
            gender=gender,
        ))
    return out


# ---------------------------------------------------------------------------
# Source 3: TSE 2022 CSV (25 UFs, excluding SP + MG)
# ---------------------------------------------------------------------------

TSE_ZIP_URL = "https://cdn.tse.jus.br/estatistica/sead/odsele/consulta_cand/consulta_cand_2022.zip"
_ELECTED_STATUSES = {"ELEITO", "ELEITO POR QP", "ELEITO POR MÉDIA"}
_TARGET_CARGOS = {"DEPUTADO ESTADUAL", "DEPUTADO DISTRITAL"}


def _ensure_tse_zip() -> Optional[Path]:
    if TSE_ZIP.exists() and TSE_ZIP.stat().st_size > 100_000:
        print(f"  TSE zip cache hit → {TSE_ZIP} ({TSE_ZIP.stat().st_size//1024} KB)", flush=True)
        return TSE_ZIP
    try:
        print(f"  Baixando TSE zip → {TSE_ZIP} …", flush=True)
        data = _get(TSE_ZIP_URL, timeout=180, accept="application/zip,*/*;q=0.5")
        TSE_ZIP.write_bytes(data)
        print(f"  TSE zip salvo ({len(data)//1024} KB)", flush=True)
        return TSE_ZIP
    except Exception as e:
        print(f"  TSE zip download FALHOU ({e})", flush=True)
        return None


def fetch_tse_2022(skip_ufs: set[str]) -> list[Person]:
    """Elected state/distrital deputies from TSE 2022, skipping UFs handled elsewhere."""
    out: list[Person] = []
    zpath = _ensure_tse_zip()
    if zpath is None:
        return out

    try:
        zf = zipfile.ZipFile(zpath)
    except zipfile.BadZipFile as e:
        print(f"  TSE zip corrupto ({e}) — removendo", flush=True)
        try:
            zpath.unlink()
        except Exception:
            pass
        return out

    with zf:
        # Per-UF CSVs named like `consulta_cand_2022_XX.csv`.
        members = [m for m in zf.namelist() if m.lower().endswith(".csv") and "_2022_" in m.lower()]
        members.sort()

        for name in members:
            # Extract UF from filename tail: consulta_cand_2022_SP.csv -> SP.
            base = Path(name).name
            stem = base.rsplit(".", 1)[0]
            uf_from_name = stem.split("_")[-1].upper()
            if uf_from_name in skip_ufs:
                continue
            # `BR` and `BRASIL` are rollup files that duplicate per-UF rows —
            # skip them entirely to avoid inflating per-UF counts.
            if uf_from_name in {"BR", "BRASIL"}:
                continue

            try:
                with zf.open(name) as raw:
                    # csv.DictReader wants text.
                    text = io.TextIOWrapper(raw, encoding="latin-1", newline="")
                    reader = csv.DictReader(text, delimiter=";")
                    seen: set[tuple[str, str]] = set()
                    kept_this_uf = 0
                    for row in reader:
                        cargo = (row.get("DS_CARGO") or "").strip().upper()
                        if cargo not in _TARGET_CARGOS:
                            continue
                        sit = (row.get("DS_SIT_TOT_TURNO") or "").strip().upper()
                        if sit not in _ELECTED_STATUSES:
                            continue

                        sg_uf = (row.get("SG_UF") or uf_from_name).strip().upper()
                        # Belt-and-suspenders: skip SP/MG rows even if we somehow read them.
                        if sg_uf in skip_ufs:
                            continue
                        nr = (row.get("NR_CANDIDATO") or "").strip()
                        if not nr:
                            continue
                        dedup_key = (sg_uf, nr)
                        if dedup_key in seen:
                            continue
                        seen.add(dedup_key)

                        nm_urna = (row.get("NM_URNA_CANDIDATO") or "").strip()
                        if not nm_urna:
                            continue
                        sg_partido = (row.get("SG_PARTIDO") or "").strip()

                        ds_genero = (row.get("DS_GENERO") or "").strip().upper()
                        gender: Optional[str] = None
                        if ds_genero == "MASCULINO":
                            gender = "homem"
                        elif ds_genero == "FEMININO":
                            gender = "mulher"

                        external_id = f"{sg_uf}-{nr}"
                        if cargo == "DEPUTADO DISTRITAL":
                            office = f"Deputado(a) Distrital — {sg_uf}"
                        else:
                            office = f"Deputado(a) Estadual — {sg_uf}"

                        out.append(Person(
                            source="tse",
                            external_id=external_id,
                            name=nm_urna,
                            party=sg_partido,
                            uf=sg_uf,
                            office=office,
                            house="assembleia",
                            sphere="estadual",
                            photo_url=None,
                            public_email=None,
                            gender=gender,
                        ))
                        kept_this_uf += 1
                    if kept_this_uf:
                        print(f"  TSE {uf_from_name}: {kept_this_uf} eleito(a)s", flush=True)
            except Exception as e:
                print(f"  TSE {name} FALHOU ({e})", flush=True)
    return out


# ---------------------------------------------------------------------------
# Photos
# ---------------------------------------------------------------------------

def _download_one(p: Person) -> tuple[str, Optional[str]]:
    if not p.photo_url:
        return p.key, None
    local = PHOTOS_DIR / f"{p.source}-{p.external_id}.jpg"
    if local.exists() and local.stat().st_size > 0:
        return p.key, str(local)
    try:
        data = _get_image(p.photo_url, timeout=25)
        if not data:
            return p.key, None
        local.write_bytes(data)
        return p.key, str(local)
    except Exception as e:
        print(f"  x foto {p.name} ({p.uf}): {e}", flush=True)
        return p.key, None


def download_photos(people: list[Person]) -> dict[str, Optional[str]]:
    PHOTOS_DIR.mkdir(parents=True, exist_ok=True)
    targets = [p for p in people if p.photo_url]
    if not targets:
        print("  (sem URLs de foto para baixar)")
        return {}
    print(f"  Fotos alvo: {len(targets)}")
    paths: dict[str, Optional[str]] = {}
    done = 0
    with concurrent.futures.ThreadPoolExecutor(max_workers=16) as ex:
        futures = {ex.submit(_download_one, p): p for p in targets}
        for fut in concurrent.futures.as_completed(futures):
            key, path = fut.result()
            paths[key] = path
            done += 1
            if done % 20 == 0 or done == len(targets):
                ok_count = sum(1 for v in paths.values() if v)
                print(f"  ... {done}/{len(targets)} concluídas ({ok_count} OK)", flush=True)
    return paths


# ---------------------------------------------------------------------------
# SQL emission
# ---------------------------------------------------------------------------

def emit_sql(people: list[Person], photo_paths: dict[str, Optional[str]]) -> None:
    SQL_OUT.parent.mkdir(parents=True, exist_ok=True)
    with SQL_OUT.open("w") as fh:
        fh.write("-- Seed deputados estaduais/distritais (ALESP + ALMG + TSE 2022).\n")
        fh.write("-- Idempotente via UNIQUE(source, source_external_id) — re-run seguro.\n")
        fh.write("BEGIN;\n")
        for p in people:
            has_photo = photo_paths.get(p.key) is not None
            avatar_key = (
                f"'mandates/{p.source}-{p.external_id}/avatar.jpg'" if has_photo else "NULL"
            )
            if p.public_email:
                email = p.public_email
            elif p.source == "tse":
                email = f"tse.{p.external_id}@parlamento.democracia.social.br"
            else:
                # source == 'assembleia' — use uf sigla for readable fallback.
                email = f"assembleia.{p.uf.lower()}.{p.external_id}@parlamento.democracia.social.br"
            new_id = str(uuid.uuid4())
            fh.write(
                "INSERT INTO mandate ("
                "id, org_id, office, display_name, public_email, is_candidate, "
                "created_at, party, uf, house, avatar_object_key, source, source_external_id, "
                "sphere, gender"
                ") VALUES ("
                f"'{new_id}', '{ORG_ID}', "
                f"{sql_quote(p.office)}, {sql_quote(p.name)}, {sql_quote(email)}, "
                f"false, now(), "
                f"{sql_quote(p.party)}, {sql_quote(p.uf)}, {sql_quote(p.house)}, "
                f"{avatar_key}, {sql_quote(p.source)}, {sql_quote(p.external_id)}, "
                f"{sql_quote(p.sphere)}, {sql_quote(p.gender)}"
                ") "
                "ON CONFLICT (source, source_external_id) "
                "WHERE source IS NOT NULL AND source_external_id IS NOT NULL "
                "DO UPDATE SET "
                "office = EXCLUDED.office, display_name = EXCLUDED.display_name, "
                "party = EXCLUDED.party, uf = EXCLUDED.uf, "
                "gender = COALESCE(EXCLUDED.gender, mandate.gender), "
                "avatar_object_key = COALESCE(EXCLUDED.avatar_object_key, mandate.avatar_object_key);"
                "\n"
            )
        fh.write("COMMIT;\n")
    print(f"\nWrote SQL → {SQL_OUT} ({SQL_OUT.stat().st_size//1024} KB)")


# ---------------------------------------------------------------------------
# Entrypoint
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    print("=== ALESP (SP) ===", flush=True)
    sp = fetch_alesp()
    print(f"  SP: {len(sp)} deputado(a)s em exercício", flush=True)

    print("\n=== ALMG (MG) ===", flush=True)
    mg = fetch_almg()
    print(f"  MG: {len(mg)} deputado(a)s em exercício", flush=True)

    print("\n=== TSE 2022 (demais UFs) ===", flush=True)
    tse = fetch_tse_2022(skip_ufs={"SP", "MG"})
    print(f"  TSE: {len(tse)} eleito(a)s em 25 UFs", flush=True)

    all_people = sp + mg + tse
    print(f"\nTotal: {len(all_people)} mandatos estaduais/distritais\n", flush=True)

    by_uf: dict[str, int] = {}
    for p in all_people:
        by_uf[p.uf] = by_uf.get(p.uf, 0) + 1
    print("Distribuição por UF (top 10):")
    for uf, n in sorted(by_uf.items(), key=lambda kv: -kv[1])[:10]:
        print(f"  {uf}: {n}")

    print("\nBaixando fotos (SP + MG apenas)…", flush=True)
    paths = download_photos(all_people)
    ok = sum(1 for v in paths.values() if v)
    print(f"Fotos OK: {ok}/{sum(1 for p in all_people if p.photo_url)}", flush=True)

    emit_sql(all_people, paths)

    print("\nPróximos passos (no operador):")
    print(f"  1. scp -r {PHOTOS_DIR} popsolutions@[2804:710:d0:9::a000]:/tmp/")
    print(f"  2. scp {SQL_OUT} popsolutions@[2804:710:d0:9::a000]:/tmp/")
    print("  3. ssh popsolutions@[...] (na VM):")
    print("       for f in /tmp/dsoc-fotos-estaduais/*.jpg; do")
    print("         id=$(basename \"$f\" .jpg)  # assembleia-<id>")
    print("         mc cp \"$f\" \"dsoc/dsoc-media/mandates/$id/avatar.jpg\"")
    print("       done")
    print("       sudo -u postgres psql democracia_social -f /tmp/dsoc-mandates-estaduais.sql")
    sys.exit(0)
