# SPDX-License-Identifier: AGPL-3.0-or-later
"""The text a record renders to: the legacy cells of a migrated record and a new finding's limits."""

from __future__ import annotations

import unittest
from dataclasses import replace

from tools.memory.canonical import render_text
from tools.memory.model import JSON, Record
from tools.tests.memory_fixtures import finding

ROW = '{"id":"M-101","path":"work/case/","sha256":null,"class":"local-private","kind":"input","date":null,"label":"Old","status":"retired","superseded_by":"M-102"}'


def migrated_finding(**extra: JSON) -> dict[str, JSON]:
    data: dict[str, JSON] = {
        "id": "F-101", "type": "finding", "status": "active", "claim": "Widget frames gadgets", "kind": "observed", "confidence": "high",
        "area": "widgets", "refs": ["U-101"], "cites": ["M-101"], "gaps": ["scope", "standing"],
        "legacy": {"path": "docs/ledgers/findings/widgets.md", "cells": {
            "claim": "Widget frames gadgets", "kind": "observed", "evidence": "static; M-101; c0ffee1", "scope_and_limits": "high; one run; prompt unread",
            "status": "active", "unknowns": "U-101", "original": "|  F-101 | raw line |"}},
    }
    data.update(extra)
    return data


class RenderTests(unittest.TestCase):
    def test_a_migrated_row_shows_every_cell_no_field_holds_next_to_its_claim(self) -> None:
        self.assertEqual(render_text(Record.from_json(migrated_finding())), "\n".join((
            "F-101 finding active", "Widget frames gadgets", "limits: high; one run; prompt unread", "evidence: static; M-101; c0ffee1",
            "unknowns: U-101", "area: widgets", "kind: observed", "confidence: high", "cites: M-101", "refs: U-101", "gaps: scope, standing",
            "migrated from docs/ledgers/findings/widgets.md")))

    def test_a_derived_status_never_hides_the_status_cell(self) -> None:
        data = migrated_finding(status="legacy:superseded-by F-102")
        legacy = data["legacy"]
        assert isinstance(legacy, dict) and isinstance(cells := legacy["cells"], dict)
        cells["status"] = "superseded-by F-102"
        legacy["superseded_by"] = ["F-102"]
        record = Record.from_json(data)
        stored, derived = render_text(record), render_text(replace(record, status="superseded"))
        self.assertNotIn("status: ", stored)
        self.assertIn("\nstatus: superseded-by F-102\n", derived)
        self.assertIn("\nsuperseded by (legacy row): F-102\n", stored)

    def test_a_status_cell_differing_only_in_case_is_the_mapped_status(self) -> None:
        record = Record.from_json({
            "id": "CP-001", "type": "compatibility", "status": "observed", "name": "Widgets", "required_proof": "Session (U-101)",
            "refs": ["U-101"], "assigned_at_migration": True, "gaps": ["qualifier", "scope"],
            "legacy": {"path": "docs/ledgers/compatibility.md", "cells": {
                "subsurface": "Widgets", "status": "Observed", "evidence": "F-102; [spec](../specs/x.md)", "remaining_gate": "Session (U-101)"}},
        })
        self.assertEqual(render_text(record).split("\n")[:4], ["CP-001 compatibility observed", "Widgets", "evidence: F-102; [spec](../specs/x.md)",
                                                               "required_proof: Session (U-101)"])

    def test_an_evidence_row_keeps_its_stored_status_visible(self) -> None:
        record = Record.from_json({
            "id": "M-101", "type": "evidence", "status": "retired", "path": "work/case/", "class": "local-private", "kind": "input", "label": "Old",
            "gaps": ["date", "sha256", "size"], "legacy": {"path": "evidence/x/001.jsonl", "cells": {"row": ROW}, "superseded_by": ["M-102"]},
        })
        self.assertNotIn("status: ", render_text(record))
        self.assertIn("\nstatus: retired\n", render_text(replace(record, status="superseded")))

    def test_a_new_finding_states_its_limits_after_its_claim(self) -> None:
        record = Record.from_json(finding("F-101", "Invented claim", limits="one run; client unread"))
        self.assertEqual(render_text(record).split("\n")[1:3], ["Invented claim", "limits: one run; client unread"])


if __name__ == "__main__":
    _ = unittest.main()
