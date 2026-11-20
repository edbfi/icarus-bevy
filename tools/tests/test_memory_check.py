# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory check --ci``, ``get --offline`` and ``protected-paths`` in invented Git repositories."""

from __future__ import annotations

import io
import json
import os
import tempfile
import unittest
from contextlib import chdir, redirect_stderr, redirect_stdout
from pathlib import Path
from typing import override
from unittest.mock import patch

from tools.memory.check import Report, citations, run_check
from tools.memory.cli import EXIT_NO_JOURNAL, main
from tools.memory.journal import Revision, read_tree, write_change_set
from tools.memory.model import JSON, Record, as_mapping, is_array, load_json
from tools.memory.paths import ProtectedPathsError, protected_paths
from tools.tests.memory_fixtures import (
    CREATED, DIGEST, NEW_SERVER, chain, clean_git_environment, commit_all, evidence, finding, new_repository, run_git, write,
)


class Repository(unittest.TestCase):
    top: Path = Path()
    root: Path = Path()

    @override
    def setUp(self) -> None:
        self.top = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.root = self.top / "primary"
        self.root.mkdir()
        self.enterContext(clean_git_environment())
        new_repository(self.root)

    def put(self, path: str, text: str) -> None:
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        _ = target.write_text(text, encoding="utf-8")

    def head(self, identifier: str) -> Revision:
        segments = read_tree(self.root)
        return next(item for segment in segments for item in segment.revisions if item.record.id == identifier)

    def revise(self, data: dict[str, JSON], prev: list[str], second: int) -> None:
        _ = write_change_set(self.root, CREATED.replace(second=second), [Revision.of(Record.from_json(data), prev)])


class CitationScanTests(unittest.TestCase):
    def test_citation_forms(self) -> None:
        text = "F-905 to F-910, F-521..F-525; M-1382/ev_7bf6c1e19cd8 fn deltas_f978() M-1404_full_tree x-F-123 REL-9.0.2.\nSL-001 M-1198/build1"
        self.assertEqual(
            [(1, "F-905"), (1, "F-910"), (1, "F-521"), (1, "F-525"), (1, "M-1382/ev_7bf6c1e19cd8"), (1, "REL-9.0.2"), (2, "SL-001"), (2, "M-1198")],
            citations(text),
        )


class CheckTests(Repository):
    def check(self, *, frozen: bool = False) -> Report:
        return run_check(self.root, "origin/main", legacy_frozen=frozen)

    def accept_chain(self) -> None:
        _ = write(self.root, chain())
        self.put("src/old.rs", "// Retired before this change (F-101).\n")
        commit_all(self.root)

    def test_no_journal_is_reported_distinctly(self) -> None:
        commit_all(self.root)
        self.assertTrue(self.check().no_journal)
        with chdir(self.root), redirect_stdout(io.StringIO()) as output:
            self.assertEqual(EXIT_NO_JOURNAL, main(["check", "--ci"]))
        self.assertIn("no journal", output.getvalue())

    def test_accepted_chain_is_clean(self) -> None:
        self.accept_chain()
        self.assertEqual(Report(), self.check())

    def test_citations(self) -> None:
        self.accept_chain()
        self.put("src/old.rs", "// Retired before this change (F-101).\n// Still F-101.\n")
        self.put("src/new.rs", "// F-101 to F-106 and M-101/ev_a1b2c3d4e5f6; fn deltas_f978() {}\n")
        self.put("docs/note.md", "F-999, M-101/ev_ffffffffffff and M-1404_full_tree.\n")
        self.put("tools/tests/fake.py", "# F-999\n")
        self.put("docs/history/old.md", "F-999\n")
        self.assertEqual([
            "docs/note.md:1: F-999: no record F-999",
            "docs/note.md:1: M-101/ev_ffffffffffff: M-101/ev_ffffffffffff: ev_ffffffffffff matches 0 listed members of M-101",
            "src/new.rs:1: new citation of superseded F-101; cite F-106",
        ], self.check().problems)

    def test_accepted_segments_are_immutable(self) -> None:
        self.accept_chain()
        for path in (self.root / "memory/journal").rglob("*.jsonl"):
            path.unlink()
        _ = write(self.root, chain()[:3])
        self.assertIn("segment of origin/main removed or changed", "\n".join(self.check().problems))

    def test_forks_against_the_base_fail_and_inherited_forks_are_notices(self) -> None:
        self.accept_chain()
        base = self.head("U-101").sha256
        self.revise(dict(chain()[5], status="investigating"), [base], 1)
        self.assertEqual([], self.check().problems)
        self.revise(dict(chain()[5], status="blocked"), [base], 2)
        report = self.check()
        self.assertTrue(any(problem.startswith("U-101: forks the record against origin/main") for problem in report.problems))
        commit_all(self.root)
        report = self.check()
        self.assertEqual([], report.problems)
        self.assertTrue(any(notice.startswith("U-101: unreconciled fork") for notice in report.notices))

    def test_fields_that_are_not_revisable(self) -> None:
        self.accept_chain()
        self.revise(dict(chain()[1], claim="Invented: rewritten claim"), [self.head("F-102").sha256], 1)
        self.revise(dict(chain()[7], status="missing"), [self.head("M-102").sha256], 2)
        problems = "\n".join(self.check().problems)
        self.assertIn("claim changed; it is not revisable", problems)
        self.assertIn("an evidence status change appends a note", problems)

    def test_appended_notes_and_status_with_note_pass(self) -> None:
        self.accept_chain()
        self.revise(dict(chain()[7], status="missing", notes=["Invented: the case was lost"]), [self.head("M-102").sha256], 1)
        self.assertEqual([], self.check().problems)

    def test_derived_high_needs_accepted_observed_sources(self) -> None:
        self.accept_chain()
        _ = write(self.root, [
            finding("F-401", "Invented derived", kind="derived", confidence="high", refs=["F-103"]),
            finding("F-402", "Invented derived from a proposal", kind="derived", confidence="high", refs=["F-403"]),
            finding("F-403", "Invented new observation"),
        ], second=1)
        self.assertEqual(
            ["F-402: confidence high needs every source an accepted observed or measured finding; F-403 is not"], self.check().problems,
        )

    def test_decision_and_gate_notices(self) -> None:
        self.accept_chain()
        self.put("docs/gates.md", "Invented gate\n")
        _ = write(self.root, [finding("F-404", "Owner chose invented policy", kind="decision", scope={"basis": "explicit", "subjects": ["decision"]},
                                      instruction="Do it.", instruction_date="2026-11-20", instruction_source="invented thread")], second=1)
        report = self.check()
        self.assertEqual([], report.problems)
        self.assertEqual(2, len(report.notices))
        self.assertTrue(report.notices[0].startswith("docs/gates.md changes"))
        self.assertTrue(report.notices[1].startswith("F-404: adds or revises a decision finding"))

    def test_salvage_check_is_off_until_cut_over(self) -> None:
        self.accept_chain()
        self.put("evidence/area/001.jsonl", "{}\n")
        self.assertEqual([], self.check().problems)
        self.assertEqual(["evidence/area/001.jsonl: adds or modifies legacy rows; turn them into proposals with memory salvage"],
                         self.check(frozen=True).problems)


class GetTests(Repository):
    def run_get(self, *args: str) -> tuple[int, str, str]:
        with chdir(self.root), redirect_stdout(io.StringIO()) as output, redirect_stderr(io.StringIO()) as error:
            status = main(["get", *args])
        return status, output.getvalue(), error.getvalue()

    def test_get_offline(self) -> None:
        self.put("config/release.toml", f"[release]\nbuild_id = {NEW_SERVER}\n")
        _ = write(self.root, chain())
        commit_all(self.root)
        _ = write(self.root, [finding("F-405", "Invented proposal")], second=1)
        status, output, _ = self.run_get("F-101", "--offline")
        self.assertEqual(0, status)
        self.assertTrue(output.startswith("F-101 finding active\nInvented: the prompt"))
        for line in ("state: accepted", "status: superseded (stored active)", "partition: other-build against REL-9.0.2; tokens server:90000001",
                     "superseded by: F-102 (terminal: F-106)"):
            self.assertIn(line, output)
        self.assertIn("state: proposed", self.run_get("F-405", "--offline")[1])
        status, output, _ = self.run_get("U-101", "--offline", "--json", "--history")
        result = as_mapping(load_json(output), "get")
        history = result["history"]
        self.assertEqual(["resolved", ["F-103"], 1], [result["status"], result["resolved_by"], len(history) if is_array(history) else None])
        self.assertEqual(output, self.run_get("U-101", "--offline", "--json", "--history")[1])
        status, output, _ = self.run_get("M-101/ev_a1b2c3d4e5f6", "--offline")
        self.assertIn("listed member ev_a1b2c3d4e5f6", output)
        status, output, error = self.run_get("F-999", "--offline")
        self.assertEqual((1, "", "memory get: no record F-999\n"), (status, output, error))
        with patch.dict(os.environ, {"ICARUS_MEMORY_HOME": str(self.top / "no-service")}):
            status, _, error = self.run_get("F-101")
        self.assertEqual(1, status)
        self.assertIn("connection.json", error)


class ProtectedPathsTests(Repository):
    def shard(self, *rows: dict[str, JSON]) -> None:
        self.put("evidence/area/001.jsonl", "".join(json.dumps(row) + "\n" for row in rows))

    def test_union_over_worktrees_resolved_against_the_primary(self) -> None:
        def row(identifier: str, path: str, status: str = "active") -> dict[str, JSON]:
            return {"id": identifier, "path": path, "sha256": DIGEST, "class": "local-private", "kind": "receipt",
                    "date": "2026-11-20", "label": "Invented", "status": status}

        self.shard(row("M-001", "work/case/fixity.sha256"), row("M-002", "work/old/", "retired"), row("M-003", "reference-data/"))
        self.put("work/case/fixity.sha256", f"{DIGEST}  a.bin\n{DIGEST}  missing.bin\n")
        self.put("work/case/a.bin", "a")
        _ = write(self.root, [evidence("M-004", "work/journal-case/file.txt"), evidence("M-005", "work/gone/", status="retired")])
        commit_all(self.root)
        linked = self.top / "linked"
        _ = run_git(self.root, "worktree", "add", "-q", "--detach", str(linked))
        _ = write_change_set(linked, CREATED.replace(second=1), [Revision.of(Record.from_json(evidence("M-006", "work/linked-case/x.txt")))])
        result = protected_paths(linked)
        self.assertEqual(
            ("reference-data/", "work/case/a.bin", "work/case/fixity.sha256", "work/journal-case/file.txt", "work/linked-case/x.txt"),
            result.paths,
        )
        self.assertEqual(("M-001: manifest entry missing: missing.bin",), result.notices)

    def test_unreadable_input_fails_closed(self) -> None:
        commit_all(self.root)
        self.shard({"id": "M-001", "path": "work/x"})
        with self.assertRaisesRegex(ProtectedPathsError, "missing keys"):
            _ = protected_paths(self.root)
        self.shard({"id": "M-001", "path": "work/x.sha256", "sha256": DIGEST, "class": "local-private", "kind": "receipt",
                    "date": "2026-11-20", "label": "Invented", "status": "active"})
        with self.assertRaisesRegex(ProtectedPathsError, "M-001: unreadable sha256 manifest"):
            _ = protected_paths(self.root)
        (self.root / "evidence/area/001.jsonl").unlink()
        self.put("memory/journal/2026/11/broken.jsonl", "{}\n")
        with self.assertRaisesRegex(ProtectedPathsError, "not a segment path"):
            _ = protected_paths(self.root)
        with chdir(self.root), redirect_stderr(io.StringIO()) as error:
            self.assertEqual(1, main(["protected-paths"]))
        self.assertIn("treat every path as protected", error.getvalue())


if __name__ == "__main__":
    _ = unittest.main()
