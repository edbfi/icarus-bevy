"""sha256 pin manifests named by evidence rows: one reading for every tool.

``tools/docs_lint.py`` verifies and backs up the files a manifest lists,
``tools/work_gc.py`` keeps them out of pruning and ``tools/cargo_guard.py``
keeps them out of build targets. All three recognise, parse and resolve a
manifest here, so they cannot disagree about which files it pins.

A row's file is a manifest when its name says so (``*sha256*``, ``*fixity*``)
or its first non-blank line is a ``sha256sum`` line, whatever its name. Lines
are ``sha256sum`` lines in text or binary mode, or ``<bytes> <path>`` size lines,
which some manifests append. An entry names a file relative to the manifest's
directory or to the repository root, since both conventions are in use; it may
not be absolute or leave the repository. A manifest that is named like one but
is missing, or that cannot be read or parsed, raises ``ManifestError``: its
entries are unknown and each tool must fail safe.
"""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import re

# Any ``sha256sum`` line, text or binary mode.
DIGEST_LINE = re.compile(r"([0-9a-fA-F]{64}) [ *](\S(?:.*\S)?)")
# A ``<bytes> <path>`` size line. At most 20 digits (any 64-bit size), so it can never be
# read as a 64-digit digest line.
SIZE_LINE = re.compile(r"([0-9]{1,20}) (\S(?:.*\S)?)")
# Recognising an unnamed manifest reads at most this much of the row's file.
SNIFF_BYTES = 4096
# Rows whose manifest is a receipt of files deliberately removed: it records what is gone,
# so its entries are expected to be absent and are neither verified nor backed up. The
# manifest itself must still parse. Changing this is a deliberate, reviewed act.
REMOVAL_RECEIPTS: frozenset[str] = frozenset({"M-1079"})


class ManifestError(ValueError):
    """A manifest whose entries cannot be known."""


class ManifestTooLarge(ManifestError):
    """A manifest larger than the caller's byte budget."""


@dataclass(frozen=True)
class Entry:
    """One manifest line: the path as written, where it may live, and what it records."""

    line: int
    path: str
    # Lexically normalised, manifest directory first, every one inside the repository.
    candidates: tuple[Path, ...]
    sha256: str | None = None
    size: int | None = None


def normal(path: Path) -> Path:
    """``path`` with ``.``, ``..`` and repeated separators removed, without touching the filesystem."""
    return Path(os.path.normpath(path))


def is_within(inner: str, outer: str) -> bool:
    """Path ``inner`` equals or lies inside ``outer``; both lexically normal. Text, not Path: a hot path."""
    return inner == outer or inner.startswith(outer if outer.endswith("/") else outer + "/")


def named_like_manifest(path: Path) -> bool:
    name = path.name.lower()
    return "sha256" in name or "fixity" in name


def entry_candidates(root: Path, manifest: Path, rel: str) -> tuple[Path, ...]:
    """Where entry ``rel`` of ``manifest`` may live inside ``root``; empty when it leaves the repository."""
    top = os.path.normpath(root)
    return tuple(Path(path) for path in candidate_texts(top, os.path.dirname(os.path.normpath(manifest)), rel))


def candidate_texts(top: str, directory: str, rel: str) -> tuple[str, ...]:
    """``entry_candidates`` as text, for a lexically normal root ``top`` and manifest ``directory``."""
    found: list[str] = []
    for base in (directory, top):
        path = os.path.normpath(os.path.join(base, rel))
        if is_within(path, top) and path not in found:
            found.append(path)
    return tuple(found)


def sniff(path: Path) -> bool:
    """The first non-blank line of the regular file ``path`` is a ``sha256sum`` line."""
    with path.open("rb") as handle:
        head = handle.read(SNIFF_BYTES)
    first = next((line for line in head.splitlines() if line.strip()), b"")
    return DIGEST_LINE.fullmatch(first.decode("utf-8", "replace")) is not None


def read_manifest(root: Path, path: Path, *, max_bytes: int | None = None) -> list[Entry] | None:
    """Entries of the manifest at ``path``, a file named by an evidence row; ``None`` when it is no manifest.

    Not a manifest: a directory not named like one, an absent path not named like one, or a regular file neither
    named like one nor starting with a ``sha256sum`` line. Raises ManifestError when a path that
    is, or is named like, a manifest is missing, not a regular file, unreadable or malformed,
    lists nothing, or has an entry that is absolute or leaves the repository; ManifestTooLarge
    when it exceeds ``max_bytes``, before reading it.
    """
    path = normal(path)
    named = named_like_manifest(path)
    try:
        if path.is_dir():
            if named:
                raise ManifestError("manifest is a directory")
            return None
        if not path.is_file():
            if named or path.is_symlink():
                raise ManifestError("manifest is missing or not a regular file")
            return None
        if not named and not sniff(path):
            return None
        if max_bytes is not None and path.stat().st_size > max_bytes:
            raise ManifestTooLarge(f"manifest exceeds {max_bytes} bytes")
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise ManifestError(f"unreadable manifest: {error}") from error
    return parse(root, path, text)


def parse(root: Path, path: Path, text: str) -> list[Entry]:
    entries: list[Entry] = []
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        digest = DIGEST_LINE.fullmatch(line)
        size = None if digest else SIZE_LINE.fullmatch(line)
        match = digest or size
        if match is None:
            raise ManifestError(f"line {number} is neither '<sha256>  <path>' nor '<bytes> <path>', so the files it lists are unknown")
        rel = match.group(2)
        if Path(rel).is_absolute():
            raise ManifestError(f"line {number} names an absolute path")
        found = entry_candidates(root, path, rel)
        if not found:
            raise ManifestError(f"line {number} leaves the repository")
        entries.append(Entry(
            number,
            rel,
            found,
            sha256=digest.group(1).lower() if digest else None,
            size=int(size.group(1)) if size else None,
        ))
    if not entries:
        raise ManifestError("manifest lists no files")
    return entries
