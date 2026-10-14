"""Authored fixtures for the documentation lint; no repository files are read."""

import contextlib
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from datetime import date
from typing import final
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools import docs_lint  # noqa: E402

DIGEST = "a" * 64
STATUS_LEGACY = "# Status\n\n## Summary\n\n## Milestones\n\n## Works\n\n## Does not work\n\n## Gates\n\n## Next tasks\n\nLast reconciled: 2026-10-13\n"
STATUS_OK = "# Status\n\nLast reconciled: 2026-10-13\n\n## Summary\n\n## Milestones\n\n## Next tasks\n"
DIGEST_OK = "# Digest\n\n## Question cluster\n\n## Current answer\n\n## Open\n\n## Artifacts\n"
DIGEST_LEGACY = "# Digest\n\n## Question cluster\n\n## Answered\n\n## Partial\n\n## Unresolved\n\n## Limitations and exclusions\n\n## Receipts\n"
FINDINGS_HEAD = "# F\n\nLast reconciled: 2026-10-13\n\n| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |\n| --- | --- | --- | --- | --- | --- | --- |\n"
TODAY = date(2026, 10, 14)
SPEC_OK = "# Spec\n\n## Status\n\nLast reconciled: 2026-10-13\n\n## Scope\n\n## Confirmed\n\n## Candidate\n\n## Open questions\n\n## Evidence\n"
EV_A = "ev_" + "1" * 12 + "a" * 52
EV_B = "ev_" + "1" * 12 + "b" * 52
EV_C = "ev_" + "2" * 64
UNK_A = "unk_" + "3" * 64


def record(number: int, **fields: object) -> str:
    """One evidence index line for M-<number>; defaults describe a valid new row."""
    row: dict[str, object] = {
        "id": f"M-{number:03d}",
        "path": f"work/case-{number}/receipt.json",
        "sha256": DIGEST,
        "class": "local-private",
        "kind": "receipt",
        "date": "2026-10-13",
        "label": f"Receipt {number}",
        "status": "active",
    }
    for key, value in fields.items():
        if value is None and key not in {"sha256", "date"}:
            _ = row.pop(key, None)
        else:
            row[key] = value
    return json.dumps(row, separators=(",", ":")) + "\n"


def legacy(number: int, **fields: object) -> str:
    return record(number, legacy=True, **fields)


@final
class Fixture:
    directory: tempfile.TemporaryDirectory[str]
    root: Path

    def __init__(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        (self.root / ".git").mkdir()
        (self.root / "docs").mkdir()

    def write(self, rel: str, text: str) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        _ = path.write_text(text, encoding="utf-8")

    def linter(self, *, local: bool = False, base: str = "HEAD", today: date = TODAY) -> docs_lint.Linter:
        linter = docs_lint.Linter(self.root, local=local, base=base, today=today)
        _ = linter.run()
        return linter

    def lint(self, *, local: bool = False, base: str = "HEAD") -> list[str]:
        return [problem.render() for problem in self.linter(local=local, base=base).problems]

    def git_init(self) -> None:
        (self.root / ".git").rmdir()
        _ = self.git("init", "-q")
        _ = self.commit("base", allow_empty=True)

    def git(self, *args: str) -> str:
        result = subprocess.run(["git", "-C", str(self.root), *args], check=True, capture_output=True, text=True)
        return result.stdout.strip()

    def commit(self, message: str, *, allow_empty: bool = False) -> str:
        _ = self.git("add", "-A")
        extra = ["--allow-empty"] if allow_empty else []
        _ = self.git("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", *extra, "-m", message)
        return self.git("rev-parse", "HEAD")

    def close(self) -> None:
        self.directory.cleanup()


@final
class DocsLintTest(unittest.TestCase):
    def make(self) -> Fixture:
        fixture = Fixture()
        self.addCleanup(fixture.close)
        return fixture

    def test_clean_tree_has_no_problems(self) -> None:
        fx = self.make()
        fx.write("README.md", "# Project\n\nSee [status](docs/implementation-status.md#summary).\n")
        fx.write("docs/implementation-status.md", STATUS_OK)
        fx.write("docs/specs/example.md", SPEC_OK)
        self.assertEqual(fx.lint(), [])

    def test_status_heading_set_is_exact(self) -> None:
        fx = self.make()
        fx.write("docs/implementation-status.md", STATUS_OK.replace("## Milestones", "## Milestones\n\n## Extra"))
        problems = fx.lint()
        self.assertTrue(any("H2 set must be exactly" in problem for problem in problems), problems)

    def test_dated_heading_rejected(self) -> None:
        fx = self.make()
        fx.write("docs/notes.md", "# Notes\n\n### Checkpoint — 2026-10-13, 06:28 UTC\n")
        problems = fx.lint()
        self.assertTrue(any("dated or timestamped heading" in problem for problem in problems), problems)

    def test_evidence_detail_in_prose_rejected_but_code_allowed(self) -> None:
        fx = self.make()
        fx.write("docs/plan.md", f"# Plan\n\nDigest {DIGEST} at 06:28 UTC under work/case/.\n\n```\n{DIGEST} work/case\n```\n")
        problems = fx.lint()
        self.assertEqual(len([p for p in problems if "docs/plan.md:3" in p]), 3, problems)
        self.assertFalse(any("docs/plan.md:6" in problem for problem in problems), problems)

    def test_runbook_and_ledger_cells_are_exempt(self) -> None:
        fx = self.make()
        fx.write("docs/runbooks/run.md", "# Run\n\nUse work/case-1 as the data directory.\n")
        fx.write("docs/ledgers/findings/a.md", f"# Findings\n\nLast reconciled: 2026-10-13\n\n| ID | claim |\n| --- | --- |\n| F-001 | digest {DIGEST} |\n")
        self.assertEqual(fx.lint(), [])

    def test_size_caps(self) -> None:
        fx = self.make()
        fx.write("docs/big.md", "# Big\n\n" + "x" * (docs_lint.DEFAULT_FILE_CAP + 1))
        problems = fx.lint()
        self.assertTrue(any("exceeds the" in problem for problem in problems), problems)
        fx.write("docs/ledgers/bugs.md", "# Bugs\n\nLast reconciled: 2026-10-13\n\n| a |\n| --- |\n| " + "y" * docs_lint.LEDGER_ROW_CAP + " |\n")
        problems = fx.lint()
        self.assertTrue(any("ledger row exceeds" in problem for problem in problems), problems)

    def test_aggregate_cap(self) -> None:
        fx = self.make()
        chunk = "z" * (docs_lint.DEFAULT_FILE_CAP - 100)
        for index in range(docs_lint.AGGREGATE_CAP // docs_lint.DEFAULT_FILE_CAP + 2):
            fx.write(f"docs/part{index}.md", f"# Part\n\n{chunk}\n")
        problems = fx.lint()
        self.assertTrue(any("current prose surface is" in problem for problem in problems), problems)

    def test_links_and_fragments(self) -> None:
        fx = self.make()
        fx.write("docs/a.md", "# A\n\n## Known section\n\n[ok](b.md#target) [bad](missing.md) [frag](b.md#nope) [self](#known-section) [out](../../etc)\n")
        fx.write("docs/b.md", "# B\n\n## Target\n")
        problems = fx.lint()
        self.assertTrue(any("dangling link: missing.md" in problem for problem in problems), problems)
        self.assertTrue(any("missing heading anchor: b.md#nope" in problem for problem in problems), problems)
        self.assertTrue(any("escapes the repository" in problem for problem in problems), problems)
        self.assertFalse(any("known-section" in problem for problem in problems), problems)

    def test_code_references_to_missing_docs(self) -> None:
        fx = self.make()
        fx.write("src/lib.rs", "//! See docs/specs/missing.md and docs/present.md\n")
        fx.write("docs/present.md", "# Present\n")
        problems = fx.lint()
        self.assertEqual(len([p for p in problems if "src/lib.rs" in p]), 1, problems)

    def test_ledger_ids(self) -> None:
        fx = self.make()
        fx.write("docs/ledgers/findings/a.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID | status |\n| --- | --- |\n| F-001 | superseded-by F-009 |\n| F-001 | active |\n")
        fx.write("docs/ledgers/unknowns.md", "# U\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| U-001 |\n")
        fx.write("docs/specs/s.md", SPEC_OK + "\nSee F-001, F-002, U-001, U-002.\n")
        problems = fx.lint()
        self.assertTrue(any("duplicate ID F-001" in problem for problem in problems), problems)
        self.assertTrue(any("superseded-by target F-009" in problem for problem in problems), problems)
        self.assertTrue(any("finding F-002 is not defined" in problem for problem in problems), problems)
        self.assertTrue(any("unknown U-002 is not defined" in problem for problem in problems), problems)
        self.assertFalse(any("F-001 is not defined" in problem for problem in problems), problems)

    def test_removed_ledger_id_detected_against_git_head(self) -> None:
        fx = self.make()
        fx.git_init()
        fx.write("docs/ledgers/findings/a.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| F-001 |\n| F-002 |\n")
        _ = fx.commit("ledger")
        fx.write("docs/ledgers/findings/a.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| F-001 |\n")
        problems = fx.lint()
        self.assertTrue(any("ID F-002 existed at HEAD" in problem for problem in problems), problems)

    def test_split_findings_keep_ids_across_files(self) -> None:
        fx = self.make()
        fx.git_init()
        fx.write("docs/ledgers/findings.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| F-001 |\n| F-002 |\n| F-003 |\n")
        _ = fx.commit("legacy ledger")
        (fx.root / "docs/ledgers/findings.md").unlink()
        fx.write("docs/ledgers/findings/a.md", FINDINGS_HEAD + "| F-001 | c | observed | review | s | none | superseded-by F-003 |\n")
        fx.write("docs/ledgers/findings/retired/a.md", FINDINGS_HEAD + "| F-003 | c | observed | review | s | none | active |\n")
        problems = fx.lint()
        self.assertTrue(any("ID F-002 existed at HEAD" in problem for problem in problems), problems)
        self.assertFalse(any("F-001" in problem or "F-003" in problem for problem in problems), problems)
        fx.write("docs/ledgers/findings/b.md", FINDINGS_HEAD + "| F-002 | c | observed | review | s | none | active |\n| F-003 | c | observed | review | s | none | active |\n")
        problems = fx.lint()
        self.assertFalse(any("F-002 existed" in problem for problem in problems), problems)
        self.assertTrue(any("duplicate ID F-003 (first at docs/ledgers/findings/b.md" in problem for problem in problems), problems)

    def test_base_revision_selects_surviving_ids(self) -> None:
        fx = self.make()
        fx.git_init()
        fx.write("docs/ledgers/unknowns.md", "# U\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| U-001 |\n| U-002 |\n")
        first = fx.commit("two unknowns")
        fx.write("docs/ledgers/unknowns.md", "# U\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| U-001 |\n")
        _ = fx.commit("dropped one")
        self.assertFalse(any("U-002" in problem for problem in fx.lint()))
        self.assertTrue(any(f"ID U-002 existed at {first}" in problem for problem in fx.lint(base=first)))

    def test_new_receipt_findings_rejected(self) -> None:
        fx = self.make()
        fx.git_init()
        fx.write("docs/ledgers/findings/a.md", FINDINGS_HEAD + "| F-001 | ran | observed | tool-receipt; M-001 | s | none | active |\n")
        _ = fx.commit("existing receipt row")
        fx.write("docs/ledgers/findings/a.md", FINDINGS_HEAD + "| F-001 | ran | observed | tool-receipt; M-001 | s | none | active |\n"
                 + "| F-002 | ran again | observed | tooling-receipt; M-002 | s | none | active |\n"
                 + "| F-003 | claim | observed | static-decompile; M-003 | s | none | active |\n")
        fx.write("evidence/foundation.jsonl", "".join(legacy(n) for n in (1, 2, 3)))
        problems = fx.lint()
        self.assertTrue(any("new finding F-002 is an execution receipt" in problem for problem in problems), problems)
        self.assertFalse(any("F-001" in problem or "F-003" in problem for problem in problems), problems)

    def test_findings_tree_caps(self) -> None:
        fx = self.make()
        row = "| F-{:03d} | " + "c" * 300 + " | observed | review | s | none | active |\n"
        rows = "".join(row.format(n) for n in range(1, 170))
        fx.write("docs/ledgers/findings/big.md", FINDINGS_HEAD + rows)
        fx.write("docs/ledgers/findings/retired/big.md", FINDINGS_HEAD + "".join(row.format(n) for n in range(200, 369)))
        problems = fx.lint()
        self.assertTrue(any("findings/big.md: " in problem and "cap" in problem for problem in problems), problems)
        self.assertFalse(any("retired/big.md" in problem for problem in problems), problems)
        fx.write("docs/ledgers/findings/big.md", FINDINGS_HEAD + "".join(row.format(n) for n in range(1, 140)))
        fx.write("docs/ledgers/findings/more.md", FINDINGS_HEAD + "".join(row.format(n) for n in range(400, 539)))
        self.assertTrue(any("active findings total" in problem for problem in fx.lint()))

    def test_legacy_layouts_rejected(self) -> None:
        fx = self.make()
        fx.write("docs/implementation-status.md", STATUS_LEGACY)
        fx.write("docs/research/area/topic.md", DIGEST_LEGACY)
        fx.write("docs/ledgers/findings.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| F-001 |\n")
        fx.write("docs/specs/s.md", SPEC_OK + "\nSee F-001.\n")
        problems = fx.lint()
        self.assertTrue(any("implementation-status.md: H2 set" in problem for problem in problems), problems)
        self.assertTrue(any("topic.md: H2 set" in problem for problem in problems), problems)
        self.assertTrue(any("finding F-001 is not defined" in problem for problem in problems), problems)
        fx.write("docs/implementation-status.md", STATUS_OK)
        fx.write("docs/research/area/topic.md", DIGEST_OK)
        fx.write("docs/specs/s.md", SPEC_OK)
        (fx.root / "docs/ledgers/findings.md").unlink()
        self.assertEqual(fx.lint(), [])

    def test_digest_cap(self) -> None:
        fx = self.make()
        fx.write("docs/research/area/topic.md", DIGEST_OK + "d" * (docs_lint.DIGEST_CAP - 200))
        self.assertEqual(fx.lint(), [])
        fx.write("docs/research/area/topic.md", DIGEST_OK + "d" * docs_lint.DIGEST_CAP)
        self.assertTrue(any("topic.md" in problem and "cap" in problem for problem in fx.lint()))

    def test_glued_tokens(self) -> None:
        fx = self.make()
        fx.write("docs/notes.md", "# Notes\n\nCase passes128authored checks at100000entities.\n\nCommit cac76ec, `passes128authored`, [x](a1b2c3d4e5f.md), memory522 and x86LE64 are fine.\n")
        fx.write("docs/a1b2c3d4e5f.md", "# Target\n")
        problems = fx.lint()
        self.assertTrue(any("notes.md:3: glued tokens passes128authored, at100000entities" in problem for problem in problems), problems)
        self.assertFalse(any("notes.md:5" in problem for problem in problems), problems)

    def test_dated_filenames_and_provenance(self) -> None:
        fx = self.make()
        fx.write("docs/research/area/2026-10-13-topic.md", DIGEST_OK)
        fx.write("docs/notes.md", "# Notes\n\nBefore restructure: abc1234:docs/old.md\n\nFull text: def5678:docs/older.md\n\n| a | 1234567:docs/cell.md |\n")
        problems = fx.lint()
        self.assertTrue(any("2026-10-13-topic.md: dated filename" in problem for problem in problems), problems)
        self.assertTrue(any("notes.md:5: more than one commit:path provenance" in problem for problem in problems), problems)
        self.assertFalse(any("notes.md:3" in problem or "notes.md:7" in problem for problem in problems), problems)

    def test_read_tiers(self) -> None:
        fx = self.make()
        fx.write("CLAUDE.md", "# Agents\n\n" + "a" * (docs_lint.STARTUP_BUDGET - 2000))
        fx.write("docs/README.md", "# Map\n")
        fx.write("docs/implementation-status.md", STATUS_OK)
        self.assertEqual(fx.lint(), [])
        fx.write("CLAUDE.md", "# Agents\n\n" + "a" * docs_lint.STARTUP_BUDGET)
        self.assertTrue(any("startup read tier is" in problem for problem in fx.lint()))
        fx.write("CLAUDE.md", "# Agents\n")
        fx.write("docs/big.md", "# Big\n\n" + "b" * (docs_lint.TASK_BUDGET + 1))
        original = docs_lint.TASK_TIERS
        docs_lint.TASK_TIERS = {"research": ("docs/big.md",), "stale": ("docs/gone.md",)}
        self.addCleanup(setattr, docs_lint, "TASK_TIERS", original)
        linter = fx.linter()
        warnings = [warning.render() for warning in linter.warnings]
        self.assertTrue(any("task read tier 'research'" in warning for warning in warnings), warnings)
        self.assertTrue(any("gone.md: task read tier 'stale' names a missing file" in warning for warning in warnings), warnings)
        self.assertFalse(any("task read tier" in problem.render() for problem in linter.problems))

    def test_staleness_warning(self) -> None:
        fx = self.make()
        fx.write("docs/implementation-status.md", STATUS_OK)
        fresh = [warning.render() for warning in fx.linter(today=date(2026, 11, 12)).warnings]
        self.assertFalse(any("last reconciled" in warning for warning in fresh), fresh)
        warnings = [warning.render() for warning in fx.linter(today=date(2026, 11, 13)).warnings]
        self.assertTrue(any("last reconciled 31 days ago" in warning for warning in warnings), warnings)

    def test_instruction_files_join_link_and_path_checks(self) -> None:
        fx = self.make()
        fx.write("CLAUDE.md", "# Agents\n\nRead [map](docs/README.md), [gone](docs/gone.md) and docs/missing.md; see F-999.\n")
        fx.write(".agents/rules/docs.md", "# Rules\n\nUse `docs/ledgers/findings/` and docs/README.md.\n")
        fx.write("docs/README.md", "# Map\n")
        problems = fx.lint()
        self.assertTrue(any("CLAUDE.md:3: dangling link: docs/gone.md" in problem for problem in problems), problems)
        self.assertTrue(any("CLAUDE.md:3: references missing document docs/missing.md" in problem for problem in problems), problems)
        self.assertFalse(any("F-999" in problem or ".agents" in problem for problem in problems), problems)

    def test_frozen_directories(self) -> None:
        fx = self.make()
        fx.write("docs/history/new-record.md", "# New\n")
        fx.write("docs/history/overnight-work-plan.md", "# Old\n\n### Batch — 2026-10-04 at 03:00 UTC\n")
        problems = fx.lint()
        self.assertTrue(any("new-record.md: frozen directory" in problem for problem in problems), problems)
        self.assertFalse(any("overnight-work-plan.md" in problem for problem in problems), problems)

    def test_record_schema(self) -> None:
        fx = self.make()
        lines = [
            record(1),
            record(2, extra=1),
            '{"id":"M-003","id":"M-003"}\n',
            record(4, note=None, superseded_by=None).replace('"status":"active"', '"status":"active","note":null'),
            record(5, kind="guess"),
            record(6, sha256="ABC"),
            "not json\n",
            "\n",
            record(9, legacy=True),
            record(271, legacy=True),
        ]
        fx.write("evidence/foundation.jsonl", "".join(lines))
        problems = fx.lint()
        for expected in (
            ":2: invalid evidence record: unknown keys extra",
            ":3: invalid evidence record: duplicate key 'id'",
            ":4: invalid evidence record: omit optional keys instead of null: note",
            "M-005: kind must be one of",
            "M-006: sha256 must be 64 lowercase hex",
            ":7: invalid evidence record: invalid JSON",
            ":8: blank line in evidence shard",
            "M-271: legacy applies only to M-001..M-270",
        ):
            self.assertTrue(any(expected in problem for problem in problems), (expected, problems))
        self.assertFalse(any(": M-001:" in problem or ": M-009:" in problem for problem in problems), problems)

    def test_new_rows_are_strict_and_legacy_rows_tolerated(self) -> None:
        fx = self.make()
        fx.write("evidence/foundation.jsonl", "".join([
            legacy(1, path="work/old/", sha256=None, date=None, label="Long " * 60 + "at 04:38 UTC"),
            legacy(2, path="bare-name/"),
            record(271, sha256=None, date=None, path="work/dir/", label="x" * 200),
            record(272, label="Checked at 04:38 UTC"),
            record(273, path="bare.json"),
            record(274, ev=[EV_A]),
            record(275, kind="rea-bundle", path="work/b/rea-bundle.json"),
        ]))
        problems = fx.lint()
        for expected in (
            "M-271: new rows need the artifact's sha256",
            "M-271: new rows need a date",
            "M-271: new rows name one file",
            "M-271: label exceeds 160 characters",
            "M-272: label carries a clock time",
            "M-273: local-private path must start with",
            "M-274: ev, unk and records belong only to rea-bundle rows",
            "M-275: rea-bundle rows record the bundle's record count",
        ):
            self.assertTrue(any(expected in problem for problem in problems), (expected, problems))
        self.assertFalse(any("M-001" in problem or "M-002" in problem for problem in problems), problems)
        self.assertTrue(any("M-002: local-private path" in warning.render() for warning in fx.linter().warnings))

    def test_ids_unique_contiguous_and_ordered(self) -> None:
        fx = self.make()
        fx.write("evidence/foundation.jsonl", legacy(1) + legacy(3) + legacy(2))
        fx.write("evidence/network.jsonl", legacy(2) + legacy(5))
        fx.write("evidence/Bad_Name.jsonl", "")
        fx.write("evidence/big.jsonl", legacy(6, label="x" * 1500) + legacy(7, label="y" * 2100))
        problems = fx.lint()
        self.assertTrue(any("evidence/network.jsonl:1: duplicate ID M-002 (first at evidence/foundation.jsonl:3)" in problem for problem in problems), problems)
        self.assertTrue(any("missing M-004" in problem for problem in problems), problems)
        self.assertTrue(any("foundation.jsonl:3: M-002 is out of order" in problem for problem in problems), problems)
        self.assertTrue(any("Bad_Name.jsonl: shard name" in problem for problem in problems), problems)
        self.assertTrue(any("big.jsonl:2: record exceeds" in problem for problem in problems), problems)
        fx.write("evidence/big.jsonl", "".join(legacy(n, label="z" * 1500) for n in (6, 7, 8) ) * 1)
        fx.write("evidence/huge.jsonl", "".join(legacy(n, label="w" * 1500) for n in range(9, 60)))
        self.assertTrue(any("huge.jsonl: " in problem and "shard cap" in problem for problem in fx.lint()))

    def test_lifecycle_and_tracked_paths(self) -> None:
        fx = self.make()
        fx.write("docs/specs/s.md", SPEC_OK)
        fx.write("evidence/foundation.jsonl", "".join([
            record(1),
            record(2, superseded_by="M-001"),
            record(3, status="retired"),
            record(4, status="retired", superseded_by="M-099", note="gone"),
            record(5),
            record(6, **{"class": "tracked"}, path="docs/specs/s.md#scope"),
            record(7, **{"class": "tracked"}, path="docs/specs/s.md#nope"),
            record(8, **{"class": "tracked"}, path="docs/specs/gone.md"),
            record(9, status="missing", note="lost", date="2026-09-01"),
        ]))
        linter = fx.linter()
        problems = [problem.render() for problem in linter.problems]
        for expected in (
            "M-002: superseded_by requires status retired",
            "M-003: status retired needs a note or superseded_by",
            "M-004: superseded_by M-099 is not another defined row",
            "M-007: tracked path has no heading anchor #nope",
            "M-008: tracked path does not exist: docs/specs/gone.md",
        ):
            self.assertTrue(any(expected in problem for problem in problems), (expected, problems))
        self.assertFalse(any("M-006" in problem or "M-009" in problem for problem in problems), problems)
        self.assertTrue(any("M-009: missing for over 30 days" in warning.render() for warning in linter.warnings))

    def test_local_verification_and_bundle_membership(self) -> None:
        fx = self.make()
        bundle = {"records": [{"evidence_id": EV_A}, {"evidence_id": EV_C}], "unknowns": [{"unknown_id": UNK_A}]}
        fx.write("work/b/rea-bundle.json", json.dumps(bundle))
        fx.write("work/r/receipt.json", "{}")
        (fx.root / "work/old").mkdir(parents=True)
        bundle_digest = hashlib.sha256((fx.root / "work/b/rea-bundle.json").read_bytes()).hexdigest()
        receipt_digest = hashlib.sha256(b"{}").hexdigest()
        fx.write("evidence/foundation.jsonl", "".join([
            legacy(1, path="work/old/", sha256=None),
            legacy(2, path="work/gone/", sha256=None),
            record(3, path="work/r/receipt.json", sha256=receipt_digest),
            record(4, path="work/r/receipt.json"),
            record(5, kind="rea-bundle", path="work/b/rea-bundle.json", sha256=bundle_digest, records=2, ev=[EV_A, EV_C], unk=[UNK_A]),
            record(6, kind="rea-bundle", path="work/b/rea-bundle.json", sha256=bundle_digest, records=3, ev=[EV_B]),
            record(7, path="work/old/x.json", status="retired", note="cleaned"),
        ]))
        self.assertEqual(fx.lint(), [])
        problems = fx.lint(local=True)
        self.assertTrue(any("M-002: local-private evidence missing: work/gone/" in problem for problem in problems), problems)
        self.assertTrue(any("M-004: local-private evidence digest mismatch" in problem for problem in problems), problems)
        self.assertTrue(any("M-006: bundle holds 2 records, row says 3" in problem for problem in problems), problems)
        self.assertTrue(any(f"M-006: ev entry {EV_B} is not in the bundle" in problem for problem in problems), problems)
        self.assertFalse(any(f"M-{n:03d}" in problem for n in (1, 3, 5, 7) for problem in problems), problems)

    def test_listed_citations(self) -> None:
        fx = self.make()
        fx.write("evidence/foundation.jsonl", legacy(1) + record(2, kind="rea-bundle", path="work/b/rea-bundle.json", records=3, ev=[EV_A, EV_B, EV_C], unk=[UNK_A]))
        fx.write("docs/specs/s.md", SPEC_OK + "\n".join([
            "",
            "Good: M-002/ev_222222222222 and M-002/unk_333333333333 and M-001/E-021.",
            "Ambiguous: M-002/ev_111111111111.",
            "Short: M-002/ev_2222.",
            f"Full: {EV_C}.",
            "Note: M-002/E-004.",
        ]) + "\n")
        problems = fx.lint()
        self.assertTrue(any(":18: M-002/ev_111111111111 matches 2 entries" in problem for problem in problems), problems)
        self.assertTrue(any(":19: M-002/ev_2222: cite exactly 12 hex digits" in problem for problem in problems), problems)
        self.assertTrue(any(":20: full ev_/unk_ identifier in prose" in problem for problem in problems), problems)
        self.assertTrue(any(":21: M-002/E-004: E- notes exist only under legacy rows" in problem for problem in problems), problems)
        self.assertFalse(any(":17:" in problem for problem in problems), problems)

    def test_manifest_history_against_base(self) -> None:
        fx = self.make()
        fx.git_init()
        fx.write("evidence/foundation.jsonl", legacy(1, path="old/") + legacy(2) + record(3) + record(4, kind="rea-bundle", records=1, ev=[EV_A]))
        _ = fx.commit("index")
        fx.write("evidence/foundation.jsonl", legacy(1, path="work/old/", kind="review", note="path corrected from old/")
                 + record(3, kind="review", date="2026-10-14", path="work/moved.json")
                 + record(4, kind="rea-bundle", records=1, ev=[EV_B]))
        problems = fx.lint()
        self.assertTrue(any("ID M-002 existed at HEAD and was removed" in problem for problem in problems), problems)
        self.assertTrue(any("M-003: kind changed" in problem for problem in problems), problems)
        self.assertTrue(any("M-003: date changed" in problem for problem in problems), problems)
        self.assertTrue(any("M-003: path changed; keep the old value in note" in problem for problem in problems), problems)
        self.assertTrue(any("M-004: ev changed" in problem for problem in problems), problems)
        self.assertFalse(any("M-001" in problem for problem in problems), problems)

    def test_markdown_manifest_ids_survive_migration(self) -> None:
        fx = self.make()
        fx.git_init()
        fx.write("evidence/manifest.md", "# M\n\n| ID | path | sha256 | date | session | class |\n| --- | --- | --- | --- | --- | --- |\n"
                 + "| M-001 | work/x | unstated | 2026-10-13 | s | local-private |\n| M-002 | work/y | unstated | 2026-10-13 | s | local-private |\n")
        _ = fx.commit("markdown manifest")
        (fx.root / "evidence/manifest.md").unlink()
        fx.write("evidence/foundation.jsonl", legacy(1, path="work/x", sha256=None))
        problems = fx.lint()
        self.assertTrue(any("ID M-002 existed at HEAD and was removed" in problem for problem in problems), problems)
        self.assertFalse(any("M-001" in problem for problem in problems), problems)

    def test_bare_case_notes_rejected_only_on_new_lines(self) -> None:
        fx = self.make()
        fx.git_init()
        fx.write("evidence/foundation.jsonl", legacy(1))
        fx.write("docs/specs/s.md", SPEC_OK + "\nOld note E-021 stays.\n")
        _ = fx.commit("spec")
        fx.write("docs/specs/s.md", SPEC_OK + "\nOld note E-021 stays.\nNew note E-022.\nCited M-001/E-023 and `E-024`.\n")
        problems = fx.lint()
        self.assertTrue(any(":18: bare case note E-022" in problem for problem in problems), problems)
        self.assertFalse(any(":17:" in problem or ":19:" in problem for problem in problems), problems)

    def test_next_id_and_list_local(self) -> None:
        fx = self.make()
        fx.write("evidence/foundation.jsonl", legacy(1, path="work/a/") + record(2, status="retired", note="cleaned") + record(3, **{"class": "external"}, path="https://example.invalid/x"))
        fx.write("docs/ledgers/findings/a.md", FINDINGS_HEAD + "| F-007 | c | observed | review | s | none | active |\n")
        for argv, expected in ((["--next-id", "M"], "M-004"), (["--next-id", "F"], "F-008"), (["--next-id", "U"], "U-001"), (["--list-local"], "work/a/")):
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(docs_lint.main(["--root", str(fx.root), *argv]), 0)
            self.assertEqual(output.getvalue().strip(), expected)

    def test_reconciled_date_required(self) -> None:
        fx = self.make()
        fx.write("docs/specs/s.md", SPEC_OK.replace("Last reconciled: 2026-10-13\n", ""))
        problems = fx.lint()
        self.assertTrue(any("missing 'Last reconciled" in problem for problem in problems), problems)

    def test_main_warn_exits_zero(self) -> None:
        fx = self.make()
        fx.write("docs/x.md", "# X\n\n[bad](nope.md)\n")
        _ = self.assertEqual(docs_lint.main(["--root", str(fx.root), "--warn"]), 0)
        _ = self.assertEqual(docs_lint.main(["--root", str(fx.root)]), 1)


if __name__ == "__main__":
    _ = unittest.main()
