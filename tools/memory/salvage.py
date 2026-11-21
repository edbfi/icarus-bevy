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
migrated record's legacy status (``Entry.legacy_holds``), so a status revision always shows.

| Changed in the row | Becomes |
| --- | --- |
| status cell naming a new successor (``superseded-by F-N``; an evidence row's ``superseded_by``) | a ``supersedes`` link on the successor: its proposal here, or a revision of its accepted record; the old row: ``superseded`` |
| status cell ``retired ...``, or a value of the type's vocabulary | a revision storing that status, with a note quoting the cell (``STATUS_NOTE``) |
| status cell saying the same legacy status in other words; the raw line; the table framing; prose line numbers | ``cosmetic``: nothing to propose |
| an unknown's next probe (changed, cleared or filled); appended notes; appended ``verified_on``/``supersedes``/``resolves`` | a revision taking it |
| any other field or legacy cell (a claim, a cited ID, a label) | ``supersede``: needs a new record superseding the old one; never proposed |
| status cell outside the vocabulary; ``superseded`` with no new successor; an unknown's ``resolved`` (no finding states ``resolves``); a successor neither proposable here nor accepted; a legacy successor withdrawn without a new stored status; a row that cannot be read; an edit the replay would not show | ``undecided``: needs a decision by hand, with the reason; never proposed |

A plan the accepted record already holds (salvage's own output, merged) is ``accepted``; a plan whose base row
main changed otherwise since the merge base is a ``conflict`` (never proposed: a revision built from the
branch's row would silently undo main's change; rules, "Revisions"). One row never aborts the run. The proposal
set is rebuilt until nothing more is held (``settle``), and ``invariants`` checks the end state before anything
is returned: no proposal links to a row this run does not salvage, the proposals replay with no new problem, and
every changed row is in exactly one class; a breach is a ``SalvageError``, never output.

Salvage must run on the branch's pre-merge tip. A branch that merged the cut-over (a merge commit in its
first-parent history since the merge base, one of whose parents is an ancestor of the accepted commit without
legacy trees) has a merge base with no legacy trees: resolving the modify/delete conflicts by deleting leaves
nothing to export, and keeping the files makes every row look changed. The merge's other parent is salvaged
against the accepted commit as the normal path does: if anything is left (a proposal, a held or conflicting row,
a changed document), ``premerge`` names that parent and nothing is classified: run ``memory salvage <premerge>``;
if all of it is accepted (its salvage merged first, as the runbook does), nothing is left to salvage.

Migration-assigned IDs (REF, CP, RP) are positional in an export, so a branch that adds or removes a document or a
table row numbers the rest differently from the accepted journal. Before anything is compared, both exports are
translated to the accepted journal's IDs by identity (``renumber``: a REF by its legacy path, a CP row by its name,
an RP row by its reported problem), in record IDs, links and citations. A record with no accepted counterpart gets a
provisional number above every accepted one: a row citing it is held until that record is accepted, and a new CP or
RP row is proposed (its number provisional) only if nothing on the branch cites it. Records sharing an identity in
one export (two CP rows of one name) are never mapped: each gets its own provisional number and is held, with every
row citing one.
A document the branch changed (a spec, digest, ledger prose, the status file) is listed by path with
the accepted REF whose body to revise, since a REF's ``legacy`` never changes and its body is revised by hand.
``unsalvaged`` holds every proposal, conflict, ``supersede`` or ``undecided`` row whose content (ID and sha256)
is not yet in ``ref``'s own journal.

The command prints the proposal change set as segment lines, or writes it under ``DIR/memory/journal`` with
``--out``; notices, conflicts and rows needing a superseding record go to stderr. ``--check`` prints each
unsalvaged row and each pending document (its path and the REF to revise; done when the accepted body equals the
branch's, or for a new document when an accepted REF holds its body) and exits 1 if there is any. ``--out`` also
writes each pending document's body, exactly as the exporter gives its REF at the salvaged tip, under
``DIR/documents/<legacy path>``, and prints the ``memory revise`` or ``memory propose reference`` command that
uses it.

Exit status: 0 success; 1 a conflict or an error; 2 usage; ``EXIT_SUPERSEDE`` (4) some row needs a superseding
record or a decision by hand (and none conflicts; the other proposals are still printed or written);
``EXIT_PREMERGE`` (5) ``ref`` merged the cut-over: salvage the named pre-merge parent instead (nothing is printed
or written).
"""

from __future__ import annotations

import argparse
import os
import re
import shlex
import stat
import sys
from collections.abc import Callable, Collection, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

from tools.memory.check import APPEND_ONLY, REVISABLE, revision_problems
from tools.memory.gitio import GitError, git, run_git
from tools.memory.journal import JournalError, Revision, Segment, read_git, split_change_set
from tools.memory.legacy import SALVAGED, TRACKED_NOTE, commit_time, export
from tools.memory.legacy_reader import CatalogError, has_legacy
from tools.memory.legacy_rules import MigrationError
from tools.memory.model import ID, JSON, TYPES, Record, RecordError, as_mapping, id_key, load_json, regap
from tools.memory.state import State, replay

ACCEPTANCE_REF = "origin/main"
EXIT_SUPERSEDE = 4
# The note a REF proposed with salvage's printed command carries: the legacy document it stands for (its identity).
DOCUMENT_NOTE = "salvaged document: {}"
DOCUMENT_NOTED = re.compile(r"salvaged document: (\S+)")
# A new document's printed command carries this in place of a scope: not JSON, so the tool refuses it until replaced.
SCOPE_PLACEHOLDER = "<STATE THE SCOPE>"
# The note a status revision carries: the row's new status cell, verbatim, and nothing about the run.
STATUS_NOTE = "status cell (salvaged legacy row): {}"
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
    # Pending documents: (path, the REF whose body to revise, or None for a new document).
    documents: list[Pending] = field(default_factory=list["Pending"])


@dataclass(frozen=True)
class Pending:
    """A document the branch changed or added: its legacy path, the accepted REF whose body to revise (``None`` for
    a new document), and the body and title the exporter gives its REF at the salvaged tip."""

    path: str
    target: str | None
    body: str
    title: str
    names: tuple[str, ...] = ()

    def command(self, file: str) -> str:
        """What the operator runs once ``--out`` wrote the body to ``file``. For a new document the scope is the
        operator's statement: the command carries ``SCOPE_PLACEHOLDER``, which the tool's parser refuses (not JSON),
        so it cannot run until they replace it; ``scope_line`` says what the exporter read from the text."""
        if self.target is not None:
            return f"memory revise {self.target} --body {shlex.quote(file)}"
        return (f"memory propose reference --set {shlex.quote('title=' + self.title)} --set-json {shlex.quote('scope=' + SCOPE_PLACEHOLDER)}"
                + f" --note {shlex.quote(DOCUMENT_NOTE.format(self.path))} --body {shlex.quote(file)}")

    def scope_line(self) -> str:
        """For a new document: who states the scope, and the build, release and commit tokens its text names."""
        named = f"the text names: {', '.join(self.names)}" if self.names else "the text names no build, release or commit"
        return (f"the scope must be stated by whoever proposes it: replace {SCOPE_PLACEHOLDER} with a JSON scope"
                + f" (salvage neither keeps nor upgrades the text's tokens); {named}")

    def line(self) -> str:
        """The pending document without ``--out``: no path is named that does not exist."""
        if self.target is not None:
            return (f"changed document; revise {self.target} with the body salvage --out writes (listed until the accepted body equals the"
                    + " branch's; a body merged by hand into other text stays listed)")
        return ("new document; salvage --out writes its body and the memory propose reference command (listed until an accepted REF names this"
                + " path, by legacy path or that command's note, with this body)")


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


def documents(records: Mapping[str, Record]) -> dict[str, Record]:
    """REF records by the document they stand for (``document_path``)."""
    return {path: record for record in records.values() if (path := document_path(record)) is not None}


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
    """The branch's edit of the migrated row ``origin`` (as of the merge base), cell by cell (module docstring table).
    ``ref`` (as typed) names the branch in reasons only: nothing stored in a record depends on how the ref was
    spelled or which commit was the tip (``STATUS_NOTE``), so a salvaged revision is the same on every run."""
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
            notes.append(STATUS_NOTE.format(cell))
    # A legacy edge holds while the stored status is the migrated one (``Entry.legacy_holds``): only a new stored status withdraws it.
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
    """Replay the accepted journal with and without ``proposals``: the proposals must replay cleanly and every edit
    must show. Any replay, revision, link or cycle problem the accepted journal alone does not have (``check
    --ci``'s tests) fails the proposals whose IDs it names, and all of them when it names none. A status edit must
    derive its status, an edit naming successors must derive ``superseded``, and every proposal must change its
    record's head. Returns the IDs that fail, with why; ``salvage`` repeats it until nothing fails."""
    accepted_state = replay(on_accepted.segments)
    proposed = replay([*on_accepted.segments, Segment("memory/journal/salvage-proposals.jsonl", "", tuple(proposals))])
    found: dict[str, str] = {}
    ids = {revision.record.id for revision in proposals}

    def problems(state: State) -> list[str]:
        return [*state.problems, *revision_problems(state), *state.link_problems(),
                *(f"supersession cycle {cycle}" for cycle in state.supersession_cycles())]

    known = set(problems(accepted_state))
    for problem in problems(proposed):
        if problem in known:
            continue
        named = sorted({match[0] for match in ID.finditer(problem)} & ids, key=id_key)
        for identifier in named or sorted(ids, key=id_key):
            _ = found.setdefault(identifier, f"it would not replay cleanly: {problem}")
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
        if edit.successors and shown != "superseded":
            found[identifier] = f"its successors {', '.join(edit.successors)} would not supersede it on replay"
    return found


def without_supersedes(record: Record, old: str) -> Record:
    """``record`` with ``old`` removed from its ``supersedes``."""
    data = record.to_json()
    if kept := [item for item in record.links.supersedes if item != old]:
        data["supersedes"] = list[JSON](kept)
    else:
        _ = data.pop("supersedes", None)
    return Record.from_json(data, record.body)


class SalvageError(RuntimeError):
    """Salvage's output broke one of its invariants: an internal error, never a result to write."""


def settle(result: Salvage, own: Mapping[str, Revision], pending_links: Mapping[str, list[str]], edits: Mapping[str, Edit],
           on_accepted: Accepted, ref: str, hold: Callable[[str, str, str], None]) -> tuple[list[Revision], dict[str, str]]:
    """The proposal set, rebuilt from scratch until nothing more is held: each row's own proposal, then every
    ``supersedes`` link an edit still in play (``revision``/``superseded``) names, on its successor's proposal or a
    revision of its accepted record; a link to a successor held, in conflict or unable to carry it holds the old row;
    then the replay check (``honoured``) holds what fails. The held set only grows, so it ends; nothing is patched in
    place, so no order of discovery leaves a stale link. Returns the proposals and the accepted records that could
    not carry links, with why."""
    accepted = on_accepted.records
    unlinkable: dict[str, str] = {}
    playing = ("revision", "superseded")
    while True:
        proposals = {identifier: revision for identifier, revision in own.items() if result.kinds.get(identifier) not in ("supersede", "undecided")}
        carriers: dict[str, Revision] = {}
        stuck: dict[str, str] = {}
        for old, targets in pending_links.items():
            if result.kinds.get(old) not in playing:
                continue
            for target in targets:
                if target in proposals or target in carriers:
                    base = proposals.get(target) or carriers[target]
                    linked = Revision.of(with_supersedes(base.record, old), base.prev)
                    if target in proposals:
                        proposals[target] = linked
                    else:
                        carriers[target] = linked
                elif target in accepted and target not in unlinkable and result.kinds.get(target) in (None, "accepted", "cosmetic", "revision", "superseded"):
                    carriers[target] = Revision.of(with_supersedes(accepted[target], old), on_accepted.heads[target])
                else:
                    why = unlinkable.get(target) or result.undecided.get(target) or result.kinds.get(target) or "not proposable here nor accepted"
                    _ = stuck.setdefault(old, f"its row on {ref} names successor {target}, which cannot carry the link ({why})")
        if stuck:
            for old, reason in stuck.items():
                hold(old, "undecided", reason)
            continue
        everything = [*proposals.values(), *carriers.values()]
        failing = honoured(on_accepted, everything, {identifier: edit for identifier, edit in edits.items() if result.kinds.get(identifier) in playing})
        if not failing:
            for target in carriers:
                if result.kinds.get(target) in (None, "accepted", "cosmetic", "superseded"):
                    result.kinds[target] = "revision"
            return everything, unlinkable
        for identifier, reason in failing.items():
            if identifier in carriers:
                unlinkable[identifier] = reason
            else:
                hold(identifier, "undecided", reason)


def invariants(result: Salvage, changed: Sequence[str], on_accepted: Accepted, after: Mapping[str, Record] | None = None) -> None:
    """Raise ``SalvageError`` (naming the row) unless (a) no proposal links to a row this run holds, reports as a
    conflict or leaves unproposed, unless the accepted journal holds it (and an added ``supersedes``/``resolves``
    names only a row in play); (b) replaying the accepted journal with the proposals adds no problem; (c) every
    changed row is in exactly one class, its proposal present exactly when the class has one; (d) every notice
    about a successor's link agrees with that successor's proposal (absent, with the link, or without it); (e) no
    proposal carries a migration-assigned ID (REF, CP, RP) that is not the accepted journal's ID for that identity; (f) no
    proposal cites a REF, CP or RP whose accepted identity differs from that of the branch record it stood for."""
    accepted = on_accepted.records
    proposed = {revision.record.id: revision.record for revision in result.proposals}
    if len(proposed) != len(result.proposals):
        raise SalvageError("two proposals for one ID")
    out = {name for name, kind in result.kinds.items() if kind in ("supersede", "undecided", "conflict")}
    for identifier, record in proposed.items():
        base = accepted.get(identifier)
        for name in ("supersedes", "resolves"):
            added = set(record.links.named(name)) - set(base.links.named(name) if base is not None else ())
            if bad := sorted(target for target in added if result.kinds.get(target) not in ("revision", "superseded")):
                raise SalvageError(f"{identifier}: {name} {', '.join(bad)}, which this run does not salvage")
        for target in (*record.links.refs, *(cite.split("/", 1)[0] for cite in record.links.cites)):
            if target in out and target not in accepted and target not in proposed:
                raise SalvageError(f"{identifier}: links {target}, which this run holds or reports as a conflict")
    if found := honoured(on_accepted, result.proposals, {}):
        identifier, reason = next(iter(found.items()))
        raise SalvageError(f"{identifier}: {reason}")
    with_proposal = ("new", "revision")
    without = ("superseded", "accepted", "cosmetic", "supersede", "undecided", "conflict")
    conflicts = {new.id for _old, new in result.conflicts}
    for identifier in changed:
        kind = result.kinds.get(identifier)
        if kind not in (*with_proposal, *without):
            raise SalvageError(f"{identifier}: changed on the branch but in no class")
        if (kind in with_proposal) != (identifier in proposed):
            raise SalvageError(f"{identifier}: classed {kind} but {'not ' if identifier not in proposed else ''}proposed")
        if (kind == "conflict") != (identifier in conflicts) or (kind in ("supersede", "undecided")) != (identifier in result.unrevisable or identifier in result.undecided):
            raise SalvageError(f"{identifier}: classed {kind} but reported otherwise")
        if kind == "superseded" and not any(identifier in record.links.supersedes for record in proposed.values()):
            raise SalvageError(f"{identifier}: classed superseded but no proposal supersedes it")
    for identifier in proposed:
        if result.kinds.get(identifier) not in with_proposal:
            raise SalvageError(f"{identifier}: proposed but classed {result.kinds.get(identifier)}")
    # (e) No proposal carries a migration-assigned ID other than the accepted journal's (a new CP or RP row's own provisional ID aside).
    for identifier, record in proposed.items():
        assigned = [name for name in (*record.links.targets(), *(record.legacy.superseded_by if record.legacy is not None else ()))
                    if name.split("-", 1)[0] in ASSIGNED and name not in accepted]
        if identifier.split("-", 1)[0] in ASSIGNED and identifier not in accepted and result.kinds.get(identifier) != "new":
            assigned.append(identifier)
        if assigned:
            raise SalvageError(f"{identifier}: carries {', '.join(assigned)}, not the accepted journal's ID for that identity")
    # (f) A proposal's REF, CP or RP link names the accepted record of the identity the branch's record had.
    for identifier, record in proposed.items():
        for target in record.links.targets():
            if after is not None and target in after and target in accepted and target.split("-", 1)[0] in ASSIGNED \
                    and identity(after[target]) != identity(accepted[target]):
                raise SalvageError(f"{identifier}: cites {target}, which on main is {identity(accepted[target])}, not the branch's {identity(after[target])}")
    # (d) A notice about a link agrees with the links of the proposal it names.
    for notice in result.notices:
        claim = re.fullmatch(r"([A-Z]+-[0-9]+): ([A-Z]+-[0-9]+) would supersede it, but .*; \2 (is proposed without that link|is not revised"
                             + r"|keeps the link its accepted record already holds)", notice)
        if claim is None:
            continue
        old, target, outcome = claim[1], claim[2], claim[3]
        record = proposed.get(target)
        holds = record is not None and old in record.links.supersedes
        if (outcome == "is not revised") != (record is None) or (record is not None and holds != outcome.startswith("keeps")):
            raise SalvageError(f"{target}: a notice says it {outcome} about {old}, which its proposal contradicts")


def salvage(root: Path, ref: str, main: str = ACCEPTANCE_REF) -> Salvage:
    head = commit(root, ref)
    accepted_commit = commit(root, main)
    base = merge_base(root, accepted_commit, head)
    result = Salvage(ref, head, base, accepted_commit)
    if (merged := cut_over_merge(root, head, base, accepted_commit)) is not None:
        merge, parent = merged
        # The pre-merge tip, salvaged against the accepted commit as the normal path does: anything left to do
        # (a proposal, a row held or in conflict, a changed document) refuses; all of it accepted means done.
        left = salvage(root, parent, main)
        if left.proposals or left.conflicts or left.unrevisable or left.undecided or left.documents or left.premerge:
            result.merge, result.premerge = merge, parent
            result.notices.append(f"{ref} merged the cut-over in {merge[:12]}; its legacy rows are on the pre-merge tip {parent}: "
                                  + f"salvage must run there (memory salvage {parent})")
        else:
            result.notices.append(f"{ref} merged the cut-over in {merge[:12]}; its pre-merge tip {parent[:12]} leaves nothing to salvage")
        return result
    if not has_legacy(root, head):
        result.notices.append(f"{ref} has no legacy ledgers or evidence index at its tip; nothing to salvage")
        return result
    after = export(root, head).records()
    before = export(root, base).records() if has_legacy(root, base) else {}
    on_accepted = accepted_on(root, accepted_commit)
    raw = (after, before)
    (after, before), unmapped, shared = renumber((after, before), on_accepted.records)
    superseding = classify(result, after, before, on_accepted, unmapped, shared, raw)
    accepted = on_accepted.records
    main_refs, base_bodies = documents(accepted), {path: record.body for path, record in documents(before).items()}
    # A document is matched by identity only (its legacy path, or a REF's "salvaged document:" note), never by body.
    for path, record in sorted(documents(after).items()):
        on_main = main_refs.get(path)
        body, title = record.body or "", record.text("title") or path
        if on_main is None:
            result.documents.append(Pending(path, None, body, title, record.scope.tokens if record.scope is not None else ()))
            result.notices.append(f"{path}: {result.documents[-1].line()}")
        elif record.body not in (on_main.body, base_bodies.get(path)):
            result.documents.append(Pending(path, on_main.id, body, title))
            result.notices.append(f"{path}: {result.documents[-1].line()}")
    journal = {(revision.record.id, revision.sha256) for segment in read_git(root, head) for revision in segment.revisions}
    result.unsalvaged = [revision.record.id for revision in [*result.proposals, *superseding] if (revision.record.id, revision.sha256) not in journal]
    result.unsalvaged += [new.id for _old, new in result.conflicts]
    return result


# Migration-assigned IDs are positional in an export: a branch that adds a document or a table row numbers the rest
# differently from the accepted journal. They are translated by identity before anything is compared.
ASSIGNED = ("REF", "CP", "RP")


def document_path(record: Record) -> str | None:
    """The legacy document a REF stands for: its legacy path, or (a REF proposed with the command salvage prints) the
    path its ``DOCUMENT_NOTE`` names; nothing else, never its body."""
    if record.type != "reference":
        return None
    if record.legacy is not None:
        return record.legacy.path
    named = [match[1] for note in record.notes if (match := DOCUMENT_NOTED.fullmatch(note))]
    return named[0] if len(named) == 1 else None


def identity(record: Record) -> str | None:
    """What a migration-assigned record is, whatever its number: a REF its document (``document_path``), a CP row its
    name (its first cell, the Surface or Subsurface), an RP row its reported problem."""
    if record.type == "reference":
        return f"document {path}" if (path := document_path(record)) is not None else None
    if record.type == "compatibility":
        return f"compatibility row {record.text('name')!r}"
    if record.type == "report":
        return f"report row {record.text('problem')!r}"
    return None


def name_of(record: Record) -> str:
    """The cell a CP or RP row is known by (its identity, without the table)."""
    return record.text("name") or record.text("problem") or record.id


def relink(record: Record, mapping: Mapping[str, str]) -> Record:
    """``record`` with its ID and every linked ID (``supersedes``, ``resolves``, ``refs``, ``cites``, legacy successors, and the
    exporter's ``TRACKED_NOTE`` naming a REF) mapped."""
    targets = {record.id, *record.links.targets(), *(record.legacy.superseded_by if record.legacy is not None else ())}
    if not targets & mapping.keys():
        return record
    data = record.to_json()
    data["id"] = mapping.get(record.id, record.id)
    for name in ("supersedes", "resolves", "refs"):
        if isinstance(items := data.get(name), list):
            data[name] = [mapping.get(item, item) if isinstance(item, str) else item for item in items]
    if isinstance(items := data.get("cites"), list):
        data["cites"] = [mapping.get(item.split("/", 1)[0], item.split("/", 1)[0]) + item[len(item.split("/", 1)[0]):] if isinstance(item, str) else item
                         for item in items]
    # The exporter's own note naming the REF a tracked file became carries the number too.
    renamed = {TRACKED_NOTE.format(old): TRACKED_NOTE.format(new) for old, new in mapping.items() if old.startswith("REF-")}
    if isinstance(notes := data.get("notes"), list):
        data["notes"] = [renamed.get(note, note) if isinstance(note, str) else note for note in notes]
    if isinstance(legacy := data.get("legacy"), dict) and isinstance(successors := legacy.get("superseded_by"), list):
        legacy["superseded_by"] = [mapping.get(item, item) if isinstance(item, str) else item for item in successors]
    return Record.from_json(data, record.body)


def renumber(exports: Sequence[Mapping[str, Record]], accepted: Mapping[str, Record]) -> tuple[list[dict[str, Record]], dict[str, str], set[str]]:
    """Each export with its migration-assigned IDs translated to the accepted journal's, by ``identity``.

    A record whose identity the accepted journal holds once, and the export holds once, takes the accepted ID (a REF's
    identity is its legacy path or its ``DOCUMENT_NOTE``; no body is ever matched). A record with no accepted
    counterpart gets a provisional number above every accepted one, the same in every export. Records
    that share an identity within one export, or whose identity the accepted journal holds more than once, cannot be
    told apart, so none of them is mapped: EACH gets its own provisional number (the k-th record of that identity,
    in the export's order, the same in every export) and is returned in the third set, so that it and every row
    citing it are held. Every record survives under a distinct
    ID; a collision raises ``SalvageError`` naming the two IDs. Returns the exports, the provisional IDs with what
    each is, and the provisional IDs of records sharing an identity."""
    counts: dict[str, int] = {}
    for record in accepted.values():
        if (key := identity(record)) is not None:
            counts[key] = counts.get(key, 0) + 1
    known = {key: record.id for record in accepted.values() if (key := identity(record)) is not None and counts[key] == 1}
    top = {prefix: max((int(name.split("-", 1)[1]) for name in accepted if name.split("-", 1)[0] == prefix), default=0) for prefix in ASSIGNED}
    provisional: dict[tuple[str, int], str] = {}
    unmapped: dict[str, str] = {}
    shared: set[str] = set()
    result: list[dict[str, Record]] = []
    for export in exports:
        seen: dict[str, int] = {}
        for record in export.values():
            if (key := identity(record)) is not None:
                seen[key] = seen.get(key, 0) + 1
        mapping: dict[str, str] = {}
        ordinal: dict[str, int] = {}
        for record in export.values():
            key = identity(record)
            if key is None or record.id.split("-", 1)[0] not in ASSIGNED:
                continue
            if key in known and seen[key] == 1:
                target = known[key]
            else:
                # Ambiguity on either side is ambiguity: the export or the accepted journal holding the identity twice.
                ambiguous = seen[key] > 1 or counts.get(key, 0) > 1
                place = (key, ordinal.get(key, 0) if ambiguous else -1)
                ordinal[key] = ordinal.get(key, 0) + 1
                if place not in provisional:
                    prefix = record.id.split("-", 1)[0]
                    top[prefix] += 1
                    provisional[place] = f"{prefix}-{top[prefix]:03d}"
                    if counts.get(key, 0) > 1:
                        holders = sorted((name for name, other in accepted.items() if identity(other) == key), key=id_key)
                        unmapped[provisional[place]] = f"a record of the {key}, which the accepted journal holds {len(holders)} times ({', '.join(holders)})"
                    elif seen[key] > 1:
                        unmapped[provisional[place]] = f"one of {seen[key]} records sharing the {key}"
                    else:
                        unmapped[provisional[place]] = (f"the {key}, which no accepted REF names (by legacy path or a note {DOCUMENT_NOTE.format('<path>')!r})"
                                                        if record.type == "reference" else f"the new {key}")
                target = provisional[place]
                if ambiguous:
                    shared.add(target)
            if target != record.id:
                mapping[record.id] = target
        translated: dict[str, Record] = {}
        for record in export.values():
            moved = relink(record, mapping)
            if (other := translated.get(moved.id)) is not None:
                raise SalvageError(f"{record.id} and {other.id if other.id != moved.id else 'another record'} would both be {moved.id}")
            translated[moved.id] = moved
        if len(translated) != len(export):
            raise SalvageError("renumbering lost a record")
        result.append(translated)
    return result, unmapped, shared


def classify(result: Salvage, after: Mapping[str, Record], before: Mapping[str, Record], on_accepted: Accepted,
             unmapped: Mapping[str, str] | None = None, shared: Collection[str] = (),
             raw: tuple[Mapping[str, Record], Mapping[str, Record]] | None = None) -> list[Revision]:
    """Classify every row ``after`` (the branch's export) changes against ``before`` (the merge base's) and the
    accepted journal, filling ``result``; checked by ``invariants`` before it returns. Returns the revisions of the
    rows held or in conflict (for ``unsalvaged``). Pure: no Git, no files. ``unmapped``: the provisional numbers
    ``renumber`` gave records with no accepted counterpart; a row linking to one is held until that record is accepted.
    ``shared``: those of records sharing an identity, each held with every row citing it. ``raw``: the untranslated
    branch and merge-base exports, on which a possible rename is decided."""
    ref = result.ref
    unmapped = unmapped or {}
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
        if identifier in after:
            superseding.append(Revision.of(after[identifier], on_accepted.heads.get(identifier, ())))

    # Within one table, an accepted identity the branch lost beside one it gained may be a rename (a name or problem
    # cell edited): salvage cannot tell a rename from a delete and an add, so it proposes neither and holds both.
    # Lost and gained are decided on the untranslated exports (merge base against branch), before any numbering.
    renamed: dict[str, str] = {}
    raw_after, raw_before = raw if raw is not None else (after, before)
    for kind in ("compatibility", "report"):
        def counted(export: Mapping[str, Record]) -> dict[str, int]:
            found: dict[str, int] = {}
            for other in export.values():
                if other.type == kind and (key := identity(other)) is not None:
                    found[key] = found.get(key, 0) + 1
            return found

        # Counted, not as sets: renaming one of two rows of a name loses one of that name.
        base_counts, branch_counts = counted(raw_before), counted(raw_after)
        held_keys = {identity(other) for other in accepted.values() if other.type == kind}
        lost_keys = {key for key, count in base_counts.items() if count > branch_counts.get(key, 0)} & held_keys
        gained_keys = {key for key, count in branch_counts.items() if count > base_counts.get(key, 0)}
        if not (lost_keys and gained_keys):
            continue
        lost = sorted((name for name, other in accepted.items() if identity(other) in lost_keys), key=id_key)
        gone = sorted((name for name, other in before.items() if identity(other) in lost_keys and name not in after), key=id_key)
        gained = sorted((name for name, other in after.items() if identity(other) in gained_keys), key=id_key)
        appeared = ", ".join(f"{name_of(after[item])!r} as {item}" for item in gained)
        for name in sorted({*lost, *gone}, key=id_key):
            renamed[name] = (f"it is gone from the branch (accepted as {', '.join(lost)}) and new {kind} rows appeared ({appeared}): "
                             + "a rename needs a superseding record; decide by hand")
        for name in gained:
            renamed[name] = (f"it is a new {kind} row {name_of(after[name])!r}, and {', '.join(lost)} is gone from the branch: "
                             + "a rename needs a superseding record; decide by hand")
    changed: list[str] = []
    for name in sorted(set(renamed) - set(after), key=id_key):
        hold(name, "undecided", renamed[name])
    for identifier in sorted(after, key=id_key):
        record = after[identifier]
        if identifier in renamed:
            changed.append(identifier)
            hold(identifier, "undecided", renamed[identifier])
            continue
        if record.type not in SALVAGED or before.get(identifier) == record:
            continue
        changed.append(identifier)
        on_main, origin = accepted.get(identifier), before.get(identifier)
        named = {*record.links.targets(), *(record.legacy.superseded_by if record.legacy is not None else ())}
        if identifier in shared:
            hold(identifier, "undecided", f"it is {unmapped[identifier]}, which cannot be told apart ({identifier} is a provisional number)")
            continue
        if cited := sorted((named - {identifier}) & unmapped.keys(), key=id_key):
            hold(identifier, "undecided", "it cites " + ", ".join(f"{unmapped[name]} ({name}, a provisional number)" for name in cited)
                 + ", whose record must be accepted first")
            continue
        if identifier in unmapped and (citing := sorted((name for name, other in after.items() if name != identifier and identifier in {
                *other.links.targets(), *(other.legacy.superseded_by if other.legacy is not None else ())}), key=id_key)):
            hold(identifier, "undecided", f"its provisional number is cited by {', '.join(citing)}; propose it alone first")
            continue
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
        # What a run of salvage may have proposed for it: the row, with ``supersedes`` for any of the old rows naming
        # it (settling may have kept only some), so the accepted record is compared without those links.
        linked = on_main
        for old in links.get(identifier, ()):
            if linked is not None and old in linked.links.supersedes and old not in record.links.supersedes:
                linked = without_supersedes(linked, old)
        if on_main is None:
            result.kinds[identifier] = "new"
            if record.type in ("compatibility", "report"):
                result.notices.append(f"{identifier}: new {record.type} row; its migration ID is positional and provisional")
            # Without links: the linking pass below adds each only from an edit that is neither a conflict nor held.
            result.proposals.append(Revision.of(record))
        elif linked == record:
            result.kinds[identifier] = "accepted"
        else:
            result.kinds[identifier] = "conflict"
            result.conflicts.append((on_main, record))
    pending_links: dict[str, list[str]] = {}
    for identifier, edit in edits.items():
        record, origin, on_main = after[identifier], before[identifier], accepted.get(identifier)
        if on_main is None:
            # A revision must name a head the accepted journal holds; with no record there is nothing to revise.
            hold(identifier, "undecided", f"the accepted journal has no record {identifier} to revise (main removed or never journalled it)")
            continue
        try:
            revision = apply(edit, on_main)
        except RecordError as error:
            hold(identifier, "undecided", f"its edit on {ref} does not make a valid revision: {error}")
            continue
        pending = [target for target in edit.successors
                   if (held := accepted.get(target)) is None or identifier not in held.links.supersedes]
        if edit.empty:
            result.kinds[identifier] = "cosmetic"
        elif revision is None and not pending:
            result.kinds[identifier] = "accepted"
        elif on_main != origin and on_main != apply(edit, origin):
            result.kinds[identifier] = "conflict"
            result.conflicts.append((on_main, record))
        else:
            result.kinds[identifier] = "revision" if revision is not None else "superseded"
            if revision is not None:
                result.proposals.append(Revision.of(revision, on_accepted.heads[identifier]))
            if pending:
                pending_links[identifier] = pending
    own = {revision.record.id: revision for revision in result.proposals}
    result.proposals, unlinkable = settle(result, own, pending_links, edits, on_accepted, ref, hold)
    for target, olds in sorted(links.items()):
        for old in olds:
            if result.kinds.get(old) in ("conflict", "supersede", "undecided"):
                proposal = next((item for item in result.proposals if item.record.id == target), None)
                if proposal is None:
                    outcome = f"{target} is not revised"
                elif old in proposal.record.links.supersedes:
                    outcome = f"{target} keeps the link its accepted record already holds"
                else:
                    outcome = f"{target} is proposed without that link"
                result.notices.append(f"{old}: {target} would supersede it, but {old} is {result.kinds[old]}; {outcome}")
    for target, reason in sorted(unlinkable.items()):
        result.notices.append(f"{target}: not revised to carry supersedes links ({reason})")
    # A cosmetic row that ends up carrying a link is a revision now; the notice is for rows still cosmetic.
    result.notices.extend(f"{identifier}: only verbatim legacy text changed on {ref}; nothing to propose"
                          for identifier in changed if result.kinds.get(identifier) == "cosmetic")
    shared_keys = {identity(after[name]) for name in shared if name in after}
    result.notices.extend(f"{identifier}: removed on {ref} (a deletion needs no proposal)" for identifier in sorted(set(before) - set(after), key=id_key)
                          if before[identifier].type in SALVAGED and identity(before[identifier]) not in shared_keys and identifier not in renamed)
    result.proposals.sort(key=lambda revision: (revision.record.type, id_key(revision.record.id)))
    invariants(result, changed, on_accepted, after)
    return superseding


class PublishError(RuntimeError):
    """``--out`` refused: a target or a file under the output directory that this run may not create or keep."""


PUBLISHED = ("memory/journal", "documents")


def same_regular_file(path: Path, data: bytes) -> bool | None:
    """``None`` when nothing is at ``path`` (``lstat``, never following a link); True when a regular file holds exactly
    ``data``; False for anything else (other bytes, a symlink, dangling or not, a directory)."""
    try:
        info = os.lstat(path)
    except FileNotFoundError:
        return None
    if not stat.S_ISREG(info.st_mode):
        return False
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    with os.fdopen(descriptor, "rb") as handle:
        return handle.read(len(data) + 1) == data


def publish(out: Path, planned: Sequence[tuple[Path, bytes]]) -> list[Path]:
    """Write ``planned`` under ``out`` without ever replacing anything. Preflight: each target must be absent or a
    regular file with identical bytes (a symlink, even to identical bytes, refuses), and nothing else may be under
    ``out``'s ``memory/journal`` or ``documents`` (an earlier run's segment would be copied with the new ones). Then
    each new file is created exclusively (``O_CREAT | O_EXCL | O_NOFOLLOW``), written and closed; if a create fails,
    the files this run created are removed. Raises ``PublishError``; returns the files created."""
    wanted = {target: data for target, data in planned}
    for target, data in wanted.items():
        if same_regular_file(target, data) is False:
            raise PublishError(f"{target} exists and is not a regular file with this content (symlinks are never followed); "
                               + "nothing written (move it away or use another --out)")
    for root in PUBLISHED:
        top = out / root
        if not os.path.lexists(top):
            continue
        if os.path.islink(top) or not top.is_dir():
            raise PublishError(f"{top} is not a directory; nothing written")
        for parent, directories, files in os.walk(top, followlinks=False):
            for name in (*directories, *files):
                path = Path(parent) / name
                if path not in wanted and (os.path.islink(path) or not path.is_dir()):
                    raise PublishError(f"{path} was left by an earlier run: use a new --out or remove it; nothing written")
    created: list[Path] = []
    for target, data in wanted.items():
        if os.path.lexists(target):
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        try:
            descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
        except OSError as error:
            for path in created:
                path.unlink()
            raise PublishError(f"{target}: {error.strerror or error}; removed the {len(created)} file(s) this run had created") from error
        with os.fdopen(descriptor, "wb") as handle:
            _ = handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        created.append(target)
    return created


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
            for document in result.documents:
                print(f"{ref}: {document.path}: {document.line()}")
            print(f"memory salvage --check: {len(result.unsalvaged)} unsalvaged row(s), {len(result.documents)} pending document(s) on {ref} "
                  + f"(merge-base {result.base[:12]})")
            return 1 if result.unsalvaged or result.documents else 0
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
        segments = split_change_set(commit_time(root, result.head), result.proposals) if result.proposals else []
        if out is not None:
            planned = [(out / "documents" / document.path, document.body.encode()) for document in result.documents]
            planned += [(out / path, data) for path, data in segments]
            try:
                _ = publish(out, planned)
            except PublishError as error:
                print(f"memory salvage: {error}", file=sys.stderr)
                return 1
            for document in result.documents:
                target = out / "documents" / document.path
                print(f"memory salvage: {document.path}: {document.command(str(target))}", file=sys.stderr)
                if document.target is None:
                    print(f"memory salvage: {document.path}: {document.scope_line()}", file=sys.stderr)
            if segments:
                print(f"memory salvage: wrote {len(segments)} segment(s) under {out}", file=sys.stderr)
        else:
            for path, data in segments:
                print(f"# {path}")
                print(data.decode(), end="")
    except SalvageError as error:
        print(f"memory salvage: internal error, nothing written: {error}", file=sys.stderr)
        return 1
    except (CatalogError, GitError, JournalError, MigrationError, RecordError, OSError) as error:
        print(f"memory salvage: {error}", file=sys.stderr)
        return 1
    return 1 if result.conflicts else EXIT_SUPERSEDE if result.unrevisable or result.undecided else 0
