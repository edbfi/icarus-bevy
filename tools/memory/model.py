# SPDX-License-Identifier: AGPL-3.0-or-later
"""Typed memory records and their strict plain-JSON form.

``Record.from_json`` is the only way in from untyped data and ``Record.to_json`` the only way out.
Every failure is a ``RecordError`` whose message starts with the record ID (``record`` while the ID
itself is unusable) and names the field.

Invariants of every parsed ``Record``:

- ``id`` matches ``ID`` and its prefix names ``type``. ``status`` is in the type's stored vocabulary
  (``superseded`` and ``resolved`` never are: replay derives them), or ``legacy:<verbatim>``.
- A record is *migrated* exactly when ``legacy`` is set. Only a migrated record may carry
  ``legacy:<verbatim>`` enum values, ``gaps``, ``assigned_at_migration`` (required on migrated CP and RP
  records, forbidden elsewhere), a ``legacy-text`` scope basis or an ``any:<id>`` build.
- Every required field is present, or, on a migrated record only, named in ``gaps``. A gap is a field
  name or ``scope.<part>``, optionally followed by ``: <detail>`` (``scope.commit: several commits in
  row (a, b)``); the field it names is absent.
- Strings are NFC, carry no control character other than TAB and LF, and are non-empty. Field values
  are single trimmed lines; only a REF ``body`` and verbatim ``legacy`` cells may span lines.
- No ``null``, no ``false`` flag and no empty list is ever stored: absent means omitted.
- Link, scope and gap lists are sorted (``id_key``) and unique; ``members``, ``notes``,
  ``verified_on`` and other text lists keep their order and are unique.
- New records only: a derived or inferred finding names ``refs``; a decision finding quotes
  ``instruction`` with ``instruction_date`` and ``instruction_source`` (no other finding may); an
  explicit ``native`` scope names its ``commit``; ``run`` evidence carries ``run``.

Rules that need other records (link targets, bundle membership, accepted sources of the confidence
rule, immutable fields across revisions) live in ``state`` and ``check``.
"""

from __future__ import annotations

import hashlib
import json
import re
import unicodedata
from collections.abc import Mapping
from dataclasses import dataclass, field
from datetime import date
from typing import Final, NoReturn, TypeIs, cast

type JSON = None | bool | int | str | list[JSON] | dict[str, JSON]
type Value = str | int | bool | tuple[str, ...] | dict[str, str]


class RecordError(ValueError):
    """A record, or the JSON it came from, breaks the schema."""


NUMBER = r"(?:00[1-9]|0[1-9][0-9]|[1-9][0-9]{2,})"
VERSION = r"[0-9]+\.[0-9]+\.[0-9]+"
PREFIXES: Final[dict[str, str]] = {
    "F": "finding", "U": "unknown", "N": "bug", "SL": "lead", "RP": "report",
    "CP": "compatibility", "REF": "reference", "REL": "release", "M": "evidence",
}
ID = re.compile(rf"(?:F|U|N|SL|RP|CP|REF|M)-{NUMBER}|REL-{VERSION}")
# ``M-N/ev_<12 hex>``, ``M-N/unk_<12 hex>`` or the legacy case note ``M-N/E-N``.
SELECTOR = re.compile(rf"(M-{NUMBER})/(?:((?:ev|unk)_[0-9a-f]{{12}})|(E-[0-9]{{3,}}))")
MEMBER = re.compile(r"(?:ev|unk)_(?:[0-9a-f]{12}|[0-9a-f]{64})")
SHA256 = re.compile(r"[0-9a-f]{64}")
COMMIT = re.compile(r"[0-9a-f]{7,40}")
BUILD = re.compile(r"(server|client|any):([0-9]+)")
RELEASE = re.compile(rf"{VERSION}(?:\.[0-9]+)?")
DIGITS = re.compile(r"[0-9]+")
DAY = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}")
TIMESTAMP = re.compile(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z")
# Control characters a record may not hold. Hindsight strips ASCII ones on ingress, so a digest
# would otherwise cover text the bank never stores; CR is excluded because text is LF only.
FORBIDDEN = re.compile(r"[\x00-\x08\x0b-\x1f\x7f-\x9f]")
LEGACY = "legacy:"

FINDING_KINDS = ("observed", "measured", "derived", "inferred", "reported", "decision")
CONFIDENCES = ("high", "medium", "low")
STANDINGS = ("confirmed", "candidate", "refuted")
SEVERITIES = ("low", "medium", "high", "critical")
EVIDENCE_CLASSES = ("local-private", "tracked", "external")
EVIDENCE_KINDS = ("receipt", "rea-bundle", "report", "review", "revision", "input", "build", "manifest", "run")
OUTCOMES = ("pass", "fail", "aborted")
SUBJECTS = ("original-server", "original-client", "native", "decision", "private-input", "tooling")
BASES = ("explicit", "legacy-text")
SCOPE_PARTS = ("subjects", "builds", "releases", "commit")
IMPLIED = ("superseded", "resolved", "retired")
DECISION_FIELDS = ("instruction", "instruction_date", "instruction_source")


@dataclass(frozen=True)
class Spec:
    """One stored field. ``kind``: text, enum, count, flag, list, date, sha256, dates, members, timestamp, build."""

    kind: str
    vocabulary: tuple[str, ...] = ()


TEXT = Spec("text")
LIST = Spec("list")
DATE = Spec("date")


def enum(*values: str) -> Spec:
    return Spec("enum", values)


@dataclass(frozen=True)
class TypeSpec:
    """A record type: ID prefix, stored statuses, the field rendered first, fields and required names.

    ``required`` may also name ``scope``, ``refs`` and ``body``, which live outside ``fields``.
    """

    prefix: str
    statuses: tuple[str, ...]
    headline: str
    fields: Mapping[str, Spec]
    required: tuple[str, ...]


COMMON: Final[dict[str, Spec]] = {"date": DATE, "area": TEXT}
TYPES: Final[dict[str, TypeSpec]] = {
    "finding": TypeSpec("F", ("active", "retired"), "claim", {
        **COMMON, "claim": TEXT, "kind": enum(*FINDING_KINDS), "confidence": enum(*CONFIDENCES),
        "standing": enum(*STANDINGS), "negative": Spec("flag"),
        "instruction": TEXT, "instruction_date": DATE, "instruction_source": TEXT,
    }, ("claim", "kind", "confidence", "standing", "scope", "area")),
    "unknown": TypeSpec("U", ("open", "investigating", "blocked", "contradicted"), "question", {
        **COMMON, "question": TEXT, "severity": enum(*SEVERITIES), "next_probe": TEXT,
    }, ("question", "severity", "next_probe", "area")),
    "bug": TypeSpec("N", ("open", "fixed"), "trigger", {
        **COMMON, "trigger": TEXT, "change": TEXT, "tests": LIST,
    }, ("trigger", "change", "refs", "tests")),
    "lead": TypeSpec("SL", ("open", "established", "refuted"), "lead", {
        **COMMON, "lead": TEXT, "static_evidence": TEXT, "refute_by": TEXT,
    }, ("lead", "static_evidence", "refute_by")),
    "report": TypeSpec("RP", ("unreproduced", "reproduced", "fixed"), "problem", {
        **COMMON, "problem": TEXT, "priority": Spec("count"), "next_evidence": TEXT,
    }, ("problem", "priority", "next_evidence")),
    "compatibility": TypeSpec("CP", ("unknown", "observed", "specified", "implemented", "validated"), "name", {
        **COMMON, "name": TEXT, "qualifier": TEXT, "required_proof": TEXT,
    }, ("name", "qualifier", "required_proof", "scope")),
    "reference": TypeSpec("REF", ("current", "retired"), "title", {
        **COMMON, "title": TEXT,
    }, ("title", "body", "scope")),
    "release": TypeSpec("REL", ("current", "past"), "version_string", {
        **COMMON, "version_string": TEXT, "server_build": Spec("build"), "client_build": Spec("build"),
        "dates": Spec("dates"), "sources": LIST,
    }, ("version_string", "server_build", "client_build", "dates", "sources")),
    "evidence": TypeSpec("M", ("active", "retired", "missing"), "label", {
        **COMMON, "path": TEXT, "sha256": Spec("sha256"), "size": Spec("count"),
        "class": enum(*EVIDENCE_CLASSES), "kind": enum(*EVIDENCE_KINDS), "label": TEXT,
        "records": Spec("count"), "members": Spec("members"),
    }, ("path", "sha256", "size", "class", "kind", "date", "label")),
}
RUN: Final[dict[str, Spec]] = {
    "workload": TEXT, "targets": LIST, "matched_reference": TEXT, "identities": LIST,
    "start": Spec("timestamp"), "end": Spec("timestamp"), "environment": TEXT, "thresholds": LIST,
    "outcome": enum(*OUTCOMES), "deviations": LIST,
}
RUN_REQUIRED = tuple(name for name in RUN if name != "deviations")
LINKS = ("supersedes", "resolves", "cites", "refs", "verified_on")
ENVELOPE = frozenset({"id", "type", "status", "scope", *LINKS, "notes", "legacy", "gaps", "assigned_at_migration"})


def type_of(identifier: str) -> str:
    """The record type an ID names; raises ``RecordError`` for anything that is not an ID."""
    if not ID.fullmatch(identifier):
        raise RecordError(f"{identifier!r} is not a record ID")
    return PREFIXES[identifier.split("-", 1)[0]]


def id_key(citation: str) -> tuple[str, tuple[int, ...], str]:
    """Natural order of IDs and selectors: by prefix, then numbers (each version part) as integers;
    any other text sorts before them, as plain text."""
    identifier, _, selector = citation.partition("/")
    if not ID.fullmatch(identifier):
        return "", (), citation
    prefix, _, number = identifier.partition("-")
    return prefix, tuple(int(part) for part in number.split(".")), selector


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def string_problem(value: str) -> str | None:
    """Why ``value`` cannot be stored (not NFC, a control character, a lone surrogate), or ``None``."""
    if match := FORBIDDEN.search(value):
        return f"control character U+{ord(match[0]):04X} at offset {match.start()}"
    try:
        _ = value.encode("utf-8")
    except UnicodeEncodeError:
        return "lone surrogate"
    if not unicodedata.is_normalized("NFC", value):
        return "not NFC-normalised"
    return None


def is_object(value: object) -> TypeIs[dict[object, object]]:
    return isinstance(value, dict)


def is_array(value: object) -> TypeIs[list[object]]:
    return isinstance(value, list)


def _unique_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
    found: dict[str, object] = {}
    for key, value in pairs:
        if key in found:
            raise RecordError(f"duplicate JSON key {key!r}")
        found[key] = value
    return found


def _no_float(text: str) -> object:
    raise RecordError(f"non-integer number {text}")


def load_json(data: str | bytes) -> object:
    """Strict ``json.loads``: duplicate keys, floats, NaN and Infinity are ``RecordError``."""
    try:
        # json.loads is untyped; the result is an object until narrowed by the caller.
        return cast(object, json.loads(data, object_pairs_hook=_unique_pairs, parse_float=_no_float, parse_constant=_no_float))
    except (json.JSONDecodeError, UnicodeDecodeError) as error:
        raise RecordError(f"invalid JSON: {error}") from error


def as_mapping(value: object, label: str) -> dict[str, object]:
    if not is_object(value):
        raise RecordError(f"{label}: must be a JSON object")
    result: dict[str, object] = {}
    for key, item in value.items():
        if not isinstance(key, str):
            raise RecordError(f"{label}: keys must be strings")
        result[key] = item
    return result


@dataclass(frozen=True)
class Scope:
    """What a claim is about. ``basis`` is ``explicit`` or ``legacy-text`` (only literal tokens of a legacy row)."""

    basis: str
    subjects: tuple[str, ...] = ()
    builds: tuple[str, ...] = ()
    releases: tuple[str, ...] = ()
    commit: str | None = None

    @property
    def tokens(self) -> tuple[str, ...]:
        return (*self.builds, *self.releases, *((self.commit,) if self.commit else ()))


@dataclass(frozen=True)
class VerifiedOn:
    """An unchanged fact re-checked on ``build`` (``server:<id>`` or ``client:<id>``) by the evidence it cites."""

    build: str
    cites: tuple[str, ...]


@dataclass(frozen=True)
class Links:
    """Edges stored once, on the newer record. Inverses are derived by ``state.replay``."""

    supersedes: tuple[str, ...] = ()
    resolves: tuple[str, ...] = ()
    cites: tuple[str, ...] = ()
    refs: tuple[str, ...] = ()
    verified_on: tuple[VerifiedOn, ...] = ()

    def named(self, name: str) -> tuple[str, ...]:
        """The ID list called ``name``: supersedes, resolves, cites or refs."""
        match name:
            case "supersedes":
                return self.supersedes
            case "resolves":
                return self.resolves
            case "cites":
                return self.cites
            case "refs":
                return self.refs
            case _:
                raise KeyError(name)

    def citations(self) -> tuple[str, ...]:
        """Every evidence citation, from ``cites`` and ``verified_on``, in stored order."""
        return (*self.cites, *(cite for item in self.verified_on for cite in item.cites))

    def targets(self) -> tuple[str, ...]:
        """Every linked record ID (selectors reduced to their evidence ID), sorted and unique."""
        found = {*self.supersedes, *self.resolves, *self.refs, *(c.split("/", 1)[0] for c in self.citations())}
        return tuple(sorted(found, key=id_key))


@dataclass(frozen=True)
class Legacy:
    """Where a migrated record came from, verbatim, and the lifecycle its legacy row stated.

    ``cells`` maps each source column to its verbatim cell (a JSONL row is kept whole under ``row``).
    ``superseded_by`` holds edges the old row stated about itself (``superseded-by F-NNN``, an evidence
    row's ``superseded_by``); ``resolved_by`` the findings a resolved unknown's row names as resolving it;
    ``implied_status`` (superseded, resolved or retired) is the status its legacy cell implied. Replay
    unions all three with the links of newer records.
    """

    path: str
    cells: dict[str, str]
    superseded_by: tuple[str, ...] = ()
    resolved_by: tuple[str, ...] = ()
    implied_status: str | None = None


@dataclass(frozen=True)
class Record:
    """One record revision's content. Build it only through ``from_json`` (or validate with it)."""

    id: str
    type: str
    status: str
    fields: dict[str, Value]
    scope: Scope | None = None
    links: Links = field(default_factory=Links)
    notes: tuple[str, ...] = ()
    run: dict[str, Value] | None = None
    body: str | None = None
    legacy: Legacy | None = None
    gaps: tuple[str, ...] = ()
    assigned_at_migration: bool = False

    @property
    def migrated(self) -> bool:
        return self.legacy is not None

    @property
    def spec(self) -> TypeSpec:
        return TYPES[self.type]

    def text(self, name: str) -> str | None:
        value = self.fields.get(name)
        return value if isinstance(value, str) else None

    def items(self, name: str) -> tuple[str, ...]:
        value = self.fields.get(name)
        return value if isinstance(value, tuple) else ()

    def number(self, name: str) -> int | None:
        value = self.fields.get(name)
        return value if isinstance(value, int) and not isinstance(value, bool) else None

    @classmethod
    def from_json(cls, data: object, body: str | None = None) -> Record:
        """Parse and validate one record object; ``body`` is a REF's Markdown, held outside its JSON."""
        return _parse(as_mapping(data, "record"), body)

    def to_json(self) -> dict[str, JSON]:
        """The plain-JSON form (a REF holds ``body_sha256``, never its body); ``canonical`` orders it."""
        result: dict[str, JSON] = {"id": self.id, "type": self.type, "status": self.status}
        result.update({name: _dump(value) for name, value in self.fields.items()})
        if self.scope is not None:
            scope: dict[str, JSON] = {"basis": self.scope.basis}
            for name, items in (("subjects", self.scope.subjects), ("builds", self.scope.builds), ("releases", self.scope.releases)):
                if items:
                    scope[name] = list(items)
            if self.scope.commit:
                scope["commit"] = self.scope.commit
            result["scope"] = scope
        for name in ("supersedes", "resolves", "cites", "refs"):
            if items := self.links.named(name):
                result[name] = list(items)
        if self.links.verified_on:
            result["verified_on"] = [{"build": item.build, "cites": list(item.cites)} for item in self.links.verified_on]
        if self.notes:
            result["notes"] = list(self.notes)
        if self.run is not None:
            result["run"] = {name: _dump(value) for name, value in self.run.items()}
        if self.body is not None:
            result["body_sha256"] = sha256_hex(self.body.encode())
        if self.legacy is not None:
            legacy: dict[str, JSON] = {"path": self.legacy.path, "cells": dict[str, JSON](self.legacy.cells)}
            if self.legacy.superseded_by:
                legacy["superseded_by"] = list(self.legacy.superseded_by)
            if self.legacy.resolved_by:
                legacy["resolved_by"] = list(self.legacy.resolved_by)
            if self.legacy.implied_status:
                legacy["implied_status"] = self.legacy.implied_status
            result["legacy"] = legacy
        if self.gaps:
            result["gaps"] = list(self.gaps)
        if self.assigned_at_migration:
            result["assigned_at_migration"] = True
        return result


def _dump(value: Value) -> JSON:
    if isinstance(value, tuple):
        return list(value)
    if isinstance(value, dict):
        return dict[str, JSON](value)
    return value


class _Reader:
    """Field parsers bound to one record, so every message names the record and the field."""

    def __init__(self, identifier: str, migrated: bool) -> None:
        self.identifier: str = identifier
        self.migrated: bool = migrated

    def fail(self, name: str, problem: str) -> NoReturn:
        raise RecordError(f"{self.identifier}: {name}: {problem}")

    def string(self, name: str, value: object, *, line: bool = True, empty: bool = False) -> str:
        if not isinstance(value, str):
            self.fail(name, "must be a string")
        if not value and not empty:
            self.fail(name, "must not be empty; omit the field instead")
        if problem := string_problem(value):
            self.fail(name, problem)
        if line and ("\n" in value or value.strip() != value):
            self.fail(name, "must be one trimmed line")
        return value

    def strings(self, name: str, value: object, *, pattern: re.Pattern[str] | None = None, ordered: bool = False) -> tuple[str, ...]:
        if not is_array(value) or not value:
            self.fail(name, "must be a non-empty array; omit the field instead")
        items = tuple(self.string(name, item) for item in value)
        for item in items:
            if pattern is not None and not pattern.fullmatch(item):
                self.fail(name, f"invalid entry {item!r}")
        if len(set(items)) != len(items):
            self.fail(name, "duplicate entries")
        return tuple(sorted(items, key=id_key)) if ordered else items

    def enum(self, name: str, value: object, vocabulary: tuple[str, ...]) -> str:
        text = self.string(name, value)
        if text in vocabulary or (self.migrated and text.startswith(LEGACY) and len(text) > len(LEGACY)):
            return text
        allowed = ", ".join(vocabulary)
        self.fail(name, f"{text!r} is not one of {allowed}" + ("" if self.migrated else " (legacy: values need a migrated record)"))

    def value(self, name: str, spec: Spec, value: object) -> Value:
        match spec.kind:
            case "text":
                return self.string(name, value)
            case "enum":
                return self.enum(name, value, spec.vocabulary)
            case "count":
                if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                    self.fail(name, "must be a non-negative integer")
                return value
            case "flag":
                if value is not True:
                    self.fail(name, "must be true; omit the field otherwise")
                return True
            case "list":
                return self.strings(name, value)
            case "members":
                return self.strings(name, value, pattern=MEMBER)
            case "date":
                return self.date(name, value)
            case "sha256":
                return self.matching(name, value, SHA256)
            case "timestamp":
                return self.matching(name, value, TIMESTAMP)
            case "build":
                return self.matching(name, value, DIGITS)
            case "dates":
                raw = as_mapping(value, f"{self.identifier}: {name}")
                if not raw:
                    self.fail(name, "must not be empty")
                return {self.string(name, key): self.date(f"{name}.{key}", item) for key, item in sorted(raw.items())}
            case _:
                raise AssertionError(spec.kind)

    def matching(self, name: str, value: object, pattern: re.Pattern[str]) -> str:
        text = self.string(name, value)
        if not pattern.fullmatch(text):
            self.fail(name, f"malformed value {text!r}")
        return text

    def date(self, name: str, value: object) -> str:
        text = self.matching(name, value, DAY)
        try:
            _ = date.fromisoformat(text)
        except ValueError:
            self.fail(name, f"invalid date {text!r}")
        return text

    def ids(self, name: str, value: object, prefixes: tuple[str, ...] | None = None) -> tuple[str, ...]:
        items = self.strings(name, value, pattern=ID, ordered=True)
        for item in items:
            if prefixes is not None and item.split("-", 1)[0] not in prefixes:
                self.fail(name, f"{item} has the wrong type")
            if item == self.identifier:
                self.fail(name, "a record cannot link to itself")
        return items

    def cites(self, name: str, value: object) -> tuple[str, ...]:
        items = self.strings(name, value, ordered=True)
        for item in items:
            if not (SELECTOR.fullmatch(item) or (ID.fullmatch(item) and item.startswith("M-"))):
                self.fail(name, f"{item!r} is not M-N, M-N/ev_<12 hex>, M-N/unk_<12 hex> or M-N/E-N")
        return items


def _parse(raw: dict[str, object], body: str | None) -> Record:
    identifier = raw.get("id")
    if not isinstance(identifier, str) or not ID.fullmatch(identifier):
        raise RecordError(f"record: id: {identifier!r} is not a record ID")
    kind = type_of(identifier)
    reader = _Reader(identifier, "legacy" in raw)
    if raw.get("type") != kind:
        reader.fail("type", f"must be {kind!r} for this ID")
    spec = TYPES[kind]
    allowed = ENVELOPE | spec.fields.keys() | ({"run"} if kind == "evidence" else set[str]()) | ({"body_sha256"} if kind == "reference" else set[str]())
    if extra := sorted(raw.keys() - allowed):
        reader.fail(", ".join(extra), "unknown field")
    legacy = _legacy(reader, raw["legacy"], spec) if "legacy" in raw else None
    status = reader.enum("status", raw.get("status"), spec.statuses)
    fields = {name: reader.value(name, field_spec, raw[name]) for name, field_spec in spec.fields.items() if name in raw}
    scope = _scope(reader, raw["scope"]) if "scope" in raw else None
    links = Links(
        supersedes=reader.ids("supersedes", raw["supersedes"], (spec.prefix,)) if "supersedes" in raw else (),
        resolves=reader.ids("resolves", raw["resolves"], ("U", "SL", "RP")) if "resolves" in raw else (),
        cites=reader.cites("cites", raw["cites"]) if "cites" in raw else (),
        refs=reader.ids("refs", raw["refs"]) if "refs" in raw else (),
        verified_on=_verified_on(reader, raw["verified_on"]) if "verified_on" in raw else (),
    )
    notes = reader.strings("notes", raw["notes"]) if "notes" in raw else ()
    run = None
    if "run" in raw:
        run_raw = as_mapping(raw["run"], f"{identifier}: run")
        if extra := sorted(run_raw.keys() - RUN.keys()):
            reader.fail("run", f"unknown fields {', '.join(extra)}")
        if missing := [name for name in RUN_REQUIRED if name not in run_raw]:
            reader.fail("run", f"missing {', '.join(missing)}")
        run = {name: reader.value(f"run.{name}", RUN[name], run_raw[name]) for name in RUN if name in run_raw}
    if kind == "reference":
        if body is None:
            reader.fail("body", "a reference needs its body")
        body = reader.string("body", body, line=False)
        if raw.get("body_sha256") != sha256_hex(body.encode()):
            reader.fail("body_sha256", "does not match the body")
    elif body is not None:
        reader.fail("body", "only a reference has a body")
    gaps = reader.strings("gaps", raw["gaps"], ordered=True) if "gaps" in raw else ()
    assigned = raw.get("assigned_at_migration", False)
    if assigned not in (True, False) or ("assigned_at_migration" in raw and assigned is not True):
        reader.fail("assigned_at_migration", "must be true; omit the field otherwise")
    record = Record(identifier, kind, status, fields, scope, links, notes, run, body, legacy, gaps, assigned is True)
    _validate(reader, record)
    return record


def _legacy(reader: _Reader, value: object, spec: TypeSpec) -> Legacy:
    raw = as_mapping(value, f"{reader.identifier}: legacy")
    if extra := sorted(raw.keys() - {"path", "cells", "superseded_by", "resolved_by", "implied_status"}):
        reader.fail("legacy", f"unknown fields {', '.join(extra)}")
    if "path" not in raw or "cells" not in raw:
        reader.fail("legacy", "needs path and cells")
    cells = {
        reader.string("legacy.cells", key): reader.string(f"legacy.cells.{key}", item, line=False, empty=True)
        for key, item in sorted(as_mapping(raw["cells"], f"{reader.identifier}: legacy.cells").items())
    }
    if not cells:
        reader.fail("legacy.cells", "must not be empty")
    return Legacy(
        path=reader.string("legacy.path", raw["path"]),
        cells=cells,
        superseded_by=reader.ids("legacy.superseded_by", raw["superseded_by"], (spec.prefix,)) if "superseded_by" in raw else (),
        resolved_by=_resolvers(reader, raw["resolved_by"], spec) if "resolved_by" in raw else (),
        implied_status=reader.enum("legacy.implied_status", raw["implied_status"], IMPLIED) if "implied_status" in raw else None,
    )


def _resolvers(reader: _Reader, value: object, spec: TypeSpec) -> tuple[str, ...]:
    if spec.prefix not in ("U", "SL", "RP"):
        reader.fail("legacy.resolved_by", "only an unknown, lead or report is resolved")
    return reader.ids("legacy.resolved_by", value, ("F",))


def _scope(reader: _Reader, value: object) -> Scope:
    raw = as_mapping(value, f"{reader.identifier}: scope")
    if extra := sorted(raw.keys() - {"basis", "subjects", "builds", "releases", "commit"}):
        reader.fail("scope", f"unknown fields {', '.join(extra)}")
    basis = reader.string("scope.basis", raw.get("basis"))
    if basis not in BASES:
        reader.fail("scope.basis", f"{basis!r} is not one of {', '.join(BASES)}")
    subjects = reader.strings("scope.subjects", raw["subjects"], ordered=True) if "subjects" in raw else ()
    for subject in subjects:
        if subject not in SUBJECTS:
            reader.fail("scope.subjects", f"{subject!r} is not one of {', '.join(SUBJECTS)}")
    return Scope(
        basis=basis,
        subjects=subjects,
        builds=reader.strings("scope.builds", raw["builds"], pattern=BUILD, ordered=True) if "builds" in raw else (),
        releases=reader.strings("scope.releases", raw["releases"], pattern=RELEASE, ordered=True) if "releases" in raw else (),
        commit=reader.matching("scope.commit", raw["commit"], COMMIT) if "commit" in raw else None,
    )


def _verified_on(reader: _Reader, value: object) -> tuple[VerifiedOn, ...]:
    if not is_array(value) or not value:
        reader.fail("verified_on", "must be a non-empty array; omit the field instead")
    items: list[VerifiedOn] = []
    for entry in value:
        raw = as_mapping(entry, f"{reader.identifier}: verified_on")
        if set(raw) != {"build", "cites"}:
            reader.fail("verified_on", "each entry holds exactly build and cites")
        build = reader.matching("verified_on.build", raw["build"], BUILD)
        if build.startswith("any:"):
            reader.fail("verified_on.build", "a verification names the build's role")
        items.append(VerifiedOn(build, reader.cites("verified_on.cites", raw["cites"])))
    if len(set(items)) != len(items):
        reader.fail("verified_on", "duplicate entries")
    return tuple(items)


def _validate(reader: _Reader, record: Record) -> None:
    """Rules across fields of one record (see the module docstring)."""
    spec, migrated = record.spec, record.migrated
    known = {*spec.fields, "scope", "refs", "body", "supersedes", "resolves", "cites", "verified_on", "notes"}
    for gap in record.gaps:
        name = gap_field(gap)
        if not migrated:
            reader.fail("gaps", "only a migrated record lists gaps")
        if name not in known and not (name.startswith("scope.") and name[6:] in SCOPE_PARTS):
            reader.fail("gaps", f"{gap!r} is not a field of a {record.type}")
        if _present(record, name):
            reader.fail("gaps", f"{gap!r} is listed as missing but present")
    missing = {gap_field(gap) for gap in record.gaps}
    for name in spec.required:
        if not _present(record, name) and name not in missing:
            reader.fail(name, "required field missing" + (" (list it in gaps if the legacy row lacks it)" if migrated else ""))
    if record.assigned_at_migration != (migrated and record.type in ("compatibility", "report")):
        reader.fail("assigned_at_migration", "is set exactly on migrated CP and RP records")
    if record.scope is not None and not migrated:
        if record.scope.basis != "explicit":
            reader.fail("scope.basis", "legacy-text belongs to migrated records")
        if any(build.startswith("any:") for build in record.scope.builds):
            reader.fail("scope.builds", "a new record names each build's role (server: or client:)")
        if "native" in record.scope.subjects and record.scope.commit is None:
            reader.fail("scope.commit", "required for the native subject")
    if record.type == "finding":
        decision = record.text("kind") == "decision"
        for name in DECISION_FIELDS:
            if name in record.fields and not decision:
                reader.fail(name, "only a decision quotes an instruction")
            if decision and not migrated and name not in record.fields:
                reader.fail(name, "a decision quotes the owner's instruction verbatim with its date and where it was given")
        if not migrated and record.text("kind") in ("derived", "inferred") and not record.links.refs:
            reader.fail("refs", "a derived or inferred finding names its sources")
    if record.type == "evidence":
        bundle = record.text("kind") == "rea-bundle"
        for name in ("records", "members"):
            if name in record.fields and not bundle:
                reader.fail(name, "only a rea-bundle lists records and members")
        if (record.run is not None) != (record.text("kind") == "run") and not (migrated and record.run is None):
            reader.fail("run", "present exactly on run evidence")


def gap_field(gap: str) -> str:
    """The field a gap names: ``<field>``, ``scope.<part>``, either optionally followed by ``: <detail>``."""
    return gap.split(":", 1)[0]


def _present(record: Record, name: str) -> bool:
    if name.startswith("scope."):
        scope = record.scope
        if scope is None:
            return False
        part = name.removeprefix("scope.")
        return bool(scope.commit) if part == "commit" else bool({"subjects": scope.subjects, "builds": scope.builds, "releases": scope.releases}.get(part))
    match name:
        case "scope":
            return record.scope is not None
        case "body":
            return record.body is not None
        case "notes":
            return bool(record.notes)
        case "verified_on":
            return bool(record.links.verified_on)
        case "supersedes" | "resolves" | "cites" | "refs":
            return bool(record.links.named(name))
        case _:
            return name in record.fields
