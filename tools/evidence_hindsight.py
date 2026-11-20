# SPDX-License-Identifier: AGPL-3.0-or-later
"""Prepare, publish, and query reviewed catalog topics through local Hindsight."""

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from fcntl import LOCK_EX, LOCK_NB, flock
from os import O_CREAT, O_DIRECTORY, O_NOFOLLOW, O_RDONLY, O_RDWR, close, environ, fdopen, fsync, open as os_open
from pathlib import Path
from tempfile import NamedTemporaryFile
from typing import TypedDict

from tools.docs_lint import parse_json_object
from tools.evidence_catalog_model import (
    ID,
    CatalogError,
    Index,
    as_object,
    as_text,
    digest,
    expect_keys,
    json_line,
)
from tools.evidence_catalog_publish import resolved_catalog
from tools.evidence_catalog_store import AUTHORITY, LEGACY_ROOTS, commit_id, git, new_output
from tools.evidence_hindsight_client import Hindsight, HindsightTimeout
from tools.evidence_hindsight_projection import (
    MAX_ARCHIVE_BYTES,
    MAX_REVIEW_BYTES,
    PROHIBITED_TEXT,
    SELECTOR,
    Projection,
    Review,
    Row,
    checked_items,
    checked_topic,
    committed,
    compact,
    full_revision,
    load,
    read_bounded,
    require_ancestor,
    review_bytes,
    review_file,
    safe_row,
    source_digest,
)

HANDLE = re.compile(rf"(?<![\w-]){ID.pattern}(?:/[A-Za-z0-9_-]*)?(?![\w-])")
MAX_REQUEST = 4096
MAX_CANDIDATES = 16


def prepare(root: Path, topic: str, allowlist: Path, out: Path, rev: str | None) -> dict[str, object]:
    raw = parse_json_object(b'{"ids":' + read_bounded(allowlist, MAX_REVIEW_BYTES) + b"}")
    expect_keys(raw, {"ids"})
    ids = checked_items(raw["ids"])
    base = full_revision(root, rev) if rev is not None else commit_id(root, "HEAD")
    tree = base if rev is not None else git(root, "write-tree").decode().strip()
    if rev is None and commit_id(root, "HEAD") != base:
        raise CatalogError("HEAD changed while reading the staging area")
    review = Review(checked_topic(topic), base, rev is None, source_digest(root, tree), ids)
    projection = load(root, tree, review)
    archive = projection.archive()
    archive_digest = digest(archive)
    encoded = review_bytes(review, archive_digest)
    new_output(root, state_directory(root, out), {"projection.zip": archive, "review.json": encoded})
    return {
        "topic": review.topic, "base": base, "staged": review.staged, "bank": review.bank,
        "documents": len(ids), "archive_sha256": archive_digest, "review_sha256": digest(encoded),
    }


@dataclass(frozen=True)
class Publication:
    revision: str
    projection: Projection
    archive_sha256: str
    published_revision: str | None = None
    stale: bool = False


def state_directory(root: Path, directory: Path) -> Path:
    resolved = directory.resolve()
    common_git = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir").decode().strip())
    protected = [root / name for name in (*LEGACY_ROOTS, AUTHORITY, ".git", "icarus-orig",
                                         "icarus-client-orig", "reference-data")]
    canonical_work = common_git.parent / "work"
    if (
        resolved in (root, canonical_work)
        or not (resolved.is_relative_to(root) or resolved.is_relative_to(canonical_work))
        or any(resolved.is_relative_to(path.resolve()) for path in [*protected, common_git])
    ):
        raise CatalogError("choose a dedicated publication directory in the checkout or its canonical work directory")
    return resolved


def read_publication(root: Path, pointer: Path, topic: str) -> Publication:
    envelope = parse_json_object(read_bounded(pointer, MAX_REVIEW_BYTES))
    expect_keys(envelope, {"revision", "review", "archive_sha256"})
    review = Review.parse(as_object(envelope["review"], "active review"))
    if review.topic != topic:
        raise CatalogError("active topic does not match requested topic")
    revision = as_text(envelope["revision"], "source revision")
    projection = committed(root, revision, review)
    archive_digest = digest(projection.archive())
    if envelope["archive_sha256"] != archive_digest:
        raise CatalogError("active topic digest differs from the reviewed Git projection")
    return Publication(revision, projection, archive_digest)


def current_publication(root: Path, publication: Publication) -> Publication:
    head = commit_id(root, "HEAD")
    review = publication.projection.review
    if source_digest(root, head) == review.source_sha256:
        return Publication(head, publication.projection, publication.archive_sha256, publication.revision)
    index = Index(resolved_catalog(root, head).documents)
    rows: dict[str, Row] = {}
    for item in review.ids:
        if item not in index.entries:
            continue
        try:
            rows[item] = safe_row(index, item)
        except CatalogError:
            # A changed row may now contain values outside the transport-safe schema.
            continue
    return Publication(
        head, Projection(review, index, rows), publication.archive_sha256, publication.revision, stale=True,
    )


def write_pointer(pointer: Path, payload: bytes) -> None:
    with NamedTemporaryFile(dir=pointer.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            _ = stream.write(payload)
            stream.flush()
            fsync(stream.fileno())
            stream.close()
            _ = temporary.replace(pointer)
            directory_fd = os_open(pointer.parent, O_RDONLY | O_DIRECTORY)
            try:
                fsync(directory_fd)
            finally:
                close(directory_fd)
        finally:
            temporary.unlink(missing_ok=True)


def publish(
    root: Path, directory: Path, approved: str, revision: str, state: Path, url: str,
) -> dict[str, object]:
    encoded, review, archive_digest = review_file(directory)
    if digest(encoded) != approved:
        raise CatalogError("approval SHA-256 does not match this projection review")
    projection = committed(root, revision, review)
    archive = projection.archive()
    if digest(archive) != archive_digest or read_bounded(
        directory / "projection.zip", MAX_ARCHIVE_BYTES,
    ) != archive:
        raise CatalogError("archive differs from the transport-safe reviewed Git projection")
    directory = state_directory(root, state)
    directory.mkdir(mode=0o700, exist_ok=True)
    pointer = directory / f"{review.topic}.json"
    descriptor = os_open(directory / f".{review.topic}.lock", O_RDWR | O_CREAT | O_NOFOLLOW, 0o600)
    with fdopen(descriptor, "rb") as lock:
        try:
            flock(lock, LOCK_EX | LOCK_NB)
        except BlockingIOError:
            raise CatalogError("another publisher owns this topic") from None

        try:
            previous = read_publication(root, pointer, review.topic)
        except FileNotFoundError:
            previous = None
        if previous is not None:
            require_ancestor(root, previous.revision, revision, "active publication commit")

        client = Hindsight(url, review.bank)
        client.require_authenticated_service()
        counts = client.transfer(archive, len(projection.rows))
        client.verify_bank(projection)
        write_pointer(pointer, compact({
            "revision": revision, "review": review.payload(), "archive_sha256": archive_digest,
        }))
    return {
        "topic": review.topic, "revision": revision, "bank": review.bank,
        "archive_sha256": archive_digest, **counts,
    }


class Reference(TypedDict):
    handle: str
    status: str | None


class RowResult(TypedDict):
    item_id: str
    source_path: str
    source_line: int
    status: str
    fields: dict[str, str]
    references: list[Reference]


def row_result(projection: Projection, row: Row) -> RowResult:
    return {
        "item_id": row.id, "source_path": row.path, "source_line": row.line,
        "status": row.status, "fields": row.fields,
        "references": [
            {"handle": ref, "status": reference_status(projection, ref)} for ref in row.references
        ],
    }


def reference_status(projection: Projection, reference: str) -> str | None:
    target = projection.rows.get(reference.partition("/")[0])
    return target.status if target is not None else None


def exact_rows(projection: Projection, handles: Sequence[str]) -> tuple[list[str], list[str], list[RowResult]]:
    selected: dict[str, Row] = {}
    verified: list[str] = []
    unresolved: list[str] = []
    for handle in dict.fromkeys(handles):
        canonical = projection.canonical_handle(handle)
        if canonical is None:
            item, separator, _ = handle.partition("/")
            unresolved.append(f"{item}/<invalid-selector>" if separator and not SELECTOR.fullmatch(handle) else handle)
            continue
        verified.append(canonical)
        item = canonical.partition("/")[0]
        selected[item] = projection.rows[item]
    for row in tuple(selected.values()):
        for ref in row.references:
            target = projection.rows.get(ref.partition("/")[0])
            if target is not None and (
                target.id.startswith(("M-", "U-")) or row.status == f"superseded-by {target.id}"
            ):
                _ = selected.setdefault(target.id, target)
    return verified, unresolved, [row_result(projection, row) for row in selected.values()]


def query(
    publication: Publication, request: str, candidates: Sequence[str], url: str | None, draft: bool,
) -> dict[str, object]:
    if not request.strip() or len(request) > MAX_REQUEST:
        raise CatalogError("request is empty or exceeds its size limit")
    handles = list(dict.fromkeys(match[0] for match in HANDLE.finditer(request)))
    if len(handles) > MAX_CANDIDATES or len(candidates) > MAX_CANDIDATES or any(
        not HANDLE.fullmatch(candidate) for candidate in candidates
    ):
        raise CatalogError(f"expected at most {MAX_CANDIDATES} exact handles or candidate IDs")
    projection = publication.projection
    retrieval = "stale-exact-fallback" if publication.stale else "exact"
    generated = None
    retrieval_detail = None
    draft_status = "unavailable" if draft else "not-requested"
    hits = ()
    key = environ.get("HINDSIGHT_API_KEY")
    # Explicit handles always win. Only sanitized semantic questions leave the process.
    if (
        not publication.stale and url is not None and (not handles or draft)
        and not (PROHIBITED_TEXT.search(request) or (key and key in request))
    ):
        try:
            client = Hindsight(url, projection.review.bank)
            client.require_authenticated_service()
            if not handles:
                hits = client.recall(projection, request)[:MAX_CANDIDATES]
                retrieval = "verified-recall" if hits else "exact-fallback"
            if draft:
                if handles:
                    items = tuple(dict.fromkeys(
                        candidate.partition("/")[0] for handle in handles
                        if (candidate := projection.canonical_handle(handle)) is not None
                    ))
                    if items:
                        hits = client.recall(projection, request, items)
                if hits:
                    try:
                        text, cited = client.draft(request, hits)
                        allowed = set(cited)
                        for item in cited:
                            allowed.update(projection.rows[item].references)
                        if not PROHIBITED_TEXT.search(text.replace("\n", " ")) and {match[0] for match in HANDLE.finditer(text)} <= allowed:
                            generated = {"status": "draft", "text": text, "verified_source_items": cited}
                            draft_status = "available"
                    except HindsightTimeout:
                        draft_status = "timeout"
                    except ValueError:
                        pass  # Canonical verification remains independent of generated text.
        except HindsightTimeout:
            retrieval = "exact-fallback"
            retrieval_detail = "timeout"
        except ValueError:
            retrieval = "exact-fallback"
    if hits and not handles:
        handles = [hit.row.id for hit in hits[:MAX_CANDIDATES]]
    verified, unresolved, rows = exact_rows(projection, handles or candidates)
    return {
        "topic": projection.review.topic, "revision": publication.revision,
        "bank": projection.review.bank, "retrieval": retrieval, "retrieval_detail": retrieval_detail,
        "published_revision": publication.published_revision or publication.revision, "stale": publication.stale,
        "verified_handles": verified, "unresolved_handles": unresolved, "rows": rows,
        "draft": generated, "draft_status": draft_status,
    }


class Options(argparse.Namespace):
    repo: Path = Path(__file__).resolve().parents[1]
    command: str = ""
    topic: str = ""
    allowlist: Path = Path()
    out: Path = Path()
    rev: str | None = None
    prepared: Path = Path()
    approved_sha256: str = ""
    state: Path = Path()
    url: str | None = None
    request: str = ""
    candidate: list[str] | None = None
    draft: bool = False
    as_of_published: bool = False


def options(argv: Sequence[str] | None) -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--repo", type=Path)
    commands = parser.add_subparsers(dest="command", required=True)
    preparing = commands.add_parser("prepare", help="prepare a reviewable ZIP from an exact Git snapshot")
    _ = preparing.add_argument("--topic", required=True)
    _ = preparing.add_argument("--allowlist", type=Path, required=True)
    _ = preparing.add_argument("--out", type=Path, required=True)
    source = preparing.add_mutually_exclusive_group(required=True)
    _ = source.add_argument("--rev")
    _ = source.add_argument("--staged", action="store_true")
    publishing = commands.add_parser("publish", help="verify, import and activate a reviewed committed topic")
    _ = publishing.add_argument("--prepared", type=Path, required=True)
    _ = publishing.add_argument("--approved-sha256", required=True)
    _ = publishing.add_argument("--rev", required=True)
    checking = commands.add_parser("check", help="verify every active topic document against Git")
    querying = commands.add_parser("query", help="return verified rows for explicit handles or candidate recall")
    for action in (publishing, checking, querying):
        _ = action.add_argument("--state", type=Path, required=True)
        _ = action.add_argument("--url", required=action is not querying)
        if action is not publishing:
            _ = action.add_argument("--topic", required=True)
            _ = action.add_argument("--as-of-published", action="store_true")
    _ = querying.add_argument("--candidate", action="append")
    _ = querying.add_argument("--draft", action="store_true")
    _ = querying.add_argument("request")
    return parser.parse_args(argv, namespace=Options())


def run(args: Options) -> dict[str, object]:
    root = args.repo.resolve()
    if args.command == "prepare":
        return prepare(root, args.topic, args.allowlist, args.out, args.rev)
    if args.command == "publish" and args.rev is not None and args.url is not None:
        return publish(root, args.prepared, args.approved_sha256, args.rev, args.state, args.url)
    topic = checked_topic(args.topic)
    pointer = state_directory(root, args.state) / f"{topic}.json"
    publication = read_publication(root, pointer, topic)
    if not args.as_of_published:
        publication = current_publication(root, publication)
    if args.command == "check" and args.url is not None:
        if publication.stale:
            raise CatalogError("active projection is stale for the current Git catalog snapshot")
        client = Hindsight(args.url, publication.projection.review.bank)
        client.require_authenticated_service()
        client.verify_bank(publication.projection)
        return {
            "topic": topic, "revision": publication.revision,
            "published_revision": publication.published_revision or publication.revision,
            "bank": publication.projection.review.bank, "documents": len(publication.projection.rows),
            "archive_sha256": publication.archive_sha256,
        }
    return query(publication, args.request, args.candidate or (), args.url, args.draft)


def main(argv: Sequence[str] | None = None) -> int:
    try:
        print(json_line(run(options(argv)), sort_keys=True))
    except (OSError, ValueError) as error:
        print(f"evidence hindsight: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
