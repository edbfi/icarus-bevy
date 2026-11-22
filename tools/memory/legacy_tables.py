# SPDX-License-Identifier: AGPL-3.0-or-later
"""The legacy sources the evidence catalog keeps only as text: compatibility and bug tables, and releases.

Conversions follow ``tools.memory.legacy_rules`` (verbatim cells, ``legacy:`` values, gaps, legacy-text scope).

- **Tables** (``table_rows``): a Markdown table is a header row, a ``| --- |`` separator and body rows,
  split on unescaped pipes; cells are stripped and ``\\|`` decoded, as the catalog does. Legacy cell
  names are the header texts slugged (lower case, runs of other characters to ``_``). A row whose line
  is not the canonical ``| a | b |`` rendering of its cells also keeps the line as ``original``.
  ``compatibility.md`` rows (headers ``Surface | Status | Required proof`` and ``Subsurface | Status |
  Evidence | Remaining gate``) become ``CP-001...`` in file order: name from the first cell,
  ``required_proof`` from the last, ``qualifier`` a gap. ``bugs.md`` campaign rows (``Priority | Reported
  problem | Status | Next evidence``) become ``RP-001...`` in file order and static-lead rows (``Lead |
  ...``) keep the ``SL-NNN`` that opens their Lead cell; the lead text is the rest of that cell, the
  status ``legacy:unstated`` with the table's ``framing`` as the ``table`` cell. Any other table in these
  two files, except the catalog's own N table, is an error. Each row links the IDs of its cells
  (``table_links``).
- **Framing** (``framing``): the lines from the nearest ``#`` heading above a table to the line before
  its header, verbatim, trailing blank lines dropped: where a status the table does not state would come from.
- Specs and other documents become references in ``tools.memory.legacy_prose``.
- **Releases** (``releases``): ``REL-x.y.z`` from explicit statements only, each kept in ``sources``:
  the ``[release]`` table of ``config/release.toml`` (status ``current``) and of each
  ``tests/fixtures/releases/*.toml`` (``past``), and finding rows that pair a build with a release
  (``RELEASE_PAIRS``). A profile's ``build_id`` is a server build when its ``app_id`` is the app
  ``README.md`` names as the official dedicated server; a header comment's role next to a build
  (``ROLE``) also states it. A role is set only when every statement of it agrees; any disagreement,
  and every pairing that names no role, is kept as a note. Dates are never stated in these sources and
  stay a gap.
"""

from __future__ import annotations

import re
import tomllib
from collections import defaultdict
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path

from tools.memory.legacy_reader import PIPE, Catalog, Entry, Index, references
from tools.memory.gitio import read_blobs, tree_files
from tools.memory.legacy_rules import (
    BUILD, EVIDENCE_CELL, RELEASE, REVISION_CELLS, ROLE, SELECTOR_SUFFIX, SELECTORS, UNSTATED, UNSTATED_NOTE, Migrated, MigrationError, extract, finish,
    vocabulary,
)
from tools.memory.model import JSON, TYPES, as_mapping, id_key


COMPATIBILITY = "docs/ledgers/compatibility.md"
BUGS = "docs/ledgers/bugs.md"
RELEASE_CONFIG = "config/release.toml"
RELEASE_FIXTURES = "tests/fixtures/releases/"
SURFACES = ("Surface", "Status", "Required proof")
SUBSURFACES = ("Subsurface", "Status", "Evidence", "Remaining gate")
CAMPAIGNS = ("Priority", "Reported problem", "Status", "Next evidence")
LEADS = ("Lead", "Static evidence", "What would establish or refute a problem")
NATIVE = ("ID", "Trigger and former behavior", "Change", "Findings")
LEAD_ID = re.compile(r"(SL-[0-9]{3,}): (.+)")
SERVER_APP = re.compile(r"dedicated server \(Steam app ([0-9]+)\)")
_BUILD = BUILD.pattern
_RELEASE = RELEASE.pattern
# A finding pairs a build with a release: ``N (R)``, ``R (N)`` or ``N pins|declares R``.
RELEASE_PAIRS = (
    re.compile(rf"{_BUILD}\s*\({_RELEASE}[^)]*\)"),
    re.compile(rf"{_RELEASE}\s*\((?:build\s+)?{_BUILD}\)"),
    re.compile(rf"{_BUILD}\s+(?:pins|declares)\s+{_RELEASE}"),
)


@dataclass(frozen=True)
class Row:
    path: str
    line: int
    header: tuple[str, ...]
    cells: tuple[str, ...]
    original: str | None


def slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")


def split_row(line: str) -> tuple[str, ...]:
    parts = PIPE.split(line)
    if parts[0].strip() or parts[-1].strip() or len(parts) < 3:
        raise MigrationError(f"malformed table row {line!r}")
    return tuple(part.strip().replace(r"\|", "|") for part in parts[1:-1])


def table_rows(path: str, lines: Iterator[tuple[int, str]]) -> Iterator[Row]:
    """Body rows of every table in ``lines``; header and separator rows are consumed."""
    header: tuple[str, ...] | None = None
    previous: str | None = None
    for number, line in lines:
        if not line.startswith("|"):
            header, previous = None, None
            continue
        if header is None:
            if previous is None:
                previous = line
                continue
            if not re.fullmatch(r"\|(?:\s*:?-+:?\s*\|)+", line):
                raise MigrationError(f"{path}:{number}: table without a separator row")
            header, previous = split_row(previous), None
            continue
        cells = split_row(line)
        if len(cells) != len(header):
            raise MigrationError(f"{path}:{number}: {len(cells)} cells under a {len(header)}-column header")
        canonical = "| " + " | ".join(cell.replace("|", r"\|") for cell in cells) + " |"
        yield Row(path, number, header, cells, None if canonical == line else line)


def legacy_of(row: Row) -> dict[str, JSON]:
    cells: dict[str, JSON] = {slug(name): cell for name, cell in zip(row.header, row.cells, strict=True)}
    if row.original is not None:
        cells["original"] = row.original
    return {"path": row.path, "cells": cells}


def text_lines(catalog: Catalog, path: str) -> Iterator[tuple[int, str]]:
    document = catalog.documents.get(path)
    if document is None:
        return
    for number, line in enumerate(document.lines, 1):
        yield number, line if isinstance(line, str) else ""


def framing(catalog: Catalog, path: str, line: int) -> str:
    """The heading and preamble of the table holding ``path:line`` (module docstring)."""
    document = catalog.documents.get(path)
    # A row the catalog parsed is a record, not text: it is a table row too.
    lines = [text if isinstance(text, str) else "|" for text in (document.lines if document is not None else ())]
    index = line - 1
    while index > 0 and lines[index - 1].startswith("|"):
        index -= 1
    start = index
    while start > 0 and not lines[start - 1].startswith("#"):
        start -= 1
    kept = lines[max(start - 1, 0):index]
    while kept and not kept[-1].strip():
        _ = kept.pop()
    return "\n".join(kept)


def table_links(cells: dict[str, str], evidence_cell: str | None, known: dict[str, Entry]) -> dict[str, JSON]:
    """``cites`` and ``refs`` of a table row: every selector, and every evidence ID of ``evidence_cell``, is
    cited; every other ID the catalog defines is a ref (ranges expand over defined IDs only)."""
    cites: set[str] = set()
    refs: set[str] = set()
    for name, text in cells.items():
        stripped = text
        for pattern in SELECTORS:
            cites.update(match[0] for match in pattern.finditer(text))
            stripped = pattern.sub(" ", stripped)
        bases = {match[1] for pattern in SELECTORS for match in pattern.finditer(text)}
        bare = set(references(stripped, known)) | (set(references(SELECTOR_SUFFIX.sub(r"\1", text), known)) - bases)
        for target in bare & known.keys():
            (cites if target.startswith("M-") and name == evidence_cell else refs).add(target)
    links: dict[str, JSON] = {}
    if cites:
        links["cites"] = list[JSON](sorted(cites, key=id_key))
    if refs:
        links["refs"] = list[JSON](sorted(refs, key=id_key))
    return links


def tables(catalog: Catalog, index: Index) -> list[Migrated]:
    """CP, RP and SL records of the compatibility and bug ledgers (see the module docstring)."""
    result: list[Migrated] = []
    counters = {"CP": 0, "RP": 0}
    for row in table_rows(COMPATIBILITY, text_lines(catalog, COMPATIBILITY)):
        if row.header not in (SURFACES, SUBSURFACES):
            raise MigrationError(f"{row.path}:{row.line}: unrecognised table {' | '.join(row.header)}")
        counters["CP"] += 1
        named = dict(zip(row.header, row.cells, strict=True))
        data: dict[str, JSON] = {
            "id": f"CP-{counters['CP']:03d}", "type": "compatibility", "assigned_at_migration": True,
            "status": vocabulary(named["Status"], TYPES["compatibility"].statuses), "name": row.cells[0],
            "required_proof": row.cells[-1],
        }
        cells = {slug(name): cell for name, cell in named.items()}
        extraction = extract(cells, (), REVISION_CELLS["CP"])
        if extraction.scope is not None:
            data["scope"] = extraction.scope
        data.update(table_links(cells, EVIDENCE_CELL["CP"], index.entries))
        result.append(Migrated(finish(data, legacy_of(row), extraction), f"{row.path}:{row.line}", extraction))
    for row in table_rows(BUGS, text_lines(catalog, BUGS)):
        named = dict(zip(row.header, row.cells, strict=True))
        cells = {slug(name): cell for name, cell in named.items()}
        if row.header == CAMPAIGNS:
            counters["RP"] += 1
            data = {
                "id": f"RP-{counters['RP']:03d}", "type": "report", "assigned_at_migration": True,
                "status": vocabulary(named["Status"], TYPES["report"].statuses), "problem": named["Reported problem"],
                "next_evidence": named["Next evidence"],
            }
            if named["Priority"].isdigit():
                data["priority"] = int(named["Priority"])
            extraction = extract(cells)
            data.update(table_links(cells, None, index.entries))
            legacy = legacy_of(row)
        elif row.header == LEADS:
            match = LEAD_ID.fullmatch(named["Lead"])
            if match is None:
                raise MigrationError(f"{row.path}:{row.line}: a lead cell must open with SL-NNN: ")
            data = {
                "id": match[1], "type": "lead", "status": UNSTATED, "lead": match[2],
                "static_evidence": named["Static evidence"], "refute_by": named["What would establish or refute a problem"],
                "notes": [UNSTATED_NOTE],
            }
            extraction = extract(cells, (), REVISION_CELLS["SL"])
            data.update(table_links(cells, EVIDENCE_CELL["SL"], index.entries))
            legacy = legacy_of(row)
            cell_map = legacy["cells"]
            assert isinstance(cell_map, dict)
            cell_map["table"] = framing(catalog, row.path, row.line)
        elif row.header == NATIVE:
            raise MigrationError(f"{row.path}:{row.line}: a native defect row the catalog did not parse")
        else:
            raise MigrationError(f"{row.path}:{row.line}: unrecognised table {' | '.join(row.header)}")
        if extraction.scope is not None:
            data["scope"] = extraction.scope
        result.append(Migrated(finish(data, legacy, extraction), f"{row.path}:{row.line}", extraction))
    return result


def git_texts(root: Path, revision: str, prefix: str) -> dict[str, str]:
    files = tree_files(root, revision, prefix)
    paths = sorted(files)
    texts: dict[str, str] = {}
    for path, data in zip(paths, read_blobs(root, [files[path] for path in paths]), strict=True):
        if data is None:
            raise MigrationError(f"{revision}:{path}: blob missing")
        texts[path] = data.decode("utf-8")
    return texts


@dataclass(frozen=True)
class Statement:
    """One explicit statement about a release. ``field`` is version_string, version_token (a release token
    in a finding), server_build, client_build or any_build (a pairing that names no role). ``key`` and
    ``cell`` are the verbatim source kept in the record's ``legacy`` cells; ``source`` is a short locator."""

    release: str
    field: str
    value: str
    source: str
    key: str
    cell: str


def profile_statements(path: str, text: str, server_app: str | None, readme: str) -> list[Statement]:
    """Statements of one release profile: its ``[release]`` values and its header comment's roles."""
    raw = as_mapping(tomllib.loads(text).get("release"), f"{path}: release")
    version_string, build, app = raw.get("project_version"), raw.get("build_id"), raw.get("app_id")
    if not isinstance(version_string, str) or not isinstance(build, int) or not isinstance(app, int):
        raise MigrationError(f"{path}: [release] needs project_version, build_id and app_id")
    match = RELEASE.match(version_string)
    if match is None:
        raise MigrationError(f"{path}: project_version {version_string!r} names no release")
    version = ".".join(match[1].split(".")[:3])
    table = "[release]" + text.split("\n[release]", 1)[1].split("\n\n", 1)[0]
    header = "\n".join(line for line in text.split("[release]", 1)[0].split("\n") if line.startswith("#"))
    statements = [Statement(version, "version_string", version_string, path, f"{path} [release]", table)]
    if server_app is not None and str(app) == server_app:
        statements.append(Statement(version, "server_build", str(build), f"{path} ({readme})", f"{path} [release]", table))
    for found in BUILD.finditer(header):
        if (role := ROLE.search(header[:found.start()])) is not None:
            statements.append(Statement(version, f"{role[1].lower()}_build", found[1], f"{path} header", f"{path} header", header))
    return statements


def finding_statements(index: Index) -> list[Statement]:
    """Build/release pairs stated in finding cells (``RELEASE_PAIRS``); a role only where stated (``ROLE``)."""
    statements: list[Statement] = []
    for identifier in sorted(index.entries, key=id_key):
        entry = index.entries[identifier]
        if entry.record.kind != "F":
            continue
        for name, value in entry.record.fields.items():
            text = str(value)
            for pattern in RELEASE_PAIRS:
                for match in pattern.finditer(text):
                    build = BUILD.search(text, match.start(), match.end())
                    release = RELEASE.search(text, match.start(), match.end())
                    if build is None or release is None:
                        raise MigrationError(f"{identifier}: release pair {match[0]!r} without its build or release")
                    role = ROLE.search(text[:build.start()]) if build.start() == match.start() else None
                    version = ".".join(release[1].split(".")[:3])
                    source = f"{identifier} {name}"
                    key = f"{entry.path} {identifier} {name}"
                    statements.append(Statement(version, "version_token", release[1], source, key, text))
                    statements.append(Statement(version, f"{role[1].lower()}_build" if role else "any_build", build[1], source, key, text))
    return statements


def releases(root: Path, revision: str, index: Index) -> list[Migrated]:
    """REL records from the release profiles and the finding rows at ``revision`` (see the module docstring)."""
    readme_text = git_texts(root, revision, "README.md").get("README.md", "")
    server_app, readme = None, "README.md"
    for number, line in enumerate(readme_text.split("\n"), 1):
        if match := SERVER_APP.search(line):
            server_app, readme = match[1], f"app {match[1]}, README.md:{number}"
            break
    profiles = {
        path: text for path, text in {**git_texts(root, revision, RELEASE_CONFIG), **git_texts(root, revision, RELEASE_FIXTURES)}.items()
        if path.endswith(".toml")
    }
    statements: list[Statement] = []
    for path in sorted(profiles, key=lambda item: (item != RELEASE_CONFIG, item)):
        statements.extend(profile_statements(path, profiles[path], server_app, readme))
    statements.extend(finding_statements(index))
    by_release: dict[str, list[Statement]] = defaultdict(list)
    for statement in statements:
        by_release[statement.release].append(statement)
    current = {statement.release for statement in statements if statement.source.startswith(RELEASE_CONFIG)}
    return [
        release_record(version, by_release[version], version in current)
        for version in sorted(by_release, key=lambda item: tuple(int(part) for part in item.split(".")))
    ]


def release_record(version: str, statements: list[Statement], current: bool) -> Migrated:
    """One REL record: a field is set only when its statements agree; everything else becomes a note."""
    data: dict[str, JSON] = {"id": f"REL-{version}", "type": "release", "status": "current" if current else "past"}
    notes: list[str] = []
    for name in ("version_string", "server_build", "client_build"):
        stated = [item for item in statements if item.field == name] or [item for item in statements if name == "version_string" and item.field == "version_token"]
        values = sorted({item.value for item in stated})
        if len(values) == 1:
            data[name] = values[0]
        elif values:
            notes.append(f"{name} not set; the sources disagree: " + "; ".join(f"{item.value} ({item.source})" for item in stated))
    roles: dict[str, set[str]] = defaultdict(set)
    for item in statements:
        if item.field in ("server_build", "client_build"):
            roles[item.value].add(item.field.removesuffix("_build"))
        elif item.field == "any_build":
            notes.append(f"build {item.value} paired with {version} without a role in {item.source}")
    notes.extend(f"build {value} is stated both as server and as client build" for value, found in sorted(roles.items()) if len(found) > 1)
    data["sources"] = list[JSON](dict.fromkeys(f"{item.field} {item.value}: {item.source}" for item in statements))
    if notes:
        data["notes"] = list[JSON](dict.fromkeys(notes))
    cells: dict[str, JSON] = {}
    for item in statements:
        _ = cells.setdefault(item.key, item.cell)
    legacy: dict[str, JSON] = {"path": statements[0].key.split(" ", 1)[0], "cells": cells}
    return Migrated(finish(data, legacy), statements[0].source)
