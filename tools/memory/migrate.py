# SPDX-License-Identifier: AGPL-3.0-or-later
"""Commands of the legacy migration: ``migrate``, ``parity`` and ``salvage`` (``register`` adds them).

- ``memory migrate --rev REV --out DIR`` writes the export of ``REV`` as segments under
  ``DIR/memory/journal`` and the migration report as ``DIR/migration-report.md``. ``DIR`` may not be
  the repository itself: this command never writes the repository's own journal.
- ``memory parity --rev REV --journal DIR`` runs the gate (``tools.memory.parity``); exit 1 on any
  mismatch, each printed with its ID.
- ``memory salvage REF [--check] [--out DIR]`` converts the legacy rows ``REF`` adds or modifies
  against its merge-base with ``main`` (``tools.memory.salvage``) into one proposal change set, printed
  as segment lines or written under ``DIR/memory/journal``; notices and conflicts go to stderr, and a
  conflict makes the exit status 1. ``--check`` prints each unsalvaged row and exits 1 if there is any.
"""

from __future__ import annotations

import argparse
import sys
import time
from pathlib import Path
from typing import Protocol

from tools.evidence_catalog_model import CatalogError
from tools.memory.gitio import GitError, git
from tools.memory.journal import JournalError, split_change_set, write_change_set
from tools.memory.legacy import commit_time, export, write
from tools.memory.legacy_rules import MigrationError
from tools.memory.model import RecordError, id_key
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
    salvage_parser = subparsers.add_parser("salvage", help="turn a branch's added or modified legacy rows into proposals")
    _ = salvage_parser.add_argument("ref", help="branch, commit or worktree HEAD to salvage")
    _ = salvage_parser.add_argument("--check", action="store_true", help="only report unsalvaged legacy rows")
    _ = salvage_parser.add_argument("--out", type=Path, help="write the proposal segments under DIR/memory/journal")
    _ = salvage_parser.set_defaults(handler=run_salvage)


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


def run_salvage(options: argparse.Namespace) -> int:
    from tools.memory.salvage import salvage

    ref = text_option(options, "ref")
    out = path_option(options, "out")
    try:
        root = repository_root()
        result = salvage(root, ref)
        if option(options, "check") is True:
            for identifier in result.unsalvaged:
                print(f"{ref}: {identifier}: {result.kinds[identifier]} legacy row not yet salvaged")
            print(f"memory salvage --check: {len(result.unsalvaged)} unsalvaged row(s) on {ref} (merge-base {result.base[:12]})")
            return 1 if result.unsalvaged else 0
        for notice in result.notices:
            print(f"notice: {notice}", file=sys.stderr)
        for identifier, fields in result.unrevisable.items():
            print(f"notice: {identifier}: changes {', '.join(fields)}, which a revision may not change; propose a new record that supersedes it", file=sys.stderr)
        for old, new in result.conflicts:
            print(f"conflict: {new.id}: main holds different content under this ID ({old.legacy.path if old.legacy else '?'}); not proposed", file=sys.stderr)
        accepted = sorted((identifier for identifier, kind in result.kinds.items() if kind == "accepted"), key=id_key)
        print(f"memory salvage: {ref}: {len(result.proposals)} proposal(s), {len(result.conflicts)} conflict(s), "
              + f"{len(accepted)} row(s) already on main", file=sys.stderr)
        if not result.proposals:
            return 1 if result.conflicts else 0
        created = commit_time(root, result.head)
        if out is not None:
            written = write_change_set(out, created, result.proposals)
            print(f"memory salvage: wrote {len(written)} segment(s) under {out}", file=sys.stderr)
        else:
            for path, data in split_change_set(created, result.proposals):
                print(f"# {path}")
                print(data.decode(), end="")
    except (CatalogError, GitError, JournalError, MigrationError, RecordError, OSError) as error:
        print(f"memory salvage: {error}", file=sys.stderr)
        return 1
    return 1 if result.conflicts else 0
