# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory salvage REF [--check] [--out DIR] [--main REF]``: a branch's added or modified legacy rows as proposals, with the export's conversion.

It is meant to run after the cut-over, when ``origin/main`` holds ``memory/journal/`` and no legacy trees, on
branches and worktrees that still carry unmerged legacy rows; before it, the accepted tree's legacy export
stands in for the journal it will become.

``salvage(root, ref, main)`` reads everything through Git plumbing (nothing is checked out). ``main`` (default
``origin/main``, the acceptance ref, ``--main`` overrides it) is resolved to one commit, used both for the
merge base and for what is accepted; a stale or missing local ``main`` branch plays no part. The rows ``ref``
adds or modifies are found by exporting its legacy trees and those of its merge-base with that commit
(``tools.memory.legacy.export``); a merge-base that predates the journal is the normal case. Each changed row
is then classified against what the accepted commit holds: its replayed journal when it has one (it wins over
any legacy trees still there), else its legacy export, whose records the cut-over journal holds as first revisions.

- ``accepted``: the accepted head of the ID is the same record (for example through a squash merge); no proposal.
- ``new``: nothing accepted has the ID; a first revision.
- ``revision``: the accepted record is unchanged since the merge-base and the row changes only what a
  revision may change (``check``'s allow-list: status, appended notes, verified_on, supersedes and resolves, a
  REF body, an unknown's next probe): a revision naming the accepted head (its ``rev``, every head of a fork) as ``prev``.
- ``supersede``: as ``revision``, but the row changes a field no revision may change (a migrated row's verbatim
  ``legacy`` cells among them). Never proposed, since ``check --ci`` would refuse the segment: ``unrevisable``
  lists the fields, and the row needs a new record that supersedes the accepted one.
- ``conflict``: the accepted commit holds the ID with other content, and either the merge-base did not hold it
  (both sides allocated the number, or main took an earlier draft) or main changed it since the merge-base
  (by a journal revision or a legacy edit). Never proposed: a revision built from the branch's row would
  silently undo main's change, so it needs a new ID or a reconciling revision by hand (rules, "Revisions").

Salvage must run on the branch's pre-merge tip. A branch that merged the cut-over (a merge commit in its
first-parent history since the merge base, one of whose parents is an ancestor of the accepted commit without
legacy trees) has a merge base with no legacy trees: resolving the modify/delete conflicts by deleting leaves
nothing to export, and keeping the files makes every row look changed. When the merge's other parent holds
changed legacy rows, ``premerge`` names that parent and nothing is classified: run ``memory salvage <premerge>``.

New compatibility and campaign rows get positional IDs at export, so their IDs are only provisional.
A document the branch changed (a spec, digest, ledger prose, the status file) is listed by path with
the accepted REF whose body to revise, since a REF's ``legacy`` never changes and its body is revised by hand.
``unsalvaged`` holds every proposal, conflict or row needing a superseding record whose content (ID and
sha256) is not yet in ``ref``'s own journal.

The command prints the proposal change set as segment lines, or writes it under ``DIR/memory/journal`` with
``--out``; notices, conflicts and rows needing a superseding record go to stderr. ``--check`` prints each
unsalvaged row and exits 1 if there is any. Exit status: 0 success; 1 a conflict or an error; 2 usage;
``EXIT_SUPERSEDE`` (4) some row needs a superseding record (and none conflicts; the other proposals are
still printed or written); ``EXIT_PREMERGE`` (5) ``ref`` merged the cut-over: salvage the named pre-merge
parent instead (nothing is printed or written).
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

from tools.evidence_catalog_model import CatalogError
from tools.evidence_catalog_store import LEGACY_ROOTS
from tools.memory.check import APPEND_ONLY, REVISABLE
from tools.memory.gitio import GitError, git, run_git
from tools.memory.journal import JournalError, Revision, read_git, split_change_set, write_change_set
from tools.memory.legacy import commit_time, export
from tools.memory.legacy_rules import MigrationError
from tools.memory.model import Record, RecordError, id_key
from tools.memory.state import replay

ACCEPTANCE_REF = "origin/main"
SALVAGED = ("finding", "unknown", "bug", "evidence", "lead", "report", "compatibility")
EXIT_SUPERSEDE = 4
EXIT_PREMERGE = 5


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
    premerge: str | None = None
    merge: str | None = None


@dataclass(frozen=True)
class Accepted:
    """What the accepted commit holds per ID: the head record, the revision identities a revision of it names
    as ``prev``, and whether that came from its journal or its legacy export."""

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


def has_legacy(root: Path, revision: str) -> bool:
    """Whether ``revision`` has a legacy ledger or evidence index tree."""
    return bool(git(root, "ls-tree", "--name-only", "--end-of-options", revision, "--", *LEGACY_ROOTS).strip())


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


def merge_base(root: Path, main: str, head: str) -> str:
    return git(root, "merge-base", "--end-of-options", main, head).decode().strip()


def is_ancestor(root: Path, ancestor: str, descendant: str) -> bool:
    return run_git(root, "merge-base", "--is-ancestor", "--end-of-options", ancestor, descendant, ok=(0, 1))[0] == 0


def changed_rows(root: Path, head: str, main: str) -> dict[str, Record]:
    """The salvaged-type rows ``head``'s legacy trees add or modify against its merge-base with ``main``."""
    after = export(root, head).records()
    base = merge_base(root, main, head)
    before = export(root, base).records() if has_legacy(root, base) else {}
    return {identifier: record for identifier, record in after.items() if record.type in SALVAGED and before.get(identifier) != record}


def cut_over_merge(root: Path, head: str, base: str, main: str) -> tuple[str, str] | None:
    """The first merge commit in ``head``'s first-parent history since ``base`` that merged the cut-over (a parent
    that is an ancestor of ``main`` and has no legacy trees) with a parent holding legacy trees: ``(merge, that parent)``."""
    merges = git(root, "rev-list", "--first-parent", "--merges", "--reverse", "--end-of-options", f"{base}..{head}").decode().split()
    for merge in merges:
        parents = git(root, "rev-list", "--parents", "-n", "1", "--end-of-options", merge).decode().split()[1:]
        accepted = [parent for parent in parents if is_ancestor(root, parent, main) and not has_legacy(root, parent)]
        branch = [parent for parent in parents if parent not in accepted and has_legacy(root, parent)]
        if accepted and branch:
            return merge, branch[0]
    return None


def salvage(root: Path, ref: str, main: str = ACCEPTANCE_REF) -> Salvage:
    head = commit(root, ref)
    accepted_commit = commit(root, main)
    base = merge_base(root, accepted_commit, head)
    result = Salvage(ref, head, base, accepted_commit)
    if (merged := cut_over_merge(root, head, base, accepted_commit)) is not None:
        merge, parent = merged
        if changed_rows(root, parent, accepted_commit):
            result.merge, result.premerge = merge, parent
            result.notices.append(f"{ref} merged the cut-over in {merge[:12]}; its legacy rows are on the pre-merge tip {parent}: "
                                  + f"salvage must run there (memory salvage {parent})")
        else:
            result.notices.append(f"{ref} merged the cut-over in {merge[:12]}; its pre-merge tip {parent[:12]} changes no legacy rows; nothing to salvage")
        return result
    if not has_legacy(root, head):
        result.notices.append(f"{ref} has no legacy ledgers or evidence index at its tip; nothing to salvage")
        return result
    after = export(root, head).records()
    before = export(root, base).records() if has_legacy(root, base) else {}
    on_accepted = accepted_on(root, accepted_commit)
    result.source, accepted = on_accepted.source, on_accepted.records
    superseding: list[Revision] = []
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
        prev = on_main if on_main is not None else before.get(identifier) if on_accepted.source == "legacy" else None
        # The accepted heads; a row the accepted legacy export lost names the first revision the cut-over would have journalled.
        heads = on_accepted.heads[identifier] if on_main is not None else (Revision.of(prev).rev,) if prev is not None else ()
        if prev is not None and (fields := unrevisable_fields(prev, record)):
            result.kinds[identifier] = "supersede"
            result.unrevisable[identifier] = fields
            superseding.append(Revision.of(record, heads))
            continue
        result.kinds[identifier] = "new" if prev is None else "revision"
        if record.type in ("compatibility", "report") and prev is None:
            result.notices.append(f"{identifier}: new {record.type} row; its migration ID is positional and provisional")
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
    result.unsalvaged = [revision.record.id for revision in [*result.proposals, *superseding] if (revision.record.id, revision.sha256) not in journal]
    result.unsalvaged += [new.id for _old, new in result.conflicts]
    result.proposals.sort(key=lambda revision: (revision.record.type, id_key(revision.record.id)))
    return result


class Subparsers(Protocol):
    """What ``register`` needs of ``ArgumentParser.add_subparsers()``."""

    def add_parser(self, name: str, *, help: str) -> argparse.ArgumentParser: ...


def register(subparsers: Subparsers) -> None:
    parser = subparsers.add_parser("salvage", help="turn a branch's added or modified legacy rows into proposals")
    _ = parser.add_argument("ref", help="branch, commit or worktree HEAD to salvage (its pre-merge tip if it merged the cut-over)")
    _ = parser.add_argument("--check", action="store_true", help="only report unsalvaged legacy rows")
    _ = parser.add_argument("--out", type=Path, help="write the proposal segments under DIR/memory/journal")
    _ = parser.add_argument("--main", default=ACCEPTANCE_REF, help=f"the accepted commit to classify against (default {ACCEPTANCE_REF})")
    _ = parser.set_defaults(handler=run)


def repository_root() -> Path:
    return Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())


def option(options: argparse.Namespace, name: str) -> object:
    value: object = getattr(options, name, None)
    return value


def run(options: argparse.Namespace) -> int:
    """The command; exit statuses in the module docstring."""
    ref = text if isinstance(text := option(options, "ref"), str) else ""
    check = option(options, "check") is True
    out = path if isinstance(path := option(options, "out"), Path) else None
    main = name if isinstance(name := option(options, "main"), str) else ACCEPTANCE_REF
    try:
        root = repository_root()
        result = salvage(root, ref, main)
        if result.premerge is not None:
            for notice in result.notices:
                print(f"notice: {notice}", file=sys.stderr)
            print(f"memory salvage: {ref} merged the cut-over; run memory salvage {result.premerge}", file=sys.stderr)
            return EXIT_PREMERGE
        if check:
            for identifier in result.unsalvaged:
                print(f"{ref}: {identifier}: {result.kinds[identifier]} legacy row not yet salvaged")
            print(f"memory salvage --check: {len(result.unsalvaged)} unsalvaged row(s) on {ref} (merge-base {result.base[:12]})")
            return 1 if result.unsalvaged else 0
        for notice in result.notices:
            print(f"notice: {notice}", file=sys.stderr)
        for identifier, fields in result.unrevisable.items():
            print(f"needs a superseding record: {identifier}: changes {', '.join(fields)}, which a revision may not change; "
                  + "not proposed: propose a new record that supersedes it", file=sys.stderr)
        for old, new in result.conflicts:
            print(f"conflict: {new.id}: {main} holds other content under this ID, allocated or changed there since the merge-base "
                  + f"({old.legacy.path if old.legacy else '?'}); not proposed", file=sys.stderr)
        accepted = sorted((identifier for identifier, kind in result.kinds.items() if kind == "accepted"), key=id_key)
        print(f"memory salvage: {ref}: {len(result.proposals)} proposal(s), {len(result.unrevisable)} needing a superseding record, "
              + f"{len(result.conflicts)} conflict(s), {len(accepted)} row(s) already accepted "
              + f"(against {main} {result.main[:12]}'s {'journal' if result.source == 'journal' else 'legacy export'})", file=sys.stderr)
        if result.proposals:
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
    return 1 if result.conflicts else EXIT_SUPERSEDE if result.unrevisable else 0
