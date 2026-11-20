# SPDX-License-Identifier: AGPL-3.0-or-later
"""The commands that touch the bank, each a plain function returning an ``Output`` the CLI prints.

Every command takes the host lock, gathers the journal (``bank.gather``) and reconciles the bank with
it before doing anything else, so a change set a killed process left half applied is finished first.
Writes then follow the rules' publication order: validate, write and fsync the segment
(``journal.write_change_set``), reconcile, and read every published document back by exact GET.

Reads use two replays. The bank, links and derived statuses come from the union of every known
segment (``Universe.state``). What this checkout proposes, revises and summarises (``revise`` heads,
``area``, ``status``, ``verify``) comes from ``origin/main`` plus this worktree (``Universe.view``).

``verify`` and ``check --pin`` read only the journal and the evidence bytes, so they work without the
service (the prek hook verifies staged evidence that way). ``check --bank`` changes nothing: it
reports what differs, including documents not yet applied.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import time
import tomllib
from collections.abc import Callable, Generator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Final, Protocol

from tools import sha256_manifest
from tools.memory.bank import (
    Applied, Bank, BankError, Desired, Universe, allocate, desired, diff, gather, host_lock, read_back, reconcile, state_directory,
)
from tools.memory.canonical import render_text
from tools.memory.check import CITATION_EXCLUDED, CITATION_ROOTS, citations, confidence_problems, revision_problems
from tools.memory.gitio import GitError, descends, git, listed_files
from tools.memory.hindsight import AllOf, AnyOf, Client, Group, HindsightError, Not, Tags, connection
from tools.memory.journal import JournalError, Revision, Segment, write_change_set
from tools.memory.model import (
    ID, JSON, MEMBER, RUN, TYPES, Record, RecordError, as_mapping, id_key, is_array, is_object, load_json, sha256_hex,
)
from tools.memory.paths import ProtectedPathsError, primary_checkout
from tools.memory.state import PARTITIONS, RELEASE_CONFIG, State, partition, pinned_build, replay

FETCH_TIMEOUT = 120
DISK_FLOOR = 30 * 1024**3
BUNDLE_PARSE_CAP = 64 * 1024 * 1024
LOCAL_PREFIXES = ("work/", "reference-data/", "icarus-client-orig/", "icarus-orig/")
OPEN_UNKNOWN = ("open", "investigating", "blocked", "contradicted")
OUTCOMES = ("verified", "unavailable", "mismatched", "legacy-unverifiable")
# Expected effective bank config (rules, Instance). Keys absent here are not asserted.
EXPECTED_CONFIG: Final[dict[str, object]] = {
    "retain_extraction_mode": "chunks", "retain_chunk_size": 1800, "enable_observations": False,
    "enable_auto_consolidation": False, "store_document_text": True, "enable_reranking": True, "memory_defense": None,
    "entity_labels": None, "retain_strategies": None, "retain_default_strategy": None, "mcp_enabled_tools": None,
}
QUERY_ID = re.compile(ID.pattern, re.IGNORECASE)


class Refused(ValueError):
    """A command refused its input before writing anything."""


@dataclass
class Output:
    """A command's result: JSON for ``--json`` and the MCP server, text lines, exit status."""

    data: dict[str, JSON]
    lines: list[str]
    status: int = 0


@dataclass
class Session:
    root: Path
    bank: Bank
    universe: Universe
    applied: Applied
    now: datetime


def now_utc() -> datetime:
    return datetime.now(UTC).replace(microsecond=0)


@contextmanager
def session(bank: Bank, root: Path, *, now: datetime | None = None, orphan: bool = False, bulk: bool = False) -> Generator[Session]:
    """Under the host lock: gather the journal and reconcile the bank with it (replay first)."""
    with host_lock():
        universe = gather(root)
        applied = reconcile(bank, universe, orphan=orphan, bulk=bulk)
        yield Session(root, bank, universe, applied, now or now_utc())


def validate(universe: Universe, revisions: Sequence[Revision]) -> list[str]:
    """Every problem the new revisions would add to this checkout's view (schema is checked by the model)."""
    pending = Segment("memory/journal/pending", "", tuple(revisions))
    before, after = universe.view, replay([*universe.own, pending])
    names = {revision.record.id for revision in revisions}

    def mine(problems: Sequence[str]) -> list[str]:
        return [problem for problem in problems if problem.split(":", 1)[0] in names]

    problems = mine(after.problems) + mine(after.link_problems()) + mine(revision_problems(after))
    problems += mine(confidence_problems(after, replay(universe.main)))
    problems += [f"supersession cycle: {cycle}" for cycle in after.supersession_cycles()
                 if names & set(cycle.split(" -> ")) and cycle not in before.supersession_cycles()]
    for identifier in sorted(names, key=id_key):
        entry, earlier = after.entries[identifier], before.entries.get(identifier)
        if entry.conflict and not (earlier is not None and earlier.conflict):
            problems.append(f"{identifier}: would fork the record; revise its current head")
    return problems


def publish(current: Session, revisions: Sequence[Revision]) -> tuple[str, ...]:
    """Validate, write and fsync the segment, apply it, read it back. Returns the segment paths."""
    if problems := validate(current.universe, revisions):
        raise Refused("; ".join(problems))
    written = write_change_set(current.root, current.now, revisions)
    current.universe = gather(current.root)
    current.applied = reconcile(current.bank, current.universe)
    if problems := read_back(current.bank, desired(current.universe), revisions):
        raise BankError("; ".join(problems))
    return tuple(path.relative_to(current.root).as_posix() for path in written)


def published(current: Session, revisions: Sequence[Revision], paths: Sequence[str]) -> Output:
    wanted = desired(current.universe)
    documents = [item.document_id for item in wanted.values() if any(item.revision == revision for revision in revisions)]
    lines = [f"{revision.record.id}: {revision.sha256[:12]} written to {', '.join(paths)}" for revision in revisions]
    lines += [f"applied and read back: {', '.join(documents)}"]
    data: dict[str, JSON] = {"ids": [revision.record.id for revision in revisions], "segments": list(paths), "documents": list(documents),
                             "sha256": [revision.sha256 for revision in revisions]}
    return Output(data, lines)


# Writing commands.


@dataclass(frozen=True)
class Changes:
    """Field edits from flags or a file: values to set (``None`` removes), a note to append, a REF body."""

    values: Mapping[str, JSON | None] = field(default_factory=dict[str, JSON | None])
    note: str | None = None
    body: str | None = None


def propose(bank: Bank, root: Path, kind: str, changes: Changes, *, now: datetime | None = None) -> Output:
    """Allocate an ID (a release names its own ``REL-x.y.z``) and publish the first revision of a record."""
    if kind not in TYPES:
        raise Refused(f"unknown record type {kind!r}; one of {', '.join(TYPES)}")
    spec = TYPES[kind]
    with session(bank, root, now=now) as current:
        data: dict[str, JSON] = {name: value for name, value in changes.values.items() if value is not None}
        given = data.get("id")
        if kind == "release":
            if not isinstance(given, str) or not given.startswith("REL-"):
                raise Refused("a release names its ID (REL-x.y.z) with --set id=REL-x.y.z")
            if given in current.universe.state.entries:
                raise Refused(f"{given} exists; revise it")
        elif given is not None:
            raise Refused("the tool allocates IDs; do not set id")
        else:
            notices: list[str] = []
            data["id"] = allocate(current.universe, bank.documents(), spec.prefix, notices)
        data["type"] = kind
        _ = data.setdefault("status", spec.statuses[0])
        _ = data.setdefault("date", current.now.date().isoformat())
        if changes.note is not None:
            data["notes"] = [changes.note]
        if changes.body is not None:
            data["body_sha256"] = sha256_hex(changes.body.encode())
        revision = Revision.of(Record.from_json(data, changes.body))
        return published(current, [revision], publish(current, [revision]))


def allowed_changes(old: Record, new: Record) -> list[str]:
    """Why ``new`` is not an in-place revision of ``old``: only REF body, unknown next_probe, any stored
    status (evidence with an appended note), and appended notes and ``verified_on`` may change."""
    before, after = old.to_json(), new.to_json()
    allowed = {"status", "notes", "verified_on", *{"reference": ("body_sha256",), "unknown": ("next_probe",)}.get(old.type, ())}
    problems = [f"{name} is not revisable in place; supersede with a new record"
                for name in sorted(before.keys() | after.keys()) if before.get(name) != after.get(name) and name not in allowed]
    for name, kept, now in (("notes", old.notes, new.notes), ("verified_on", old.links.verified_on, new.links.verified_on)):
        if not set(kept) <= set(now):
            problems.append(f"{name} may only be appended to")
    if old.type == "evidence" and old.status != new.status and len(new.notes) <= len(old.notes):
        problems.append("an evidence status change appends a note")
    return problems


def _revision_of(current: Session, identifier: str, edit: Mapping[str, JSON | None], note: str | None, body: str | None) -> tuple[Revision, list[Record]]:
    """A revision of ``identifier`` after this checkout's heads (all of them: a reconciling revision)."""
    entry = current.universe.view.entries.get(identifier)
    if entry is None:
        raise Refused(f"no record {identifier} in this checkout (origin/main plus this worktree)")
    heads = [entry.revisions[key].revision.record for key in entry.heads]
    base = heads[0]
    data = base.to_json()
    notes: list[str] = list(base.notes)
    for other in heads[1:]:
        notes.extend(note_text for note_text in other.notes if note_text not in notes)
    for name, value in edit.items():
        if value is None:
            _ = data.pop(name, None)
        else:
            data[name] = value
    if note is not None:
        notes.append(note)
    if notes:
        data["notes"] = list[JSON](notes)
    new_body = body if body is not None else base.body
    if new_body is not None:
        data["body_sha256"] = sha256_hex(new_body.encode())
    record = Record.from_json(data, new_body)
    if record == base and len(heads) == 1:
        raise Refused(f"{identifier}: nothing changes")
    return Revision.of(record, entry.heads), heads


def revise(bank: Bank, root: Path, identifier: str, changes: Changes, *, now: datetime | None = None) -> Output:
    """Publish an in-place revision; anything outside the revisable allow-list is refused before writing."""
    with session(bank, root, now=now) as current:
        if "id" in changes.values or "type" in changes.values:
            raise Refused("id and type never change")
        revision, heads = _revision_of(current, identifier, changes.values, changes.note, changes.body)
        problems = [problem for head in heads for problem in allowed_changes(head, revision.record)]
        if problems:
            raise Refused(f"{identifier}: " + "; ".join(dict.fromkeys(problems)))
        return published(current, [revision], publish(current, [revision]))


def _link(bank: Bank, root: Path, newer: str, name: str, target: str, now: datetime | None) -> Output:
    with session(bank, root, now=now) as current:
        view = current.universe.view
        if target not in view.entries:
            raise Refused(f"no record {target} in this checkout")
        entry = view.entries.get(newer)
        if entry is None:
            raise Refused(f"no record {newer} in this checkout; propose it first")
        if any(key in current.universe.accepted for key in entry.revisions):
            raise Refused(f"{newer} is accepted; its links are fixed. Propose a new record that {name} {target}")
        links = list(entry.record.links.named(name))
        if target in links:
            raise Refused(f"{newer} already {name} {target}")
        linked: list[JSON] = [*sorted([*links, target], key=id_key)]
        revision, _heads = _revision_of(current, newer, {name: linked}, None, None)
        return published(current, [revision], publish(current, [revision]))


def supersede(bank: Bank, root: Path, old: str, new: str, *, now: datetime | None = None) -> Output:
    """Record on ``new`` (a proposal not yet accepted) that it supersedes ``old``; ``old`` gets no revision."""
    return _link(bank, root, new, "supersedes", old, now)


def resolve(bank: Bank, root: Path, target: str, finding: str, *, now: datetime | None = None) -> Output:
    """Record on finding ``finding`` (not yet accepted) that it resolves unknown, lead or report ``target``."""
    if not finding.startswith("F-"):
        raise Refused("only a finding resolves")
    return _link(bank, root, finding, "resolves", target, now)


def bundle_members(data: bytes) -> tuple[int, list[str]]:
    """``records`` (distinct ``records[].evidence_id``) and ``members`` (those, then ``unknowns[].unknown_id``,
    in bundle order) of an REA bundle, read as ``tools/docs_lint.py`` reads bundle membership."""
    raw = load_json(data)
    if not is_object(raw):
        raise Refused("REA bundle is not a JSON object")
    found: list[list[str]] = []
    for section, key in (("records", "evidence_id"), ("unknowns", "unknown_id")):
        items = raw.get(section)
        if not is_array(items):
            raise Refused(f"REA bundle {section} must be a list")
        names: list[str] = []
        for item in items:
            if is_object(item) and isinstance(value := item.get(key), str) and value not in names:
                if not MEMBER.fullmatch(value):
                    raise Refused(f"REA bundle {section}: {value!r} is not ev_/unk_ followed by hex")
                names.append(value)
        found.append(names)
    return len(found[0]), found[0] + found[1]


def evidence_path(root: Path, path: str) -> tuple[Path, str]:
    """The file and its path relative to the primary checkout (relative input resolves against it)."""
    primary = primary_checkout(root)
    given = Path(path)
    target = given if given.is_absolute() else primary / given
    try:
        relative = target.resolve().relative_to(primary.resolve()).as_posix()
    except ValueError:
        raise Refused(f"{path}: evidence lives in the primary checkout {primary}") from None
    if target.is_dir():
        raise Refused(f"{relative}: a directory; add a sha256 manifest of it instead")
    if not target.is_file():
        raise Refused(f"{relative}: no such file")
    return target, relative


def evidence_add(bank: Bank, root: Path, path: str, kind: str, label: str, *, run: Mapping[str, JSON] | None = None,
                 now: datetime | None = None) -> Output:
    """Publish an M record whose sha256, size, date and bundle members come from the bytes."""
    target, relative = evidence_path(root, path)
    with target.open("rb") as handle:
        digest = hashlib.file_digest(handle, "sha256").hexdigest()
    info = target.stat()
    data: dict[str, JSON] = {
        "type": "evidence", "status": "active", "path": relative, "sha256": digest, "size": info.st_size, "kind": kind, "label": label,
        "class": "local-private" if relative.startswith(LOCAL_PREFIXES) else "tracked",
        "date": datetime.fromtimestamp(info.st_mtime, UTC).date().isoformat(),
    }
    if kind == "rea-bundle":
        records, members = bundle_members(target.read_bytes())
        data["records"] = records
        if members:
            data["members"] = list[JSON](members)
    if run is not None:
        data["run"] = dict(run)
    with session(bank, root, now=now) as current:
        for entry in current.universe.view.entries.values():
            record = entry.record
            if record.type == "evidence" and record.text("path") == relative and record.text("sha256") == digest and record.status == "active":
                raise Refused(f"{relative} with these bytes is {record.id} already")
        notices: list[str] = []
        data["id"] = allocate(current.universe, bank.documents(), "M", notices)
        revision = Revision.of(Record.from_json(data))
        return published(current, [revision], publish(current, [revision]))


# Evidence verification (journal and bytes only).


@dataclass(frozen=True)
class Verdict:
    id: str
    outcome: str
    detail: str
    failing: bool


def verify_record(primary: Path, record: Record) -> Verdict:
    """One evidence record against the bytes under the primary checkout, as the legacy ``--local`` lint reads them."""
    path, digest = record.text("path"), record.text("sha256")
    active_private = record.status == "active" and record.text("class") == "local-private"

    def verdict(outcome: str, detail: str) -> Verdict:
        failing = outcome == "mismatched" or (outcome == "unavailable" and active_private)
        return Verdict(record.id, outcome, detail, failing)

    if path is None or digest is None:
        return verdict("legacy-unverifiable", "no " + ("path" if path is None else "sha256"))
    target = primary / path
    if not target.exists():
        return verdict("unavailable", f"missing: {path}")
    if not target.is_file():
        if record.migrated:
            return verdict("legacy-unverifiable", f"directory locator: {path}")
        return verdict("mismatched", f"not a file: {path}")
    with target.open("rb") as handle:
        if hashlib.file_digest(handle, "sha256").hexdigest() != digest:
            return verdict("mismatched", f"sha256 differs: {path}")
    size = record.number("size")
    if size is not None and target.stat().st_size != size:
        return verdict("mismatched", f"size differs: {path}")
    if record.text("kind") == "rea-bundle" and target.stat().st_size <= BUNDLE_PARSE_CAP:
        try:
            count, members = bundle_members(target.read_bytes())
        except (Refused, RecordError) as error:
            return verdict("mismatched", f"unreadable REA bundle: {error}")
        listed = record.items("members")
        if (expected := record.number("records")) is not None and expected != count:
            return verdict("mismatched", f"bundle holds {count} records, the record says {expected}")
        if missing := [member for member in listed if member not in members]:
            return verdict("mismatched", f"members not in the bundle: {', '.join(missing)}")
    try:
        entries = sha256_manifest.read_manifest(primary, target)
    except sha256_manifest.ManifestError as error:
        return verdict("mismatched", f"unreadable sha256 manifest: {error}")
    if entries is not None and record.id not in sha256_manifest.REMOVAL_RECEIPTS:
        for entry in entries:
            files = [candidate for candidate in entry.candidates if candidate.is_file() and not candidate.is_symlink()]
            if not files:
                return verdict("unavailable", f"manifest entry missing: {entry.path}")
            if entry.sha256 is not None and all(hashlib.sha256(item.read_bytes()).hexdigest() != entry.sha256 for item in files):
                return verdict("mismatched", f"manifest entry sha256 differs: {entry.path}")
            if entry.size is not None and all(item.stat().st_size != entry.size for item in files):
                return verdict("mismatched", f"manifest entry size differs: {entry.path}")
    return verdict("verified", path)


def verify(root: Path, identifiers: Sequence[str], *, every: bool = False) -> Output:
    """``verified``, ``unavailable``, ``mismatched`` or ``legacy-unverifiable`` per evidence record; exit 1 on
    ``mismatched`` or an active local-private record ``unavailable``."""
    universe = gather(root)
    view = universe.view
    primary = primary_checkout(root)
    if every:
        names = sorted((name for name, entry in view.entries.items() if entry.record.type == "evidence"), key=id_key)
    else:
        names = [name.split("/", 1)[0] for name in identifiers]
    verdicts: list[Verdict] = []
    for name in names:
        record = view.record(name)
        if record is None or record.type != "evidence":
            raise Refused(f"{name}: no evidence record in this checkout")
        verdicts.append(verify_record(primary, record))
    counts = {outcome: sum(1 for item in verdicts if item.outcome == outcome) for outcome in OUTCOMES}
    lines = [f"{item.id}: {item.outcome} ({item.detail})" for item in verdicts]
    lines.append("verify: " + ", ".join(f"{count} {outcome}" for outcome, count in counts.items()))
    data: dict[str, JSON] = {"records": [{"id": item.id, "outcome": item.outcome, "detail": item.detail, "failing": item.failing} for item in verdicts],
                             "counts": dict[str, JSON](counts)}
    return Output(data, lines, 1 if any(item.failing for item in verdicts) else 0)


# Reading commands.


def release_for(universe: Universe, root: Path, name: str | None) -> tuple[Record | None, str]:
    """The REL record partitions are computed against: ``name`` (``REL-x.y.z`` or ``x.y.z``) or the pin."""
    state = universe.state
    if name is not None:
        identifier = name if name.startswith("REL-") else f"REL-{name}"
        found = state.record(identifier)
        return found, "" if found is not None else f"no release {identifier}"
    try:
        build = pinned_build(root)
    except (OSError, RecordError, tomllib.TOMLDecodeError) as error:
        return None, f"{RELEASE_CONFIG}: {error}"
    found = state.release(build)
    return found, "" if found is not None else f"no REL record for server build {build}"


def place(root: Path, record: Record, release: Record | None, reason: str) -> tuple[dict[str, JSON], str]:
    """A record's partition as JSON and as one text phrase."""
    if release is None:
        return {"name": "unknown", "reason": reason}, f"unknown ({reason})"
    placed = partition(record, release, lambda commit: descends(root, commit))
    detail = [f"tokens {', '.join(placed.tokens)}"] if placed.tokens else []
    if placed.name == "legacy-scope":
        detail.append(f"names the pin: {'yes' if placed.names_pin else 'no'}")
    if placed.name == "native":
        detail.append(f"HEAD descends from {placed.commit}: {'unknown' if placed.descends is None else 'yes' if placed.descends else 'no'}")
    data: dict[str, JSON] = {"name": placed.name, "release": release.id, "tokens": list(placed.tokens), "names_pin": placed.names_pin,
                             "commit": placed.commit, "descends": placed.descends}
    return data, f"{placed.name} against {release.id}" + "".join(f"; {item}" for item in detail)


def headline(text: str) -> str:
    """The first two lines of a document (``ID type status`` and the headline field) as one line."""
    return " | ".join(text.split("\n")[:2])[:200]


def summary(bank: Bank, wanted: Mapping[str, Desired], identifier: str) -> tuple[dict[str, JSON], str]:
    """A linked record by exact GET of its ``<ID>`` document: ID, state, first line."""
    target = wanted.get(identifier)
    document = bank.document(identifier)
    if target is None or document is None or document.text is None:
        return {"id": identifier, "state": "absent"}, f"{identifier}: not in the bank"
    ok = document.text == target.item.content
    data: dict[str, JSON] = {"id": identifier, "state": target.state, "status": target.status, "line": headline(document.text), "verified": ok}
    return data, f"{identifier} [{target.state}, {target.status}]{'' if ok else ' (differs from the journal)'}: {headline(document.text)}"


def check_document(bank: Bank, target: Desired) -> list[str]:
    document = bank.document(target.document_id)
    if document is None:
        return [f"{target.document_id}: missing from the bank"]
    problems: list[str] = []
    if document.text != target.item.content or document.content_hash != sha256_hex(target.item.content.encode()):
        problems.append(f"{target.document_id}: text differs from the journal")
    if document.metadata != dict(target.item.metadata):
        problems.append(f"{target.document_id}: metadata differs from the journal")
    if document.tags != tuple(sorted(target.item.tags)):
        problems.append(f"{target.document_id}: tags differ from the journal")
    return problems


def get(bank: Bank, root: Path, citation: str, *, history: bool = False) -> Output:
    """Exact document GET, verified against the replayed journal, with state, partition, derived links,
    pending revisions, conflicts and evidence status."""
    with session(bank, root) as current:
        universe, state = current.universe, current.universe.state
        resolution = state.resolve(citation)
        if resolution.problem is not None:
            raise LookupError(resolution.problem)
        identifier = resolution.target
        wanted = desired(universe)
        target = wanted[identifier]
        problems = check_document(bank, target)
        pending = [item for item in wanted.values() if item.record_id == identifier and item.state == "pending"]
        for item in pending:
            problems.extend(check_document(bank, item))
        record = target.revision.record
        release, reason = release_for(universe, root, None)
        placed, phrase = place(root, record, release, reason)
        entry = state.entries[identifier]
        data: dict[str, JSON] = {
            "id": identifier, "document": target.document_id, "state": target.state, "status": target.status, "stored_status": record.status,
            "sha256": target.revision.sha256, "segment": target.segment, "text": target.item.content, "record": record.to_json(),
            "partition": placed, "superseded_by": list(state.superseded_by.get(identifier, ())), "successors": list(state.successors(identifier)),
            "resolved_by": list(state.resolved_by.get(identifier, ())), "verified": not problems, "problems": list(problems),
            "pending": [{"document": item.document_id, "sha256": item.revision.sha256, "segment": item.segment} for item in pending],
        }
        lines = [target.item.content, "", f"state: {target.state}", f"status: {target.status}" + (f" (stored {record.status})" if target.status != record.status else ""),
                 f"partition: {phrase}", f"document: {target.document_id} sha256 {target.revision.sha256} in {target.segment}"]
        if "/" in citation:
            data["selector"], data["member"] = citation, resolution.member
            lines.insert(0, f"{citation}: " + (f"listed member {resolution.member}" if resolution.member else "legacy case note") + f" of {identifier}\n")
        for name in ("supersedes", "resolves", "refs"):
            if items := record.links.named(name):
                lines.append(f"{name}: {', '.join(items)}")
        if superseded := state.superseded_by.get(identifier):
            lines.append(f"superseded by: {', '.join(superseded)} (terminal: {', '.join(state.successors(identifier))})")
        if resolvers := state.resolved_by.get(identifier):
            lines.append(f"resolved by: {', '.join(resolvers)}")
        lines.extend(f"pending revision: {item.document_id} sha256 {item.revision.sha256} in {item.segment}" for item in pending)
        if target.state == "conflict" or entry.conflict:
            lines.extend(f"conflict head: {entry.revisions[key].revision.sha256} in {entry.revisions[key].segment}" for key in entry.heads)
            data["heads"] = [entry.revisions[key].revision.sha256 for key in entry.heads]
        primary = primary_checkout(root)
        if record.type == "evidence":
            verdict = verify_record(primary, record)
            data["evidence"] = {"outcome": verdict.outcome, "detail": verdict.detail}
            lines.append(f"evidence: {verdict.outcome} ({verdict.detail})")
        cited: list[JSON] = []
        for name in sorted({item.split("/", 1)[0] for item in record.links.citations()}, key=id_key):
            evidence = state.record(name)
            shown = state.status(name) if evidence is not None else "absent"
            cited.append({"id": name, "status": shown})
            lines.append(f"cites {name}: {shown}")
        data["cited_evidence"] = cited
        if history:
            data["history"] = [{"sha256": item.revision.sha256, "segment": item.segment, "prev": list(item.revision.prev), "record": item.revision.record.to_json()}
                               for item in entry.history()]
            lines.append("history:")
            for item in entry.history():
                lines.extend(["", f"{item.revision.sha256} in {item.segment}", render_text(item.revision.record)])
        lines.extend(f"problem: {problem}" for problem in problems)
        return Output(data, lines, 1 if problems else 0)


def visibility(mine: frozenset[str], *, include_evidence: bool, others: bool = False) -> list[Group]:
    """Strict tag groups of default recall (accepted plus this worktree's proposals) or of other worktrees' proposals."""
    groups: list[Group] = []
    own = Tags(tuple(f"seg:{stem}" for stem in sorted(mine)), every=False) if mine else None
    if others:
        groups.append(Tags(("state:proposed",)))
        if own is not None:
            groups.append(Not(own))
    else:
        choices: list[Group] = [Tags(("state:accepted",)), Tags(("state:conflict",))]
        if own is not None:
            choices.append(AllOf((Tags(("state:proposed",)), own)))
        groups.append(AnyOf(tuple(choices)))
    if not include_evidence:
        groups.append(Not(Tags(("type:evidence",))))
    return groups


def visible(labels: Sequence[str], mine: frozenset[str], *, include_evidence: bool) -> bool:
    """``visibility`` evaluated on one document's tags (for substring hits, which the API cannot filter by group)."""
    found = set(labels)
    if "type:evidence" in found and not include_evidence:
        return False
    return bool(found & {"state:accepted", "state:conflict"}) or ("state:proposed" in found and any(f"seg:{stem}" in found for stem in mine))


def pinned_timestamp(state: State) -> str:
    """``query_timestamp``: the end of the newest record date in the journal, so ranking never follows the clock."""
    dates = [date for entry in state.entries.values() if (date := entry.record.text("date"))]
    return f"{max(dates)}T23:59:59Z" if dates else "2026-01-01T00:00:00Z"


@dataclass
class Shown:
    identifier: str
    document: str
    rank: int
    retired: list[str] = field(default_factory=list[str])


def recall(bank: Bank, root: Path, query: str, *, release: str | None = None, text: str | None = None,
           all_proposals: bool = False, include_evidence: bool = False, limit: int = 10) -> Output:
    """The rules' Reading section: exact GET of named IDs first, strict-tag recall, partitions, retired
    hits under their terminal successor, one hop of links and resolvers by exact GET."""
    with session(bank, root) as current:
        universe, state = current.universe, current.universe.state
        wanted = desired(universe)
        mine = universe.local_stems
        stamp = pinned_timestamp(state)
        rel, reason = release_for(universe, root, release)
        named = list(dict.fromkeys(match[0].upper() for match in QUERY_ID.finditer(query)))
        exact: list[JSON] = []
        lines: list[str] = []
        for identifier in named:
            data, line = summary(bank, wanted, identifier)
            exact.append(data)
            lines.append(f"named: {line}")

        def ranked(groups: list[Group]) -> list[str]:
            order: list[str] = []
            for hit in bank.recall(query, groups, query_timestamp=stamp):
                if hit.document_id is not None and hit.document_id not in order and (text is None or text.casefold() in hit.text.casefold()):
                    order.append(hit.document_id)
            if text is not None:
                extra = {unit.document_id for unit in bank.units(text=text)
                         if unit.document_id is not None and visible(unit.tags, mine, include_evidence=include_evidence)}
                order.extend(sorted(extra - set(order), key=id_key))
            return order

        documents = ranked(visibility(mine, include_evidence=include_evidence))[:limit]
        shown: dict[str, Shown] = {}
        for rank, document_id in enumerate(documents):
            identifier = document_id.split("@", 1)[0]
            if identifier not in state.entries:
                _ = shown.setdefault(identifier, Shown(identifier, document_id, rank))
                continue
            heads = state.successors(identifier) if state.retired(identifier) else ()
            for successor in heads or (identifier,):
                item = shown.setdefault(successor, Shown(successor, successor, rank))
                if heads:
                    item.retired.append(identifier)
        sections: dict[str, list[JSON]] = {name: [] for name in (*PARTITIONS, "unknown", "not-in-journal")}
        texts: dict[str, list[str]] = {name: [] for name in sections}
        listed = set(shown)
        for item in sorted(shown.values(), key=lambda value: value.rank):
            target = wanted.get(item.document)
            if target is None:
                sections["not-in-journal"].append({"id": item.identifier, "document": item.document})
                texts["not-in-journal"].append(f"{item.document}: in the bank but not in any gathered journal")
                continue
            document = bank.document(target.document_id)
            verified = document is not None and document.text == target.item.content
            placed, phrase = place(root, target.revision.record, rel, reason)
            name = str(placed["name"])
            links = sorted({*target.revision.record.links.targets(), *state.superseded_by.get(item.identifier, ()),
                            *state.resolved_by.get(item.identifier, ())} - listed - set(item.retired), key=id_key)
            linked = [summary(bank, wanted, link) for link in links]
            retired = [summary(bank, wanted, old) for old in sorted(set(item.retired), key=id_key)]
            sections[name].append({
                "id": item.identifier, "document": target.document_id, "state": target.state, "status": target.status, "partition": placed,
                "text": target.item.content, "verified": verified, "retired": [data for data, _ in retired], "links": [data for data, _ in linked],
            })
            block = [f"{item.identifier} [{target.state}, {target.status}; {phrase}]" + ("" if verified else " (differs from the journal)"),
                     *(f"  {line}" for line in target.item.content.split("\n"))]
            derived = [f"  {label}: {', '.join(names)}" for label, names in (
                ("superseded by", state.superseded_by.get(item.identifier, ())), ("resolved by", state.resolved_by.get(item.identifier, ()))) if names]
            block += derived
            block += [f"  retired under it: {line}" for _, line in retired]
            block += [f"  link: {line}" for _, line in linked]
            texts[name].append("\n".join(block))
        for name, blocks in texts.items():
            if blocks:
                lines.append(f"== {name} ==")
                lines.extend(blocks)
        data: dict[str, JSON] = {"query": query, "query_timestamp": stamp, "named": exact,
                                 "partitions": {name: items for name, items in sections.items() if items}}
        if all_proposals:
            others = ranked(visibility(mine, include_evidence=include_evidence, others=True))[:limit]
            listing = [summary(bank, wanted, document_id.split("@", 1)[0]) for document_id in others]
            data["other_proposals"] = [item for item, _ in listing]
            if listing:
                lines.append("== proposals of other worktrees ==")
                lines.extend(line for _, line in listing)
        if not shown and not exact:
            lines.append("no hits")
        return Output(data, lines)


def record_rows(universe: Universe, root: Path, identifiers: Sequence[str], release: Record | None, reason: str) -> tuple[list[JSON], list[str]]:
    view, states = universe.view, {item.record_id: item.state for item in desired(universe).values() if "@" not in item.document_id}
    rows: list[JSON] = []
    lines: list[str] = []
    for identifier in identifiers:
        record = view.entries[identifier].record
        placed, phrase = place(root, record, release, reason)
        status = view.status(identifier)
        rows.append({"id": identifier, "status": status, "state": states.get(identifier, "unknown"), "partition": placed, "line": headline(render_text(record))})
        lines.append(f"  {identifier} [{states.get(identifier, 'unknown')}, {status}; {phrase}]: {headline(render_text(record)).split(' | ', 1)[-1]}")
    return rows, lines


def area(bank: Bank, root: Path, name: str) -> Output:
    """Records of one area by status and partition, its open unknowns and the closure of their links."""
    with session(bank, root) as current:
        universe = current.universe
        view = universe.view
        release, reason = release_for(universe, root, None)
        members = sorted((identifier for identifier, entry in view.entries.items() if entry.record.text("area") == name), key=id_key)
        by_status: dict[str, list[str]] = {}
        for identifier in members:
            by_status.setdefault(view.status(identifier), []).append(identifier)
        data: dict[str, JSON] = {"area": name, "release": release.id if release is not None else None}
        lines = [f"area {name}: {len(members)} record(s); partitions against {release.id if release is not None else 'unknown (' + reason + ')'}"]
        statuses: dict[str, JSON] = {}
        for status in sorted(by_status):
            rows, text = record_rows(universe, root, by_status[status], release, reason)
            statuses[status] = rows
            lines.append(f"{status}:")
            lines.extend(text)
        data["statuses"] = statuses
        unknowns = [identifier for identifier in members if view.entries[identifier].record.type == "unknown" and view.status(identifier) in OPEN_UNKNOWN]
        data["open_unknowns"] = list[JSON](unknowns)
        lines.append("open unknowns: " + (", ".join(unknowns) or "none"))
        closure: set[str] = set()
        pending = list(members)
        while pending:
            identifier = pending.pop()
            entry = view.entries.get(identifier)
            if entry is None:
                continue
            for target in (*entry.record.links.targets(), *view.superseded_by.get(identifier, ()), *view.resolved_by.get(identifier, ())):
                if target not in closure and target not in members and target in view.entries:
                    closure.add(target)
                    pending.append(target)
        outside = sorted(closure, key=id_key)
        rows, text = record_rows(universe, root, outside, release, reason)
        data["closure"] = rows
        lines.append(f"linked records outside the area ({len(outside)}):")
        lines.extend(text)
        return Output(data, lines)


def cited_not_current(root: Path, universe: Universe, release: Record | None, reason: str) -> list[tuple[str, str, str]]:
    """``(ID, why, first location)`` of each record cited in code and documents that is retired or not ``current``."""
    view = universe.view
    found: dict[str, tuple[str, str]] = {}
    for path in listed_files(root, *CITATION_ROOTS):
        if path.startswith(CITATION_EXCLUDED):
            continue
        full = root / path
        try:
            text = full.read_text(encoding="utf-8") if full.is_file() and not full.is_symlink() else ""
        except (OSError, UnicodeDecodeError):
            continue
        for number, citation in citations(text):
            identifier = citation.split("/", 1)[0]
            if identifier in found or identifier not in view.entries:
                continue
            record = view.entries[identifier].record
            if view.retired(identifier):
                found[identifier] = (view.status(identifier), f"{path}:{number}")
            elif release is None:
                found[identifier] = (f"partition unknown ({reason})", f"{path}:{number}")
            else:
                placed = partition(record, release, lambda commit: descends(root, commit))
                if placed.name != "current":
                    found[identifier] = (placed.name, f"{path}:{number}")
    return [(identifier, why, where) for identifier, (why, where) in sorted(found.items(), key=lambda item: id_key(item[0]))]


def status(bank: Bank, root: Path) -> Output:
    """The pin; counts by type, status and partition; cited records not current; open unknowns; evidence
    that is not known good on disk (missing bytes or no digest; ``verify --all`` checks digests)."""
    with session(bank, root) as current:
        universe = current.universe
        view = universe.view
        release, reason = release_for(universe, root, None)
        primary = primary_checkout(root)
        by_type: dict[str, dict[str, int]] = {}
        by_partition: dict[str, int] = {}
        unknowns: list[str] = []
        unverified: list[tuple[str, str]] = []
        for identifier in sorted(view.entries, key=id_key):
            record = view.entries[identifier].record
            shown = view.status(identifier)
            counts = by_type.setdefault(record.type, {})
            counts[shown] = counts.get(shown, 0) + 1
            if record.type == "evidence":
                path = record.text("path")
                if record.status == "active" and (record.text("sha256") is None or path is None or not (primary / path).exists()):
                    unverified.append((identifier, "no sha256" if record.text("sha256") is None else f"missing: {path}"))
                continue
            if record.type == "unknown" and shown in OPEN_UNKNOWN:
                unknowns.append(identifier)
            if release is not None and record.type != "release":
                name = partition(record, release, lambda commit: descends(root, commit)).name
                by_partition[name] = by_partition.get(name, 0) + 1
        cited = cited_not_current(root, universe, release, reason)
        lines = [f"pin: {release.id if release is not None else 'unknown'}" + (f" ({reason})" if reason else ""),
                 f"bank: {current.applied.retained} retained, {current.applied.retagged} re-tagged, {current.applied.deleted} deleted by replay"]
        for kind in sorted(by_type):
            lines.append(f"{kind}: " + ", ".join(f"{count} {name}" for name, count in sorted(by_type[kind].items())))
        lines.append("partitions (records other than evidence and releases): " + ", ".join(f"{by_partition.get(name, 0)} {name}" for name in PARTITIONS))
        lines.append(f"cited records not current ({len(cited)}):")
        lines.extend(f"  {identifier}: {why} (first cited at {where})" for identifier, why, where in cited)
        lines.append("open unknowns: " + (", ".join(unknowns) or "none"))
        lines.append(f"evidence not known good on disk ({len(unverified)}); run verify --all for digests")
        lines.extend(f"  {identifier}: {why}" for identifier, why in unverified)
        lines.extend(f"notice: {notice}" for notice in current.applied.notices)
        data: dict[str, JSON] = {
            "pin": release.id if release is not None else None, "reason": reason,
            "by_type": {kind: dict[str, JSON](counts) for kind, counts in by_type.items()},
            "by_partition": {name: by_partition.get(name, 0) for name in PARTITIONS},
            "cited_not_current": [{"id": identifier, "why": why, "where": where} for identifier, why, where in cited],
            "open_unknowns": list[JSON](unknowns), "evidence_unverified": [{"id": identifier, "why": why} for identifier, why in unverified], "notices": list[JSON](current.applied.notices),
        }
        return Output(data, lines)


def fetch(root: Path) -> str | None:
    """``git fetch origin main``; ``None`` on success, else why it failed."""
    try:
        result = subprocess.run(["git", "-C", str(root), "fetch", "--quiet", "origin", "main"], capture_output=True, check=False, timeout=FETCH_TIMEOUT)
    except (OSError, subprocess.TimeoutExpired) as error:
        return str(error)
    return None if result.returncode == 0 else result.stderr.decode(errors="replace").strip() or f"exit {result.returncode}"


def sync(bank: Bank, root: Path) -> Output:
    """Under the host lock: fetch ``origin main``, then reconcile states with it. Only after a successful
    fetch are documents of segments in no ref or worktree re-tagged ``orphaned``; they are un-orphaned
    once their segment is seen again."""
    with host_lock():
        failure = fetch(root)
        universe = gather(root)
        applied = reconcile(bank, universe, orphan=failure is None)
    counts: dict[str, int] = {}
    for item in desired(universe).values():
        counts[item.state] = counts.get(item.state, 0) + 1
    lines = ["fetch: " + ("ok" if failure is None else f"FAILED ({failure}); nothing was orphaned")]
    lines.append("documents: " + ", ".join(f"{count} {name}" for name, count in sorted(counts.items())))
    lines.append(f"applied: {applied.retained} retained, {applied.retagged} re-tagged, {applied.deleted} deleted, {len(applied.orphaned)} orphaned")
    lines.extend(f"orphaned: {name}" for name in applied.orphaned)
    lines.extend(f"unexplained (not orphaned): {name}" for name in applied.unexplained)
    lines.extend(f"notice: {notice}" for notice in applied.notices)
    data: dict[str, JSON] = {"fetched": failure is None, "fetch_error": failure, "states": dict[str, JSON](counts), "retained": applied.retained,
                             "retagged": applied.retagged, "deleted": applied.deleted, "orphaned": list[JSON](applied.orphaned),
                             "unexplained": list[JSON](applied.unexplained)}
    return Output(data, lines, 0 if failure is None else 1)


def rebuild(bank: Bank, root: Path) -> Output:
    """Bring the bank to the journal with bulk async retains (an empty bank is rebuilt from scratch), then ``check --bank``."""
    free = shutil.disk_usage(state_directory() if state_directory().exists() else root).free
    if free < DISK_FLOOR:
        raise Refused(f"free disk {free / 1024**3:.1f} GiB is below the {DISK_FLOOR / 1024**3:.0f} GiB floor")
    start = time.monotonic()
    with host_lock():
        universe = gather(root)
        applied = reconcile(bank, universe, bulk=True)
    seconds = time.monotonic() - start
    checked = check_bank(bank, root)
    lines = [f"rebuild: {applied.retained} retained, {applied.retagged} re-tagged, {applied.deleted} deleted in {seconds:.1f} s", *checked.lines]
    data: dict[str, JSON] = {"retained": applied.retained, "retagged": applied.retagged, "deleted": applied.deleted, "milliseconds": round(seconds * 1000),
                             "check": checked.data}
    return Output(data, lines, checked.status)


def check_bank(bank: Bank, root: Path) -> Output:
    """The Instance assertions and a document-by-document comparison with the journal. Changes nothing;
    holds the host lock so a concurrent write is never reported as drift."""
    with host_lock():
        return _check_bank(bank, root)


def _check_bank(bank: Bank, root: Path) -> Output:
    universe = gather(root)
    problems: list[str] = []
    notices: list[str] = list(universe.notices)
    config, overrides = bank.config()
    for key, expected in EXPECTED_CONFIG.items():
        if config.get(key) != expected:
            problems.append(f"config {key} is {config.get(key)!r}, expected {expected!r}")
    if overrides:
        problems.append(f"bank-level config overrides present: {', '.join(sorted(overrides))}")
    if (banks := bank.banks()) != (bank.bank,):
        problems.append(f"banks are {', '.join(banks) or 'none'}; {bank.bank} must be the only one")
    for kind in ("mental-models", "directives", "pages", "webhooks"):
        if count := bank.count(kind):
            problems.append(f"{count} {kind} exist; none may")
    wanted = desired(universe)
    held = bank.documents()
    plan = diff(wanted, held, universe.known)
    problems += [f"{item.document_id}: not applied (missing or differing content)" for item in plan.retain]
    problems += [f"{name}: tags differ from the journal" for name, _ in plan.retag]
    problems += [f"{name}: stale revision still in the bank" for name in plan.delete]
    for document in held:
        if not any(tag.startswith(prefix) for tag in document.tags for prefix in ("type:",)) or not any(tag.startswith("state:") for tag in document.tags):
            problems.append(f"{document.id}: untagged or missing type:/state: tags")
    for document in plan.unexplained:
        record = document.metadata.get("record", "")
        if "state:orphaned" in document.tags and sha256_hex(record.encode()) == document.metadata.get("sha256"):
            notices.append(f"{document.id}: orphaned (segment in no ref or worktree)")
        else:
            problems.append(f"{document.id}: not explained by the journal")
    units_by: dict[str, list[str]] = {}
    for unit in bank.units():
        if unit.kind != "world":
            problems.append(f"memory unit {unit.id} is of type {unit.kind}; only world facts may exist")
        if unit.document_id is None:
            problems.append(f"memory unit {unit.id} has no document")
            continue
        units_by.setdefault(unit.document_id, []).append(unit.text)
    for document_id, target in wanted.items():
        texts = units_by.get(document_id, [])
        if document_id in {item.id for item in held} and texts != [target.item.content]:
            problems.append(f"{document_id}: {len(texts)} memory unit(s); expected one holding the document text")
    lines = [*problems, *(f"notice: {notice}" for notice in notices), f"check --bank: {len(wanted)} journal document(s), {len(held)} bank document(s), {len(problems)} problem(s)"]
    return Output({"problems": list[JSON](problems), "notices": list[JSON](notices), "documents": len(held), "expected": len(wanted)}, lines, 1 if problems else 0)


def check_pin(root: Path) -> Output:
    universe = gather(root)
    release, reason = release_for(universe, root, None)
    cited = cited_not_current(root, universe, release, reason)
    lines = [f"{identifier}: {why} (first cited at {where})" for identifier, why, where in cited]
    lines.append(f"check --pin: {len(cited)} cited record(s) not current against {release.id if release is not None else 'unknown (' + reason + ')'}")
    return Output({"cited_not_current": [{"id": identifier, "why": why, "where": where} for identifier, why, where in cited]}, lines)


# Command line.


class Subparsers(Protocol):
    """The part of ``argparse``'s subparser action ``register`` uses."""

    choices: dict[str, argparse.ArgumentParser]

    def add_parser(self, name: str, *, help: str | None = ...) -> argparse.ArgumentParser: ...  # noqa: A002 - argparse's name


def _field_flags(parser: argparse.ArgumentParser) -> None:
    _ = parser.add_argument("--set", action="append", default=[], metavar="NAME=TEXT", help="set a field to a string")
    _ = parser.add_argument("--set-json", action="append", default=[], metavar="NAME=JSON", help="set a field to a JSON value")
    _ = parser.add_argument("--unset", action="append", default=[], metavar="NAME", help="remove a field")
    _ = parser.add_argument("--file", help="JSON object of fields to set ('-' for standard input)")
    _ = parser.add_argument("--note", help="append a note")
    _ = parser.add_argument("--body", help="file holding a reference's Markdown body")


def register(subparsers: Subparsers) -> None:
    """Add the online commands, and ``--bank``/``--pin`` to ``check``; the CLI routes them to ``run``."""
    propose_parser = subparsers.add_parser("propose", help="propose a new record")
    _ = propose_parser.add_argument("type", choices=sorted(TYPES))
    _field_flags(propose_parser)
    revise_parser = subparsers.add_parser("revise", help="revise a record in place (revisable fields only)")
    _ = revise_parser.add_argument("id")
    _field_flags(revise_parser)
    supersede_parser = subparsers.add_parser("supersede", help="record that NEW supersedes OLD")
    _ = supersede_parser.add_argument("old")
    _ = supersede_parser.add_argument("--by", required=True, dest="new")
    resolve_parser = subparsers.add_parser("resolve", help="record that finding F-N resolves an unknown, lead or report")
    _ = resolve_parser.add_argument("id")
    _ = resolve_parser.add_argument("--by", required=True, dest="finding")
    evidence_parser = subparsers.add_parser("evidence", help="evidence records")
    evidence_commands = evidence_parser.add_subparsers(dest="action", required=True)
    add_parser = evidence_commands.add_parser("add", help="record an evidence file from its bytes")
    _ = add_parser.add_argument("path")
    _ = add_parser.add_argument("--kind", required=True, choices=TYPES["evidence"].fields["kind"].vocabulary)
    _ = add_parser.add_argument("--label", required=True)
    _ = add_parser.add_argument("--run", help="JSON file of the run fields (kind run)")
    verify_parser = subparsers.add_parser("verify", help="verify evidence bytes")
    _ = verify_parser.add_argument("ids", nargs="*")
    _ = verify_parser.add_argument("--all", action="store_true", dest="every")
    recall_parser = subparsers.add_parser("recall", help="ranked search, partitioned")
    _ = recall_parser.add_argument("query")
    _ = recall_parser.add_argument("--release")
    _ = recall_parser.add_argument("--text")
    _ = recall_parser.add_argument("--all-proposals", action="store_true")
    _ = recall_parser.add_argument("--include-evidence", action="store_true")
    area_parser = subparsers.add_parser("area", help="one area's records, unknowns and links")
    _ = area_parser.add_argument("area")
    status_parser = subparsers.add_parser("status", help="pin, counts, cited records not current, open unknowns")
    _ = subparsers.add_parser("sync", help="fetch origin main, then reconcile states")
    _ = subparsers.add_parser("rebuild", help="rebuild the bank from the journal")
    for parser in (recall_parser, area_parser, status_parser, verify_parser):
        _ = parser.add_argument("--json", action="store_true", help="print JSON")
    check_parser = subparsers.choices["check"]
    _ = check_parser.add_argument("--bank", action="store_true", help="assert the instance and compare every document")
    _ = check_parser.add_argument("--pin", action="store_true", help="list cited records not current")


def _string(values: Mapping[str, object], name: str) -> str | None:
    value = values.get(name)
    return value if isinstance(value, str) else None


def _flag(values: Mapping[str, object], name: str) -> bool:
    return values.get(name) is True


def _strings(values: Mapping[str, object], name: str) -> list[str]:
    value = values.get(name)
    return [item for item in value if isinstance(item, str)] if is_array(value) else []


def _read(path: str) -> str:
    if path == "-":
        return sys.stdin.read()
    return Path(path).read_text(encoding="utf-8")


def changes_from(values: Mapping[str, object]) -> Changes:
    edits: dict[str, JSON | None] = {}
    if (source := _string(values, "file")) is not None:
        loaded = load_json(_read(source))
        for name, value in as_mapping(loaded, source).items():
            edits[name] = _as_json(value)
    for item in _strings(values, "set"):
        name, sep, value = item.partition("=")
        if not sep:
            raise Refused(f"--set {item!r}: expected NAME=TEXT")
        edits[name] = value
    for item in _strings(values, "set_json"):
        name, sep, value = item.partition("=")
        if not sep:
            raise Refused(f"--set-json {item!r}: expected NAME=JSON")
        edits[name] = _as_json(load_json(value))
    for name in _strings(values, "unset"):
        edits[name] = None
    body = _string(values, "body")
    return Changes(edits, _string(values, "note"), _read(body) if body is not None else None)


def _as_json(value: object) -> JSON:
    """``load_json`` output as ``JSON`` (it yields only JSON values)."""
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if is_array(value):
        return [_as_json(item) for item in value]
    if is_object(value):
        return {str(key): _as_json(item) for key, item in value.items()}
    raise Refused(f"unsupported JSON value {value!r}")


def execute(values: Mapping[str, object], root: Path) -> Output:
    """Run one parsed command (``values`` is ``vars`` of the parsed options)."""
    command = values.get("command")
    if command == "verify":
        return verify(root, _strings(values, "ids"), every=_flag(values, "every"))
    if command == "check":
        merged = Output({}, [], 0)
        plain = not (_flag(values, "ci") or _flag(values, "bank") or _flag(values, "pin"))
        for wanted, produce in ((_flag(values, "pin"), lambda: check_pin(root)), (_flag(values, "bank") or plain, lambda: check_bank(Client(connection()), root))):
            if wanted:
                output = produce()
                merged.data.update(output.data)
                merged.lines.extend(output.lines)
                merged.status = max(merged.status, output.status)
        return merged
    bank = Client(connection())
    match command:
        case "propose":
            return propose(bank, root, _string(values, "type") or "", changes_from(values))
        case "revise":
            return revise(bank, root, _string(values, "id") or "", changes_from(values))
        case "supersede":
            return supersede(bank, root, _string(values, "old") or "", _string(values, "new") or "")
        case "resolve":
            return resolve(bank, root, _string(values, "id") or "", _string(values, "finding") or "")
        case "evidence":
            run_file = _string(values, "run")
            run = None
            if run_file is not None:
                run = {name: _as_json(value) for name, value in as_mapping(load_json(_read(run_file)), run_file).items()}
                if extra := sorted(set(run) - set(RUN)):
                    raise Refused(f"--run: unknown fields {', '.join(extra)}")
            return evidence_add(bank, root, _string(values, "path") or "", _string(values, "kind") or "", _string(values, "label") or "", run=run)
        case "get":
            return get(bank, root, _string(values, "id") or "", history=_flag(values, "history"))
        case "recall":
            return recall(bank, root, _string(values, "query") or "", release=_string(values, "release"), text=_string(values, "text"),
                          all_proposals=_flag(values, "all_proposals"), include_evidence=_flag(values, "include_evidence"))
        case "area":
            return area(bank, root, _string(values, "area") or "")
        case "status":
            return status(bank, root)
        case "sync":
            return sync(bank, root)
        case "rebuild":
            return rebuild(bank, root)
        case _:
            raise Refused(f"unknown command {command!r}")


def run(options: argparse.Namespace, report_check: Callable[[Path, str], int]) -> int:
    """Print one online command's output. Exit 0, or 1 for problems, refusals and an unavailable service.
    ``check`` without flags runs ``--ci`` and ``--bank``; ``--ci`` output comes from the core."""
    values: dict[str, object] = dict(vars(options))
    command = values.get("command")
    try:
        root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())
        core = 0
        if command == "check" and (_flag(values, "ci") or not (_flag(values, "bank") or _flag(values, "pin"))):
            core = report_check(root, _string(values, "base") or "origin/main")
        output = execute(values, root)
    except (Refused, RecordError, JournalError, GitError, LookupError, HindsightError, BankError, ProtectedPathsError, TimeoutError, OSError) as error:
        print(f"memory {command}: {error}", file=sys.stderr)
        return 1
    if output.lines or output.data:
        print(json.dumps(output.data, ensure_ascii=False, indent=2, sort_keys=True) if _flag(values, "json") else "\n".join(output.lines))
    return output.status or core
