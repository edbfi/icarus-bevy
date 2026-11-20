# SPDX-License-Identifier: AGPL-3.0-or-later
"""The Git reads the memory tool shares: plumbing only, never a checkout or a fetch.

Every failure is ``GitError`` with Git's own message; callers decide whether that fails closed.
"""

from __future__ import annotations

import subprocess
from collections.abc import Sequence
from pathlib import Path


class GitError(RuntimeError):
    """A Git command failed or answered in an unexpected form."""


def git(root: Path, *args: str, stdin: bytes | None = None) -> bytes:
    result = subprocess.run(["git", "--no-replace-objects", "-C", str(root), *args], input=stdin, capture_output=True, check=False)
    if result.returncode:
        raise GitError(f"git {args[0]}: {result.stderr.decode(errors='replace').strip()}")
    return result.stdout


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
