# SPDX-License-Identifier: AGPL-3.0-or-later
"""Migration of the legacy knowledge system, as it stands at one Git revision, into journal records.

``export(root, rev)`` reads the revision through the frozen legacy reader (``tools.memory.legacy_reader``:
one parse path for F, U, N and M rows of every topic), converts each row by the rules in
``tools.memory.legacy_rules``, adds what the catalog keeps only as text (``tools.memory.legacy_tables``:
compatibility, campaign and lead tables, releases) and every document the cut-over deletes as a
reference (``tools.memory.legacy_prose``: specs, digests, ledger prose, the status file, the evidence
README), and returns every record once, as a first revision. Nothing is checked out and no artifact is read or hashed.

``write(result, out)`` writes the segments under ``out/memory/journal``, one change set per record
type, each segment created at the revision's commit time: exporting one revision twice yields
byte-identical files.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path

from tools.memory import legacy_prose, legacy_tables
from tools.memory.gitio import git
from tools.memory.journal import JOURNAL, Revision, read_tree, split_change_set, write_change_set
from tools.memory.legacy_reader import Catalog, Entry, Index, commit_id, references, topic
from tools.memory.legacy_rules import (
    COMMIT_CELLS, EVIDENCE_CELL, PROSE_SUPERSESSION, SELECTOR_SUFFIX, SELECTORS, Migrated, MigrationError, extract, finish,
    implied_status, standing, vocabulary,
)
from tools.memory.model import CONFIDENCES, EVIDENCE_CLASSES, EVIDENCE_KINDS, FINDING_KINDS, JSON, SEVERITIES, TYPES, Record, id_key

# The record types a branch's legacy rows become (``salvage``, ``check``); references and releases are not rows.
SALVAGED = ("finding", "unknown", "bug", "evidence", "lead", "report", "compatibility")
ORDER = ("evidence", "finding", "unknown", "bug", "lead", "report", "compatibility", "reference", "release")


@dataclass(frozen=True)
class ProseLink:
    old: str
    new: str
    path: str
    line: int
    text: str


@dataclass(frozen=True)
class ChangeSet:
    name: str
    revisions: tuple[Revision, ...]


@dataclass
class Export:
    """Every record of one revision, in ``ORDER`` and ID order, and what the migration report needs."""

    revision: str
    created: datetime
    migrated: list[Migrated] = field(default_factory=list[Migrated])
    prose_links: list[ProseLink] = field(default_factory=list[ProseLink])
    commits: dict[str, tuple[str, ...]] = field(default_factory=dict[str, tuple[str, ...]])
    catalog: Catalog | None = None
    index: Index | None = None

    def records(self) -> dict[str, Record]:
        return {item.record.id: item.record for item in self.migrated}

    def change_sets(self) -> list[ChangeSet]:
        """One change set per record type, in ``ORDER``; every revision is a first revision."""
        result: list[ChangeSet] = []
        for kind in ORDER:
            records = sorted((item.record for item in self.migrated if item.record.type == kind), key=lambda r: id_key(r.id))
            if records:
                result.append(ChangeSet(kind, tuple(Revision.of(record) for record in records)))
        return result

    def segments(self) -> list[tuple[str, bytes]]:
        """Repository-relative paths and bytes of every segment, in change-set order."""
        return [item for change_set in self.change_sets() for item in split_change_set(self.created, change_set.revisions)]


def legacy_cells(entry: Entry) -> dict[str, JSON]:
    cells: dict[str, JSON] = {name: str(value) for name, value in entry.record.fields.items()}
    if entry.record.original is not None:
        cells["original"] = entry.record.original
    return cells


def row_links(entry: Entry, index: Index, *, resolved: bool) -> dict[str, list[str]]:
    """``cites``, ``refs``, ``superseded_by`` and ``resolved_by`` of one ledger row (see the module docstring)."""
    record = entry.record
    prefix = record.kind
    evidence_cell = EVIDENCE_CELL.get(prefix)
    found: dict[str, set[str]] = {"cites": set(), "refs": set(), "superseded_by": set(), "resolved_by": set()}
    bare: dict[str, set[str]] = {}
    for name, value in record.fields.items():
        text = stripped = str(value)
        for pattern in SELECTORS:
            stripped = pattern.sub(" ", stripped)
        # A range may end in a selector (``M-1..M-4/ev_x``): its inner IDs are bare citations.
        bases = {match[1] for pattern in SELECTORS for match in pattern.finditer(text)}
        bare[name] = set(references(stripped, index.entries)) | (set(references(SELECTOR_SUFFIX.sub(r"\1", text), index.entries)) - bases)
    retired = {link.target for link in index.links[record.identifier] if link.kind == "supersedes_to"}
    for link in index.links[record.identifier]:
        target = link.target
        if link.kind == "supersedes_to":
            found["superseded_by"].add(target)
        elif "/" in target:
            found["cites"].add(target)
        elif target not in bare[link.field] or (link.field == "status" and target in retired):
            continue
        elif target.startswith("M-") and link.field == evidence_cell:
            found["cites"].add(target)
        elif resolved and link.field == "findings" and target.startswith("F-"):
            found["resolved_by"].add(target)
        else:
            found["refs"].add(target)
    found["refs"] -= found["resolved_by"]
    return {name: sorted(items, key=id_key) for name, items in found.items() if items}


def put_links(data: dict[str, JSON], legacy: dict[str, JSON], links: Mapping[str, list[str]]) -> None:
    for name in ("cites", "refs"):
        if name in links:
            data[name] = list[JSON](links[name])
    for name in ("superseded_by", "resolved_by"):
        if name in links:
            legacy[name] = list[JSON](links[name])


def convert_finding(entry: Entry, index: Index, prose: Sequence[ProseLink]) -> Migrated:
    cells = {name: str(value) for name, value in entry.record.fields.items()}
    identifier = entry.record.identifier
    status = cells["status"]
    data: dict[str, JSON] = {
        "id": identifier, "type": "finding", "status": vocabulary(status, TYPES["finding"].statuses),
        "claim": cells["claim"], "kind": vocabulary(cells["kind"], FINDING_KINDS),
    }
    if confidence := cells["scope_and_limits"].split(";", 1)[0].strip():
        data["confidence"] = vocabulary(confidence, CONFIDENCES)
    if (stated := standing(cells.values())) is not None:
        data["standing"] = stated
    if (area := topic(entry.path)) is not None:
        data["area"] = area
    extraction = extract(cells, COMMIT_CELLS["F"])
    if extraction.scope is not None:
        data["scope"] = extraction.scope
    legacy: dict[str, JSON] = {"path": entry.path, "cells": legacy_cells(entry)}
    links = dict(row_links(entry, index, resolved=False))
    stated_prose = [link for link in prose if link.old == identifier]
    if stated_prose:
        links["superseded_by"] = sorted({*links.get("superseded_by", []), *(link.new for link in stated_prose)}, key=id_key)
        cell_map = legacy["cells"]
        assert isinstance(cell_map, dict)
        for link in stated_prose:
            cell_map[f"prose {link.path}:{link.line}"] = link.text
    put_links(data, legacy, links)
    if (implied := implied_status(status, "finding")) is not None:
        legacy["implied_status"] = implied
    return Migrated(finish(data, legacy, extraction), f"{entry.path}:{entry.line}", extraction)


def convert_unknown(entry: Entry, index: Index) -> Migrated:
    cells = {name: str(value) for name, value in entry.record.fields.items()}
    status = cells["status"]
    implied = implied_status(status, "unknown")
    data: dict[str, JSON] = {
        "id": entry.record.identifier, "type": "unknown", "status": vocabulary(status, TYPES["unknown"].statuses),
        "question": cells["question"],
    }
    if cells["severity"]:
        data["severity"] = vocabulary(cells["severity"], SEVERITIES)
    if cells["next_probe"]:
        data["next_probe"] = cells["next_probe"]
    extraction = extract(cells, COMMIT_CELLS["U"])
    if extraction.scope is not None:
        data["scope"] = extraction.scope
    legacy: dict[str, JSON] = {"path": entry.path, "cells": legacy_cells(entry)}
    put_links(data, legacy, row_links(entry, index, resolved=implied == "resolved"))
    if implied is not None:
        legacy["implied_status"] = implied
    return Migrated(finish(data, legacy, extraction), f"{entry.path}:{entry.line}", extraction)


def convert_bug(entry: Entry, index: Index) -> Migrated:
    cells = {name: str(value) for name, value in entry.record.fields.items()}
    data: dict[str, JSON] = {
        "id": entry.record.identifier, "type": "bug", "status": "fixed",
        "trigger": cells["trigger_and_former_behavior"], "change": cells["change"],
    }
    extraction = extract(cells)
    if extraction.scope is not None:
        data["scope"] = extraction.scope
    legacy: dict[str, JSON] = {"path": entry.path, "cells": legacy_cells(entry)}
    put_links(data, legacy, row_links(entry, index, resolved=False))
    return Migrated(finish(data, legacy, extraction), f"{entry.path}:{entry.line}", extraction)


def convert_evidence(entry: Entry) -> Migrated:
    row = entry.record.evidence()
    data: dict[str, JSON] = {
        "id": row.identifier, "type": "evidence", "status": vocabulary(row.status, TYPES["evidence"].statuses),
        "path": row.path, "class": vocabulary(row.klass, EVIDENCE_CLASSES), "kind": vocabulary(row.kind, EVIDENCE_KINDS),
        "label": row.label,
    }
    if (area := topic(entry.path)) is not None:
        data["area"] = area
    if row.sha256 is not None:
        data["sha256"] = row.sha256
    if row.date is not None:
        data["date"] = row.date
    if row.records is not None:
        data["records"] = row.records
    if members := [*row.ev, *row.unk]:
        data["members"] = list[JSON](members)
    if row.note is not None:
        data["notes"] = [row.note]
    legacy: dict[str, JSON] = {"path": entry.path, "cells": {"row": entry.record.render()}}
    if row.superseded_by is not None:
        legacy["superseded_by"] = [row.superseded_by]
    return Migrated(finish(data, legacy), f"{entry.path}:{entry.line}")


def prose_links(catalog: Catalog) -> list[ProseLink]:
    """Supersessions stated in ledger lines outside table rows (``PROSE_SUPERSESSION``)."""
    found: list[ProseLink] = []
    for path, document in sorted(catalog.documents.items()):
        if not path.startswith("docs/ledgers/"):
            continue
        for number, line in enumerate(document.lines, 1):
            if isinstance(line, str) and not line.startswith("|"):
                found.extend(ProseLink(match[1], match[2], path, number, line) for match in PROSE_SUPERSESSION.finditer(line))
    return found


def commit_time(root: Path, revision: str) -> datetime:
    return datetime.fromtimestamp(int(git(root, "show", "-s", "--format=%ct", revision).decode().strip()), UTC)


def export(root: Path, rev: str) -> Export:
    """Every record of the legacy system at ``rev``, converted; raises ``MigrationError``."""
    revision = commit_id(root, rev)
    catalog = Catalog.at(root, revision)
    index = Index(catalog.documents)
    result = Export(revision, commit_time(root, revision), catalog=catalog, index=index)
    result.prose_links = prose_links(catalog)
    for link in result.prose_links:
        if link.old not in index.entries or link.new not in index.entries:
            raise MigrationError(f"{link.path}:{link.line}: prose supersession names an undefined ID: {link.text}")
    for identifier in sorted(index.entries, key=id_key):
        entry = index.entries[identifier]
        match entry.record.kind:
            case "F":
                result.migrated.append(convert_finding(entry, index, result.prose_links))
            case "U":
                result.migrated.append(convert_unknown(entry, index))
            case "N":
                result.migrated.append(convert_bug(entry, index))
            case _:
                result.migrated.append(convert_evidence(entry))
    result.migrated.extend(legacy_tables.tables(catalog))
    result.migrated.extend(legacy_prose.references(root, revision))
    result.migrated.extend(legacy_tables.releases(root, revision, index))
    seen: set[str] = set()
    for item in result.migrated:
        if item.record.id in seen:
            raise MigrationError(f"{item.record.id}: defined twice ({item.source})")
        seen.add(item.record.id)
        if item.extraction is not None and len(item.extraction.commits) > 1:
            result.commits[item.record.id] = item.extraction.commits
    return result


def write(result: Export, out: Path) -> tuple[Path, ...]:
    """Write every segment under ``out/memory/journal``; fails if the journal also holds any other segment."""
    existing = {segment.path for segment in read_tree(out)}
    written: list[Path] = []
    for change_set in result.change_sets():
        written.extend(write_change_set(out, result.created, change_set.revisions))
    if extra := sorted(existing - {path.relative_to(out).as_posix() for path in written}):
        raise MigrationError(f"{out / JOURNAL} also holds segments of another export, for example {extra[0]}")
    return tuple(written)
