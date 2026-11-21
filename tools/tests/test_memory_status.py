# SPDX-License-Identifier: AGPL-3.0-or-later
"""The latest accepted statement wins: a migrated record's legacy status holds until a revision stores another
status, and links derive ``superseded``/``resolved`` whatever the revisions say. Invented records only."""

from __future__ import annotations

import unittest

from tools.memory.bank import derived_status
from tools.memory.check import revision_problems
from tools.memory.journal import Revision, Segment
from tools.memory.model import JSON, Record
from tools.memory.state import State, replay
from tools.tests.memory_fixtures import evidence, finding, unknown


def migrated(data: dict[str, JSON], status: str, *, implied: str | None = None, superseded_by: tuple[str, ...] = ()) -> Record:
    legacy: dict[str, JSON] = {"path": "docs/ledgers/invented.md", "cells": {"status": status.removeprefix("legacy:")}}
    if implied is not None:
        legacy["implied_status"] = implied
    if superseded_by:
        legacy["superseded_by"] = list(superseded_by)
    return Record.from_json({**data, "status": status, "legacy": legacy})


def revised(record: Record, prev: Revision, **changes: JSON) -> Revision:
    return Revision.of(Record.from_json({**record.to_json(), **changes}, record.body), [prev.rev])


def state_of(*revisions: Revision) -> State:
    return replay([Segment(f"memory/journal/2026/11/{number:02d}.jsonl", "2026-11-20T12:00:00Z", (revision,)) for number, revision in enumerate(revisions)])


class LatestStatementTests(unittest.TestCase):
    def check(self, revisions: tuple[Revision, ...], identifier: str, expected: str) -> None:
        state = state_of(*revisions)
        self.assertEqual([], revision_problems(state))
        self.assertEqual(expected, state.status(identifier))
        self.assertEqual(expected, derived_status(state, state.entries[identifier].record), "the bank's status agrees")

    def test_a_legacy_retirement_is_overridden_by_a_stored_active(self) -> None:
        old = migrated(finding("F-201", "Invented"), "legacy:retired: receipt", implied="retired")
        first = Revision.of(old)
        self.check((first,), "F-201", "retired")
        self.check((first, revised(old, first, status="active")), "F-201", "active")

    def test_a_legacy_supersession_is_overridden_by_a_stored_retired(self) -> None:
        successor = Revision.of(Record.from_json(finding("F-202", "Invented successor")))
        old = migrated(finding("F-201", "Invented"), "legacy:superseded-by F-202", implied="superseded", superseded_by=("F-202",))
        first = Revision.of(old)
        self.check((successor, first), "F-201", "superseded")
        later = (successor, first, revised(old, first, status="retired"))
        self.check(later, "F-201", "retired")
        self.assertEqual((), state_of(*later).successors("F-201"), "the legacy edge no longer holds")

    def test_a_legacy_resolution_is_overridden_by_a_stored_open(self) -> None:
        old = migrated(unknown("U-201", "Invented?"), "legacy:resolved", implied="resolved")
        first = Revision.of(old)
        self.check((first,), "U-201", "resolved")
        self.check((first, revised(old, first, status="open")), "U-201", "open")

    def test_active_to_retired_and_to_superseded_by_a_link(self) -> None:
        old = migrated(finding("F-201", "Invented"), "active")
        first = Revision.of(old)
        self.check((first, revised(old, first, status="retired")), "F-201", "retired")
        by_link = Revision.of(Record.from_json(finding("F-203", "Invented replacement", supersedes=["F-201"])))
        self.check((first, by_link), "F-201", "superseded")
        reopened = revised(old, first, status="retired")
        self.check((first, reopened, by_link), "F-201", "superseded")

    def test_a_note_keeps_the_legacy_status_and_an_evidence_edge_follows_the_rule(self) -> None:
        successor = Revision.of(Record.from_json(finding("F-202", "Invented successor")))
        old = migrated(finding("F-201", "Invented"), "legacy:superseded-by F-202", implied="superseded", superseded_by=("F-202",))
        first = Revision.of(old)
        self.check((successor, first, revised(old, first, notes=["Invented note"])), "F-201", "superseded")
        receipt = migrated(evidence("M-201", "work/a.txt"), "retired", superseded_by=("M-202",))
        newer = Revision.of(Record.from_json(evidence("M-202", "work/b.txt")))
        start = Revision.of(receipt)
        self.check((newer, start), "M-201", "superseded")
        self.check((newer, start, revised(receipt, start, status="active", notes=["Invented: back in use"])), "M-201", "active")


class WithdrawnForGoodTests(unittest.TestCase):
    """Once a revision stores another status, storing the migration status again does not bring the legacy status back."""

    newer: Revision = Revision.of(Record.from_json(evidence("M-202", "work/b.txt")))
    receipt: Record = migrated(evidence("M-201", "work/a.txt"), "retired", superseded_by=("M-202",))
    start: Revision = Revision.of(receipt)

    def test_active_then_retired_replays_to_plain_retired(self) -> None:
        active = revised(self.receipt, self.start, status="active", notes=["Invented: back in use"])
        again = Revision.of(Record.from_json({**active.record.to_json(), "status": "retired", "notes": ["Invented: back in use", "Invented: done"]}),
                            [active.rev])
        steps = [(self.newer, self.start), (self.newer, self.start, active), (self.newer, self.start, active, again)]
        self.assertEqual(["superseded", "active", "retired"], [state_of(*step).status("M-201") for step in steps])
        state = state_of(*steps[-1])
        self.assertEqual(([], (), "retired"), (revision_problems(state), state.successors("M-201"), derived_status(state, again.record)))

    def test_a_note_only_chain_keeps_the_legacy_status(self) -> None:
        one = revised(self.receipt, self.start, notes=["Invented: one"])
        two = Revision.of(Record.from_json({**one.record.to_json(), "notes": ["Invented: one", "Invented: two"]}), [one.rev])
        state = state_of(self.newer, self.start, one, two)
        self.assertEqual(("superseded", ("M-202",)), (state.status("M-201"), state.successors("M-201")))

    def test_each_head_of_a_fork_is_judged_on_its_path_and_a_reconciliation_on_both(self) -> None:
        withdrew = revised(self.receipt, self.start, status="active", notes=["Invented: withdrew"])
        kept = revised(self.receipt, self.start, notes=["Invented: kept"])
        forked = state_of(self.newer, self.start, withdrew, kept)
        entry = forked.entries["M-201"]
        self.assertEqual({withdrew.rev: False, kept.rev: True}, {rev: entry.legacy_holds(rev) for rev in entry.heads})
        reconciled = Revision.of(Record.from_json({**self.receipt.to_json(), "notes": ["Invented: withdrew", "Invented: kept"]}),
                                 [withdrew.rev, kept.rev])
        state = state_of(self.newer, self.start, withdrew, kept, reconciled)
        self.assertEqual(("retired", ()), (state.status("M-201"), state.successors("M-201")), "the withdrawing path wins")


if __name__ == "__main__":
    _ = unittest.main()
