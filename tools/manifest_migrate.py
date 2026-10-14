#!/usr/bin/env python3
"""One-shot migration of evidence/manifest.md into evidence/<area>.jsonl shards.

Every Markdown row becomes one legacy JSON record with its ID, path, digest,
date, session label and class preserved. The script refuses to write unless the
records reproduce the original rows exactly. Shards follow the findings areas
that cite each row (most citations win), then digests, then specs, then
``foundation``.

Usage::

    python3 tools/manifest_migrate.py            # write shards, remove nothing
"""

from __future__ import annotations

from collections import Counter
from dataclasses import dataclass
import json
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "evidence" / "manifest.md"
ROW_ID = re.compile(r"M-\d{3}")
CITATION = re.compile(r"\bM-(\d{3,})(?:\.\.M-(\d{3,}))?\b")
AREAS: tuple[str, ...] = ("foundation", "network", "save-format", "save-format-c001", "save-format-classes")
DIGEST_AREAS: dict[str, str] = {
    "docs/research/save-format/c001-constructor.md": "save-format-c001",
    "docs/research/save-format/save-class-audits.md": "save-format-classes",
    "docs/research/network/control-stores.md": "network",
    "docs/research/foundation/foundation.md": "foundation",
}
SPEC_AREAS: dict[str, str] = {"docs/specs/save-format/": "save-format", "docs/specs/network/": "network"}
INPUT_PREFIXES: tuple[str, ...] = ("icarus-orig/", "icarus-client-orig/", "reference-data/", "Icarus/", "steamapps/")
REVIEW_WORDS = re.compile(r"review|verdict|analysis|proposal|interpretation|additive note", re.IGNORECASE)


@dataclass(frozen=True)
class Row:
    identifier: str
    path: str
    digest: str
    day: str
    session: str
    klass: str


def read_rows() -> list[Row]:
    rows: list[Row] = []
    for line in MANIFEST.read_text(encoding="utf-8").split("\n"):
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if line.startswith("|") and len(cells) == 6 and ROW_ID.fullmatch(cells[0]):
            rows.append(Row(*cells))
    return rows


def tracked_files() -> set[str]:
    result = subprocess.run(["git", "-C", str(ROOT), "ls-files"], check=True, capture_output=True, text=True)
    return set(result.stdout.split("\n"))


def is_tracked(path: str, files: set[str]) -> bool:
    target = path.partition("#")[0]
    if target.endswith("/"):
        return any(name.startswith(target) for name in files)
    return target in files


def cited(text: str) -> list[int]:
    numbers: list[int] = []
    for match in CITATION.finditer(text):
        first = int(match.group(1))
        last = int(match.group(2)) if match.group(2) else first
        numbers.extend(range(first, last + 1) if last >= first else (first,))
    return numbers


def citation_areas() -> tuple[dict[int, Counter[str]], dict[int, Counter[str]], dict[int, Counter[str]]]:
    findings: dict[int, Counter[str]] = {}
    digests: dict[int, Counter[str]] = {}
    specs: dict[int, Counter[str]] = {}
    for path in sorted((ROOT / "docs").rglob("*.md")):
        rel = path.relative_to(ROOT).as_posix()
        area: str | None = None
        target = findings
        if rel.startswith("docs/ledgers/findings/"):
            area = path.stem.removesuffix("-2")
        elif rel in DIGEST_AREAS:
            area, target = DIGEST_AREAS[rel], digests
        else:
            for prefix, spec_area in SPEC_AREAS.items():
                if rel.startswith(prefix):
                    area, target = spec_area, specs
        if area is None:
            continue
        if area not in AREAS:
            raise SystemExit(f"manifest_migrate: unknown area {area!r} from {rel}")
        for number in cited(path.read_text(encoding="utf-8")):
            target.setdefault(number, Counter())[area] += 1
    return findings, digests, specs


def shard_for(number: int, tables: tuple[dict[int, Counter[str]], ...]) -> str:
    for table in tables:
        counts = table.get(number)
        if counts:
            best = max(counts.values())
            return min(area for area, count in counts.items() if count == best)
    return "foundation"


def kind_for(row: Row, klass: str) -> str:
    path = row.path
    if path.startswith("commit ") or " commit " in path:
        return "revision"
    if "release library" in path or path.endswith(".rlib"):
        return "build"
    if path.startswith(INPUT_PREFIXES):
        return "input"
    if path == "tools/":
        return "build"
    if klass == "tracked" or path.startswith("history/") or path == "bug-ledger.md":
        return "report"
    if REVIEW_WORDS.search(path.rsplit("/", 1)[-1]) or REVIEW_WORDS.search(row.session):
        return "review"
    return "receipt"


def record_for(row: Row, files: set[str]) -> dict[str, object]:
    klass = "tracked" if row.klass == "external" and is_tracked(row.path, files) else row.klass
    return {
        "id": row.identifier,
        "path": row.path,
        "sha256": None if row.digest == "unstated" else row.digest,
        "class": klass,
        "kind": kind_for(row, klass),
        "date": None if row.day == "unstated" else row.day,
        "label": row.session,
        "status": "active",
        "legacy": True,
    }


def round_trip(record: dict[str, object]) -> Row:
    def text(key: str) -> str:
        value = record[key]
        return value if isinstance(value, str) else "unstated"

    klass = "external" if record["class"] == "tracked" else text("class")
    return Row(text("id"), text("path"), text("sha256"), text("date"), text("label"), klass)


def main() -> int:
    rows = read_rows()
    files = tracked_files()
    tables = citation_areas()
    shards: dict[str, list[dict[str, object]]] = {area: [] for area in AREAS}
    for row in rows:
        record = record_for(row, files)
        if round_trip(record) != row:
            raise SystemExit(f"manifest_migrate: {row.identifier} does not round-trip")
        shards[shard_for(int(row.identifier[2:]), tables)].append(record)
    written = sorted(str(record["id"]) for records in shards.values() for record in records)
    if written != sorted(row.identifier for row in rows) or len(set(written)) != len(rows):
        raise SystemExit("manifest_migrate: ID set changed")
    for area, records in shards.items():
        records.sort(key=lambda record: int(str(record["id"])[2:]))
        lines = [json.dumps(record, ensure_ascii=False, separators=(",", ":")) for record in records]
        _ = (ROOT / "evidence" / f"{area}.jsonl").write_text("".join(f"{line}\n" for line in lines), encoding="utf-8")
        kinds = Counter(str(record["kind"]) for record in records)
        print(f"{area}: {len(records)} rows; " + ", ".join(f"{kind} {count}" for kind, count in sorted(kinds.items())))
    print(f"manifest_migrate: {len(rows)} rows round-trip exactly")
    return 0


if __name__ == "__main__":
    sys.exit(main())
