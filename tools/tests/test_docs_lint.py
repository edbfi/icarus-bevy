"""Authored fixtures for the documentation lint; no repository files are read."""

import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
from typing import final
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools import docs_lint  # noqa: E402

DIGEST = "a" * 64
STATUS_OK = "# Status\n\n## Summary\n\n## Milestones\n\n## Works\n\n## Does not work\n\n## Gates\n\n## Next tasks\n\nLast reconciled: 2026-10-13\n"
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

    def lint(self, *, local: bool = False) -> list[str]:
        linter = docs_lint.Linter(self.root, local=local)
        _ = linter.run()
        return [problem.render() for problem in linter.problems]

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
        fx.write("docs/implementation-status.md", STATUS_OK.replace("## Works", "## Works\n\n## Extra"))
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
        self.assertTrue(any("current surface is" in problem for problem in problems), problems)

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
        root = fx.root
        (root / ".git").rmdir()

        def run(*args: str) -> None:
            _ = subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True)

        run("init", "-q")
        run("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "--allow-empty", "-m", "base")
        fx.write("docs/ledgers/findings.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| F-001 |\n| F-002 |\n")
        run("add", "docs/ledgers/findings.md")
        run("-c", "user.name=t", "-c", "user.email=t@example.invalid", "commit", "-q", "-m", "ledger")
        fx.write("docs/ledgers/findings.md", "# F\n\nLast reconciled: 2026-10-13\n\n| ID |\n| --- |\n| F-001 |\n")
        problems = fx.lint()
        self.assertTrue(any("ID F-002 existed at HEAD" in problem for problem in problems), problems)

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
