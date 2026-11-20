# SPDX-License-Identifier: AGPL-3.0-or-later
"""Build and verify a transport-safe projection of an exact Git catalog snapshot."""

from __future__ import annotations

import re
from dataclasses import dataclass
from io import BytesIO
from pathlib import Path
from typing import Literal, TypedDict
from zipfile import ZIP_STORED, ZipFile, ZipInfo

from tools.docs_lint import RECORD_KINDS, RECORD_STATUSES, parse_json_object
from tools.evidence_catalog_model import (
    COLUMNS,
    ID,
    CatalogError,
    Index,
    as_object,
    as_text,
    digest,
    expect_keys,
    json_line,
    text_list,
)
from tools.evidence_catalog_publish import iso_day, resolved_catalog
from tools.evidence_catalog_store import AUTHORITY, COMMIT, LEGACY_ROOTS, SHA256, commit_id, git

MAX_ITEMS = 128
MAX_REVIEW_BYTES = 16_384
MAX_ARCHIVE_BYTES = 2_000_000
TOPIC = re.compile(r"[a-z][a-z0-9]*(?:-[a-z0-9]+)*")
SELECTOR = re.compile(rf"{ID.pattern}/(?:ev|unk)_[0-9a-f]{{12}}")
PROHIBITED_TEXT = re.compile(
    r"(?i)(?:[a-z][a-z0-9+.-]*://|(?:^|[^\w-])(?:work|icarus-orig|icarus-client-orig|"
    + r"reference-data|local-private)[/\\]|(?:^|[\s(\x60])(?:~[/\\]|/(?:home|tmp|mnt|var|root|etc)/|"
    + r"[a-z]:[\\/])|\b(?:password|token|secret|api[-_ ]?key)\s*[:=]\s*(?!empty\b)\S|"
    + r"\b[0-9a-f]{64}\b|[\x00-\x1f\x7f])"
)
BANK_PREFIX = "icarus-catalog-"


def compact(value: object) -> bytes:
    return (json_line(value, sort_keys=True) + "\n").encode()


def checked_topic(value: str) -> str:
    if len(value) > 40 or not TOPIC.fullmatch(value):
        raise CatalogError("expected a lower-case topic slug of at most 40 characters")
    return value


def checked_items(raw: object) -> tuple[str, ...]:
    items = text_list(raw, "allowlist")
    if (
        not 0 < len(items) <= MAX_ITEMS
        or len(set(items)) != len(items)
        or any(not ID.fullmatch(item) or len(item) > 16 for item in items)
    ):
        raise CatalogError(f"allowlist must contain 1 to {MAX_ITEMS} distinct exact catalog IDs")
    return tuple(sorted(items))


def read_bounded(path: Path, limit: int) -> bytes:
    with path.open("rb") as stream:
        data = stream.read(limit + 1)
    if len(data) > limit:
        raise CatalogError("local projection input exceeds its size limit")
    return data


def source_digest(root: Path, tree: str) -> str:
    return digest(git(root, "ls-tree", "-r", "-z", tree, "--", *LEGACY_ROOTS, AUTHORITY))


def full_revision(root: Path, value: str) -> str:
    if not COMMIT.fullmatch(value) or commit_id(root, value) != value:
        raise CatalogError("expected a full Git commit ID")
    return value


@dataclass(frozen=True)
class Review:
    topic: str
    base: str
    staged: bool
    source_sha256: str
    ids: tuple[str, ...]

    @property
    def bank(self) -> str:
        # The review schema versions the wire format as well as its source binding.
        return f"{BANK_PREFIX}{digest(compact(self.payload()))}"

    def payload(self) -> dict[str, object]:
        return {
            "schema": 1, "topic": self.topic, "base": self.base, "staged": self.staged,
            "source_sha256": self.source_sha256, "ids": self.ids,
        }

    @classmethod
    def parse(cls, raw: dict[str, object]) -> Review:
        expect_keys(raw, {"schema", "topic", "base", "staged", "source_sha256", "ids"})
        base = as_text(raw["base"], "base")
        stamp = as_text(raw["source_sha256"], "source_sha256")
        staged = raw["staged"]
        if type(raw["schema"]) is not int or raw["schema"] != 1 or not isinstance(staged, bool):
            raise CatalogError("invalid review schema or source mode")
        if not COMMIT.fullmatch(base) or not SHA256.fullmatch(stamp):
            raise CatalogError("invalid review source identity")
        return cls(checked_topic(as_text(raw["topic"], "topic")), base, staged, stamp, checked_items(raw["ids"]))


@dataclass(frozen=True)
class Row:
    id: str
    path: str
    line: int
    status: str
    fields: dict[str, str]
    references: tuple[str, ...]

    @property
    def text(self) -> str:
        return "\n".join((f"{self.id} ({self.status})", *(
            f"{key}: {value}" for key, value in self.fields.items() if value
        )))


class Chunk(TypedDict):
    chunk_index: int
    chunk_text: str


class Fact(TypedDict):
    text: str
    fact_type: Literal["world"]
    chunk_index: int
    context: str
    metadata: dict[str, str]
    tags: list[str]
    mentioned_at: str


class TransferDocument(TypedDict):
    id: str
    original_text: str
    tags: list[str]
    chunks: list[Chunk]
    facts: list[Fact]


@dataclass(frozen=True)
class Projection:
    review: Review
    index: Index
    rows: dict[str, Row]

    def document(self, row: Row) -> TransferDocument:
        tags = [f"id:{row.id}", f"type:{row.id[0]}"]
        fact: Fact = {
            "text": row.text,
            "fact_type": "world",
            "chunk_index": 0,
            "context": f"ICARUS evidence catalog / {self.review.topic}",
            "metadata": {
                "item_id": row.id, "source_snapshot_sha256": self.review.source_sha256,
                "source_path": row.path, "source_line": str(row.line),
                "ledger_status": row.status, "item_references": ", ".join(row.references),
            },
            "tags": tags,
            # Use a fixed mention date for undated rows so import time cannot change it.
            "mentioned_at": row.fields.get("date", "1970-01-01") + "T00:00:00Z",
        }
        return {
            "id": row.id, "original_text": row.text, "tags": tags,
            "chunks": [{"chunk_index": 0, "chunk_text": row.text}], "facts": [fact],
        }

    def archive(self) -> bytes:
        stream = BytesIO()
        with ZipFile(stream, "w", compression=ZIP_STORED) as archive:
            archive.writestr(ZipInfo("manifest.json"), compact({
                "schema_version": 1, "source_bank_id": self.review.bank,
            }))
            for row in self.rows.values():
                archive.writestr(ZipInfo(f"documents/{row.id}.json"), compact(self.document(row)))
        payload = stream.getvalue()
        if len(payload) > MAX_ARCHIVE_BYTES:
            raise CatalogError("projection exceeds archive size limit")
        return payload

    def canonical_handle(self, handle: str) -> str | None:
        item, slash, _member = handle.partition("/")
        if item not in self.rows:
            return None
        if not slash:
            return item
        if SELECTOR.fullmatch(handle) and item.startswith("M-") and any(
            handle in row.references for row in self.rows.values()
        ):
            result = self.index.selector(handle)
            return as_text(result["selector"], "canonical selector")
        return None


def safe_row(index: Index, item: str) -> Row:
    entry = index.get(item)
    record = entry.record
    if record.kind == "M":
        evidence = record.evidence(entry.path, entry.line)
        if evidence.kind not in RECORD_KINDS or evidence.status not in RECORD_STATUSES:
            raise CatalogError(f"{item}: evidence disposition or kind is not approved for projection")
        fields = {"kind": evidence.kind}
        if evidence.date is not None:
            fields["date"] = iso_day(evidence.date)
        return Row(item, entry.path, entry.line, evidence.status, fields, ())

    fields = {
        key: as_text(record.fields[key], key)
        for key in COLUMNS[record.kind] if key not in {"status", "rea_id"}
    }
    status = as_text(record.fields.get("status", "tracked"), "status")
    if any(PROHIBITED_TEXT.search(value) for value in (*fields.values(), status)):
        raise CatalogError(f"{item}: potential private material in ledger text")
    targets: set[str] = set()
    for link in index.links[item]:
        if link.field == "rea_id":
            continue
        if SELECTOR.fullmatch(link.target):
            try:
                result = index.selector(link.target)
            except CatalogError:
                continue
            targets.add(as_text(result["selector"], "canonical selector"))
        elif ID.fullmatch(link.target):
            targets.add(link.target)
    return Row(item, entry.path, entry.line, status, fields, tuple(sorted(targets)))


def load(root: Path, tree: str, review: Review) -> Projection:
    if source_digest(root, tree) != review.source_sha256:
        raise CatalogError("Git catalog and source views differ from the reviewed snapshot")
    index = Index(resolved_catalog(root, tree).documents)
    return Projection(review, index, {item: safe_row(index, item) for item in review.ids})


def committed(root: Path, revision: str, review: Review) -> Projection:
    rev = full_revision(root, revision)
    if review.staged:
        _ = git(root, "merge-base", "--is-ancestor", full_revision(root, review.base), rev)
    elif rev != review.base:
        raise CatalogError("publication commit differs from the reviewed commit")
    return load(root, rev, review)


def review_file(directory: Path) -> tuple[bytes, Review, str]:
    encoded = read_bounded(directory / "review.json", MAX_REVIEW_BYTES)
    envelope = parse_json_object(encoded)
    expect_keys(envelope, {"review", "archive_sha256"})
    review = Review.parse(as_object(envelope["review"], "review"))
    archive_digest = as_text(envelope["archive_sha256"], "archive_sha256")
    if not SHA256.fullmatch(archive_digest) or encoded != review_bytes(review, archive_digest):
        raise CatalogError("review file differs from canonical encoding")
    return encoded, review, archive_digest


def review_bytes(review: Review, archive_digest: str) -> bytes:
    return compact({"review": review.payload(), "archive_sha256": archive_digest})
