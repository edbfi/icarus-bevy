# SPDX-License-Identifier: AGPL-3.0-or-later
"""Commands of the legacy migration: ``migrate`` and ``parity`` (``register`` adds them; ``salvage`` is ``tools.memory.salvage``).

- ``memory migrate --rev REV --out DIR`` writes the export of ``REV`` as segments under
  ``DIR/memory/journal`` and the migration report as ``DIR/migration-report.md``. ``DIR`` may not be
  the repository itself: this command never writes the repository's own journal.
- ``memory parity --rev REV --journal DIR`` runs the gate (``tools.memory.parity``); exit 1 on any
  mismatch, each printed with its ID.
"""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path
from typing import Protocol

from tools.evidence_catalog_model import CatalogError
from tools.memory.gitio import GitError, git
from tools.memory.journal import JournalError
from tools.memory.legacy import export, write
from tools.memory.legacy_rules import MigrationError
from tools.memory.model import RecordError
from tools.memory.paths import ProtectedPathsError



class Subparsers(Protocol):
    """What ``register`` needs of ``ArgumentParser.add_subparsers()``."""

    def add_parser(self, name: str, *, help: str) -> argparse.ArgumentParser: ...


def register(subparsers: Subparsers) -> None:
    migrate = subparsers.add_parser("migrate", help="export the legacy ledgers and evidence index at a revision as segments")
    _ = migrate.add_argument("--rev", required=True, help="Git revision to export")
    _ = migrate.add_argument("--out", required=True, type=Path, help="directory to write DIR/memory/journal and the report into")
    _ = migrate.set_defaults(handler=run_migrate)
    gate = subparsers.add_parser("parity", help="compare a migration journal with the legacy system at a revision")
    _ = gate.add_argument("--rev", required=True, help="Git revision the journal was exported from")
    _ = gate.add_argument("--journal", required=True, type=Path, help="directory holding memory/journal")
    _ = gate.set_defaults(handler=run_parity)


def option(options: argparse.Namespace, name: str) -> object:
    value: object = getattr(options, name, None)
    return value


def text_option(options: argparse.Namespace, name: str) -> str:
    value = option(options, name)
    if not isinstance(value, str):
        raise TypeError(f"--{name} must be text")
    return value


def path_option(options: argparse.Namespace, name: str) -> Path | None:
    value = option(options, name)
    return value if isinstance(value, Path) else None


def repository_root() -> Path:
    return Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())


def run_migrate(options: argparse.Namespace) -> int:
    from tools.memory.legacy_report import render

    out = path_option(options, "out")
    try:
        root = repository_root()
        if out is None or out.resolve() == root.resolve():
            print("memory migrate: --out must not be the repository; this command never writes its journal", file=sys.stderr)
            return 2
        started = time.monotonic()
        result = export(root, text_option(options, "rev"))
        written = write(result, out)
        _ = (out / "migration-report.md").write_text(render(result, root), encoding="utf-8")
    except (CatalogError, GitError, JournalError, MigrationError, RecordError, OSError) as error:
        print(f"memory migrate: {error}", file=sys.stderr)
        return 1
    print(f"memory migrate: {len(result.migrated)} records in {len(written)} segment(s) from {result.revision} in {time.monotonic() - started:.1f} s")
    return 0


def run_parity(options: argparse.Namespace) -> int:
    from tools.memory.parity import parity

    journal = path_option(options, "journal")
    try:
        started = time.monotonic()
        result = parity(repository_root(), text_option(options, "rev"), journal or Path())
    except (CatalogError, GitError, JournalError, MigrationError, ProtectedPathsError, RecordError, OSError) as error:
        print(f"memory parity: {error}", file=sys.stderr)
        return 1
    for mismatch in result.mismatches:
        print(mismatch)
    for name, value in result.measured.items():
        print(f"{name}: {value}")
    print(f"memory parity: {len(result.mismatches)} mismatch(es) in {time.monotonic() - started:.1f} s")
    return 1 if result.mismatches else 0
