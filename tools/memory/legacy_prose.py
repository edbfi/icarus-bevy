# SPDX-License-Identifier: AGPL-3.0-or-later
"""Reference records for every legacy document the cut-over deletes, and the byte accounting of them.

``references(root, revision)`` returns ``REF`` records whose body is source text verbatim, in this order
(numbers are assigned in it, so specs keep ``REF-001`` to ``REF-011``):

1. every ``docs/specs/**/*.md``, in path order: the whole file;
2. every research digest ``docs/research/<area>/*.md`` outside ``docs/research/archive/``, in path order:
   the whole file;
3. every ledger ``docs/ledgers/**/*.md``, in path order, that has text outside its table rows: that
   text, in file order, with each table (``split``: a ``|`` line followed by a separator line, and the
   ``|`` lines after it) replaced by the marker line ``MARKER``; the rows are records of their own and the
   header and separator lines are structural. The marker names no line numbers or counts, so adding a
   row to a ledger leaves its prose reference unchanged;
4. ``docs/implementation-status.md``: the whole file;
5. ``evidence/README.md``: the whole file.

A REF's title is the text of the file's first ``# `` line (a gap otherwise); its area the folder under
``docs/specs/`` or ``docs/research/``, or the catalog topic of a ledger; its ``legacy`` cells the first
line verbatim (``heading``) and the full commit ID (``revision``). Scope is ``legacy-text``: a spec's
tokens come from its ``## Scope`` section (commits included); any other body's builds and releases come
from the whole text, and no commit is taken, since prose has no source-revision cell.

``account(root, revision, records)`` classifies every byte (each line with its LF) of every file under
``ACCOUNTED`` at ``revision``: ``record`` (a table row or shard line reproduced by a record's verbatim
``legacy`` cells), ``reference`` (text a REF body holds verbatim, in order), ``structural`` (table header
and separator lines, blank shard lines) or ``unaccounted``.
"""

from __future__ import annotations

import re
from collections import Counter
from collections.abc import Iterable, Mapping
from dataclasses import dataclass, field
from pathlib import Path

from tools.evidence_catalog_model import COLUMNS, topic
from tools.memory.gitio import git, read_blobs
from tools.memory.legacy_rules import Migrated, MigrationError, extract, finish
from tools.memory.model import JSON, Record, sha256_hex

SPECS = "docs/specs/"
DIGESTS = "docs/research/"
ARCHIVE = "docs/research/archive/"
LEDGERS = "docs/ledgers/"
STATUS = "docs/implementation-status.md"
EVIDENCE = "evidence/"
EVIDENCE_README = "evidence/README.md"
ACCOUNTED = (LEDGERS, SPECS, DIGESTS, STATUS, EVIDENCE)
SEPARATOR = re.compile(r"\|(?:\s*:?-+:?\s*\|)+")
MARKER = "<!-- table: its rows are migrated as records -->"


@dataclass(frozen=True)
class Line:
    """One source line: ``kind`` is prose, header, separator or row; ``size`` counts its bytes with the LF."""

    number: int
    text: str
    kind: str
    size: int


def lines_of(text: str) -> list[Line]:
    """Every line of ``text`` classified (``split``'s table rule); the empty tail after a final LF is dropped."""
    raw = text.split("\n")
    sizes = [len(line.encode()) + 1 for line in raw[:-1]] + [len(raw[-1].encode())]
    kinds = ["prose"] * len(raw)
    index = 0
    while index < len(raw):
        if raw[index].startswith("|") and index + 1 < len(raw) and SEPARATOR.fullmatch(raw[index + 1]):
            kinds[index], kinds[index + 1] = "header", "separator"
            index += 2
            while index < len(raw) and raw[index].startswith("|"):
                kinds[index] = "row"
                index += 1
            continue
        index += 1
    found = [Line(number, line, kind, size) for number, (line, kind, size) in enumerate(zip(raw, kinds, sizes, strict=True), 1)]
    return found[:-1] if raw[-1] == "" else found


def prose_body(text: str) -> str | None:
    """The text outside the tables of ``text`` with a marker per table, or ``None`` when it has no such text."""
    out: list[str] = []
    lines = lines_of(text)
    in_table = False
    for line in lines:
        if line.kind != "prose":
            if not in_table:
                out.append(MARKER)
            in_table = True
            continue
        in_table = False
        out.append(line.text)
    if not any(line.kind == "prose" and line.text.strip() for line in lines):
        return None
    return "\n".join(out) + ("\n" if text.endswith("\n") else "")


def texts(root: Path, revision: str, *prefixes: str) -> dict[str, str]:
    """UTF-8 text of the regular files under ``prefixes`` at ``revision``, by path."""
    files: dict[str, str] = {}
    for item in git(root, "ls-tree", "-r", "-z", "--end-of-options", revision, "--", *prefixes).decode().split("\0"):
        metadata, _, path = item.partition("\t")
        if item:
            mode, _kind, oid = metadata.split()
            if mode not in ("100644", "100755"):
                raise MigrationError(f"{revision}:{path}: not a regular file (mode {mode})")
            files[path] = oid
    paths = sorted(files)
    found: dict[str, str] = {}
    for path, data in zip(paths, read_blobs(root, [files[path] for path in paths]), strict=True):
        if data is None:
            raise MigrationError(f"{revision}:{path}: blob missing")
        found[path] = data.decode("utf-8")
    return found


def sources(root: Path, revision: str) -> list[tuple[str, str, str]]:
    """``(path, body, kind)`` of every REF in number order; ``kind`` is spec, digest, ledger, status or readme."""
    found = texts(root, revision, SPECS, DIGESTS, LEDGERS, STATUS, EVIDENCE_README)
    ordered: list[tuple[str, str, str]] = []
    ordered += [(path, text, "spec") for path, text in sorted(found.items()) if path.startswith(SPECS) and path.endswith(".md")]
    ordered += [(path, text, "digest") for path, text in sorted(found.items())
                if path.startswith(DIGESTS) and not path.startswith(ARCHIVE) and path.endswith(".md") and path.count("/") == 3]
    for path, text in sorted(found.items()):
        if path.startswith(LEDGERS) and path.endswith(".md") and (body := prose_body(text)) is not None:
            ordered.append((path, body, "ledger"))
    ordered += [(path, found[path], kind) for path, kind in ((STATUS, "status"), (EVIDENCE_README, "readme")) if path in found]
    return ordered


def section(text: str, heading: str) -> str:
    """The lines under ``## heading`` up to the next ``## `` heading."""
    lines = text.split("\n")
    for number, line in enumerate(lines):
        if line == f"## {heading}":
            end = next((index for index in range(number + 1, len(lines)) if lines[index].startswith("## ")), len(lines))
            return "\n".join(lines[number + 1:end])
    return ""


def references(root: Path, revision: str) -> list[Migrated]:
    """REF records of every source at ``revision`` (module docstring)."""
    result: list[Migrated] = []
    for number, (path, body, kind) in enumerate(sources(root, revision), 1):
        data: dict[str, JSON] = {"id": f"REF-{number:03d}", "type": "reference", "status": "current"}
        heading = next((line for line in body.split("\n") if line.startswith("# ") and line[2:].strip()), None)
        if heading is not None:
            data["title"] = heading[2:].strip()
        parts = path.split("/")
        area = parts[2] if kind in ("spec", "digest") and len(parts) == 4 else topic(path) if kind == "ledger" else None
        if area:
            data["area"] = area
        extraction = extract({"scope": section(body, "Scope")}, ("scope",)) if kind == "spec" else extract({"body": body})
        if extraction.scope is not None:
            data["scope"] = extraction.scope
        data["body_sha256"] = sha256_hex(body.encode())
        cells: dict[str, JSON] = {"heading": body.split("\n", 1)[0], "revision": revision}
        record = finish(data, {"path": path, "cells": cells}, extraction, body)
        result.append(Migrated(record, f"{path}:1", extraction))
    return result


@dataclass
class Accounting:
    """Bytes per class and per file; ``problems`` names every unaccounted line."""

    bytes: Counter[str] = field(default_factory=Counter[str])
    files: int = 0
    problems: list[str] = field(default_factory=list[str])


def renderings(record: Record, headers: Iterable[tuple[str, ...]]) -> set[str]:
    """Every line ``record``'s verbatim legacy cells reproduce: its kept original, its shard row, or its
    cells in the column order of the catalog or of a table header in its file."""
    legacy = record.legacy
    if legacy is None:
        return set()
    cells = legacy.cells
    found = {cells[name] for name in ("original", "row") if name in cells}
    orders: list[tuple[str, ...]] = []
    prefix = record.id.split("-", 1)[0]
    if prefix in COLUMNS:
        orders.append((record.id, *(cells.get(name, "\0") for name in COLUMNS[prefix])))
    for header in headers:
        orders.append(tuple(cells.get(re.sub(r"[^a-z0-9]+", "_", name.lower()).strip("_"), "\0") for name in header))
    found.update("| " + " | ".join(cell.replace("|", r"\|") for cell in order) + " |" for order in orders if "\0" not in order)
    return found


def account(root: Path, revision: str, records: Mapping[str, Record]) -> Accounting:
    result = Accounting()
    by_path: dict[str, list[Record]] = {}
    for record in records.values():
        if record.legacy is not None:
            by_path.setdefault(record.legacy.path, []).append(record)
    for path, text in sorted(texts(root, revision, *ACCOUNTED).items()):
        if path.startswith(ARCHIVE):
            continue
        result.files += 1
        owned = by_path.get(path, [])
        body = next((record.body for record in owned if record.type == "reference" and record.body is not None), None)
        if body == text:
            result.bytes["reference"] += len(text.encode())
            continue
        lines = lines_of(text)
        if path.endswith(".jsonl"):
            rows = {cell for record in owned if record.type == "evidence" and record.legacy for cell in (record.legacy.cells.get("row"),) if cell}
            for line in lines:
                kind = "record" if line.text in rows else "structural" if not line.text else "unaccounted"
                tally(result, path, line, kind)
            continue
        headers = [tuple(cell.strip() for cell in line.text.strip("|").split("|")) for line in lines if line.kind == "header"]
        rows = {line for record in owned if record.type != "reference" for line in renderings(record, headers)}
        prose = [line for line in (body or "").split("\n") if line != MARKER]
        expected = [line.text for line in lines if line.kind == "prose"]
        in_body = prose[:len(expected)] == expected and prose[len(expected):] in ([], [""])
        for line in lines:
            match line.kind:
                case "header" | "separator":
                    kind = "structural"
                case "row":
                    kind = "record" if line.text in rows else "unaccounted"
                case _:
                    kind = "reference" if in_body else "unaccounted"
            tally(result, path, line, kind)
    return result


def tally(result: Accounting, path: str, line: Line, kind: str) -> None:
    result.bytes[kind] += line.size
    if kind == "unaccounted":
        result.problems.append(f"{path}:{line.number}: {line.size} byte(s) in no record, reference or structural line")
