# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory salvage REF [--check] [--out DIR]``: a branch's added or modified legacy rows as proposals.

It is meant to run after the cut-over, when ``main`` holds ``memory/journal/`` and no legacy trees, on
branches and worktrees that still carry unmerged legacy rows; before it, ``main``'s legacy export
stands in for the journal it will become.

``salvage(root, ref)`` reads everything through Git plumbing (nothing is checked out). The rows ``ref``
adds or modifies are found by exporting its legacy trees and those of its merge-base with ``main``
(``tools.memory.legacy.export``, through the frozen ``tools.memory.legacy_reader``); a merge-base that predates the journal is the normal case. A ``ref``
whose tip has no legacy ledgers or evidence index has nothing to salvage. Each changed row is then
classified against what ``main`` accepts: its replayed journal when it has one (it wins over any legacy
trees still there), else its legacy export, whose records the cut-over journal holds as first revisions.

- ``accepted``: ``main``'s head of the ID is the same record (for example through a squash merge); no proposal.
- ``new``: ``main`` has no record with the ID; a first revision.
- ``revision``: ``main`` holds the ID unchanged since the merge-base: a revision naming ``main``'s head
  (its ``rev`` in the journal, every head of a fork) as ``prev``. Fields the rules do not let a revision
  change (``check``'s allow-list: anything but status, appended notes and verified_on, a REF body, an
  unknown's next probe) are listed in ``unrevisable``: such a row must become a new record that supersedes the old one.
- ``conflict``: ``main`` holds the ID with other content, and either the merge-base did not hold it
  (both sides allocated the number, or ``main`` took an earlier draft) or ``main`` changed it since the
  merge-base (by a journal revision or a legacy edit). Never proposed: a revision built from the branch's
  row would silently undo ``main``'s change, so it needs a new ID or a reconciling revision by hand
  (rules, "Revisions").

New compatibility and campaign rows get positional IDs at export, so their IDs are only provisional.
A document the branch changed (a spec, digest, ledger prose, the status file) is listed by path with
the main REF whose body to revise, since a REF's ``legacy`` never changes and its body is revised by hand. ``unsalvaged`` holds every proposal or
conflict whose content (ID and sha256) is not yet in ``ref``'s own journal.

The command prints the proposals as segment lines, or writes them under ``DIR/memory/journal``; notices and
conflicts go to stderr, and a conflict makes the exit status 1. ``--check`` prints each unsalvaged row and
exits 1 if there is any.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

from tools.memory.check import APPEND_ONLY, REVISABLE
from tools.memory.gitio import GitError, git
from tools.memory.journal import JOURNAL, JournalError, Revision, read_git, split_change_set, write_change_set
from tools.memory.legacy import SALVAGED, commit_time, export
from tools.memory.legacy_reader import CatalogError, has_legacy
from tools.memory.legacy_rules import MigrationError
from tools.memory.model import Record, RecordError, id_key
from tools.memory.state import replay

MAIN = "main"


@dataclass
class Salvage:
    ref: str
    head: str
    base: str
    main: str
    source: str = "journal"
    proposals: list[Revision] = field(default_factory=list[Revision])
    kinds: dict[str, str] = field(default_factory=dict[str, str])
    unrevisable: dict[str, tuple[str, ...]] = field(default_factory=dict[str, tuple[str, ...]])
    conflicts: list[tuple[Record, Record]] = field(default_factory=list[tuple[Record, Record]])
    unsalvaged: list[str] = field(default_factory=list[str])
    notices: list[str] = field(default_factory=list[str])


@dataclass(frozen=True)
class Accepted:
    """What ``main`` accepts per ID: the head record, the revision identities a revision of it names as
    ``prev``, and whether that came from its journal or its legacy export."""

    records: dict[str, Record]
    heads: dict[str, tuple[str, ...]]
    source: str


def accepted_on(root: Path, main: str) -> Accepted:
    """``main``'s replayed journal when it has one; else its legacy export, as the cut-over will journal it."""
    if segments := read_git(root, main):
        state = replay(segments)
        return Accepted({identifier: entry.record for identifier, entry in state.entries.items()},
                        {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal")
    records = export(root, main).records()
    return Accepted(records, {identifier: (Revision.of(record).rev,) for identifier, record in records.items()}, "legacy")


def documents(records: dict[str, Record]) -> dict[str, Record]:
    """REF records by source path."""
    return {record.legacy.path: record for record in records.values() if record.type == "reference" and record.legacy is not None}


def unrevisable_fields(old: Record, new: Record) -> tuple[str, ...]:
    """Fields that differ although ``check``'s allow-list (``REVISABLE``, ``APPEND_ONLY``, status) does not let a revision change them."""
    before, after = old.to_json(), new.to_json()
    allowed = {"status", *APPEND_ONLY, *REVISABLE.get(new.type, ())}
    return tuple(name for name in sorted(before.keys() | after.keys()) if name not in allowed and before.get(name) != after.get(name))


def commit(root: Path, ref: str) -> str:
    return git(root, "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}").decode().strip()


def salvage(root: Path, ref: str) -> Salvage:
    head = commit(root, ref)
    base = git(root, "merge-base", "--end-of-options", MAIN, head).decode().strip()
    result = Salvage(ref, head, base, commit(root, MAIN))
    if not has_legacy(root, head):
        result.notices.append(f"{ref} has no legacy ledgers or evidence index at its tip; nothing to salvage")
        return result
    after = export(root, head).records()
    before = export(root, base).records() if has_legacy(root, base) else {}
    main = accepted_on(root, result.main)
    result.source, accepted = main.source, main.records
    for identifier in sorted(after, key=id_key):
        record = after[identifier]
        if record.type not in SALVAGED or before.get(identifier) == record:
            continue
        on_main = accepted.get(identifier)
        if on_main == record:
            result.kinds[identifier] = "accepted"
            continue
        if on_main is not None and on_main != before.get(identifier):
            result.kinds[identifier] = "conflict"
            result.conflicts.append((on_main, record))
            continue
        prev = on_main if on_main is not None else before.get(identifier) if main.source == "legacy" else None
        result.kinds[identifier] = "new" if prev is None else "revision"
        if prev is not None and (fields := unrevisable_fields(prev, record)):
            result.unrevisable[identifier] = fields
        if record.type in ("compatibility", "report") and prev is None:
            result.notices.append(f"{identifier}: new {record.type} row; its migration ID is positional and provisional")
        # Main's heads; a row main's legacy export lost names the first revision the cut-over would have journalled.
        heads = main.heads[identifier] if on_main is not None else (Revision.of(prev).rev,) if prev is not None else ()
        result.proposals.append(Revision.of(record, heads))
    result.notices.extend(f"{identifier}: removed on {ref} (a deletion needs no proposal)" for identifier in sorted(set(before) - set(after), key=id_key)
                          if before[identifier].type in SALVAGED)
    main_refs, base_bodies = documents(accepted), {path: record.body for path, record in documents(before).items()}
    for path, record in sorted(documents(after).items()):
        on_main = main_refs.get(path)
        if on_main is None:
            result.notices.append(f"{path}: new document; propose a REF for it by hand")
        elif record.body not in (on_main.body, base_bodies.get(path)):
            result.notices.append(f"{path}: changed on {ref}; revise the body of {on_main.id} by hand")
    journal = {(revision.record.id, revision.sha256) for segment in read_git(root, head) for revision in segment.revisions}
    result.unsalvaged = [revision.record.id for revision in result.proposals if (revision.record.id, revision.sha256) not in journal]
    result.unsalvaged += [new.id for _old, new in result.conflicts]
    result.proposals.sort(key=lambda revision: (revision.record.type, id_key(revision.record.id)))
    return result


# Command line.


class Subparsers(Protocol):
    """What ``register`` needs of ``ArgumentParser.add_subparsers()``."""

    def add_parser(self, name: str, *, help: str) -> argparse.ArgumentParser: ...


def register(subparsers: Subparsers) -> None:
    parser = subparsers.add_parser("salvage", help="turn a branch's added or modified legacy rows into proposals")
    _ = parser.add_argument("ref", help="branch, commit or worktree HEAD to salvage")
    _ = parser.add_argument("--check", action="store_true", help="only report unsalvaged legacy rows")
    _ = parser.add_argument("--out", type=Path, help=f"write the proposal segments under DIR/{JOURNAL}")
    _ = parser.set_defaults(handler=run_salvage)


def repository_root() -> Path:
    return Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())


def run_salvage(options: argparse.Namespace) -> int:
    ref = str(getattr(options, "ref", ""))
    out = getattr(options, "out", None)
    try:
        root = repository_root()
        result = salvage(root, ref)
        if getattr(options, "check", False) is True:
            for identifier in result.unsalvaged:
                print(f"{ref}: {identifier}: {result.kinds[identifier]} legacy row not yet salvaged")
            print(f"memory salvage --check: {len(result.unsalvaged)} unsalvaged row(s) on {ref} (merge-base {result.base[:12]})")
            return 1 if result.unsalvaged else 0
        for notice in result.notices:
            print(f"notice: {notice}", file=sys.stderr)
        for identifier, fields in result.unrevisable.items():
            print(f"notice: {identifier}: changes {', '.join(fields)}, which a revision may not change; propose a new record that supersedes it", file=sys.stderr)
        for old, new in result.conflicts:
            print(f"conflict: {new.id}: main holds other content under this ID, allocated or changed there since the merge-base "
                  + f"({old.legacy.path if old.legacy else '?'}); not proposed", file=sys.stderr)
        accepted = sorted((identifier for identifier, kind in result.kinds.items() if kind == "accepted"), key=id_key)
        print(f"memory salvage: {ref}: {len(result.proposals)} proposal(s), {len(result.conflicts)} conflict(s), "
              + f"{len(accepted)} row(s) already on main (against main's {'journal' if result.source == 'journal' else 'legacy export'})", file=sys.stderr)
        if not result.proposals:
            return 1 if result.conflicts else 0
        created = commit_time(root, result.head)
        if isinstance(out, Path):
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
