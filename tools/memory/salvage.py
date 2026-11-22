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
A document the branch changed or added (a spec, digest, ledger prose, the status file) is listed by path
(``pending_documents``), matched to the accepted REF of that path by identity only (its legacy path or its
``DOCUMENT_NOTE``, never its body): a body revision when the text's tokens equal that REF's scope tokens, else a
superseding REF; a new REF for a new document; nothing but a decision by hand when two accepted REFs name the path.
``unsalvaged`` holds every proposal, conflict, ``supersede`` or ``undecided`` row whose content (ID and sha256)
is not yet in ``ref``'s own journal.

The command prints the proposal change set as segment lines, or writes it under ``DIR/memory/journal`` with
``--out DIR``, a directory that must not exist (salvage creates it, and removes it again if anything fails); notices,
conflicts and rows needing a superseding record go to stderr. ``--check`` prints each
unsalvaged row and each pending document (done when the accepted REF of its path holds the branch's body) and exits
1 if there is any. ``--out`` also
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
import shutil
import sys
from collections.abc import Callable, Collection, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Protocol

from tools.evidence_catalog_model import CatalogError
from tools.evidence_catalog_store import LEGACY_ROOTS
from tools.memory.check import APPEND_ONLY, REVISABLE, revision_problems
from tools.memory.gitio import GitError, git, run_git
from tools.memory.journal import JournalError, Revision, Segment, read_git, split_change_set
from tools.memory.legacy import TRACKED_NOTE, commit_time, export
from tools.memory.legacy_rules import MigrationError, commit_words, extract
from tools.memory.model import ID, JSON, TYPES, Record, RecordError, as_mapping, id_key, load_json, regap
from tools.memory.state import State, replay

ACCEPTANCE_REF = "origin/main"
SALVAGED = ("finding", "unknown", "bug", "evidence", "lead", "report", "compatibility")
EXIT_SUPERSEDE = 4
# The note a REF proposed with salvage's printed command carries: the legacy document it stands for (its identity).
DOCUMENT_NOTE = "salvaged document: {}"
DOCUMENT_NOTED = re.compile(r"salvaged document: (.+)")
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
    # Pending documents: what each changed, added or cited document needs (``Pending``).
    documents: list[Pending] = field(default_factory=list["Pending"])
    # Documents (by path) that held rows cite as the branch has them, with those rows.
    cited: dict[str, list[str]] = field(default_factory=dict[str, list[str]])


@dataclass(frozen=True)
class Pending:
    """A document the branch changed, added or cites: its legacy path, what it needs (``kind``: ``new``, ``revise`` the
    body of ``target``, ``supersede`` ``target`` with a new REF because the text names other tokens than its scope,
    ``decide`` by hand (the body ``newly`` names tokens, or ``main_changed`` ``target`` since the base), ``ambiguous``
    when the accepted journal holds the path more than once (``holders``), or ``cited`` as the branch has it while
    ``target`` holds other text), and the body, title and scope tokens (``names``) the exporter gives its REF at the
    salvaged tip. ``citing``: the held rows citing a document the branch did not change (listed for them alone)."""

    path: str
    kind: str
    body: str
    title: str
    names: tuple[str, ...] = ()
    target: str | None = None
    holders: tuple[str, ...] = ()
    scope: tuple[str, ...] = ()
    scopes: tuple[str, ...] = ()
    newly: tuple[str, ...] = ()
    main_changed: bool = False
    citing: tuple[str, ...] = ()

    def commands(self, file: str) -> list[str]:
        """What the operator runs once ``--out`` wrote the body to ``file`` (nothing for an ambiguous path). A new REF's
        scope is the operator's statement: the command carries ``SCOPE_PLACEHOLDER``, which the tool's parser refuses
        (not JSON) until replaced; ``scope_line`` says what the exporter read from the text."""
        if self.kind == "revise":
            return [f"memory revise {self.target} --body {shlex.quote(file)}"]
        if self.kind == "ambiguous":
            return [f"memory supersede {other} --by {self.target}" for other in self.holders if other != self.target] if self.target is not None else []
        propose = (f"memory propose reference --set {shlex.quote('title=' + self.title)} --set-json {shlex.quote('scope=' + SCOPE_PLACEHOLDER)}"
                   + f" --note {shlex.quote(DOCUMENT_NOTE.format(self.path))} --body {shlex.quote(file)}")
        if self.kind == "cited":
            return []
        if self.kind == "decide":
            conditions = [*([f"{self.target}'s scope still holds for the new text"] if self.newly else []),
                          *([f"this body carries main's changes to {self.target} since this branch's base"] if self.main_changed else [])]
            revise = f"if {' and '.join(conditions)}: memory revise {self.target} --body {shlex.quote(file)}"
            if not self.newly:
                return [revise]
            return [revise, f"otherwise: {propose}", f"otherwise, then: memory supersede {self.target} --by <the new REF>"]
        return [propose, *([f"memory supersede {self.target} --by <the new REF>"] if self.kind == "supersede" else [])]

    def printed(self, file: str) -> list[str]:
        """Everything ``--out`` prints for this document: its commands, then the scope line only when one of them
        proposes a REF (carries ``SCOPE_PLACEHOLDER``), or the plain line for a kind that names its IDs or rows."""
        commands = self.commands(file)
        if any(SCOPE_PLACEHOLDER in command for command in commands):
            return [*commands, self.scope_line()]
        return [*commands, *([self.line()] if self.kind in ("ambiguous", "cited") else [])]

    def main_note(self) -> str:
        return (f"main added or changed {self.target} since this branch's base, so check that the branch's body carries"
                + " main's text before it replaces it")

    def scope_line(self) -> str:
        """For a new REF: who states the scope, and the build, release and commit tokens its text names."""
        return (f"the scope must be stated by whoever proposes it: replace {SCOPE_PLACEHOLDER} with a JSON scope"
                + f" (salvage neither keeps nor upgrades the text's tokens); {spoken(self.names, 'the text names')}")

    def line(self) -> str:
        """The pending document without ``--out``: no path is named that does not exist."""
        opening = f"unchanged document cited by {', '.join(self.citing)} as this branch has it" if self.citing else "changed document"
        if self.kind == "revise":
            return (f"changed document; revise {self.target} with the body salvage --out writes (listed until the accepted body equals the"
                    + " branch's; a body merged by hand into other text stays listed)")
        if self.kind == "supersede":
            return (f"changed document; {spoken(self.names, 'its text names')}; {self.target}'s scope is {', '.join(self.scope) or 'empty'}; "
                    + f"a body revision cannot change scope; propose it as a new reference and supersede {self.target}"
                    + (f"; {self.main_note()}" if self.main_changed else "") + " (salvage --out writes its body and the commands)")
        if self.kind == "cited":
            return (f"{opening}; {self.target} holds other text: settle the document first"
                    + f" (decide whether those rows apply to {self.target}'s text, or propose this text as a new REF)")
        if self.kind == "decide":
            reasons = [*([f"its body newly names {', '.join(self.newly)}; {self.target}'s scope is {', '.join(self.scope) or 'empty'};"
                          + " a body revision keeps that scope"] if self.newly else []),
                       *([self.main_note()] if self.main_changed else [])]
            return f"changed document; {'; '.join(reasons)}: decide by hand (salvage --out writes the body and the commands, labelled)"
        if self.kind == "ambiguous":
            if self.target is not None:
                others = ", ".join(other for other in self.holders if other != self.target)
                return (f"{opening}; the accepted journal holds {', '.join(self.holders)} for this path; {self.target} holds this body:"
                        + f" supersede {others} by it (" + "; ".join(f"memory supersede {other} --by {self.target}" for other in self.holders
                                                                    if other != self.target) + ") or decide by hand")
            held = "; ".join(f"{holder} (scope: {scope})" for holder, scope in zip(self.holders, self.scopes, strict=True))
            return f"{opening}; the accepted journal holds {held} for this path: decide by hand (no command)"
        return ((f"{opening}; no accepted REF in force names this path" if self.citing else "new document")
                + "; salvage --out writes its body and the memory propose reference command (listed until an accepted REF names this"
                + " path, by legacy path or that command's note, with this body)")


def body_tokens(body: str) -> set[str]:
    """The build, release and commit tokens a whole body names, read with the exporter's own rules (``extract``,
    ``commit_words``) over the body as one cell."""
    scope = extract({"body": body}).scope or {}
    named = [item for key in ("builds", "releases") if isinstance(items := scope.get(key), list) for item in items if isinstance(item, str)]
    commits, _digits = commit_words(body)
    return {*named, *commits}


def spoken(tokens: Sequence[str], subject: str) -> str:
    return f"{subject}: {', '.join(tokens)}" if tokens else f"{subject} no build, release or commit"


def pending_documents(after: Mapping[str, Record], before: Mapping[str, Record], on_accepted: Accepted,
                      cited: Mapping[str, Sequence[str]] | None = None) -> list[Pending]:
    """The documents the branch changed or added, matched by identity only (``document_path``; never by body) against
    the accepted REFs still in force (not superseded or retired). More than one: ambiguous, listed with their IDs
    (and, when one holds the branch's body, the supersession that would leave it alone). Exactly one: done when it
    holds the branch's body, else a body revision when the text's tokens equal its scope tokens, else a superseding
    REF. None: a new document. A document the branch did not change but held rows cite (``cited``) while the in-force
    REF holds other text is listed too: those rows wait for it to be settled."""
    holders: dict[str, list[Record]] = {}
    for record in on_accepted.records.values():
        if (path := document_path(record)) is not None and record.id not in on_accepted.retired:
            holders.setdefault(path, []).append(record)
    base_bodies = {path: record.body for path, record in documents(before).items()}
    found: list[Pending] = []
    for path, record in sorted(documents(after).items()):
        body, title = record.body or "", record.text("title") or path
        names = record.scope.tokens if record.scope is not None else ()
        on_main = holders.get(path, [])
        citing: tuple[str, ...] = ()
        if record.body == base_bodies.get(path):
            # The branch did not change it: listed only for held rows citing it as the branch has it, while main does not
            # hold exactly that text under one REF (main's later edits alone are not this branch's to salvage).
            if not (cited and path in cited) or (len(on_main) == 1 and on_main[0].body == record.body):
                continue
            citing = tuple(cited[path])
            if len(on_main) == 1:
                found.append(Pending(path, "cited", body, title, names, target=on_main[0].id, citing=citing))
                continue
        if not on_main:
            found.append(Pending(path, "new", body, title, names, citing=citing))
        elif len(on_main) > 1:
            # Ambiguity before done: two REFs in force for one document is never done. A supersession is offered only when
            # exactly one of them holds this body; with several, which one's scope is right is the operator's call.
            matching = [other.id for other in on_main if other.body == record.body]
            ordered = sorted(on_main, key=lambda other: id_key(other.id))
            found.append(Pending(path, "ambiguous", body, title, names, target=matching[0] if len(matching) == 1 else None, citing=citing,
                                 holders=tuple(other.id for other in ordered),
                                 scopes=tuple(", ".join(other.scope.tokens) if other.scope is not None and other.scope.tokens else "none"
                                              for other in ordered)))
        elif record.body != on_main[0].body:
            held = on_main[0].scope.tokens if on_main[0].scope is not None else ()
            # The scope reads Status and Scope only (the exporter's rule); the whole body may name more.
            newly = tuple(sorted(body_tokens(body) - body_tokens(on_main[0].body or ""), key=id_key))
            # Main added or changed the document since the merge base: a body revision would drop its text (no three-way merge).
            main_changed = on_main[0].body != base_bodies.get(path)
            kind = "supersede" if sorted(names) != sorted(held) else "decide" if newly or main_changed else "revise"
            found.append(Pending(path, kind, body, title, names, target=on_main[0].id, scope=held, newly=newly, main_changed=main_changed))
    return found


@dataclass(frozen=True)
class Accepted:
    """What the accepted commit holds per ID: the head record, the revision identities a revision of it names
    as ``prev``, and whether that came from its journal or its legacy export."""

    records: dict[str, Record]
    heads: dict[str, tuple[str, ...]]
    source: str
    segments: tuple[Segment, ...] = ()
    # Records superseded or retired on main: a REF among them no longer stands for its document.
    retired: frozenset[str] = frozenset()


def accepted_on(root: Path, main: str) -> Accepted:
    """``main``'s replayed journal when it has one; else its legacy export, as the cut-over will journal it."""
    if segments := read_git(root, main):
        state = replay(segments)
        return Accepted({identifier: entry.record for identifier, entry in state.entries.items()},
                        {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal", segments,
                        frozenset(identifier for identifier in state.entries if state.retired(identifier)))
    records = export(root, main).records()
    revisions = tuple(Revision.of(record) for record in records.values())
    return Accepted(records, {revision.record.id: (revision.rev,) for revision in revisions}, "legacy",
                    (Segment("memory/journal/accepted-legacy-export.jsonl", "", revisions),))


def has_legacy(root: Path, revision: str) -> bool:
    """Whether ``revision`` has a legacy ledger or evidence index tree."""
    return bool(git(root, "ls-tree", "--name-only", "--end-of-options", revision, "--", *LEGACY_ROOTS).strip())


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
        return Edit(undecided=f"its row on {ref} withdraws the legacy successor {', '.join(withdrawn)}; it has no new stored status, "
                    + "and a revision cannot withdraw a legacy edge")
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
            found[identifier] = f"on replay it would not be superseded by its successors {', '.join(edit.successors)}"
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
                raise SalvageError(f"{identifier}: {name} records this run does not salvage: {', '.join(bad)}")
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
            raise SalvageError(f"{identifier}: carries IDs other than the accepted journal's for their identity: {', '.join(assigned)}")
    # (f) A proposal's REF, CP or RP link names the accepted record of the identity the branch's record had.
    for identifier, record in proposed.items():
        for target in record.links.targets():
            if after is not None and target in after and target in accepted and target.split("-", 1)[0] in ASSIGNED \
                    and identity(after[target]) != identity(accepted[target]):
                raise SalvageError(f"{identifier}: cites {target}, which on main is {identity(accepted[target])}, not the branch's {identity(after[target])}")
            # A citation of a document is of the document as the branch has it: the accepted REF must hold that body.
            if after is not None and target in after and target in accepted and after[target].type == "reference" \
                    and after[target].body != accepted[target].body:
                raise SalvageError(f"{identifier}: cites {target}, whose text on main differs from the branch's document")
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
    (after, before), unmapped, shared = renumber((after, before), on_accepted.records, on_accepted.retired)
    superseding = classify(result, after, before, on_accepted, unmapped, shared, raw)
    result.documents = pending_documents(after, before, on_accepted, result.cited)
    result.notices.extend(f"{document.path}: {document.line()}" for document in result.documents)
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


def renumber(exports: Sequence[Mapping[str, Record]], accepted: Mapping[str, Record],
             retired: Collection[str] = ()) -> tuple[list[dict[str, Record]], dict[str, str], set[str]]:
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
    # A REF superseded or retired on main no longer stands for its document (a superseding REF replaced it).
    standing = {name: record for name, record in accepted.items() if not (record.type == "reference" and name in retired)}
    counts: dict[str, int] = {}
    for record in standing.values():
        if (key := identity(record)) is not None:
            counts[key] = counts.get(key, 0) + 1
    known = {key: record.id for record in standing.values() if (key := identity(record)) is not None and counts[key] == 1}
    top = {prefix: max((int(name.split("-", 1)[1]) for name in accepted if name.split("-", 1)[0] == prefix), default=0) for prefix in ASSIGNED}
    provisional: dict[tuple[str, int], str] = {}
    unmapped: dict[str, str] = {}
    shared: set[str] = set()
    result: list[dict[str, Record]] = []
    for position, export in enumerate(exports):
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
            # A citation of a document means the document as the branch has it: the branch's REF takes the accepted ID
            # only when the in-force REF holds the same body; otherwise its number is provisional and rows citing it wait.
            elsewhere = (position == 0 and record.type == "reference" and key in known and seen[key] == 1
                         and standing[known[key]].body != record.body)
            if key in known and seen[key] == 1 and not elsewhere:
                target = known[key]
            elif elsewhere:
                place = (f"{key} as the branch has it", -1)
                if place not in provisional:
                    top["REF"] += 1
                    provisional[place] = f"REF-{top['REF']:03d}"
                    unmapped[provisional[place]] = f"{document_path(record)} as this branch has it"
                target = provisional[place]
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
                        holders = sorted((name for name, other in standing.items() if identity(other) == key), key=id_key)
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
    lost_identities: set[str] = set()
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
        gained = sorted((name for name, other in after.items() if identity(other) in gained_keys), key=id_key)
        appeared = ", ".join(f"{name_of(after[item])!r} as {item}" for item in gained)
        lost_identities.update(lost_keys)
        for name in lost:
            renamed[name] = (f"it is gone from the branch (accepted as {', '.join(lost)}) and new {kind} rows appeared ({appeared}): "
                             + "a rename needs a superseding record; decide by hand")
        for name in gained:
            renamed[name] = (f"it is a new {kind} row {name_of(after[name])!r}, and the branch lost {', '.join(lost)}; "
                             + "a rename needs a superseding record; decide by hand")
    # A branch REF numbered provisionally only because its text differs from the in-force REF's (``renumber``) stands,
    # for a row the branch did not change, for the accepted REF the merge base's export cited: such a row is unchanged.
    standins = {name: other for name in unmapped if name in after and name not in before and after[name].type == "reference"
                for other, base in before.items() if other in accepted and identity(base) == identity(after[name])}
    # A REF and the accepted REF superseding it for the same document are one citation: main may supersede a document's
    # REF after a row citing it was accepted (by a cut-over or a run of salvage), so the branch's row, translated against
    # today's journal, cites the successor while main's record still cites the old one.
    successors: dict[str, list[str]] = {}
    for name, other in accepted.items():
        if other.type == "reference" and (key := identity(other)) is not None:
            for old in other.links.supersedes:
                if old in accepted and identity(accepted[old]) == key:
                    successors.setdefault(old, []).append(name)
    current: dict[str, str] = {}
    for old in successors:
        name, visited = old, {old}
        while len(successors.get(name, ())) == 1 and (following := successors[name][0]) not in visited:
            name = following
            visited.add(name)
        if name != old:
            current[old] = name

    def same(one: Record | None, other: Record | None) -> bool:
        """Equal, or equal once each cited REF is the one superseding it (``current``)."""
        return one == other or (one is not None and other is not None and bool(current) and relink(one, current) == relink(other, current))

    changed: list[str] = []
    for name in sorted(set(renamed) - set(after), key=id_key):
        hold(name, "undecided", renamed[name])
    for identifier in sorted(after, key=id_key):
        record = after[identifier]
        if identifier in renamed:
            changed.append(identifier)
            hold(identifier, "undecided", renamed[identifier])
            continue
        if record.type not in SALVAGED or before.get(identifier) in (record, relink(record, standins)):
            continue
        changed.append(identifier)
        on_main, origin = accepted.get(identifier), before.get(identifier)
        named = {*record.links.targets(), *(record.legacy.superseded_by if record.legacy is not None else ())}
        if identifier in shared:
            hold(identifier, "undecided", f"it is {unmapped[identifier]}, which cannot be told apart ({identifier} is a provisional number)")
            continue
        if cited := sorted((named - {identifier}) & unmapped.keys(), key=id_key):
            # A document cited as the branch has it waits for the document; any other provisional record for its acceptance.
            settle_first = [name for name in cited if name in after and after[name].type == "reference" and (key := identity(after[name])) is not None
                            and any(identity(other) == key and other_name not in on_accepted.retired for other_name, other in accepted.items())]
            clauses = [f"it cites {unmapped[name]} ({name}, a provisional number); "
                       + ", ".join(sorted(other_name for other_name, other in accepted.items() if other_name not in on_accepted.retired
                                          and identity(other) == identity(after[name])))
                       + " holds other text: settle the document first" for name in settle_first]
            if others := [name for name in cited if name not in settle_first]:
                clauses.append("it cites " + ", ".join(f"{unmapped[name]} ({name}, a provisional number)" for name in others)
                               + ("; that record must be accepted first" if len(others) == 1 else "; those records must be accepted first"))
            hold(identifier, "undecided", "; ".join(clauses))
            for name in cited:
                if name in after and (path := document_path(after[name])) is not None:
                    result.cited.setdefault(path, []).append(identifier)
            continue
        if identifier in unmapped and (citing := sorted((name for name, other in after.items() if name != identifier and identifier in {
                *other.links.targets(), *(other.legacy.superseded_by if other.legacy is not None else ())}), key=id_key)):
            hold(identifier, "undecided", f"its provisional number is cited by {', '.join(citing)}; propose it alone first")
            continue
        if same(on_main, record):
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
            if (edit.blocked or edit.undecided is not None) and on_main is not None and not same(on_main, origin):
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
        elif same(linked, record):
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
        elif not same(on_main, origin) and not same(on_main, apply(edit, origin)):
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
                          if before[identifier].type in SALVAGED and identity(before[identifier]) not in shared_keys and identity(before[identifier]) not in lost_identities and identifier not in renamed)
    result.proposals.sort(key=lambda revision: (revision.record.type, id_key(revision.record.id)))
    invariants(result, changed, on_accepted, after)
    return superseding


class PublishError(RuntimeError):
    """``--out`` refused or failed; nothing it created is left behind."""


def publish(out: Path, planned: Sequence[tuple[str, bytes]]) -> None:
    """Write ``planned`` (paths relative to ``out``) into ``out``, a directory that must not exist: one ``os.mkdir`` creates
    it (its parent must exist; anything at the path, a dangling symlink included, refuses), and every subdirectory and
    file is created inside it, files exclusively (``O_EXCL | O_NOFOLLOW``). On any failure or interrupt after the mkdir
    the whole tree is removed (and the message says whether that worked; an interrupt is re-raised).
    A concurrent process of the same user changing that directory during the run, and symlinks in the path leading
    to its parent, are outside what this guards against. Raises ``PublishError``."""
    try:
        os.mkdir(out, 0o700)
    except FileExistsError as error:
        raise PublishError(f"{out} exists: --out must name a new directory") from error
    except OSError as error:
        raise PublishError(f"{out}: {error.strerror or error}") from error
    try:
        for relative, data in planned:
            target = out / relative
            for parent in reversed([*target.relative_to(out).parents][:-1]):
                if not (out / parent).is_dir():
                    os.mkdir(out / parent, 0o700)
            descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
            with os.fdopen(descriptor, "wb") as handle:
                _ = handle.write(data)
                handle.flush()
                os.fsync(handle.fileno())
    except BaseException as error:
        # Any failure or interrupt after the mkdir: remove the tree, and say truthfully whether that worked.
        try:
            shutil.rmtree(out)
            outcome = f"removed {out}, which this run had created"
        except OSError as cleanup:
            outcome = f"could not remove {out}: {cleanup}; remove it by hand"
        if not isinstance(error, OSError):
            print(f"memory salvage: {outcome}", file=sys.stderr)
            raise
        raise PublishError(f"{error}; {outcome}") from error


class Subparsers(Protocol):
    """What ``register`` needs of ``ArgumentParser.add_subparsers()``."""

    def add_parser(self, name: str, *, help: str) -> argparse.ArgumentParser: ...


def register(subparsers: Subparsers) -> None:
    parser = subparsers.add_parser("salvage", help="turn a branch's added or modified legacy rows into proposals")
    _ = parser.add_argument("ref", help="branch, commit or worktree HEAD to salvage (its pre-merge tip if it merged the cut-over)")
    _ = parser.add_argument("--check", action="store_true", help="only report unsalvaged legacy rows")
    _ = parser.add_argument("--out", type=Path, help="a NEW directory (it must not exist) to write the proposal segments (DIR/memory/journal) and "
                            + "pending document bodies (DIR/documents) into")
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
            print(f"needs a superseding record: {identifier}: a revision may not change {', '.join(fields)}; "
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
            planned = [(f"documents/{document.path}", document.body.encode()) for document in result.documents]
            planned += [(path, data) for path, data in segments]
            try:
                publish(out, planned)
            except PublishError as error:
                print(f"memory salvage: {error}", file=sys.stderr)
                return 1
            for document in result.documents:
                target = out / "documents" / document.path
                for printed in document.printed(str(target)):
                    print(f"memory salvage: {document.path}: {printed}", file=sys.stderr)
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
