# SPDX-License-Identifier: AGPL-3.0-or-later
"""The text a record renders to (legacy cells of a migrated record, a new finding's limits) and the
rendering version the bank's marker carries, so tools of different versions never rewrite each other."""

from __future__ import annotations

import os
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path
from typing import override
from unittest.mock import patch

from tools.memory.bank import MARKER, OlderTool, gather, marker_rendering, reconcile
from tools.memory.canonical import RENDERING, render_text
from tools.memory.hindsight import Item
from tools.memory.model import JSON, Record
from tools.memory.service import home
from tools.tests.memory_fixtures import NEW_SERVER, chain, clean_git_environment, commit_all, finding, new_repository, write
from tools.tests.test_memory_online import FakeBank

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


class RenderingVersionTests(unittest.TestCase):
    root: Path = Path()
    bank: FakeBank = FakeBank()

    @override
    def setUp(self) -> None:
        top = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.enterContext(clean_git_environment())
        self.enterContext(patch.dict(os.environ, {"ICARUS_MEMORY_HOME": str(top / "home")}))
        home().state.mkdir(parents=True, mode=0o700)
        self.root = top / "primary"
        self.root.mkdir()
        new_repository(self.root)
        release = self.root / "config/release.toml"
        release.parent.mkdir(parents=True)
        _ = release.write_text(f"[release]\nbuild_id = {NEW_SERVER}\n", encoding="utf-8")
        _ = write(self.root, chain())
        commit_all(self.root)
        self.bank = FakeBank()

    def held_text(self, document_id: str) -> str:
        document = self.bank.document(document_id)
        assert document is not None and document.text is not None
        return document.text

    def test_a_newer_tool_re_renders_and_raises_the_marker(self) -> None:
        _ = reconcile(self.bank, gather(self.root))
        expected = self.held_text("F-101")
        held = self.bank.held["F-101"]
        # A bank written by a tool from before rendering versions: older text, a marker without a version.
        self.bank.put(Item("F-101", "F-101 finding active\nolder rendering", held.tags, held.metadata, "unset", held.entities))
        self.bank.put(Item(MARKER, "memory stamp " + "0" * 64, ("marker:stamp",), {"stamp": "0" * 64}, "unset", ()))
        self.assertEqual(marker_rendering(self.bank), 1)
        _ = reconcile(self.bank, gather(self.root))
        self.assertEqual(self.held_text("F-101"), expected)
        self.assertEqual(marker_rendering(self.bank), RENDERING)
        self.assertTrue(self.held_text(MARKER).endswith(f" rendering {RENDERING}"))

    def test_an_older_tool_refuses_to_write_a_newer_bank(self) -> None:
        _ = reconcile(self.bank, gather(self.root))
        newer = f"memory stamp {'1' * 64} rendering {RENDERING + 1}"
        self.bank.put(Item(MARKER, newer, ("marker:stamp",), {"stamp": "1" * 64}, "unset", ()))
        held = self.bank.held["F-101"]
        self.bank.put(Item("F-101", "F-101 finding active\nnewer rendering", held.tags, held.metadata, "unset", held.entities))
        with self.assertRaisesRegex(OlderTool, f"version {RENDERING}, older than the bank's {RENDERING + 1}.*merge main"):
            _ = reconcile(self.bank, gather(self.root))
        self.assertEqual(self.held_text("F-101"), "F-101 finding active\nnewer rendering")
        self.assertEqual(self.held_text(MARKER), newer)


if __name__ == "__main__":
    _ = unittest.main()
