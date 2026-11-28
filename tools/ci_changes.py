#!/usr/bin/env python3
"""Classify pull-request paths for the CI planner.

The classifier is deliberately conservative. Only a small allowlist of paths
can avoid the Rust and release jobs. Every other path, including an unavailable
or malformed diff, requests the full validation path.
"""

from __future__ import annotations

from collections.abc import Sequence
import argparse
from dataclasses import dataclass
from pathlib import Path


SKIP_ONLY_PREFIXES = ("docs/", "memory/")
SKIP_ONLY_FILES = frozenset({"README.md", "CLAUDE.md"})


@dataclass(frozen=True)
class Decision:
    """The jobs the workflow must run for one event."""

    run_rust: bool
    run_release: bool
    classification: str

    def output(self) -> str:
        """Render values suitable for ``GITHUB_OUTPUT``."""
        return "\n".join(
            (
                f"run_rust={'true' if self.run_rust else 'false'}",
                f"run_release={'true' if self.run_release else 'false'}",
                f"classification={self.classification}",
            )
        )


@dataclass(frozen=True)
class Options:
    """Validated command-line values."""

    event: str
    changed_files: Path
    diff_available: bool


class ParsedArguments(argparse.Namespace):
    """The argparse namespace with the types established by the parser."""

    event: str = ""
    changed_files: Path = Path()
    diff_available: str = "true"


def skip_only(path: str) -> bool:
    """Return whether a path is known to affect only documentation or memory."""
    return path.startswith(SKIP_ONLY_PREFIXES) or path in SKIP_ONLY_FILES


def classify(event_name: str, changed_paths: Sequence[str], *, diff_available: bool = True) -> Decision:
    """Choose validation jobs, failing closed for unknown PR inputs."""
    if event_name != "pull_request":
        return Decision(True, True, "full-event")
    if not diff_available:
        return Decision(True, True, "diff-unavailable")
    if not changed_paths:
        return Decision(True, True, "empty-diff")
    if all(skip_only(path) for path in changed_paths):
        return Decision(False, False, "skip-only")
    return Decision(True, True, "rust-affecting")


def read_paths(path: Path) -> tuple[str, ...]:
    """Read the NUL-delimited path list emitted by ``git diff``."""
    raw = path.read_bytes()
    return tuple(item.decode("utf-8") for item in raw.split(b"\0") if item)


def arguments() -> Options:
    """Parse planner arguments."""
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--event", required=True)
    _ = parser.add_argument("--changed-files", type=Path, required=True)
    _ = parser.add_argument("--diff-available", choices=("true", "false"), default="true")
    parsed = parser.parse_args(namespace=ParsedArguments())
    if parsed.diff_available not in {"true", "false"}:
        raise RuntimeError("argparse returned an invalid planner value")
    return Options(parsed.event, parsed.changed_files, parsed.diff_available == "true")


def main() -> int:
    """Print the planner decision and continue full validation on read errors."""
    options = arguments()
    try:
        paths = read_paths(options.changed_files)
    except (OSError, UnicodeDecodeError):
        paths = ()
        available = False
    else:
        available = options.diff_available
    print(classify(options.event, paths, diff_available=available).output())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
