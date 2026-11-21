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

A row the merge base did not hold is ``new`` (a first revision), ``accepted`` when the accepted commit holds
it (with any ``supersedes`` links salvage adds), else a ``conflict`` (both sides allocated the number). A row
the merge base held is planned cell by cell against that base row (``plan``), the plan applied to the
ACCEPTED record (``apply``: its ``legacy``, ``gaps`` and every immutable field are kept; ``gaps`` follow a
next probe filled or cleared), and checked by replay (``honoured``). The latest stored status wins over a
migrated record's legacy status (``state.legacy_holds``), so a status revision always shows.

| Changed in the row | Becomes |
| --- | --- |
| status cell naming a new successor (``superseded-by F-N``; an evidence row's ``superseded_by``) | a ``supersedes`` link on the successor: its proposal here, or a revision of its accepted record; the old row: ``superseded`` |
| status cell ``retired ...``, or a value of the type's vocabulary | a revision storing that status, with a note quoting the cell |
| status cell saying the same legacy status in other words; the raw line; the table framing; prose line numbers | ``cosmetic``: nothing to propose |
| an unknown's next probe (changed, cleared or filled); appended notes; appended ``verified_on``/``supersedes``/``resolves`` | a revision taking it |
| any other field or legacy cell (a claim, a cited ID, a label) | ``supersede``: needs a new record superseding the old one; never proposed |
| status cell outside the vocabulary; ``superseded`` with no new successor; an unknown's ``resolved`` (no finding states ``resolves``); a successor neither proposable here nor accepted; a legacy successor withdrawn without a new stored status; a row that cannot be read; an edit the replay would not show | ``undecided``: needs a decision by hand, with the reason; never proposed |

A plan the accepted record already holds (salvage's own output, merged) is ``accepted``; a plan whose base row
main changed otherwise since the merge base is a ``conflict`` (never proposed: a revision built from the
branch's row would silently undo main's change; rules, "Revisions"). One row never aborts the run.

Salvage must run on the branch's pre-merge tip. A branch that merged the cut-over (a merge commit in its
first-parent history since the merge base, one of whose parents is an ancestor of the accepted commit without
legacy trees) has a merge base with no legacy trees: resolving the modify/delete conflicts by deleting leaves
nothing to export, and keeping the files makes every row look changed. When the merge's other parent holds
changed legacy rows, ``premerge`` names that parent and nothing is classified: run ``memory salvage <premerge>``.

New compatibility and campaign rows get positional IDs at export, so their IDs are only provisional.
A document the branch changed (a spec, digest, ledger prose, the status file) is listed by path with
the accepted REF whose body to revise, since a REF's ``legacy`` never changes and its body is revised by hand.
``unsalvaged`` holds every proposal, conflict, ``supersede`` or ``undecided`` row whose content (ID and sha256)
is not yet in ``ref``'s own journal.

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
from collections.abc import Mapping, Sequence
from typing import Protocol

from tools.memory.check import APPEND_ONLY, REVISABLE
from tools.memory.gitio import GitError, git, run_git
from tools.memory.journal import JournalError, Revision, Segment, read_git, split_change_set, write_change_set
from tools.memory.legacy import SALVAGED, commit_time, export
from tools.memory.legacy_reader import CatalogError, has_legacy
from tools.memory.legacy_rules import MigrationError
from tools.memory.model import JSON, TYPES, Record, RecordError, as_mapping, id_key, load_json, regap
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
    segments: tuple[Segment, ...] = ()


def accepted_on(root: Path, main: str) -> Accepted:
    """``main``'s replayed journal when it has one; else its legacy export, as the cut-over will journal it."""
    if segments := read_git(root, main):
        state = replay(segments)
        return Accepted({identifier: entry.record for identifier, entry in state.entries.items()},
                        {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal", segments)
    records = export(root, main).records()
    revisions = tuple(Revision.of(record) for record in records.values())
    return Accepted(records, {revision.record.id: (revision.rev,) for revision in revisions}, "legacy",
                    (Segment("memory/journal/accepted-legacy-export.jsonl", "", revisions),))


def documents(records: dict[str, Record]) -> dict[str, Record]:
    """REF records by source path."""
    return {record.legacy.path: record for record in records.values() if record.type == "reference" and record.legacy is not None}


def unrevisable_fields(old: Record, new: Record) -> tuple[str, ...]:
    """Fields that differ although ``check``'s allow-list (``REVISABLE``, ``APPEND_ONLY``, status) does not let a revision change them."""
    before, after = old.to_json(), new.to_json()
    allowed = {"status", *APPEND_ONLY, *REVISABLE.get(new.type, ())}
    return tuple(name for name in sorted(before.keys() | after.keys()) if name not in allowed and before.get(name) != after.get(name))


# Legacy cells whose change is not a change of the row: the raw line holding the cells, the table's framing
# (heading and preamble, a document change), and prose supersession cells, whose key carries a line number
# and whose effect is ``legacy.superseded_by`` (compared as successors).
FOLLOWED_CELLS = {"original", "table"}
# Evidence row keys a plan follows: status, its note, its successor.
FOLLOWED_ROW_KEYS = {"status", "note", "superseded_by"}


@dataclass(frozen=True)
class Edit:
    """What a branch's edit of a migrated row says, relative to the row at the merge base (``plan``): a stored
    status to write, notes to append, an unknown's next probe (``probe`` set; ``None`` clears it), appended
    links, successors to link, or why it cannot be proposed (``blocked``: supersede; ``undecided``: by hand)."""

    status: str | None = None
    notes: tuple[str, ...] = ()
    probe: bool = False
    next_probe: str | None = None
    appended: tuple[tuple[str, tuple[str, ...]], ...] = ()
    successors: tuple[str, ...] = ()
    blocked: tuple[str, ...] = ()
    undecided: str | None = None

    @property
    def empty(self) -> bool:
        return not (self.status or self.notes or self.probe or self.appended or self.successors)


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
    """Legacy cells and row keys that differ beyond what a plan follows (``FOLLOWED_CELLS``, prose cells, the
    status cell, an unknown's next probe, ``FOLLOWED_ROW_KEYS``)."""
    if old.legacy is None or new.legacy is None:
        return ("legacy",) if old.legacy != new.legacy else ()
    followed = FOLLOWED_CELLS | {"status", "row"} | ({"next_probe"} if new.type == "unknown" else set[str]())
    found = [f"legacy cell {name}" for name in sorted(old.legacy.cells.keys() | new.legacy.cells.keys())
             if name not in followed and not name.startswith("prose ") and old.legacy.cells.get(name) != new.legacy.cells.get(name)]
    before, after = row_of(old) or {}, row_of(new) or {}
    found += [f"legacy row {key}" for key in sorted(before.keys() | after.keys()) if key not in FOLLOWED_ROW_KEYS and before.get(key) != after.get(key)]
    if old.legacy.path != new.legacy.path:
        found.append("legacy path")
    return tuple(found)


def plan(origin: Record, record: Record, ref: str) -> Edit:
    """The branch's edit of the migrated row ``origin`` (as of the merge base), cell by cell (module docstring table)."""
    fields = tuple(name for name in unrevisable_fields(origin, record) if name not in ("legacy", "gaps"))
    if blocked := fields or changed_cells(origin, record):
        return Edit(blocked=blocked)
    appended: list[tuple[str, tuple[str, ...]]] = []
    for name in ("verified_on", "supersedes", "resolves"):
        before, after = origin.to_json().get(name), record.to_json().get(name)
        if before != after:
            if not isinstance(after, list) or (isinstance(before, list) and after[:len(before)] != before):
                return Edit(blocked=(name,))
            appended.append((name, tuple(str(item) for item in after[len(before) if isinstance(before, list) else 0:])))
    notes = [note for note in record.notes if note not in origin.notes]
    probe = record.type == "unknown" and record.text("next_probe") != origin.text("next_probe")
    old_edges = origin.legacy.superseded_by if origin.legacy is not None else ()
    new_edges = record.legacy.superseded_by if record.legacy is not None else ()
    withdrawn = [item for item in old_edges if item not in new_edges]
    successors = tuple(item for item in new_edges if item not in old_edges)
    status: str | None = None
    cell = status_cell(record)
    if cell is not None and cell != status_cell(origin) and not successors:
        implied = record.legacy.implied_status if record.legacy is not None else None
        statuses = TYPES[record.type].statuses
        was = origin.legacy.implied_status if origin.legacy is not None else None
        if implied in ("superseded", "resolved") and implied == was:
            pass  # the same legacy status in other words
        elif implied == "superseded":
            return Edit(undecided=f"its status cell on {ref} says superseded but names no new successor: {cell!r}")
        elif implied == "resolved":
            return Edit(undecided=f"its status cell on {ref} says resolved; resolving needs a finding that states resolves, and the row does not say which")
        elif implied == "retired" and "retired" in statuses:
            status = "retired"
        elif record.status in statuses:
            status = record.status
        else:
            return Edit(undecided=f"its status cell on {ref} is outside the {record.type} vocabulary: {cell!r}")
        if status is not None:
            notes.append(f"status cell on {ref}: {cell}")
    # A legacy edge holds while the stored status is the migrated one (``state.legacy_holds``): only a new stored status withdraws it.
    if withdrawn and (status is None or status == origin.status):
        return Edit(undecided=f"its row on {ref} withdraws the legacy successor {', '.join(withdrawn)} without a new stored status; "
                    + "a revision cannot withdraw a legacy edge")
    return Edit(status, tuple(notes), probe, record.text("next_probe") if probe else None, tuple(appended), successors)


def apply(edit: Edit, base: Record) -> Record | None:
    """``base`` with ``edit`` applied (only what may change in place; ``gaps`` follow a filled or cleared next
    probe); ``None`` when ``base`` already holds all of it."""
    data = base.to_json()
    if edit.status is not None:
        data["status"] = edit.status
    if missing := [note for note in edit.notes if note not in base.notes]:
        data["notes"] = [*base.notes, *missing]
    if edit.probe:
        if edit.next_probe is None:
            _ = data.pop("next_probe", None)
        else:
            data["next_probe"] = edit.next_probe
        regap(data, "next_probe")
    for name, items in edit.appended:
        held = data.get(name)
        current = [item for item in held if isinstance(item, (str, dict))] if isinstance(held, list) else list[JSON]()
        added = [item for item in items if item not in current]
        if added:
            data[name] = [*current, *added]
    return None if data == base.to_json() else Record.from_json(data, base.body)


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


def honoured(on_accepted: Accepted, proposals: Sequence[Revision], edits: Mapping[str, Edit]) -> dict[str, str]:
    """Replay the accepted journal with and without ``proposals``: every edit must show. A status edit must
    derive its status, a successor must derive ``superseded`` on the old row, and every proposal must change
    its record's head. Returns the IDs that would replay to no change, with why (never expected: a guard)."""
    accepted_state = replay(on_accepted.segments)
    proposed = replay([*on_accepted.segments, Segment("memory/journal/salvage-proposals.jsonl", "", tuple(proposals))])
    found: dict[str, str] = {}
    for revision in proposals:
        identifier = revision.record.id
        before = accepted_state.entries.get(identifier)
        if before is not None and before.record == proposed.entries[identifier].record:
            found[identifier] = "its revision would replay to no change"
    for identifier, edit in edits.items():
        if identifier not in proposed.entries:
            continue
        shown = proposed.status(identifier)
        if edit.status is not None and shown not in (edit.status, "superseded", "resolved"):
            found[identifier] = f"its status {edit.status} would replay as {shown}"
        if edit.successors and shown != "superseded" and any(revision.record.id in edit.successors for revision in proposals):
            found[identifier] = f"its successors {', '.join(edit.successors)} would not supersede it on replay"
    return found


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
    edits: dict[str, Edit] = {}
    fresh: list[str] = []

    def hold(identifier: str, kind: str, reason: str | tuple[str, ...]) -> None:
        """A row salvage does not propose: ``supersede`` with the fields, or ``undecided`` with the reason."""
        result.kinds[identifier] = kind
        if isinstance(reason, tuple):
            result.unrevisable[identifier] = reason
        else:
            result.undecided[identifier] = reason
        superseding.append(Revision.of(after[identifier], on_accepted.heads.get(identifier, ())))

    for identifier in sorted(after, key=id_key):
        record = after[identifier]
        if record.type not in SALVAGED or before.get(identifier) == record:
            continue
        on_main, origin = accepted.get(identifier), before.get(identifier)
        if on_main == record:
            result.kinds[identifier] = "accepted"
        elif origin is None:
            fresh.append(identifier)
        elif origin.legacy is None:
            hold(identifier, "supersede", unrevisable_fields(origin, record) or ("content",))
        else:
            try:
                edit = plan(origin, record, ref)
            except RecordError as error:
                hold(identifier, "undecided", f"its edit on {ref} cannot be read: {error}")
                continue
            if (edit.blocked or edit.undecided is not None) and on_main is not None and on_main != origin:
                # Main changed the row since the merge base too: reconcile by hand before anything else.
                result.kinds[identifier] = "conflict"
                result.conflicts.append((on_main, record))
            elif edit.blocked:
                hold(identifier, "supersede", edit.blocked)
            elif edit.undecided is not None:
                hold(identifier, "undecided", edit.undecided)
            else:
                edits[identifier] = edit
    # The successors each edit links: target -> the old rows it supersedes.
    links: dict[str, list[str]] = {}
    for identifier, edit in edits.items():
        for target in edit.successors:
            links.setdefault(target, []).append(identifier)
    for identifier in fresh:
        record, on_main = after[identifier], accepted.get(identifier)
        linked = record
        for old in links.get(identifier, ()):
            linked = with_supersedes(linked, old)
        if on_main is None:
            result.kinds[identifier] = "new"
            if record.type in ("compatibility", "report"):
                result.notices.append(f"{identifier}: new {record.type} row; its migration ID is positional and provisional")
            result.proposals.append(Revision.of(linked))
        elif on_main in (record, linked):
            result.kinds[identifier] = "accepted"
        else:
            result.kinds[identifier] = "conflict"
            result.conflicts.append((on_main, record))
    linking: list[tuple[str, list[str]]] = []
    for identifier, edit in edits.items():
        record, origin, on_main = after[identifier], before[identifier], accepted.get(identifier)
        # A row main's legacy export lost names the first revision the cut-over would have journalled.
        base, heads = (on_main, on_accepted.heads[identifier]) if on_main is not None else (origin, (Revision.of(origin).rev,))
        try:
            revision = apply(edit, base)
        except RecordError as error:
            hold(identifier, "undecided", f"its edit on {ref} does not make a valid revision: {error}")
            continue
        pending = [target for target in edit.successors
                   if (held := accepted.get(target)) is None or identifier not in held.links.supersedes]
        if edit.empty:
            result.kinds[identifier] = "cosmetic"
            result.notices.append(f"{identifier}: only verbatim legacy text changed on {ref}; nothing to propose")
        elif revision is None and not pending:
            result.kinds[identifier] = "accepted"
        elif on_main is not None and on_main != origin and on_main != apply(edit, origin):
            result.kinds[identifier] = "conflict"
            result.conflicts.append((on_main, record))
        else:
            result.kinds[identifier] = "revision" if revision is not None else "superseded"
            if revision is not None:
                result.proposals.append(Revision.of(revision, heads))
            if pending:
                linking.append((identifier, pending))
    # The successor carries the link: on its proposal here, or on a revision of its accepted record.
    for identifier, pending in linking:
        if stuck := [target for target in pending if not any(item.record.id == target for item in result.proposals)
                     and (target not in accepted or result.kinds.get(target) not in (None, "accepted"))]:
            result.proposals = [item for item in result.proposals if item.record.id != identifier]
            hold(identifier, "undecided", f"its row on {ref} names successor {', '.join(stuck)}, neither proposable here nor accepted")
            continue
        for target in pending:
            index = next((number for number, item in enumerate(result.proposals) if item.record.id == target), None)
            if index is not None:
                proposal = result.proposals[index]
                result.proposals[index] = Revision.of(with_supersedes(proposal.record, identifier), proposal.prev)
            else:
                result.proposals.append(Revision.of(with_supersedes(accepted[target], identifier), on_accepted.heads[target]))
                result.kinds[target] = "revision"
    no_op = honoured(on_accepted, result.proposals, edits)
    for identifier, reason in no_op.items():
        result.proposals = [item for item in result.proposals if item.record.id != identifier]
        hold(identifier, "undecided", reason)
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
