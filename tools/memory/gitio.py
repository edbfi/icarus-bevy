# SPDX-License-Identifier: AGPL-3.0-or-later
"""The Git calls the memory tool shares: plumbing, and the one ``fetch`` of ``memory sync``; never a checkout.

Every failure is ``GitError`` with Git's own message (a ``timeout`` that expires included); callers decide
whether that fails closed.
"""

from __future__ import annotations

import subprocess
from collections.abc import Sequence
from pathlib import Path

# Seconds a read-only history query of ``get`` may take before it answers "unknown".
READ_TIMEOUT = 30.0


class GitError(RuntimeError):
    """A Git command failed or answered in an unexpected form."""


def git(root: Path, *args: str, stdin: bytes | None = None, timeout: float | None = None) -> bytes:
    """Standard output of a Git command that must exit 0."""
    return run_git(root, *args, stdin=stdin, timeout=timeout)[1]


def run_git(root: Path, *args: str, stdin: bytes | None = None, ok: tuple[int, ...] = (0,), timeout: float | None = None) -> tuple[int, bytes]:
    """Exit status and standard output of a Git command; a status outside ``ok`` raises ``GitError`` naming the
    subcommand (``git grep`` exits 1 when nothing matches, which a caller passes as ``ok=(0, 1)``), and so
    does a ``timeout`` (seconds) that expires or a Git that cannot be run."""
    try:
        result = subprocess.run(["git", "--no-replace-objects", "-C", str(root), *args], input=stdin, capture_output=True, check=False, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise GitError(f"git {_subcommand(args)}: no answer within {timeout:g} s") from None
    except OSError as error:
        raise GitError(f"git {_subcommand(args)}: {error.strerror or error}") from None
    if result.returncode not in ok:
        raise GitError(f"git {_subcommand(args)}: exit status {result.returncode}: {result.stderr.decode(errors='replace').strip()}")
    return result.returncode, result.stdout


def _subcommand(args: Sequence[str]) -> str:
    """The Git subcommand in ``args``, past global options such as ``-c name=value``."""
    skip = False
    for item in args:
        if skip:
            skip = False
        elif item in ("-c", "-C"):
            skip = True
        elif not item.startswith("-"):
            return item
    return args[0] if args else "git"


def tree_files(root: Path, treeish: str, prefix: str) -> dict[str, str]:
    """Regular files under ``prefix`` in ``treeish``: path -> blob ID. Anything else under it is an error."""
    files: dict[str, str] = {}
    for item in git(root, "ls-tree", "-r", "-z", "--end-of-options", treeish, "--", prefix).decode().split("\0"):
        if not item:
            continue
        metadata, path = item.split("\t", 1)
        mode, kind, oid = metadata.split()
        if kind != "blob" or mode not in ("100644", "100755"):
            raise GitError(f"{treeish}:{path}: not a regular file (mode {mode})")
        files[path] = oid
    return files


def read_blobs(root: Path, objects: Sequence[str]) -> list[bytes | None]:
    """Contents of each object name (``<oid>`` or ``<treeish>:<path>``) in order; ``None`` when absent."""
    if not objects:
        return []
    if any("\n" in name for name in objects):
        raise GitError("object names cannot contain a newline")
    output = git(root, "cat-file", "--batch", stdin="".join(f"{name}\n" for name in objects).encode())
    results: list[bytes | None] = []
    offset = 0
    for _name in objects:
        end = output.index(b"\n", offset)
        header = output[offset:end].split()
        offset = end + 1
        if len(header) != 3 or not header[2].isdigit():
            results.append(None)
            continue
        size = int(header[2])
        results.append(output[offset:offset + size] if header[1] == b"blob" else None)
        offset += size + 1
    return results


def changed_files(root: Path, base: str, *prefixes: str, kinds: str = "ACMRT") -> tuple[str, ...]:
    """Paths under ``prefixes`` whose working tree differs from ``base`` as ``kinds`` (``git diff --diff-filter``
    letters; deletions are excluded by default), plus untracked files."""
    diff = git(root, "diff", "--name-only", "--no-renames", f"--diff-filter={kinds}", "-z", "--end-of-options", base, "--", *prefixes)
    untracked = git(root, "ls-files", "-z", "--others", "--exclude-standard", "--", *prefixes)
    return tuple(sorted({path for path in (diff + untracked).decode().split("\0") if path}))


def listed_files(root: Path, *prefixes: str) -> tuple[str, ...]:
    """Tracked and untracked, non-ignored files under ``prefixes``."""
    output = git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *prefixes)
    return tuple(sorted({path for path in output.decode().split("\0") if path}))


def descends(root: Path, commit: str) -> bool | None:
    """Whether HEAD descends from ``commit``; ``None`` when Git cannot tell (unknown commit, shallow clone)."""
    result = subprocess.run(
        ["git", "-C", str(root), "merge-base", "--is-ancestor", "--end-of-options", commit, "HEAD"],
        capture_output=True, check=False,
    )
    return {0: True, 1: False}.get(result.returncode)


def added_by(root: Path, path: str) -> str | None:
    """The oldest commit in HEAD's history that added ``path``; ``None`` when none did or Git cannot tell."""
    try:
        output = git(root, "log", "--diff-filter=A", "--format=%H", "--end-of-options", "HEAD", "--", path, timeout=READ_TIMEOUT)
    except GitError:
        return None
    lines = output.decode().split()
    return lines[-1] if lines else None


def same_tree_outside(root: Path, first: str, second: str, excluded: str) -> bool | None:
    """Whether the trees of commits ``first`` and ``second`` are equal outside ``excluded``; ``None`` when Git
    cannot compare them (a commit not in this clone)."""
    try:
        status, _ = run_git(root, "diff", "--quiet", "--no-ext-diff", "--end-of-options", f"{first}^{{commit}}", f"{second}^{{commit}}",
                            "--", ".", f":(exclude){excluded}", ok=(0, 1), timeout=READ_TIMEOUT)
    except GitError:
        return None
    return status == 0


def merge_base(root: Path, base: str) -> str:
    """``git merge-base <base> HEAD``; a ``GitError`` naming the cause when ``base`` is unknown or shares no history."""
    try:
        _ = git(root, "rev-parse", "--verify", "--quiet", "--end-of-options", f"{base}^{{commit}}")
    except GitError as error:
        raise GitError(f"{base} is not a known commit here; fetch it before checking") from error
    try:
        return git(root, "merge-base", "--end-of-options", base, "HEAD").decode().strip()
    except GitError as error:
        raise GitError(f"no merge base of {base} and HEAD (unrelated histories or a shallow clone; fetch full history): {error}") from error
