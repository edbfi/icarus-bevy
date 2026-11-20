# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory protected-paths``: what scratch cleanup must never delete. It fails closed.

``protected_paths(start)`` is the interface for ``tools/work_gc.py`` and ``tools/cargo_guard.py``.
It reads every linked worktree of the repository containing ``start`` (``git worktree list``) and
takes the union of:

- every head revision of an evidence record in that worktree's journal that is ``active`` and
  ``local-private``;
- while the worktree still has an ``evidence/`` directory, every such row of its legacy shards,
  parsed by ``tools.docs_lint.parse_shard`` (imported only then).

For each such record it protects the path exactly as recorded (directories keep their trailing
``/``) and every existing file the record's sha256 manifest lists (``tools/sha256_manifest.py``
decides what is a manifest and where entries live; removal receipts list nothing to keep). Paths are
relative to the primary checkout, found from the Git common directory, never a linked worktree. This
reproduces ``tools/docs_lint.py --list-local`` for the legacy rows.

``ProtectedPathsError`` (unreadable or malformed journal, shard or manifest; Git failure) means the
list cannot be known: callers must treat every path as protected. A listed file that does not exist is
only a notice, since nothing absent can be deleted.
"""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from tools import sha256_manifest
from tools.memory.gitio import GitError, git
from tools.memory.journal import JournalError, read_tree
from tools.memory.state import replay


class ProtectedPathsError(RuntimeError):
    """The protected set cannot be known; protect everything."""


@dataclass(frozen=True)
class Protected:
    """Sorted paths relative to the primary checkout, and notices about listed files that are absent."""

    paths: tuple[str, ...]
    notices: tuple[str, ...]


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
    legacy = checkout / "evidence"
    if legacy.is_dir():
        try:
            from tools.docs_lint import parse_shard
        except ImportError as error:
            raise ProtectedPathsError(f"{legacy}: legacy shard parser unavailable: {error}") from error
        for shard in sorted(legacy.rglob("*.jsonl")):
            try:
                parsed, problems = parse_shard(str(shard), shard.read_text(encoding="utf-8"))
            except (OSError, UnicodeDecodeError) as error:
                raise ProtectedPathsError(f"{shard}: unreadable: {error}") from error
            if problems:
                raise ProtectedPathsError(problems[0].render())
            rows.update((row.identifier, row.path) for row in parsed if row.status == "active" and row.klass == "local-private")
    return rows


def protected_paths_for(primary: Path, checkouts: Sequence[Path]) -> Protected:
    """The union over ``checkouts``, with every path resolved against ``primary``."""
    rows: set[tuple[str, str]] = set()
    for checkout in checkouts:
        rows |= evidence_rows(checkout)
    root = sha256_manifest.normal(primary)
    paths: set[str] = set()
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
        if entries is None or identifier in sha256_manifest.REMOVAL_RECEIPTS:
            continue
        for entry in entries:
            present = [candidate for candidate in entry.candidates if candidate.exists()]
            if not present:
                notices.append(f"{identifier}: manifest entry missing: {entry.path}")
            paths.update(candidate.relative_to(root).as_posix() for candidate in present)
    return Protected(tuple(sorted(paths)), tuple(dict.fromkeys(notices)))


def protected_paths(start: Path) -> Protected:
    """Protected paths of the repository containing ``start``, over every worktree; raises ``ProtectedPathsError``."""
    primary = primary_checkout(start)
    checkouts, notices = worktrees(start)
    result = protected_paths_for(primary, checkouts)
    return Protected(result.paths, notices + result.notices)
