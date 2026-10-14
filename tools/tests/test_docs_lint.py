"""Authored fixtures for the documentation lint; no repository files are read."""

import hashlib
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

    def linter(self, *, local: bool = False, strict: bool = False, base: str = "HEAD", today: date = TODAY) -> docs_lint.Linter:
        linter = docs_lint.Linter(self.root, local=local, strict=strict, base=base, today=today)
        _ = linter.run()
        return linter

    def lint(self, *, local: bool = False, strict: bool = False, base: str = "HEAD") -> list[str]:
        return [problem.render() for problem in self.linter(local=local, strict=strict, base=base).problems]

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
        fx.write("docs/ledgers/findings.md", f"# Findings\n\nLast reconciled: 2026-10-13\n\n| ID | claim |\n| --- | --- |\n| F-001 | digest {DIGEST} |\n")
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
        fx.write("docs/ledgers/findings.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID | status |\n| --- | --- |\n| F-001 | superseded-by F-009 |\n| F-001 | active |\n")
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
        fx.write("docs/ledgers/findings.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| F-001 |\n| F-002 |\n")
        _ = fx.commit("ledger")
        fx.write("docs/ledgers/findings.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| F-001 |\n")
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
        fx.write("evidence/manifest.md", "# M\n\n| ID | path | sha256 | date | session | class |\n| --- | --- | --- | --- | --- | --- |\n"
                 + "".join(f"| M-00{n} | work/x | unstated | 2026-10-13 | s | local-private |\n" for n in (1, 2, 3)))
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
        self.assertFalse(any("active findings total" in problem for problem in fx.lint()))
        self.assertTrue(any("active findings total" in problem for problem in fx.lint(strict=True)))

    def test_legacy_layouts_accepted_until_strict(self) -> None:
        fx = self.make()
        fx.write("docs/implementation-status.md", STATUS_LEGACY)
        fx.write("docs/research/area/topic.md", DIGEST_LEGACY)
        self.assertEqual(fx.lint(), [])
        problems = fx.lint(strict=True)
        self.assertTrue(any("implementation-status.md: H2 set" in problem for problem in problems), problems)
        self.assertTrue(any("topic.md: H2 set" in problem for problem in problems), problems)
        fx.write("docs/implementation-status.md", STATUS_OK)
        fx.write("docs/research/area/topic.md", DIGEST_OK)
        fx.write("CLAUDE.md", "# Agents\n")
        fx.write("docs/README.md", "# Map\n")
        self.assertEqual(fx.lint(strict=True), [])

    def test_strict_digest_cap(self) -> None:
        fx = self.make()
        fx.write("docs/research/area/topic.md", DIGEST_OK + "d" * docs_lint.DIGEST_CAP)
        self.assertEqual(fx.lint(), [])
        self.assertTrue(any("topic.md" in problem and "cap" in problem for problem in fx.lint(strict=True)))

    def test_glued_tokens(self) -> None:
        fx = self.make()
        fx.write("docs/notes.md", "# Notes\n\nCase passes128authored checks at100000entities.\n\nCommit cac76ec, `passes128authored`, [x](a1b2c3d4e5f.md), memory522 and x86LE64 are fine.\n")
        fx.write("docs/a1b2c3d4e5f.md", "# Target\n")
        self.assertEqual(fx.lint(), [])
        problems = fx.lint(strict=True)
        self.assertTrue(any("notes.md:3: glued tokens passes128authored, at100000entities" in problem for problem in problems), problems)
        self.assertFalse(any("notes.md:5" in problem for problem in problems), problems)

    def test_dated_filenames_and_provenance(self) -> None:
        fx = self.make()
        fx.write("docs/research/area/2026-10-13-topic.md", DIGEST_OK)
        fx.write("docs/notes.md", "# Notes\n\nBefore restructure: abc1234:docs/old.md\n\nFull text: def5678:docs/older.md\n\n| a | 1234567:docs/cell.md |\n")
        self.assertEqual(fx.lint(), [])
        problems = fx.lint(strict=True)
        self.assertTrue(any("2026-10-13-topic.md: dated filename" in problem for problem in problems), problems)
        self.assertTrue(any("notes.md:5: more than one commit:path provenance" in problem for problem in problems), problems)
        self.assertFalse(any("notes.md:3" in problem or "notes.md:7" in problem for problem in problems), problems)

    def test_read_tiers(self) -> None:
        fx = self.make()
        fx.write("CLAUDE.md", "# Agents\n\n" + "a" * docs_lint.STARTUP_BUDGET)
        fx.write("docs/README.md", "# Map\n")
        fx.write("docs/implementation-status.md", STATUS_OK)
        self.assertEqual(fx.lint(), [])
        self.assertTrue(any("startup read tier is" in problem for problem in fx.lint(strict=True)))
        fx.write("CLAUDE.md", "# Agents\n")
        fx.write("docs/big.md", "# Big\n\n" + "b" * (docs_lint.TASK_BUDGET + 1))
        original = docs_lint.TASK_TIERS
        docs_lint.TASK_TIERS = {"research": ("docs/big.md",), "stale": ("docs/gone.md",)}
        self.addCleanup(setattr, docs_lint, "TASK_TIERS", original)
        linter = fx.linter(strict=True)
        self.assertTrue(any("task read tier 'research'" in warning.render() for warning in linter.warnings))
        self.assertTrue(any("gone.md: task read tier 'stale' names a missing file" in problem.render() for problem in linter.problems))

    def test_manifest_row_cap_is_strict(self) -> None:
        fx = self.make()
        long_session = "s" * docs_lint.MANIFEST_ROW_CAP
        fx.write("evidence/manifest.md", f"# M\n\n| ID | path | sha256 | date | session | class |\n| --- | --- | --- | --- | --- | --- |\n| M-001 | work/x | unstated | 2026-10-13 | {long_session} | local-private |\n")
        self.assertEqual(fx.lint(), [])
        self.assertTrue(any("manifest row exceeds" in problem for problem in fx.lint(strict=True)))

    def test_staleness_warning(self) -> None:
        fx = self.make()
        fx.write("docs/implementation-status.md", STATUS_OK)
        self.assertEqual(fx.linter(today=date(2026, 11, 12)).warnings, [])
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

    def test_manifest_rows_and_local_verification(self) -> None:
        fx = self.make()
        (fx.root / "work").mkdir()
        _ = (fx.root / "work" / "bundle.json").write_bytes(b"{}")
        real = hashlib.sha256(b"{}").hexdigest()
        rows = (
            "# Manifest\n\n| ID | path | sha256 | date | session | class |\n| --- | --- | --- | --- | --- | --- |\n"
            f"| M-001 | work/bundle.json | {real} | 2026-10-13 | s1 | local-private |\n"
            f"| M-002 | work/gone.json | {DIGEST} | 2026-10-13 | s1 | local-private |\n"
            f"| M-003 | work/x | nothex | 2026-10-13 | s1 | tracked |\n"
        )
        fx.write("evidence/manifest.md", rows)
        fx.write("docs/ledgers/findings.md", f"# F\n\nLast reconciled: 2026-10-13\n\n| ID | evidence |\n| --- | --- |\n| F-001 | M-001 ev_{DIGEST} |\n| F-002 | ev_{DIGEST} |\n")
        problems = fx.lint()
        self.assertTrue(any("class must be" in problem for problem in problems), problems)
        self.assertTrue(any("sha256 column" in problem for problem in problems), problems)
        self.assertTrue(any("ev_ identifier cited without a manifest row" in problem and ":8:" in problem for problem in problems), problems)
        self.assertFalse(any("missing: work/gone.json" in problem for problem in problems), problems)
        problems = fx.lint(local=True)
        self.assertTrue(any("missing: work/gone.json" in problem for problem in problems), problems)
        self.assertFalse(any("mismatch: work/bundle.json" in problem for problem in problems), problems)

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
