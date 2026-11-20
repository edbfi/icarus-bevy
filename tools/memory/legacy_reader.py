# SPDX-License-Identifier: AGPL-3.0-or-later
"""Frozen reader of the legacy ledgers and evidence index, kept for ``memory salvage`` after the cut-over.

The cut-over deleted ``docs/ledgers/``, ``evidence/``, ``evidence-catalog/`` and their tools
(``tools/docs_lint.py``, ``tools/evidence_catalog*.py``). Branches older than it still hold legacy rows,
so the exporter (``tools.memory.legacy``) keeps exactly the parse it needs, copied from those tools:
the line-by-line parse of findings, unknowns, native-bug and evidence-shard
files at one Git revision, and the index of IDs and links. It never writes, renders views, publishes,
digests requests or checks catalog authority: no revision on ``main`` held an authority that differed
from its Markdown and JSONL views (the removed hook rejected that), so the views alone are read.

Change it only to keep reading old revisions, never to author legacy rows.
"""

from __future__ import annotations

import json
import re
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import TypeIs, cast

from tools.memory.gitio import GitError, git, read_blobs, tree_files

ID = re.compile(r"[FUMN]-(?:[0-9]{3}|[1-9][0-9]{3,})")
ID_IN_TEXT = re.compile(rf"\b{ID.pattern}\b")
RANGE = re.compile(rf"\b({ID.pattern})(?:\.\.| to |–)({ID.pattern})\b")
PIPE = re.compile(r"(?<!\\)\|")
RETIREMENT = re.compile(r"\bsuperseded-by (F-[0-9]{3,})\b")
LISTED_SELECTOR = re.compile(r"\b(M-\d{3,})/(ev|unk)_([0-9a-f]+)")
LEGACY_NOTE = re.compile(r"\b(M-\d{3,})/(E-\d{3,})\b")
FENCE = re.compile(r"^\s*(```|~~~)")
SHARD = re.compile(r"evidence/([a-z][a-z0-9-]*(?:/[a-z][a-z0-9-]*)?)/[0-9]{3}\.jsonl")
FINDINGS = re.compile(r"docs/ledgers/findings/(?:retired/)?([a-z][a-z0-9-]*?)(?:-[0-9]+)?\.md")
OTHER_LEDGERS: dict[str, str] = {
    "docs/ledgers/unknowns.md": "unknowns",
    "docs/ledgers/bugs.md": "bugs",
    "docs/ledgers/compatibility.md": "compatibility",
}
COLUMNS: dict[str, tuple[str, ...]] = {
    "F": ("claim", "kind", "evidence", "scope_and_limits", "unknowns", "status"),
    "U": ("question", "severity", "status", "next_probe", "findings", "rea_id"),
    "N": ("trigger_and_former_behavior", "change", "findings"),
}
LEGACY_ROOTS = ("docs/ledgers", "evidence")
REQUIRED_KEYS = ("id", "path", "sha256", "class", "kind", "date", "label", "status")
OPTIONAL_KEYS = ("ev", "unk", "records", "superseded_by", "note", "legacy")


class CatalogError(ValueError):
    """A legacy file at the revision cannot be read."""


def is_dict(value: object) -> TypeIs[dict[object, object]]:
    return isinstance(value, dict)


def _unique_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
    found: dict[str, object] = {}
    for key, value in pairs:
        if key in found:
            raise CatalogError(f"duplicate key {key!r}")
        found[key] = value
    return found


def parse_json_object(text: str | bytes) -> dict[str, object]:
    """One JSON object with string keys; duplicate keys are rejected."""
    try:
        # json.loads is untyped; every value is an object until narrowed.
        value = cast(object, json.loads(text, object_pairs_hook=_unique_pairs))
    except json.JSONDecodeError as error:
        raise CatalogError(f"invalid JSON: {error.msg}") from error
    if not is_dict(value):
        raise CatalogError("record must be a JSON object")
    return {str(key): item for key, item in value.items()}


def as_text(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise CatalogError(f"{label} must be a string")
    return value


def iter_lines(lines: Sequence[str]) -> Iterator[tuple[int, str, bool]]:
    """(1-based number, line, inside a fenced block)."""
    fenced = False
    for index, line in enumerate(lines, start=1):
        if FENCE.match(line):
            fenced = not fenced
            yield index, line, True
            continue
        yield index, line, fenced


def _text(record: Mapping[str, object], key: str) -> str:
    value = record[key]
    if not isinstance(value, str) or not value:
        raise CatalogError(f"{key} must be a nonempty string")
    return value


def _optional_text(record: Mapping[str, object], key: str) -> str | None:
    value = record.get(key)
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise CatalogError(f"{key} must be a nonempty string or null")
    return value


def _texts(record: Mapping[str, object], key: str) -> tuple[str, ...]:
    if key not in record:
        return ()
    value = record[key]
    if not isinstance(value, list) or not value or not all(isinstance(item, str) for item in cast(list[object], value)):
        raise CatalogError(f"{key} must be a nonempty list of strings; omit the key instead")
    return tuple(str(item) for item in cast(list[object], value))


@dataclass(frozen=True)
class EvidenceRow:
    """One evidence index line."""

    identifier: str
    path: str
    sha256: str | None
    klass: str
    kind: str
    date: str | None
    label: str
    status: str
    ev: tuple[str, ...] = ()
    unk: tuple[str, ...] = ()
    records: int | None = None
    superseded_by: str | None = None
    note: str | None = None
    legacy: bool = False

    @classmethod
    def parse(cls, record: Mapping[str, object]) -> EvidenceRow:
        if missing := [key for key in REQUIRED_KEYS if key not in record]:
            raise CatalogError(f"missing keys {', '.join(missing)}")
        if unknown := sorted(set(record) - set(REQUIRED_KEYS) - set(OPTIONAL_KEYS)):
            raise CatalogError(f"unknown keys {', '.join(unknown)}")
        if null := [key for key in OPTIONAL_KEYS if key in record and record[key] is None]:
            raise CatalogError(f"omit optional keys instead of null: {', '.join(null)}")
        count = record.get("records")
        if count is not None and (not isinstance(count, int) or isinstance(count, bool) or count < 0):
            raise CatalogError("records must be a nonnegative integer")
        if "legacy" in record and record["legacy"] is not True:
            raise CatalogError("legacy must be true when present; omit it otherwise")
        return cls(
            identifier=_text(record, "id"), path=_text(record, "path"), sha256=_optional_text(record, "sha256"),
            klass=_text(record, "class"), kind=_text(record, "kind"), date=_optional_text(record, "date"),
            label=_text(record, "label"), status=_text(record, "status"), ev=_texts(record, "ev"), unk=_texts(record, "unk"),
            records=count if isinstance(count, int) else None, superseded_by=_optional_text(record, "superseded_by"),
            note=_optional_text(record, "note"), legacy="legacy" in record,
        )


def topic(path: str) -> str | None:
    if match := SHARD.fullmatch(path):
        return match[1].replace("/", "-")
    if match := FINDINGS.fullmatch(path):
        return match[1]
    return OTHER_LEDGERS.get(path)


def prefix_for(path: str) -> str | None:
    if SHARD.fullmatch(path):
        return "M"
    if FINDINGS.fullmatch(path):
        return "F"
    return {"unknowns": "U", "bugs": "N"}.get(OTHER_LEDGERS.get(path, ""))


def evidence_fields(row: EvidenceRow) -> dict[str, object]:
    fields: dict[str, object] = {
        "path": row.path, "sha256": row.sha256, "class": row.klass, "kind": row.kind, "date": row.date, "label": row.label, "status": row.status,
    }
    for key, value in (("ev", list(row.ev) or None), ("unk", list(row.unk) or None), ("records", row.records),
                       ("superseded_by", row.superseded_by), ("note", row.note), ("legacy", True if row.legacy else None)):
        if value is not None:
            fields[key] = value
    return fields


def json_line(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))


@dataclass(frozen=True)
class Row:
    """One ledger table row or evidence line: its ID, fields, and the verbatim line when normalising cannot reproduce it."""

    identifier: str
    fields: dict[str, object]
    original: str | None = None
    order: tuple[str, ...] = ()

    @property
    def kind(self) -> str:
        return self.identifier[0]

    def evidence(self) -> EvidenceRow:
        return EvidenceRow.parse({"id": self.identifier, **self.fields})

    def content(self) -> str:
        if self.kind == "M":
            values = {"id": self.identifier, **self.fields}
            return json_line({key: values[key] for key in self.order} if self.order else values)
        cells = (as_text(self.fields[name], name).replace("|", r"\|") for name in COLUMNS[self.kind])
        return "| " + " | ".join((self.identifier, *cells)) + " |"

    def render(self) -> str:
        return self.original if self.original is not None else self.content()


def make_row(path: str, identifier: str, fields: dict[str, object]) -> Row:
    if not ID.fullmatch(identifier) or identifier[0] != prefix_for(path):
        raise CatalogError(f"{path}: {identifier} is invalid for this file")
    if identifier[0] == "M":
        return Row(identifier, evidence_fields(Row(identifier, fields).evidence()))
    if set(fields) != set(COLUMNS[identifier[0]]):
        raise CatalogError(f"{identifier}: unexpected fields {sorted(fields)}")
    ordered: dict[str, object] = {}
    for key in COLUMNS[identifier[0]]:
        value = as_text(fields[key], key)
        if value.strip() != value or "\n" in value or "\r" in value:
            raise CatalogError(f"{identifier}: {key} must be a trimmed single line")
        ordered[key] = value
    return Row(identifier, ordered)


def parse_row(path: str, line: str) -> Row | None:
    if prefix_for(path) == "M":
        raw = parse_json_object(line)
        order = tuple(raw)
        row = make_row(path, as_text(raw.pop("id", None), "id"), raw)
        if order != ("id", *row.fields):
            row = Row(row.identifier, row.fields, order=order)
    else:
        if not line.startswith("|"):
            return None
        parts = PIPE.split(line)
        identifier = parts[1].strip()
        if not ID.fullmatch(identifier):
            if re.fullmatch(r"[FUMN]-[0-9]+", identifier):
                raise CatalogError(f"{path}: malformed ID {identifier}")
            return None
        if identifier[0] != prefix_for(path):
            raise CatalogError(f"{path}: unexpected ledger ID {identifier}")
        names = COLUMNS[identifier[0]]
        if parts[0] or parts[-1].strip() or len(parts) != len(names) + 3:
            raise CatalogError(f"{path}: malformed table row {identifier}")
        fields: dict[str, object] = {name: cell.strip().replace(r"\|", "|") for name, cell in zip(names, parts[2:-1], strict=True)}
        row = make_row(path, identifier, fields)
    # Keep the verbatim line only when the normalised fields cannot reproduce its spelling or spacing.
    return row if row.content() == line else Row(row.identifier, row.fields, line, row.order)


@dataclass(frozen=True)
class Document:
    """A legacy file as its lines, each a ``Row`` or verbatim text."""

    path: str
    lines: tuple[str | Row, ...]

    @classmethod
    def parse(cls, path: str, source: bytes) -> Document:
        if topic(path) is None:
            raise CatalogError(f"unsupported legacy path {path}")
        lines: list[str | Row] = []
        evidence = prefix_for(path) == "M"
        for number, line, fenced in iter_lines(source.decode("utf-8").split("\n")):
            if (evidence and not line) or (not evidence and fenced):
                lines.append(line)
                continue
            try:
                lines.append(parse_row(path, line) or line)
            except ValueError as error:
                raise CatalogError(f"{path}:{number}: {error}") from error
        return cls(path, tuple(lines))

    def rows(self) -> Iterator[tuple[int, Row]]:
        for number, item in enumerate(self.lines, 1):
            if isinstance(item, Row):
                yield number, item


@dataclass(frozen=True)
class Entry:
    path: str
    line: int
    record: Row


@dataclass(frozen=True)
class Link:
    kind: str
    target: str
    field: str


def references(value: str, known: Mapping[str, Entry]) -> Iterator[str]:
    """IDs a cell names; a range is expanded over the IDs ``known`` holds (historical ranges skip numbers)."""
    remainder = value
    for match in RANGE.finditer(value):
        first, last = match.groups()
        if first[0] != last[0]:
            continue
        low, high = int(first[2:]), int(last[2:])
        if low > high:
            continue
        yield first
        yield last
        yield from (key for key in known if key[0] == first[0] and low < int(key[2:]) < high)
        remainder = remainder.replace(match[0], " ")
    yield from (match[0] for match in ID_IN_TEXT.finditer(remainder))


class Index:
    """Every row by ID, and each row's links (``supersedes_to``, ``support``, ``reference``) by target."""

    def __init__(self, documents: Mapping[str, Document]) -> None:
        self.entries: dict[str, Entry] = {}
        self.links: dict[str, tuple[Link, ...]] = {}
        for path, document in sorted(documents.items()):
            for line, row in document.rows():
                if row.identifier in self.entries:
                    raise CatalogError(f"duplicate ID {row.identifier}: {self.entries[row.identifier].path}, {path}")
                self.entries[row.identifier] = Entry(path, line, row)
        for identifier, entry in self.entries.items():
            self.links[identifier] = tuple(sorted(set(self.row_links(entry)), key=lambda link: (link.kind, link.target, link.field)))

    def row_links(self, entry: Entry) -> Iterator[Link]:
        row = entry.record
        if row.kind == "M":
            if (successor := row.evidence().superseded_by) is not None:
                yield Link("supersedes_to", successor, "superseded_by")
            return
        for name, value in row.fields.items():
            content = as_text(value, name)
            for target in references(content, self.entries):
                if target != row.identifier:
                    yield Link("support" if name == "evidence" and target.startswith("M-") else "reference", target, name)
            for pattern in (LISTED_SELECTOR, LEGACY_NOTE):
                for match in pattern.finditer(content):
                    yield Link("support" if name == "evidence" else "reference", match[0], name)
            if row.kind == "F" and name == "status":
                for match in RETIREMENT.finditer(content):
                    yield Link("supersedes_to", match[1], name)


def commit_id(root: Path, rev: str) -> str:
    try:
        return git(root, "rev-parse", "--verify", "--end-of-options", f"{rev}^{{commit}}").decode().strip()
    except GitError as error:
        raise CatalogError(str(error)) from error


@dataclass(frozen=True)
class Catalog:
    """The legacy files of one revision that hold rows, parsed."""

    revision: str
    documents: dict[str, Document]

    @classmethod
    def at(cls, root: Path, rev: str) -> Catalog:
        files = {path: oid for prefix in LEGACY_ROOTS for path, oid in tree_files(root, rev, prefix).items()}
        for path in files:
            if topic(path) is None and path != "evidence/README.md":
                raise CatalogError(f"{path}: unsupported ledger or evidence index layout at {rev}")
        paths = sorted(path for path in files if topic(path) is not None)
        if not paths:
            raise CatalogError(f"no supported ledger or evidence index files at {rev}")
        blobs = read_blobs(root, [files[path] for path in paths])
        return cls(rev, {path: Document.parse(path, data or b"") for path, data in zip(paths, blobs, strict=True)})
