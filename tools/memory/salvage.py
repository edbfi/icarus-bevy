# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory salvage``: a branch's added or modified legacy rows as proposals, with the export's conversion.

``salvage(root, ref)`` exports three revisions with ``tools.memory.legacy.export`` (read through Git
plumbing; nothing is checked out): ``ref``, its merge-base with ``main``, and ``main``. A row ``ref``
adds or modifies against the merge-base is classified against ``main``, whose export is what the
journal holds at cut-over:

- ``accepted``: ``main`` already holds the same record (for example through a squash merge); no proposal.
- ``new``: ``main`` has no record with the ID; a first revision.
- ``revision``: ``main`` holds the ID and so did the merge-base: a revision naming ``main``'s record as
  ``prev``. Fields the rules do not let a revision change (a finding's claim, an evidence path...) are
  listed in ``unrevisable``: such a row must become a new record that supersedes the old one.
- ``conflict``: ``main`` holds the ID, the merge-base did not, and the contents differ: both sides
  allocated the number, or ``main`` took an earlier draft. Never proposed; it needs a new ID or a
  reconciliation by hand.

New compatibility and campaign rows get positional IDs at export, so their IDs are only provisional.
Spec and digest edits are not rows; they are listed for review. ``unsalvaged`` holds every proposal or
conflict whose revision is not yet in ``ref``'s own journal.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

from tools.memory.check import NOT_REVISABLE
from tools.memory.gitio import git
from tools.memory.journal import Revision, read_git
from tools.memory.legacy import Export, export
from tools.memory.model import Record, id_key

MAIN = "main"
LEGACY_DIRS = ("docs/ledgers", "evidence")
SYNTHESIS_DIRS = ("docs/specs", "docs/research")
SALVAGED = ("finding", "unknown", "bug", "evidence", "lead", "report", "compatibility")


@dataclass
class Salvage:
    ref: str
    head: str
    base: str
    main: str
    proposals: list[Revision] = field(default_factory=list[Revision])
    kinds: dict[str, str] = field(default_factory=dict[str, str])
    unrevisable: dict[str, tuple[str, ...]] = field(default_factory=dict[str, tuple[str, ...]])
    conflicts: list[tuple[Record, Record]] = field(default_factory=list[tuple[Record, Record]])
    unsalvaged: list[str] = field(default_factory=list[str])
    notices: list[str] = field(default_factory=list[str])


def changed_paths(root: Path, base: str, head: str, prefixes: tuple[str, ...]) -> list[str]:
    output = git(root, "diff", "--name-only", "--no-renames", "-z", base, head, "--", *prefixes).decode()
    return sorted(path for path in output.split("\0") if path)


def unrevisable_fields(old: Record, new: Record) -> tuple[str, ...]:
    names = NOT_REVISABLE.get(new.type, ())
    return tuple(name for name in names if (old.scope != new.scope if name == "scope" else old.fields.get(name) != new.fields.get(name)))


def commit(root: Path, ref: str) -> str:
    return git(root, "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}").decode().strip()


def salvage(root: Path, ref: str) -> Salvage:
    head = commit(root, ref)
    base = git(root, "merge-base", "--end-of-options", MAIN, head).decode().strip()
    result = Salvage(ref, head, base, commit(root, MAIN))
    exports: dict[str, Export] = {}
    for revision in dict.fromkeys((head, base, result.main)):
        exports[revision] = export(root, revision)
    after, before, accepted = (exports[item].records() for item in (head, base, result.main))
    for identifier in sorted(after, key=id_key):
        record = after[identifier]
        if record.type not in SALVAGED or before.get(identifier) == record:
            continue
        on_main = accepted.get(identifier)
        if on_main == record:
            result.kinds[identifier] = "accepted"
            continue
        if on_main is not None and identifier not in before:
            result.kinds[identifier] = "conflict"
            result.conflicts.append((on_main, record))
            continue
        prev = on_main if on_main is not None else before.get(identifier)
        result.kinds[identifier] = "new" if prev is None else "revision"
        if prev is not None and (fields := unrevisable_fields(prev, record)):
            result.unrevisable[identifier] = fields
        if record.type in ("compatibility", "report") and prev is None:
            result.notices.append(f"{identifier}: new {record.type} row; its migration ID is positional and provisional")
        result.proposals.append(Revision.of(record, [Revision.of(prev).sha256] if prev is not None else []))
    result.notices.extend(f"{identifier}: removed on {ref} (a deletion needs no proposal)" for identifier in sorted(set(before) - set(after), key=id_key))
    result.notices.extend(f"{path}: spec or digest changed; not a legacy row, review it by hand" for path in changed_paths(root, base, head, SYNTHESIS_DIRS))
    journal = {revision.sha256 for segment in read_git(root, head) for revision in segment.revisions}
    result.unsalvaged = [revision.record.id for revision in result.proposals if revision.sha256 not in journal]
    result.unsalvaged += [new.id for _old, new in result.conflicts]
    result.proposals.sort(key=lambda revision: (revision.record.type, id_key(revision.record.id)))
    return result
