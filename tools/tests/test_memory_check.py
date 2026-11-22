# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory check --ci``, ``get --offline`` and ``protected-paths`` in invented Git repositories."""

from __future__ import annotations

import io
import json
import os
from hashlib import sha256
import tempfile
import unittest
from contextlib import chdir, redirect_stderr, redirect_stdout
from pathlib import Path
from typing import override
from unittest.mock import patch

from tools.memory.check import Report, citations, revision_problems, run_check
from tools.memory.cli import EXIT_NO_JOURNAL, main
from tools.memory.journal import Revision, Segment, read_tree, write_change_set
from tools.memory.model import JSON, Record, as_mapping, is_array, load_json
from tools.memory.paths import ProtectedPathsError, protected_paths
from tools.memory.state import replay
from tools.tests.memory_fixtures import (
    CREATED, DIGEST, NEW_CLIENT, NEW_SERVER, chain, clean_git_environment, commit_all, evidence, explicit, finding, new_repository,
    release, run_git, write,
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
        self.assertIn("segment of origin/main (merge base", "\n".join(self.check().problems))
        for path in (self.root / "memory/journal").rglob("*.jsonl"):
            path.unlink()
        report = self.check()
        self.assertFalse(report.no_journal)
        self.assertEqual("no journal: memory/journal/ holds no segment, and this repository requires one", report.problems[0])
        self.assertIn("removed or changed", report.problems[1])
        self.assertEqual(["src/old.rs:1: F-101: no record F-101"], report.problems[2:])

    def test_required_journal(self) -> None:
        commit_all(self.root)
        report = run_check(self.root, "origin/main", require_journal=True)
        self.assertTrue(report.no_journal)
        self.assertEqual(["no journal: memory/journal/ holds no segment, and this repository requires one"], report.problems)

    def test_a_branch_behind_its_base_is_not_blamed_for_what_the_base_gained(self) -> None:
        self.accept_chain()
        _ = run_git(self.root, "switch", "-q", "-c", "feature")
        _ = run_git(self.root, "switch", "-q", "main")
        _ = write(self.root, [finding("F-407", "Invented later observation")], second=1)
        self.put("docs/gates.md", "Invented gate\n")
        self.put("src/later.rs", "// F-101\n")
        commit_all(self.root)
        _ = run_git(self.root, "switch", "-q", "feature")
        self.assertEqual(Report(), self.check())
        self.revise(dict(chain()[1], claim="Invented: rewritten claim"), [self.head("F-102").rev], 2)
        self.assertIn("claim changed", "\n".join(self.check().problems))

    def test_unrelated_histories_fail_clearly(self) -> None:
        self.accept_chain()
        _ = run_git(self.root, "switch", "-q", "--orphan", "stray")
        _ = run_git(self.root, "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", "stray")
        report = self.check()
        self.assertEqual(1, len(report.problems))
        self.assertIn("no merge base of origin/main and HEAD", report.problems[0])

    def test_unknown_base_is_named_as_the_cause(self) -> None:
        self.accept_chain()
        report = run_check(self.root, "origin/absent")
        self.assertEqual(["origin/absent is not a known commit here; fetch it before checking"], report.problems)

    def test_forks_against_the_base_fail_and_inherited_forks_are_notices(self) -> None:
        self.accept_chain()
        base = self.head("U-101").rev
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
        self.revise(dict(chain()[1], claim="Invented: rewritten claim"), [self.head("F-102").rev], 1)
        self.revise(dict(chain()[7], status="missing"), [self.head("M-102").rev], 2)
        problems = "\n".join(self.check().problems)
        self.assertIn("claim changed; it is not revisable", problems)
        self.assertIn("an evidence status change appends a note", problems)

    def test_appended_notes_and_status_with_note_pass(self) -> None:
        self.accept_chain()
        self.revise(dict(chain()[7], status="missing", notes=["Invented: the case was lost"]), [self.head("M-102").rev], 1)
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


    def test_a_branch_from_before_the_cut_over_is_told_so_with_the_rows_it_adds(self) -> None:
        header = "# Findings: Widgets\n\n| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |\n| --- | --- | --- | --- | --- | --- | --- |\n"
        self.put("docs/ledgers/findings/widgets.md", header + "| F-090 | Invented old claim | observed | review | high; server 29990001 | | active |\n")
        commit_all(self.root, "legacy era")
        _ = run_git(self.root, "checkout", "-q", "-b", "old")
        self.put("docs/ledgers/findings/widgets.md", header + "| F-090 | Invented old claim | observed | review | high; server 29990001 | | active |\n"
                 + "| F-099 | Invented row added on the old branch | observed | review | high | | active |\n")
        _ = run_git(self.root, "commit", "-q", "-am", "old branch row")
        _ = run_git(self.root, "checkout", "-q", "main")
        _ = run_git(self.root, "rm", "-r", "-q", "docs/ledgers")
        self.accept_chain()
        _ = run_git(self.root, "checkout", "-q", "old")
        report = self.check(frozen=True)
        self.assertFalse(report.no_journal)
        self.assertEqual(2, len(report.problems))
        self.assertTrue(report.problems[0].startswith("this branch predates the cut-over: it holds the legacy ledgers or evidence index and no journal, "
                                                      + "while origin/main holds the journal. Salvage BEFORE merging main"))
        self.assertIn("run memory salvage <this branch> --out <dir> on this tip, commit the proposals, then merge main", report.problems[0])
        self.assertEqual(["F-099: legacy finding row added or changed on this branch; memory salvage turns it into a proposal"], report.problems[1:])


def revised(old: dict[str, JSON], changes: dict[str, JSON], body: tuple[str, str] | None = None) -> list[str]:
    """Problems of one revision of ``old`` applying ``changes`` (``body``: the REF body before and after)."""
    first = Revision.of(Record.from_json(old, body[0] if body else None))
    second = Revision.of(Record.from_json(dict(old, **changes), body[1] if body else None), [first.rev])
    return [problem.split(": ", 2)[2] for problem in revision_problems(replay([Segment("s", "t", (first, second))]))]


def body_digest(body: str) -> str:
    return sha256(body.encode()).hexdigest()


class RevisabilityTests(unittest.TestCase):
    def assert_revisions(self, old: dict[str, JSON], allowed: dict[str, JSON], rejected: dict[str, JSON]) -> None:
        self.assertEqual([], revised(old, allowed))
        self.assertEqual([f"{name} changed; it is not revisable (supersede with a new record)" for name in sorted(rejected)],
                         revised(old, rejected))

    def test_one_allowed_and_one_rejected_revision_per_type(self) -> None:
        scope = explicit(subjects=("tooling",))
        self.assert_revisions(chain()[0], {"status": "retired", "notes": ["Invented note"]}, {"confidence": "high"})
        self.assert_revisions(chain()[0], {"verified_on": [{"build": f"server:{NEW_SERVER}", "cites": ["M-102"]}]}, {"area": "other"})
        self.assert_revisions(chain()[5], {"status": "blocked", "next_probe": "Another invented probe"}, {"question": "Changed?"})
        self.assert_revisions(chain()[5], {"status": "investigating"}, {"severity": "high"})
        self.assert_revisions({"id": "N-101", "type": "bug", "status": "open", "trigger": "Invented", "change": "Invented fix",
                               "refs": ["F-101"], "tests": ["invented_test"]}, {"status": "fixed"}, {"tests": ["other_test"]})
        self.assert_revisions({"id": "SL-101", "type": "lead", "status": "open", "lead": "Invented", "static_evidence": "Invented",
                               "refute_by": "Invented"}, {"status": "refuted"}, {"refute_by": "Other"})
        self.assert_revisions({"id": "RP-101", "type": "report", "status": "unreproduced", "problem": "Invented", "priority": 1,
                               "next_evidence": "Invented"}, {"status": "reproduced"}, {"priority": 2})
        self.assert_revisions({"id": "CP-101", "type": "compatibility", "status": "unknown", "name": "Invented", "qualifier": "Invented",
                               "required_proof": "Invented", "scope": scope}, {"status": "observed"}, {"qualifier": "Other"})
        self.assert_revisions(release("REL-9.0.2", NEW_SERVER, NEW_CLIENT), {"status": "past"}, {"sources": ["other"]})
        self.assert_revisions(chain()[7], {"status": "missing", "notes": ["Invented: lost"]}, {"label": "Other label"})
        self.assertEqual(["an evidence status change appends a note"], revised(chain()[7], {"status": "missing"}))
        self.assertEqual(["notes may only be appended to"], revised(dict(chain()[7], notes=["a", "b"]), {"notes": ["b", "a"]}))

    def test_supersedes_and_resolves_are_append_only_cites_and_refs_immutable(self) -> None:
        accepted = chain()[4]
        self.assertEqual([], revised(accepted, {"supersedes": ["F-101", "F-102"]}))
        self.assertEqual([], revised(accepted, {"resolves": ["U-101"]}))
        self.assertEqual(["supersedes may only be appended to"], revised(accepted, {"supersedes": ["F-101"]}))
        self.assertEqual(["resolves may only be appended to"], revised(chain()[2], {"resolves": ["U-102"]}))
        self.assertEqual(["cites changed; it is not revisable (supersede with a new record)"], revised(accepted, {"cites": ["M-101"]}))
        self.assertEqual(["refs changed; it is not revisable (supersede with a new record)"], revised(accepted, {"refs": ["F-104"]}))

    def test_reference_body_is_revisable_and_its_title_is_not(self) -> None:
        header: dict[str, JSON] = {"id": "REF-101", "type": "reference", "status": "current", "title": "Invented spec",
                                   "scope": explicit(subjects=("tooling",)), "body_sha256": body_digest("one\n")}
        self.assertEqual([], revised(header, {"body_sha256": body_digest("two\n"), "status": "retired"}, ("one\n", "two\n")))
        self.assertEqual(["title changed; it is not revisable (supersede with a new record)"],
                         revised(header, {"title": "Other"}, ("one\n", "one\n")))

    def test_decision_quote_and_legacy_never_change(self) -> None:
        decision = finding("F-404", "Owner chose invented policy", kind="decision", scope={"basis": "explicit", "subjects": ["decision"]},
                           instruction="Do it.", instruction_date="2026-11-20", instruction_source="invented thread")
        self.assertEqual(["instruction changed; it is not revisable (supersede with a new record)"], revised(decision, {"instruction": "Do it now."}))
        self.assertEqual(["instruction_date changed; it is not revisable (supersede with a new record)"],
                         revised(decision, {"instruction_date": "2026-11-21"}))
        legacy: JSON = {"path": "docs/ledgers/findings/invented.md", "cells": {"status": "active"}}
        migrated = finding("F-201", "Invented legacy", legacy=legacy, gaps=["standing"])
        del migrated["standing"]
        self.assertEqual(["legacy changed; it is not revisable (supersede with a new record)"],
                         revised(migrated, {"legacy": {"path": "docs/ledgers/findings/other.md", "cells": {"status": "active"}}}))
        self.assertEqual(["a revised status takes a value from the vocabulary"], revised(migrated, {"status": "legacy:bounded"}))


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
        self.assertTrue(output.startswith("F-101 finding superseded\nInvented: the prompt"), "the text renders the derived status, as the bank holds it")
        for line in ("state: accepted", "status: superseded (link from F-102)", "partition: other-build against REL-9.0.2; tokens server:90000001",
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
        self.assertIn("connection-v2.env", error)


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
