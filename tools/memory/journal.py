# SPDX-License-Identifier: AGPL-3.0-or-later
"""Journal segments: the machine-written record revisions under ``memory/journal/``.

A segment holds one change set, or one slice of a change set larger than ``SEGMENT_LIMIT`` bytes:

    memory/journal/YYYY/MM/<YYYYMMDDTHHMMSSZ>-<first 12 hex of the file's sha256>.jsonl

Line 1 is the header ``{"created":"<UTC timestamp>","format":"icarus-memory-journal","revisions":N,"version":1}``;
each further line is one revision ``{"body"?:...,"prev":[...],"record":{...},"rev":"...","sha256":"..."}``.
Every line is canonical JSON ending in LF. The name binds the bytes, so concurrent branches never collide
and a half-written or edited segment no longer matches its name.

A revision has two digests. ``sha256`` is the record's content digest (what the bank holds and read-back
verifies). ``rev`` is the revision's identity, the sha256 of the canonical JSON ``{"prev": [...], "record":
{...}}``; ``prev`` names earlier revisions by ``rev``. Returning a record to earlier content therefore makes
a new revision with the same ``sha256`` and a different ``rev``.

Readers are strict: anything that is not exactly this format raises ``JournalError`` naming the path
(and line). The only file they skip is the writer's temporary file, ``.<name>.<pid>.tmp``, which an
interrupted write leaves behind and which never holds an accepted revision.
"""

from __future__ import annotations

import os
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from datetime import UTC, datetime
from functools import cached_property
from pathlib import Path, PurePosixPath
import re

from tools.memory.canonical import canonical_json, record_sha256, render_text
from tools.memory.gitio import read_blobs, tree_files
from tools.memory.model import JSON, SHA256, Record, RecordError, as_mapping, is_array, load_json, sha256_hex

JOURNAL = "memory/journal"
SEGMENT_LIMIT = 256 * 1024
FORMAT = "icarus-memory-journal"
VERSION = 1
NAME = re.compile(r"(([0-9]{4})([0-9]{2})[0-9]{2}T[0-9]{6}Z)-([0-9a-f]{12})\.jsonl")
TEMPORARY = re.compile(r"\..+\.jsonl\.[0-9]+\.tmp")


class JournalError(ValueError):
    """A segment or revision is malformed, or cannot be written."""


@dataclass(frozen=True)
class Revision:
    """A record revision: its content, the content digest ``sha256``, and the revisions it follows.

    ``prev`` holds revision identities (``rev``), sorted; it is empty for a record's first revision and
    names two or more heads when the revision reconciles a fork. ``rev`` covers ``prev`` and the record.
    """

    record: Record
    sha256: str
    prev: tuple[str, ...]

    @classmethod
    def of(cls, record: Record, prev: Sequence[str] = ()) -> Revision:
        """A revision of ``record`` after ``prev``; refuses records without an admissible canonical form or text."""
        _ = render_text(record)
        if len(set(prev)) != len(prev) or not all(SHA256.fullmatch(item) for item in prev):
            raise JournalError(f"{record.id}: prev must be distinct revision identities (sha256 digests)")
        return cls(record, record_sha256(record), tuple(sorted(prev)))

    @cached_property
    def rev(self) -> str:
        """The revision identity: sha256 of the canonical JSON of ``{"prev": [...], "record": {...}}``."""
        return sha256_hex(canonical_json({"prev": list(self.prev), "record": self.record.to_json()}))

    def to_json(self) -> dict[str, JSON]:
        result: dict[str, JSON] = {"prev": list(self.prev), "record": self.record.to_json(), "rev": self.rev, "sha256": self.sha256}
        if self.record.body is not None:
            result["body"] = self.record.body
        return result

    def line(self) -> bytes:
        return canonical_json(self.to_json()) + b"\n"

    @classmethod
    def from_json(cls, data: object) -> Revision:
        raw = as_mapping(data, "revision")
        if not {"prev", "record", "rev", "sha256"} <= raw.keys() <= {"prev", "record", "rev", "sha256", "body"}:
            raise JournalError(f"revision keys must be prev, record, rev, sha256 and an optional body, not {sorted(raw)}")
        body = raw.get("body")
        if body is not None and not isinstance(body, str):
            raise JournalError("revision body must be a string")
        record = Record.from_json(raw["record"], body)
        prev = raw["prev"]
        if not is_array(prev) or not all(isinstance(item, str) for item in prev):
            raise JournalError(f"{record.id}: prev must be an array of sha256 digests")
        revision = cls.of(record, [str(item) for item in prev])
        if raw["sha256"] != revision.sha256:
            raise JournalError(f"{record.id}: sha256 {raw['sha256']!r} does not match the record ({revision.sha256})")
        if raw["rev"] != revision.rev:
            raise JournalError(f"{record.id}: rev {raw['rev']!r} does not match prev and record ({revision.rev})")
        return revision


@dataclass(frozen=True)
class Segment:
    """One segment file: its repository-relative POSIX path, header timestamp and revisions in file order."""

    path: str
    created: str
    revisions: tuple[Revision, ...]

    @property
    def name(self) -> str:
        return PurePosixPath(self.path).name


def _stamp(created: datetime) -> tuple[str, str]:
    if created.utcoffset() is None:
        raise JournalError("segment time must be timezone-aware")
    utc = created.astimezone(UTC)
    return utc.strftime("%Y-%m-%dT%H:%M:%SZ"), utc.strftime("%Y%m%dT%H%M%SZ")


def _header(created: datetime, count: int) -> bytes:
    timestamp, _ = _stamp(created)
    return canonical_json({"created": timestamp, "format": FORMAT, "revisions": count, "version": VERSION}) + b"\n"


def _assemble(created: datetime, lines: Sequence[bytes]) -> tuple[str, bytes]:
    if not lines:
        raise JournalError("a segment holds at least one revision")
    data = _header(created, len(lines)) + b"".join(lines)
    if len(data) > SEGMENT_LIMIT:
        raise JournalError(f"segment of {len(lines)} revisions is {len(data)} bytes; the limit is {SEGMENT_LIMIT}")
    _, stamp = _stamp(created)
    return f"{JOURNAL}/{stamp[:4]}/{stamp[4:6]}/{stamp}-{sha256_hex(data)[:12]}.jsonl", data


def encode_segment(created: datetime, revisions: Sequence[Revision]) -> tuple[str, bytes]:
    """The repository-relative path and bytes of one segment; ``JournalError`` past ``SEGMENT_LIMIT``."""
    return _assemble(created, [revision.line() for revision in revisions])


def split_change_set(created: datetime, revisions: Sequence[Revision]) -> list[tuple[str, bytes]]:
    """A change set as segments of at most ``SEGMENT_LIMIT`` bytes each, filled in order.

    Deterministic: the same ``created`` and revisions always give the same paths and bytes, so a
    migration that passes an explicit timestamp can regenerate its segments exactly.
    """
    segments: list[tuple[str, bytes]] = []
    current: list[bytes] = []
    size = 0
    for revision in revisions:
        line = revision.line()
        if current and len(_header(created, len(current) + 1)) + size + len(line) > SEGMENT_LIMIT:
            segments.append(_assemble(created, current))
            current, size = [], 0
        current.append(line)
        size += len(line)
    if current:
        segments.append(_assemble(created, current))
    return segments


def write_change_set(root: Path, created: datetime, revisions: Sequence[Revision]) -> tuple[Path, ...]:
    """Write a change set under ``root`` durably (temp file, fsync, rename, fsync directory); returns the paths.

    ``created`` is the explicit segment timestamp: the same ``created`` and revisions always give the same
    file names and bytes (a migration passes a fixed one; an interactive write passes the current UTC time).
    Writing a segment that already exists with the same bytes is a no-op, so a retried write is safe.
    """
    written: list[Path] = []
    for relative, data in split_change_set(created, revisions):
        target = root / relative
        _make_directories(root, target.parent)
        if target.exists():
            if target.read_bytes() != data:
                raise JournalError(f"{relative}: exists with different bytes")
            written.append(target)
            continue
        temporary = target.parent / f".{target.name}.{os.getpid()}.tmp"
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o644)
        try:
            with os.fdopen(descriptor, "wb") as handle:
                _ = handle.write(data)
                handle.flush()
                os.fsync(handle.fileno())
            os.rename(temporary, target)
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
        _fsync_directory(target.parent)
        written.append(target)
    return tuple(written)


def _make_directories(root: Path, directory: Path) -> None:
    missing: list[Path] = []
    while not directory.exists():
        missing.append(directory)
        directory = directory.parent
    for path in reversed(missing):
        path.mkdir()
        _fsync_directory(path.parent)
    if not (root / JOURNAL).is_dir() or (root / JOURNAL).is_symlink():
        raise JournalError(f"{JOURNAL} is not a directory")


def _fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def parse_segment(path: str, data: bytes) -> Segment:
    """Parse and verify one segment found at repository-relative ``path``."""
    name = PurePosixPath(path).name
    match = NAME.fullmatch(name)
    if match is None or path != f"{JOURNAL}/{match[2]}/{match[3]}/{name}":
        raise JournalError(f"{path}: not a segment path ({JOURNAL}/YYYY/MM/<YYYYMMDDTHHMMSSZ>-<12 hex>.jsonl)")
    if len(data) > SEGMENT_LIMIT:
        raise JournalError(f"{path}: {len(data)} bytes exceeds the {SEGMENT_LIMIT}-byte segment limit")
    if sha256_hex(data)[:12] != match[4]:
        raise JournalError(f"{path}: content does not match its name (half-written or edited segment)")
    try:
        lines = data.decode("utf-8").split("\n")
    except UnicodeDecodeError as error:
        raise JournalError(f"{path}: not UTF-8: {error}") from error
    if lines[-1] != "" or len(lines) < 3:
        raise JournalError(f"{path}: a segment is a header and at least one revision, each ending in LF")
    del lines[-1]
    try:
        header = as_mapping(load_json(lines[0]), "header")
    except RecordError as error:
        raise JournalError(f"{path}:1: {error}") from error
    created = header.get("created")
    if set(header) != {"created", "format", "revisions", "version"} or header.get("format") != FORMAT or header.get("version") != VERSION:
        raise JournalError(f"{path}:1: not a version {VERSION} {FORMAT} header")
    if not isinstance(created, str) or created.replace("-", "").replace(":", "") != match[1]:
        raise JournalError(f"{path}:1: created {created!r} does not match the name")
    if header.get("revisions") != len(lines) - 1:
        raise JournalError(f"{path}:1: header counts {header.get('revisions')!r} revisions; the file holds {len(lines) - 1}")
    revisions: list[Revision] = []
    for number, line in enumerate(lines[1:], start=2):
        try:
            revision = Revision.from_json(load_json(line))
        except (RecordError, JournalError) as error:
            raise JournalError(f"{path}:{number}: {error}") from error
        if revision.line() != line.encode() + b"\n":
            raise JournalError(f"{path}:{number}: {revision.record.id}: line is not canonical JSON")
        revisions.append(revision)
    expected: JSON = {"created": created, "format": FORMAT, "revisions": len(revisions), "version": VERSION}
    if canonical_json(expected) != lines[0].encode():
        raise JournalError(f"{path}:1: header is not canonical JSON")
    return Segment(path, created, tuple(revisions))


def read_tree(root: Path, parse: Callable[[str, bytes], Segment] = parse_segment) -> tuple[Segment, ...]:
    """Every segment in the working tree at ``root``, sorted by path; ``()`` when there is no journal.

    ``parse`` turns a segment's repository-relative path and bytes into a ``Segment``; a caller reading many
    worktrees passes one that reuses segments it has already parsed."""
    base = root / JOURNAL
    if not base.exists() and not base.is_symlink():
        return ()
    if base.is_symlink() or not base.is_dir():
        raise JournalError(f"{JOURNAL}: not a directory")
    segments: list[Segment] = []
    for path in sorted(base.rglob("*")):
        relative = path.relative_to(root).as_posix()
        if path.is_symlink():
            raise JournalError(f"{relative}: symlinks are not allowed in the journal")
        if path.is_dir() or TEMPORARY.fullmatch(path.name):
            continue
        try:
            data = path.read_bytes()
        except OSError as error:
            raise JournalError(f"{relative}: unreadable: {error}") from error
        segments.append(parse(relative, data))
    return tuple(segments)


def read_git(root: Path, treeish: str) -> tuple[Segment, ...]:
    """Every segment in Git ``treeish`` (a commit, branch or tree), read without a checkout; sorted by path."""
    files = tree_files(root, treeish, JOURNAL)
    paths = sorted(files)
    blobs = read_blobs(root, [files[path] for path in paths])
    segments: list[Segment] = []
    for path, data in zip(paths, blobs, strict=True):
        if data is None:
            raise JournalError(f"{treeish}:{path}: blob missing from the object store")
        segments.append(parse_segment(path, data))
    return tuple(segments)
