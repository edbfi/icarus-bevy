# SPDX-License-Identifier: AGPL-3.0-or-later
"""Prepare reproducible, reviewed topic cutovers and enforce their generated views."""

from __future__ import annotations

import difflib
import re
from collections.abc import Iterator, Mapping
from dataclasses import dataclass
from datetime import date
from pathlib import Path, PurePosixPath

from tools.docs_lint import (
    DEFAULT_FILE_CAP,
    EVIDENCE_ID,
    FILE_CAPS,
    LABEL_CAP,
    LEDGER_ROW_CAP,
    LEGACY_LAST,
    LISTED_ID_LIMIT,
    LOCAL_PREFIXES,
    NOTE_CAP,
    PREFIX_CAPS,
    REA_UNKNOWN_ID,
    RECEIPT_CLASSES,
    RECORD_CAP,
    RECORD_CLASSES,
    RECORD_KINDS,
    RECORD_STATUSES,
    SHARD_CAP,
    ManifestRow,
    parse_json_object,
)
from tools.evidence_catalog_model import (
    RETIREMENT,
    SHARD,
    CatalogError,
    Document,
    Index,
    Record,
    as_list,
    as_object,
    as_text,
    digest,
    expect_keys,
    json_line,
    make_record,
    text_list,
    topic,
)
from tools.evidence_catalog_store import (
    AUTHORITY,
    COMMIT,
    LEGACY_ROOTS,
    SHA256,
    Catalog,
    checkout_paths,
    commit_id,
    git,
    read_blobs,
    read_regular,
    tree_files,
)

TOPIC = re.compile(r"[a-z][a-z0-9]*(?:-[a-z0-9]+)*")
RECONCILED = re.compile(r"Last reconciled: [0-9]{4}-[0-9]{2}-[0-9]{2}")


def iso_day(value: str) -> str:
    if date.fromisoformat(value).isoformat() != value:
        raise CatalogError(f"expected YYYY-MM-DD, got {value!r}")
    return value


def topic_set(values: object, name: str) -> frozenset[str]:
    names = text_list(values, name)
    if len(set(names)) != len(names) or any(not TOPIC.fullmatch(item) for item in names):
        raise CatalogError(f"{name}: expected distinct topic names")
    return frozenset(names)


@dataclass(frozen=True)
class Update:
    path: str
    record: Record


@dataclass(frozen=True)
class Request:
    base: str
    activate: frozenset[str]
    release: frozenset[str]
    updates: tuple[Update, ...]
    reconciled: str | None
    payload: dict[str, object]

    @classmethod
    def parse(cls, raw: dict[str, object]) -> Request:
        expect_keys(raw, {"base"}, {"activate", "release", "upsert", "reconciled"})
        base = as_text(raw["base"], "base")
        if not COMMIT.fullmatch(base):
            raise CatalogError("request base must be the full commit ID")
        activate = topic_set(raw.get("activate", []), "activate")
        release = topic_set(raw.get("release", []), "release")
        updates: list[Update] = []
        for item in as_list(raw.get("upsert", []), "upsert"):
            change = as_object(item, "upsert entry")
            expect_keys(change, {"path", "id", "fields"})
            path = as_text(change["path"], "path")
            identifier = as_text(change["id"], "id")
            updates.append(Update(path, make_record(path, identifier, as_object(change["fields"], "fields"))))
        identifiers = {update.record.identifier for update in updates}
        if len(updates) != len(identifiers) or activate & release or not (updates or activate or release):
            raise CatalogError("request repeats records, overlaps activation/release, or contains no changes")
        reconciled = iso_day(as_text(raw["reconciled"], "reconciled")) if "reconciled" in raw else None
        # Persist the reviewed request in one canonical key order.
        payload = parse_json_object(json_line(raw, sort_keys=True))
        return cls(base, activate, release, tuple(updates), reconciled, payload)

    def approval(self) -> str:
        return digest(json_line(self.payload, sort_keys=True).encode())


def authority_topics(catalog: Catalog) -> frozenset[str]:
    authority = catalog.authority
    if authority is None:
        raise CatalogError("authority manifest lacks a reviewed request")
    expect_keys(authority, {"topics", "request", "approved_sha256"})
    request = Request.parse(as_object(authority["request"], "request"))
    if request.base != catalog.revision or request.approval() != authority["approved_sha256"]:
        raise CatalogError("authority manifest has an invalid review binding")
    topics = topic_set(authority["topics"], "topics")
    if any(topic(path) not in topics for path in catalog.documents):
        raise CatalogError("catalog contains a source outside its authority topics")
    return topics


def git_authority(root: Path, rev: str) -> tuple[Catalog | None, dict[str, bytes]]:
    files = read_blobs(root, rev, tree_files(root, rev, (AUTHORITY,)))
    if not files:
        return None, {}
    catalog = Catalog.from_files({path.removeprefix(f"{AUTHORITY}/"): data for path, data in files.items()})
    _ = authority_topics(catalog)
    return catalog, files


def view_files(catalog: Catalog) -> dict[str, bytes]:
    return {path: doc.render() for path, doc in sorted(catalog.documents.items())}


def check_owned(catalog: Catalog, views: Mapping[str, bytes]) -> None:
    topics = authority_topics(catalog)
    expected = view_files(catalog)
    actual = {path: content for path, content in views.items() if topic(path) in topics}
    if actual != expected:
        drift = sorted(path for path in expected.keys() | actual.keys() if expected.get(path) != actual.get(path))
        raise CatalogError(f"generated source drift: {', '.join(drift)}")


def resolved_catalog(root: Path, rev: str) -> Catalog:
    catalog = Catalog.from_legacy(root, rev)
    authority, _files = git_authority(root, rev)
    if authority is not None:
        check_owned(authority, view_files(catalog))
        catalog.documents.update(authority.documents)
    return catalog


def check_evidence(after: ManifestRow, before: ManifestRow | None, index: Index) -> None:
    if (after.klass not in RECORD_CLASSES or after.kind not in RECORD_KINDS or after.status not in RECORD_STATUSES
            or (after.legacy and after.number > LEGACY_LAST)):
        raise CatalogError(f"{after.identifier}: invalid evidence class, kind, status or legacy flag")
    if after.date is not None:
        _ = iso_day(after.date)
    if after.sha256 is not None and not SHA256.fullmatch(after.sha256):
        raise CatalogError(f"{after.identifier}: malformed artifact SHA-256")
    if after.note is not None and len(after.note) > NOTE_CAP:
        raise CatalogError(f"{after.identifier}: note exceeds {NOTE_CAP} characters")
    if after.status in {"retired", "missing"} and not (after.note or after.superseded_by):
        raise CatalogError(f"{after.identifier}: {after.status} needs a note or superseded_by")
    if after.superseded_by is not None and (
        after.status != "retired" or after.superseded_by == after.identifier
        or after.superseded_by not in index.entries or not after.superseded_by.startswith("M-")
    ):
        raise CatalogError(f"{after.identifier}: invalid evidence supersession")
    if after.kind != "rea-bundle" and (after.ev or after.unk or after.records is not None):
        raise CatalogError(f"{after.identifier}: member lists require a REA bundle")
    for members, pattern in ((after.ev, EVIDENCE_ID), (after.unk, REA_UNKNOWN_ID)):
        if (len(members) > LISTED_ID_LIMIT or len(set(members)) != len(members)
                or any(not pattern.fullmatch(member) for member in members)):
            raise CatalogError(f"{after.identifier}: malformed or duplicate REA bundle members")
    if not after.legacy:
        if (after.sha256 is None or after.date is None or len(after.label) > LABEL_CAP
                or after.path.endswith("/") or after.path == "unstated"
                or (after.kind == "rea-bundle" and after.records is None)):
            raise CatalogError(f"{after.identifier}: modern evidence requires date, digest, file and bundle count")
        if after.klass in {"local-private", "tracked"}:
            raw = after.path.partition("#")[0]
            locator = PurePosixPath(raw)
            if locator.is_absolute() or locator.as_posix() != raw or ".." in locator.parts:
                raise CatalogError(f"{after.identifier}: invalid relative artifact path")
            if after.klass == "local-private" and not raw.startswith(LOCAL_PREFIXES):
                raise CatalogError(f"{after.identifier}: invalid local-private artifact path")
    if before is None:
        if after.legacy:
            raise CatalogError(f"{after.identifier}: new rows cannot be legacy")
        return
    for key in ("klass", "date", "legacy", "ev", "unk", "records"):
        if getattr(before, key) != getattr(after, key):
            raise CatalogError(f"{after.identifier}: {key} is immutable")
    if before.kind != after.kind and not before.legacy:
        raise CatalogError(f"{after.identifier}: kind is immutable")
    for old, new, field in ((before.path, after.path, "path"), (before.sha256, after.sha256, "sha256")):
        if old != new and (after.note is None or str(old) not in after.note):
            raise CatalogError(f"{after.identifier}: keep the previous {field} in note")


def insert_record(lines: list[str | Record], record: Record, path: str) -> None:
    positions = [offset for offset, item in enumerate(lines) if isinstance(item, Record)]
    if record.kind == "M":
        number = int(record.identifier[2:])
        for offset in positions:
            item = lines[offset]
            if isinstance(item, Record) and int(item.identifier[2:]) > number:
                lines.insert(offset, record)
                return
    if positions:
        lines.insert(positions[-1] + 1, record)
    elif SHARD.fullmatch(path):
        lines.insert(0, record)
    else:
        raise CatalogError(f"{path}: no typed table in which to insert the record")


def validate_updates(before: Index, after: Index, updates: tuple[Update, ...]) -> None:
    for update in updates:
        record = update.record
        identifier = record.identifier
        old = before.entries.get(identifier)
        if record.kind == "M":
            check_evidence(
                record.evidence(update.path, 0),
                old.record.evidence(old.path, old.line) if old is not None else None,
                after,
            )
        elif record.kind == "F":
            status = as_text(record.fields["status"], "status")
            if not (status in {"active", "retired"} or status.startswith("retired: ") or RETIREMENT.fullmatch(status)
                    or (old is not None and status == old.record.fields["status"])):
                raise CatalogError(f"{identifier}: invalid finding status {status}")
            if record.fields["kind"] not in ("observed", "measured", "derived", "inferred", "reported", "unknown"):
                raise CatalogError(f"{identifier}: invalid finding kind")
            evidence = as_text(record.fields["evidence"], "evidence")
            if old is None and evidence.startswith(RECEIPT_CLASSES):
                raise CatalogError(f"{identifier}: record receipts in the evidence index")
        elif record.kind == "U":
            if record.fields["status"] not in ("open", "investigating", "resolved"):
                raise CatalogError(f"{identifier}: invalid unknown status")
            if record.fields["severity"] not in ("low", "medium", "high", "critical"):
                raise CatalogError(f"{identifier}: invalid unknown severity")
        added = set(after.links[identifier]) - set(before.links.get(identifier, ()))
        for link in added:
            if "/" in link.target:
                _ = after.selector(link.target)
            else:
                _ = after.get(link.target)
        if any(link.kind == "supersedes_to" for link in added):
            pending = [link.target for link in after.links[identifier] if link.kind == "supersedes_to"]
            visited: set[str] = set()
            while pending:
                target = pending.pop()
                if target == identifier:
                    raise CatalogError(f"{identifier}: supersession cycle")
                if target not in visited:
                    visited.add(target)
                    pending.extend(link.target for link in after.links.get(target, ()) if link.kind == "supersedes_to")


def apply_updates(documents: dict[str, Document], request: Request, topics: frozenset[str]) -> set[str]:
    if not request.updates:
        _ = Index(documents)
        return set()
    index = Index(documents)
    pending: dict[str, list[str | Record]] = {}
    for update in request.updates:
        path, record = update.path, update.record
        old = index.entries.get(record.identifier)
        if topic(path) not in topics or topic(path) in request.release:
            raise CatalogError(f"{path}: topic is outside requested authoring authority")
        if path not in documents and path not in pending:
            # New evidence shards must roll over in their own existing category directory.
            shard = SHARD.fullmatch(path)
            folder = str(PurePosixPath(path).parent)
            latest = max(
                (name for name in documents.keys() | pending.keys() if str(PurePosixPath(name).parent) == folder),
                default="",
            )
            if (shard is None or not latest
                    or int(PurePosixPath(path).stem) != int(PurePosixPath(latest).stem) + 1):
                raise CatalogError(f"{path}: only the next shard of an existing category can be created")
            pending[path] = [""]
        lines = pending.setdefault(path, list(documents[path].lines) if path in documents else [])
        if old is not None:
            if old.path == path:
                position = next(i for i, entry in enumerate(lines) if isinstance(entry, Record) and entry.identifier == record.identifier)
                if old.record.fields == record.fields:
                    raise CatalogError(f"{record.identifier}: update changes no fields")
                lines[position] = record
                continue
            if (record.kind != "F" or topic(old.path) != topic(path)
                    or "/retired/" not in path or "/retired/" in old.path
                    or record.fields["status"] == "active"):
                raise CatalogError(f"{record.identifier}: only a retired finding may move within its topic")
            former = pending.setdefault(old.path, list(documents[old.path].lines))
            former[:] = [item for item in former if not isinstance(item, Record) or item.identifier != record.identifier]
        insert_record(lines, record, path)

    for path, lines in pending.items():
        if not SHARD.fullmatch(path) and request.reconciled is not None:
            lines = [
                f"Last reconciled: {request.reconciled}" if isinstance(line, str) and RECONCILED.fullmatch(line)
                else line for line in lines
            ]
        document = Document(path, tuple(lines))
        rendered = document.render()
        if Document.parse(path, rendered) != document:
            raise CatalogError(f"{path}: an update cannot be represented in the legacy view")
        cap = SHARD_CAP if SHARD.fullmatch(path) else FILE_CAPS.get(
            path, next((size for prefix, size in PREFIX_CAPS if path.startswith(prefix)), DEFAULT_FILE_CAP)
        )
        if len(rendered) > cap:
            raise CatalogError(f"{path}: generated view exceeds {cap} bytes")
        if any(len(record.render().encode()) > (RECORD_CAP if SHARD.fullmatch(path) else LEDGER_ROW_CAP)
               for _, record in document.records()):
            raise CatalogError(f"{path}: generated row exceeds its byte cap")
        documents[path] = document
    validate_updates(index, Index(documents), request.updates)
    return set(pending)


@dataclass(frozen=True)
class Publication:
    before: dict[str, bytes]
    after: dict[str, bytes]
    authority: Catalog
    views: dict[str, bytes]


def prepare(root: Path, request: Request) -> Publication:
    base = Catalog.from_legacy(root, request.base)
    prior, authority_files = git_authority(root, request.base)
    active: frozenset[str] = frozenset()
    if prior is not None:
        check_owned(prior, view_files(base))
        active = authority_topics(prior)
    available = {topic(path) for path in base.documents}
    if request.activate & active or not request.activate <= available or not request.release <= active:
        raise CatalogError("activation or release does not match current topic authority")
    topics = (active | request.activate) - request.release
    original_views = view_files(base)
    touched = apply_updates(base.documents, request, topics)
    views = view_files(base)
    authority = Catalog(
        request.base, {path: doc for path, doc in base.documents.items() if topic(path) in topics},
        {"topics": sorted(topics), "request": request.payload, "approved_sha256": request.approval()},
    )
    current = {f"{AUTHORITY}/{path}": content for path, content in authority.files().items()}
    before = {**authority_files, **{path: original_views[path] for path in touched if path in original_views}}
    after = {**current, **{path: views[path] for path in touched}}
    return Publication(before, after, authority, views)


def patch_lines(path: str, before: bytes | None, after: bytes | None) -> Iterator[str]:
    yield f"diff --git a/{path} b/{path}\n"
    if before is None:
        yield "new file mode 100644\n"
    if after is None:
        yield "deleted file mode 100644\n"
    for line in difflib.unified_diff(
        git_lines(before), git_lines(after), fromfile=f"a/{path}" if before is not None else "/dev/null",
        tofile=f"b/{path}" if after is not None else "/dev/null",
    ):
        if line.endswith("\n"):
            yield line
        else:
            yield line + "\n\\ No newline at end of file\n"


def git_lines(content: bytes | None) -> list[str]:
    if content is None:
        return []
    *lines, tail = content.decode().split("\n")
    return [line + "\n" for line in lines] + ([tail] if tail else [])


def patch(publication: Publication) -> bytes:
    lines: list[str] = []
    for path in sorted(publication.before.keys() | publication.after.keys()):
        before = publication.before.get(path)
        after = publication.after.get(path)
        if before != after:
            lines.extend(patch_lines(path, before, after))
    return "".join(lines).encode()


def require_clean_sources(root: Path, base: str) -> None:
    if (commit_id(root, "HEAD") != base
            or git(root, "status", "--porcelain=v1", "--untracked-files=all", "--", *LEGACY_ROOTS, AUTHORITY)):
        raise CatalogError("stale publication base or modified catalog/legacy sources")


def check_checkout(root: Path) -> None:
    base = commit_id(root, "HEAD")
    committed = tree_files(root, base, (AUTHORITY,))
    paths = checkout_paths(root, AUTHORITY)
    if not paths and not committed:
        return
    current = {path: read_regular(root, path) for path in paths}
    catalog = Catalog.from_files({path.removeprefix(f"{AUTHORITY}/"): data for path, data in current.items()})
    topics = authority_topics(catalog)
    before = read_blobs(root, base, committed)
    transitioning = current != before
    views = {
        path: read_regular(root, path) for path in checkout_paths(root, *LEGACY_ROOTS)
        if topic(path) is not None and (transitioning or topic(path) in topics)
    }
    check_owned(catalog, views)
    if transitioning:
        authority = catalog.authority
        if authority is None:
            raise CatalogError("missing reviewed transition")
        request = Request.parse(as_object(authority["request"], "request"))
        if request.base != base:
            raise CatalogError("reviewed transition is stale for the current HEAD")
        expected = prepare(root, request)
        if current != {path: data for path, data in expected.after.items() if path.startswith(f"{AUTHORITY}/")}:
            raise CatalogError("catalog differs from replay of the reviewed request")
        if expected.views != views:
            raise CatalogError("legacy sources differ from replay of the reviewed request")
