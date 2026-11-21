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
- ``revision``: the accepted record is unchanged since the merge-base and the row changes only what may change
  in place (``in_place``): the revision is built from the ACCEPTED record, keeping its ``legacy``, ``gaps`` and
  every immutable field, and takes appended notes, an unknown's next probe and a new status cell (stored
  ``retired`` for a ``retired ...`` cell, else the cell's vocabulary value, with a note quoting the cell
  verbatim). It names the accepted head (its ``rev``, every head of a fork) as ``prev``.
- ``superseded``: the only change is a status cell ``superseded-by F-N``: superseding writes no revision of the
  old record, so the successor carries the link, appended to its proposal in this change set or to a revision
  of its accepted record (that record is then a ``revision``).
- ``cosmetic``: only verbatim cells a revision follows changed, to no effect; nothing to propose.
- ``supersede``: the row changes a field or a legacy cell no revision may change (a claim, a cited ID).
  Never proposed, since ``check --ci`` would refuse it: ``unrevisable`` lists them, and the row needs a new
  record that supersedes the accepted one.
- ``undecided``: the rules give no clean answer, and ``undecided`` says why: a status cell outside the type's
  vocabulary, a supersession naming no successor or one neither proposable here nor accepted, or an
  unknown's ``resolved`` (resolving needs a finding stating ``resolves``; the row does not say which). Never proposed.
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
``EXIT_SUPERSEDE`` (4) some row needs a superseding record or a decision by hand (and none conflicts; the other proposals are
still printed or written); ``EXIT_PREMERGE`` (5) ``ref`` merged the cut-over: salvage the named pre-merge
parent instead (nothing is printed or written).
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

from tools.memory.check import APPEND_ONLY, REVISABLE
from tools.memory.gitio import GitError, git, run_git
from tools.memory.journal import JournalError, Revision, read_git, split_change_set, write_change_set
from tools.memory.legacy import SALVAGED, commit_time, export
from tools.memory.legacy_reader import CatalogError, has_legacy
from tools.memory.legacy_rules import MigrationError
from tools.memory.model import TYPES, Record, RecordError, as_mapping, id_key, load_json
from tools.memory.state import replay

ACCEPTANCE_REF = "origin/main"
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
    undecided: dict[str, str] = field(default_factory=dict[str, str])
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


def documents(records: dict[str, Record]) -> dict[str, Record]:
    """REF records by source path."""
    return {record.legacy.path: record for record in records.values() if record.type == "reference" and record.legacy is not None}


def unrevisable_fields(old: Record, new: Record) -> tuple[str, ...]:
    """Fields that differ although ``check``'s allow-list (``REVISABLE``, ``APPEND_ONLY``, status) does not let a revision change them."""
    before, after = old.to_json(), new.to_json()
    allowed = {"status", *APPEND_ONLY, *REVISABLE.get(new.type, ())}
    return tuple(name for name in sorted(before.keys() | after.keys()) if name not in allowed and before.get(name) != after.get(name))


# Legacy cells a revision may follow: the status cell (and the raw line holding it), an unknown's next probe.
IN_PLACE_CELLS = {"status", "original"}
# Keys of an evidence row (its ``row`` cell) a revision may follow: status, its note, its successor.
IN_PLACE_ROW_KEYS = {"status", "note", "superseded_by"}


@dataclass(frozen=True)
class InPlace:
    """What a branch's edit of an accepted migrated row comes to (``in_place``): the revision of the accepted record
    (``None`` when nothing in it changes), the successors its new status cell names, or why it cannot be proposed:
    ``unrevisable`` (fields or cells no revision may change: supersede) or ``undecided`` (the rules give no clean answer)."""

    revision: Record | None = None
    successors: tuple[str, ...] = ()
    unrevisable: tuple[str, ...] = ()
    undecided: str | None = None


def status_cell(record: Record) -> str | None:
    """The verbatim status cell of a migrated row: a table's ``status`` cell, an evidence row's ``status`` key."""
    if record.legacy is None:
        return None
    if record.type == "evidence":
        row = row_of(record)
        value = row.get("status") if row is not None else None
        return value if isinstance(value, str) else None
    value = record.legacy.cells.get("status")
    return value if isinstance(value, str) else None


def row_of(record: Record) -> dict[str, object] | None:
    """An evidence record's verbatim shard line (its ``row`` cell), parsed."""
    raw = record.legacy.cells.get("row") if record.legacy is not None else None
    try:
        return as_mapping(load_json(raw), "row") if isinstance(raw, str) else None
    except RecordError:
        return None


def changed_cells(old: Record, new: Record) -> tuple[str, ...]:
    """Legacy cells (and other ``legacy`` parts) that differ beyond what a revision may follow."""
    if old.legacy is None or new.legacy is None:
        return ("legacy",) if old.legacy != new.legacy else ()
    allowed = IN_PLACE_CELLS | ({"next_probe"} if new.type == "unknown" else set[str]())
    found = [f"legacy cell {name}" for name in sorted(old.legacy.cells.keys() | new.legacy.cells.keys())
             if name not in allowed and name != "row" and old.legacy.cells.get(name) != new.legacy.cells.get(name)]
    before, after = row_of(old), row_of(new)
    if before is not None or after is not None:
        keys = (before or {}).keys() | (after or {}).keys()
        found += [f"legacy row {key}" for key in sorted(keys) if key not in IN_PLACE_ROW_KEYS and (before or {}).get(key) != (after or {}).get(key)]
    if old.legacy.path != new.legacy.path:
        found.append("legacy path")
    return tuple(found)


def in_place(prev: Record, record: Record, ref: str) -> InPlace:
    """A branch's edit of the accepted migrated row ``prev`` as the rules allow it (``Revisions``): the revision keeps
    ``prev``'s ``legacy``, ``gaps`` and every immutable field and takes only what may change in place.

    - Any other field or legacy cell changed: ``unrevisable`` (a superseding record).
    - Appended notes, an unknown's next probe: taken.
    - A new status cell: ``superseded`` (by the IDs it names) is a successor's ``supersedes`` link, never a
      stored status (the caller decides whether the successor can carry it); ``retired ...`` is stored
      ``retired``; any other cell that maps to the type's vocabulary is that status. Either way a note quotes
      the cell verbatim. A cell outside the vocabulary, a supersession naming no ID and an unknown's
      ``resolved`` (which needs a finding stating ``resolves``, and the row does not say which) are ``undecided``."""
    fields = tuple(name for name in unrevisable_fields(prev, record) if name not in ("legacy", "gaps"))
    if blocked := fields or changed_cells(prev, record):
        return InPlace(unrevisable=blocked)
    data = prev.to_json()
    added = [note for note in record.notes if note not in prev.notes]
    for name in ("verified_on", "supersedes", "resolves"):
        before, after = prev.to_json().get(name), record.to_json().get(name)
        if before != after:
            if not isinstance(after, list) or (isinstance(before, list) and after[:len(before)] != before):
                return InPlace(unrevisable=(name,))
            data[name] = after
    if record.type == "unknown" and record.text("next_probe") != prev.text("next_probe"):
        data["next_probe"] = record.to_json().get("next_probe")
    successors: tuple[str, ...] = ()
    cell = status_cell(record)
    if cell is not None and cell != status_cell(prev):
        implied = record.legacy.implied_status if record.legacy is not None else None
        named = tuple(item for item in (record.legacy.superseded_by if record.legacy is not None else ()) if prev.legacy is None or item not in prev.legacy.superseded_by)
        statuses = TYPES[record.type].statuses
        if implied == "superseded":
            if not named:
                return InPlace(undecided=f"its status cell on {ref} says superseded but names no successor: {cell!r}")
            successors = named
        elif implied == "resolved":
            return InPlace(undecided=f"its status cell on {ref} says resolved; resolving needs a finding that states resolves, and the row does not say which")
        elif implied == "retired" and "retired" in statuses:
            data["status"] = "retired"
        elif record.status in statuses:
            data["status"] = record.status
        else:
            return InPlace(undecided=f"its status cell on {ref} is outside the {record.type} vocabulary: {cell!r}")
        if not successors:
            added.append(f"status cell on {ref}: {cell}")
    if added:
        data["notes"] = [*prev.notes, *added]
    revision = Record.from_json(data, prev.body) if data != prev.to_json() else None
    return InPlace(revision, successors)


def with_supersedes(record: Record, old: str) -> Record:
    """``record`` with ``old`` appended to its ``supersedes`` (unchanged if already there)."""
    if old in record.links.supersedes:
        return record
    data = record.to_json()
    data["supersedes"] = [*record.links.supersedes, old]
    return Record.from_json(data, record.body)


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
    successors: list[tuple[str, str]] = []
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
        if prev is not None:
            edit = in_place(prev, record, ref) if prev.legacy is not None else InPlace(
                record, unrevisable=unrevisable_fields(prev, record))
            if edit.unrevisable or edit.undecided is not None:
                result.kinds[identifier] = "supersede" if edit.unrevisable else "undecided"
                if edit.unrevisable:
                    result.unrevisable[identifier] = edit.unrevisable
                else:
                    result.undecided[identifier] = edit.undecided or ""
                superseding.append(Revision.of(record, heads))
                continue
            successors.extend((identifier, target) for target in edit.successors)
            if edit.revision is None:
                result.kinds[identifier] = "superseded" if edit.successors else "cosmetic"
                if not edit.successors:
                    result.notices.append(f"{identifier}: only its verbatim legacy cells changed on {ref}; nothing to propose")
                continue
            result.kinds[identifier] = "revision"
            result.proposals.append(Revision.of(edit.revision, heads))
            continue
        result.kinds[identifier] = "new"
        if record.type in ("compatibility", "report"):
            result.notices.append(f"{identifier}: new {record.type} row; its migration ID is positional and provisional")
        result.proposals.append(Revision.of(record, heads))
    # A status cell's "superseded-by F-N" is a supersedes link on the successor: on its proposal, or on a revision
    # of its accepted record; a successor neither proposed nor accepted (or itself not proposable) is undecided.
    for old, target in successors:
        index = next((number for number, revision in enumerate(result.proposals) if revision.record.id == target), None)
        if index is not None:
            proposal = result.proposals[index]
            result.proposals[index] = Revision.of(with_supersedes(proposal.record, old), proposal.prev)
        elif (held := accepted.get(target)) is not None and result.kinds.get(target) in (None, "accepted"):
            result.proposals.append(Revision.of(with_supersedes(held, old), on_accepted.heads[target]))
            result.kinds[target] = "revision"
        else:
            result.kinds[old] = "undecided"
            result.undecided[old] = f"its status cell on {ref} names successor {target}, which is neither proposable here nor accepted"
            superseding.append(Revision.of(after[old], on_accepted.heads.get(old, ())))
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
        for identifier, reason in result.undecided.items():
            print(f"needs a decision by hand: {identifier}: {reason}; not proposed", file=sys.stderr)
        for old, new in result.conflicts:
            print(f"conflict: {new.id}: {main} holds other content under this ID, allocated or changed there since the merge-base "
                  + f"({old.legacy.path if old.legacy else '?'}); not proposed", file=sys.stderr)
        accepted = sorted((identifier for identifier, kind in result.kinds.items() if kind == "accepted"), key=id_key)
        print(f"memory salvage: {ref}: {len(result.proposals)} proposal(s), {len(result.unrevisable)} needing a superseding record, "
              + f"{len(result.undecided)} needing a decision by hand, "
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
    return 1 if result.conflicts else EXIT_SUPERSEDE if result.unrevisable or result.undecided else 0
