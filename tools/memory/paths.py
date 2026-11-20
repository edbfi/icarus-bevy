# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory protected-paths``: what scratch cleanup must never delete. It fails closed.

``protected_paths(start)`` is the interface for ``tools/work_gc.py``; ``tools/cargo_guard.py``, which
reads manifests under its own byte budget, takes ``evidence_rows`` and ``sources`` per checkout. It reads
every linked worktree of the repository containing ``start`` (``git worktree list``) and takes the union
of:

- every head revision of an evidence record in that worktree's journal that is ``active`` and
  ``local-private``;
- while the worktree still has an ``evidence/`` directory, every such row of its legacy shards, read by
  ``legacy_rows``, this module's own strict reader of the four keys protection needs (it does not depend
  on ``tools/docs_lint.py``, so old worktrees keep their branch-only rows protected after the lint is gone).

For each such record ``paths`` holds the path exactly as recorded (directories keep their trailing
``/``) and every existing file the record's sha256 manifest lists (``tools/sha256_manifest.py``
decides what is a manifest and where entries live; removal receipts list nothing to keep). Paths are
relative to the primary checkout, found from the Git common directory, never a linked worktree. This
reproduces ``tools/docs_lint.py --list-local`` for the legacy rows, and is what ``memory
protected-paths`` prints: what exists to keep and back up.

``listed`` adds the lexical pins the scratch guards also honour, as ``tools/work_gc.py`` did before it
read this module: every place an entry of such a manifest may live, whether or not a file is there now
and including removal receipts' entries. A file restored from a backup is protected the moment it
reappears, and a directory that should hold a listed file is never pruned. Guards protect
``paths`` and ``listed`` together.

``ProtectedPathsError`` (unreadable or malformed journal, shard or manifest; Git failure; no worktree
with a journal or a legacy index at all) means the list cannot be known: callers must treat every path
as protected. A listed file that does not exist is only a notice, since nothing absent can be deleted.
"""

from __future__ import annotations

import os
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from tools import sha256_manifest
from tools.memory.gitio import GitError, git
from tools.memory.journal import JOURNAL, JournalError, read_tree
from tools.memory.model import RecordError, as_mapping, load_json
from tools.memory.state import replay

LEGACY_INDEX = "evidence"
# The legacy lint capped shards at 64 KiB; a larger one is not a shard it accepted.
LEGACY_SHARD_LIMIT = 64 * 1024
LEGACY_KEYS = ("id", "class", "status", "path")
LEGACY_CLASSES = ("local-private", "tracked", "external")
LEGACY_STATUSES = ("active", "retired", "missing")


class ProtectedPathsError(RuntimeError):
    """The protected set cannot be known; protect everything."""


@dataclass(frozen=True)
class Protected:
    """Sorted paths relative to the primary checkout, and notices about listed files that are absent.

    ``listed``: every candidate location of every manifest entry, present or absent, removal receipts
    included (lexical pins for the guards; not printed by ``memory protected-paths``).
    """

    paths: tuple[str, ...]
    notices: tuple[str, ...]
    listed: tuple[str, ...] = ()


def primary_checkout(start: Path) -> Path:
    """The primary checkout: the parent of the Git common directory."""
    try:
        common = Path(git(start, "rev-parse", "--path-format=absolute", "--git-common-dir").decode().strip())
    except GitError as error:
        raise ProtectedPathsError(str(error)) from error
    if common.name != ".git":
        raise ProtectedPathsError(f"Git common directory {common} does not belong to a checkout")
    return common.parent


def worktrees(start: Path) -> tuple[tuple[Path, ...], tuple[str, ...]]:
    """Checkouts of every worktree, and notices for prunable ones (directory gone) that are skipped."""
    try:
        output = git(start, "worktree", "list", "--porcelain", "-z").decode()
    except GitError as error:
        raise ProtectedPathsError(str(error)) from error
    found: list[Path] = []
    notices: list[str] = []
    for block in output.split("\0\0"):
        fields = [item for item in block.split("\0") if item]
        if not fields:
            continue
        if not fields[0].startswith("worktree "):
            raise ProtectedPathsError(f"unexpected git worktree list output: {fields[0]!r}")
        path = Path(fields[0].removeprefix("worktree "))
        if "bare" in fields:
            continue
        if any(item.startswith("prunable") for item in fields):
            notices.append(f"skipped prunable worktree {path}")
            continue
        found.append(path)
    return tuple(found), tuple(notices)


def sources(checkout: Path) -> tuple[str, ...]:
    """Which of the journal and the legacy index ``checkout`` has. An empty answer from every worktree means
    nothing says what to keep; whether that is a refusal is the caller's rule."""
    return tuple(name for name in (JOURNAL, LEGACY_INDEX) if (checkout / name).exists() or (checkout / name).is_symlink())


def legacy_rows(name: str, text: str) -> list[tuple[str, str, str, str]]:
    """``(ID, class, status, path)`` of every line of one legacy ``evidence/`` shard, in order.

    Only the keys protection needs are read: ``id``, ``class``, ``status`` and ``path`` must be nonempty
    strings, with the class and status in the legacy vocabulary; other keys are ignored. A blank line,
    invalid JSON (duplicate keys, non-integer numbers), a non-object, a missing or empty key or an unknown
    class or status raises ``ProtectedPathsError``. Whether a row's file is a sha256 manifest, and what it
    lists, is read from the file itself (``tools/sha256_manifest.py``), as ``docs_lint.py --list-local`` did.
    """
    lines = text.split("\n")
    if lines and lines[-1] == "":
        _ = lines.pop()
    rows: list[tuple[str, str, str, str]] = []
    for number, line in enumerate(lines, start=1):
        where = f"{name}:{number}"
        if not line.strip():
            raise ProtectedPathsError(f"{where}: blank line in evidence shard")
        try:
            record = as_mapping(load_json(line), "record")
        except RecordError as error:
            raise ProtectedPathsError(f"{where}: invalid evidence record: {error}") from error
        missing = [key for key in LEGACY_KEYS if key not in record]
        if missing:
            raise ProtectedPathsError(f"{where}: invalid evidence record: missing keys {', '.join(missing)}")
        values: list[str] = []
        for key in LEGACY_KEYS:
            value = record[key]
            if not isinstance(value, str) or not value:
                raise ProtectedPathsError(f"{where}: invalid evidence record: {key} must be a nonempty string")
            values.append(value)
        identifier, klass, status, path = values
        if klass not in LEGACY_CLASSES or status not in LEGACY_STATUSES:
            raise ProtectedPathsError(f"{where}: {identifier}: unknown evidence class {klass!r} or status {status!r}")
        rows.append((identifier, klass, status, path))
    return rows


def legacy_shards(checkout: Path) -> tuple[Path, ...]:
    """Every ``*.jsonl`` file under the checkout's ``evidence/``, sorted; a symlink in it or an oversized shard is an error."""
    index = checkout / LEGACY_INDEX
    if not index.exists() and not index.is_symlink():
        return ()
    if index.is_symlink() or not index.is_dir():
        raise ProtectedPathsError(f"{index}: legacy evidence index is not a directory")
    shards: list[Path] = []
    try:
        for parent, dirs, files in os.walk(index, onerror=_raise):
            for entry in (*dirs, *files):
                if (Path(parent) / entry).is_symlink():
                    raise ProtectedPathsError(f"{Path(parent) / entry}: symlink in the legacy evidence index")
            for entry in files:
                shard = Path(parent) / entry
                if entry.endswith(".jsonl"):
                    if shard.stat().st_size > LEGACY_SHARD_LIMIT:
                        raise ProtectedPathsError(f"{shard}: exceeds the {LEGACY_SHARD_LIMIT}-byte legacy shard size")
                    shards.append(shard)
    except OSError as error:
        raise ProtectedPathsError(f"{index}: unreadable: {error}") from error
    return tuple(sorted(shards))


def _raise(error: OSError) -> None:
    raise error


def evidence_rows(checkout: Path) -> set[tuple[str, str]]:
    """``(ID, path)`` of every active local-private evidence record in one checkout's journal and legacy shards."""
    rows: set[tuple[str, str]] = set()
    try:
        state = replay(read_tree(checkout))
    except JournalError as error:
        raise ProtectedPathsError(f"{checkout}: {error}") from error
    for entry in state.entries.values():
        for sha in entry.heads:
            record = entry.revisions[sha].revision.record
            path = record.text("path")
            if record.type == "evidence" and record.status == "active" and record.text("class") == "local-private" and path:
                rows.add((record.id, path))
    for shard in legacy_shards(checkout):
        try:
            text = shard.read_bytes().decode("utf-8")
        except (OSError, UnicodeDecodeError) as error:
            raise ProtectedPathsError(f"{shard}: unreadable: {error}") from error
        for identifier, klass, status, path in legacy_rows(str(shard), text):
            if status == "active" and klass == "local-private":
                rows.add((identifier, path))
    return rows


def protected_paths_for(primary: Path, checkouts: Sequence[Path]) -> Protected:
    """The union over ``checkouts``, with every path resolved against ``primary``."""
    if not any(sources(checkout) for checkout in checkouts):
        raise ProtectedPathsError(f"no worktree has a journal ({JOURNAL}/) or a legacy index ({LEGACY_INDEX}/)")
    rows: set[tuple[str, str]] = set()
    for checkout in checkouts:
        rows |= evidence_rows(checkout)
    root = sha256_manifest.normal(primary)
    paths: set[str] = set()
    listed: set[str] = set()
    notices: list[str] = []
    manifests: dict[str, list[sha256_manifest.Entry] | None] = {}
    for identifier, path in sorted(rows):
        paths.add(path)
        if path not in manifests:
            try:
                manifests[path] = sha256_manifest.read_manifest(root, root / path)
            except sha256_manifest.ManifestError as error:
                raise ProtectedPathsError(f"{identifier}: unreadable sha256 manifest {path}: {error}") from error
        entries = manifests[path]
        if entries is None:
            continue
        listed.update(candidate.relative_to(root).as_posix() for entry in entries for candidate in entry.candidates)
        if identifier in sha256_manifest.REMOVAL_RECEIPTS:
            continue
        for entry in entries:
            present = [candidate for candidate in entry.candidates if candidate.exists()]
            if not present:
                notices.append(f"{identifier}: manifest entry missing: {entry.path}")
            paths.update(candidate.relative_to(root).as_posix() for candidate in present)
    return Protected(tuple(sorted(paths)), tuple(dict.fromkeys(notices)), tuple(sorted(listed)))


def protected_paths(start: Path) -> Protected:
    """Protected paths of the repository containing ``start``, over every worktree; raises ``ProtectedPathsError``."""
    primary = primary_checkout(start)
    checkouts, notices = worktrees(start)
    result = protected_paths_for(primary, checkouts)
    return Protected(result.paths, notices + result.notices, result.listed)
