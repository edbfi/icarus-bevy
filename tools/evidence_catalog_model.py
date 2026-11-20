# SPDX-License-Identifier: AGPL-3.0-or-later
"""Typed records, lossless legacy view codecs, and catalog relationships."""

from __future__ import annotations

import hashlib
import json
import re
from collections import defaultdict
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from pathlib import PurePosixPath
from typing import TypeIs

from tools.docs_lint import LEGACY_NOTE, LISTED_SELECTOR, ManifestRow, iter_lines, parse_json_object

ID = re.compile(r"[FUMN]-(?:[0-9]{3}|[1-9][0-9]{3,})")
ID_IN_TEXT = re.compile(rf"\b{ID.pattern}\b")
RANGE = re.compile(rf"\b({ID.pattern})(?:\.\.| to |–)({ID.pattern})\b")
PIPE = re.compile(r"(?<!\\)\|")
RETIREMENT = re.compile(r"\bsuperseded-by (F-[0-9]{3,})\b")
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


class CatalogError(ValueError):
    """A catalog or publication request failed validation."""


def digest(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def json_line(value: object, *, sort_keys: bool = False) -> str:
    return json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"), sort_keys=sort_keys)


def is_list(value: object) -> TypeIs[list[object]]:
    return isinstance(value, list)


def is_dict(value: object) -> TypeIs[dict[object, object]]:
    return isinstance(value, dict)


def as_object(value: object, label: str) -> dict[str, object]:
    if not is_dict(value):
        raise CatalogError(f"{label} must be an object")
    result: dict[str, object] = {}
    for key, item in value.items():
        if not isinstance(key, str):
            raise CatalogError(f"{label} keys must be strings")
        result[key] = item
    return result


def as_text(value: object, label: str) -> str:
    if not isinstance(value, str):
        raise CatalogError(f"{label} must be a string")
    return value


def as_list(value: object, label: str) -> list[object]:
    if not is_list(value):
        raise CatalogError(f"{label} must be an array")
    return value


def text_list(value: object, label: str) -> tuple[str, ...]:
    return tuple(as_text(item, label) for item in as_list(value, label))


def expect_keys(value: Mapping[str, object], required: set[str], optional: set[str] | None = None) -> None:
    missing = sorted(required - value.keys())
    extra = sorted(value.keys() - required - (optional or set()))
    if missing or extra:
        raise CatalogError(f"unexpected object keys (missing {missing}, extra {extra})")


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


def evidence_fields(row: ManifestRow) -> dict[str, object]:
    fields: dict[str, object] = {
        "path": row.path, "sha256": row.sha256, "class": row.klass, "kind": row.kind, "date": row.date,
        "label": row.label, "status": row.status,
    }
    for key, value in (
        ("ev", list(row.ev) or None), ("unk", list(row.unk) or None), ("records", row.records),
        ("superseded_by", row.superseded_by), ("note", row.note), ("legacy", True if row.legacy else None),
    ):
        if value is not None:
            fields[key] = value
    return fields


@dataclass(frozen=True)
class Record:
    identifier: str
    fields: dict[str, object]
    original: str | None = None
    order: tuple[str, ...] = ()

    @property
    def kind(self) -> str:
        return self.identifier[0]

    def evidence(self, path: str, line: int) -> ManifestRow:
        return ManifestRow.from_record(path, line, {"id": self.identifier, **self.fields})

    def content(self) -> str:
        if self.kind == "M":
            values = {"id": self.identifier, **self.fields}
            return json_line({key: values[key] for key in self.order} if self.order else values)
        cells = (
            as_text(self.fields[name], name).replace("|", r"\|") for name in COLUMNS[self.kind]
        )
        return "| " + " | ".join((self.identifier, *cells)) + " |"

    def render(self) -> str:
        return self.original if self.original is not None else self.content()

    def serialized(self) -> dict[str, object]:
        result: dict[str, object] = {"id": self.identifier, "fields": self.fields}
        if self.original is not None:
            result["original"] = self.original
        if self.order:
            result["order"] = self.order
        return result


def make_record(path: str, identifier: str, fields: dict[str, object]) -> Record:
    if not ID.fullmatch(identifier) or identifier[0] != prefix_for(path):
        raise CatalogError(f"{path}: {identifier} is invalid for this file")
    record = Record(identifier, fields)
    if record.kind == "M":
        row = record.evidence(path, 0)
        return Record(identifier, evidence_fields(row))
    expect_keys(fields, set(COLUMNS[record.kind]))
    ordered: dict[str, object] = {}
    for key in COLUMNS[record.kind]:
        value = as_text(fields[key], key)
        if value.strip() != value or "\n" in value or "\r" in value:
            raise CatalogError(f"{identifier}: {key} must be a trimmed single line")
        ordered[key] = value
    return Record(identifier, ordered)


def parse_record(path: str, line: str) -> Record | None:
    if prefix_for(path) == "M":
        raw = parse_json_object(line)
        order = tuple(raw)
        identifier = as_text(raw.pop("id", None), "id")
        record = make_record(path, identifier, raw)
        if order != ("id", *record.fields):
            record = Record(record.identifier, record.fields, order=order)
    else:
        if not line.startswith("|"):
            return None
        parts = PIPE.split(line)
        identifier = parts[1].strip()
        if not ID.fullmatch(identifier):
            if re.fullmatch(r"[FUMN]-[0-9]+", identifier):
                raise CatalogError(f"{path}: malformed catalog ID {identifier}")
            return None
        if identifier[0] != prefix_for(path):
            raise CatalogError(f"{path}: unexpected ledger ID {identifier}")
        names = COLUMNS[identifier[0]]
        if parts[0] or parts[-1].strip() or len(parts) != len(names) + 3:
            raise CatalogError(f"{path}: malformed table row {identifier}")
        fields: dict[str, object] = {
            name: cell.strip().replace(r"\|", "|") for name, cell in zip(names, parts[2:-1], strict=True)
        }
        record = make_record(path, identifier, fields)

    # Retain spelling or column spacing only when normalized fields cannot reproduce it.
    return record if record.content() == line else Record(record.identifier, record.fields, line, record.order)


@dataclass(frozen=True)
class Document:
    path: str
    lines: tuple[str | Record, ...]

    @classmethod
    def parse(cls, path: str, source: bytes) -> Document:
        if topic(path) is None:
            raise CatalogError(f"unsupported catalog path {path}")
        lines: list[str | Record] = []
        evidence = prefix_for(path) == "M"
        for number, line, fenced in iter_lines(source.decode("utf-8").split("\n")):
            if (evidence and not line) or (not evidence and fenced):
                lines.append(line)
                continue
            try:
                lines.append(parse_record(path, line) or line)
            except ValueError as error:
                raise CatalogError(f"{path}:{number}: {error}") from error
        return cls(path, tuple(lines))

    @classmethod
    def from_bytes(cls, path: str, serialized: bytes) -> Document:
        raw = parse_json_object(serialized)
        expect_keys(raw, {"lines"})
        lines: list[str | Record] = []
        for item in as_list(raw["lines"], "lines"):
            if isinstance(item, str):
                lines.append(item)
                continue
            envelope = as_object(item, "record")
            expect_keys(envelope, {"id", "fields"}, {"original", "order"})
            record = make_record(path, as_text(envelope["id"], "id"), as_object(envelope["fields"], "fields"))
            original = as_text(envelope["original"], "original") if "original" in envelope else None
            order = text_list(envelope["order"], "order") if "order" in envelope else ()
            if order and (record.kind != "M" or set(order) != {"id", *record.fields}):
                raise CatalogError(f"{record.identifier}: invalid evidence field order")
            record = Record(record.identifier, record.fields, original, order)
            lines.append(record)
        document = cls(path, tuple(lines))
        if cls.parse(path, document.render()) != document or document.serialized() != serialized:
            raise CatalogError(f"{path}: noncanonical or inconsistent document layout")
        return document

    def records(self) -> Iterator[tuple[int, Record]]:
        for number, item in enumerate(self.lines, 1):
            if isinstance(item, Record):
                yield number, item

    def render(self) -> bytes:
        return "\n".join(item.render() if isinstance(item, Record) else item for item in self.lines).encode()

    def serialized(self) -> bytes:
        lines = (json_line(item.serialized() if isinstance(item, Record) else item) for item in self.lines)
        return ('{"lines":[\n' + ',\n'.join(lines) + '\n]}\n').encode()


@dataclass(frozen=True)
class Entry:
    path: str
    line: int
    record: Record

    def result(self) -> dict[str, object]:
        return {"path": self.path, "line": self.line, **self.record.serialized()}


@dataclass(frozen=True)
class Link:
    kind: str
    target: str
    field: str


def references(value: str, known: Mapping[str, Entry]) -> Iterator[str]:
    # Historical range prose can skip IDs. Expand only IDs actually in the catalog.
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
    def __init__(self, documents: Mapping[str, Document]) -> None:
        self.entries: dict[str, Entry] = {}
        self.links: dict[str, tuple[Link, ...]] = {}
        self.reverse: dict[str, list[tuple[str, Link]]] = defaultdict(list)
        for path, document in sorted(documents.items()):
            for line, record in document.records():
                identifier = record.identifier
                if identifier in self.entries:
                    raise CatalogError(f"duplicate ID {identifier}: {self.entries[identifier].path}, {path}")
                self.entries[identifier] = Entry(path, line, record)
        for identifier, entry in self.entries.items():
            links = tuple(sorted(set(self.record_links(entry)), key=lambda link: (link.kind, link.target, link.field)))
            self.links[identifier] = links
            for link in links:
                self.reverse[link.target].append((identifier, link))

    def record_links(self, entry: Entry) -> Iterator[Link]:
        record = entry.record
        if record.kind == "M":
            evidence = record.evidence(entry.path, entry.line)
            if evidence.superseded_by is not None:
                yield Link("supersedes_to", evidence.superseded_by, "superseded_by")
            return
        for field, value in record.fields.items():
            content = as_text(value, field)
            for target in references(content, self.entries):
                if target == record.identifier:
                    continue
                kind = "support" if field == "evidence" and target.startswith("M-") else "reference"
                yield Link(kind, target, field)
            for pattern in (LISTED_SELECTOR, LEGACY_NOTE):
                for match in pattern.finditer(content):
                    kind = "support" if field == "evidence" else "reference"
                    yield Link(kind, match[0], field)
            if record.kind == "F" and field == "status":
                for match in RETIREMENT.finditer(content):
                    yield Link("supersedes_to", match[1], field)

    def get(self, identifier: str) -> Entry:
        try:
            return self.entries[identifier]
        except KeyError as error:
            raise CatalogError(f"unknown catalog ID {identifier}") from error

    def selector(self, selector: str) -> dict[str, object]:
        identifier, slash, member = selector.partition("/")
        entry = self.get(identifier)
        if entry.record.kind != "M" or not slash:
            raise CatalogError(f"invalid evidence selector {selector}")
        row = entry.record.evidence(entry.path, entry.line)
        if LEGACY_NOTE.fullmatch(selector) and row.legacy:
            return {"selector": selector, "legacy_note": member, "membership": "unindexed", "evidence": entry.result()}
        match = LISTED_SELECTOR.fullmatch(selector)
        if match is None or len(match[3]) not in (12, 64) or row.kind != "rea-bundle":
            raise CatalogError(f"invalid evidence selector {selector}")
        listed = row.ev if match[2] == "ev" else row.unk
        hits = [full for full in listed if full.startswith(member)]
        if len(hits) != 1:
            raise CatalogError(f"{selector} matches {len(hits)} listed members")
        full_id = hits[0]
        return {
            "selector": f"{identifier}/{full_id[:len(match[2]) + 13]}",
            "member": full_id, "membership": "listed", "evidence": entry.result(),
        }

    def lookup(self, identifier: str) -> dict[str, object]:
        if "/" in identifier:
            result = self.selector(identifier)
            result["incoming"] = self.incoming(as_text(result["selector"], "selector"))
            return result
        entry = self.get(identifier)
        outgoing: list[dict[str, object]] = []
        for link in self.links[identifier]:
            item: dict[str, object] = {"kind": link.kind, "field": link.field, "target": link.target}
            evidence = self.entries.get(link.target.split("/", 1)[0])
            if link.kind == "support" and evidence is not None:
                item["evidence"] = evidence.result()
            outgoing.append(item)
        return {**entry.result(), "outgoing": outgoing, "incoming": self.incoming(identifier)}

    def incoming(self, identifier: str) -> list[dict[str, str]]:
        return [
            {"id": source, "kind": link.kind, "field": link.field}
            for source, link in self.reverse.get(identifier, ())
        ]


def source_path(path: str) -> PurePosixPath:
    result = PurePosixPath(path)
    if topic(path) is None or result.as_posix() != path:
        raise CatalogError(f"invalid catalog source path {path!r}")
    return result
