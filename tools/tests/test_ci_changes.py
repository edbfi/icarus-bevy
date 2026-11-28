"""Tests for the fail-closed CI path classifier."""

from __future__ import annotations

import tempfile
import unittest
from contextlib import redirect_stdout
from io import StringIO
from pathlib import Path
from unittest.mock import patch

from tools.ci_changes import Decision, classify, main, read_paths


class ClassifyTests(unittest.TestCase):
    def test_non_pull_request_events_are_full(self) -> None:
        self.assertEqual(Decision(True, True, "full-event"), classify("push", ()))

    def test_documentation_and_memory_changes_can_skip_rust(self) -> None:
        self.assertEqual(
            Decision(False, False, "skip-only"),
            classify("pull_request", ("docs/README.md", "memory/journal/segment.jsonl", "README.md")),
        )

    def test_empty_or_unavailable_diff_is_full(self) -> None:
        expected_empty = Decision(True, True, "empty-diff")
        self.assertEqual(expected_empty, classify("pull_request", ()))
        self.assertEqual(Decision(True, True, "diff-unavailable"), classify("pull_request", (), diff_available=False))

    def test_runtime_inputs_require_both_jobs(self) -> None:
        for path in ("src/server.rs", "config/release.toml", "tests/fixtures/capture.json", ".github/workflows/ci.yml"):
            with self.subTest(path=path):
                self.assertEqual(Decision(True, True, "rust-affecting"), classify("pull_request", (path,)))

    def test_renames_and_deletions_are_classified_from_both_paths(self) -> None:
        self.assertEqual(
            Decision(True, True, "rust-affecting"),
            classify("pull_request", ("docs/old.md", "src/new.rs")),
        )
        self.assertEqual(Decision(True, True, "rust-affecting"), classify("pull_request", ("src/removed.rs",)))


class ReadPathsTests(unittest.TestCase):
    def test_nul_delimited_paths(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "changed"
            _ = path.write_bytes(b"docs/a.md\0src/main.rs\0")
            self.assertEqual(("docs/a.md", "src/main.rs"), read_paths(path))

    def test_main_fails_closed_when_diff_cannot_be_read(self) -> None:
        output = StringIO()
        with patch(
            "sys.argv",
            ["ci_changes.py", "--event", "pull_request", "--changed-files", "/missing/changed-files"],
        ), redirect_stdout(output):
            self.assertEqual(0, main())
        self.assertEqual(
            "run_rust=true\nrun_release=true\nclassification=diff-unavailable\n",
            output.getvalue(),
        )


if __name__ == "__main__":
    _ = unittest.main()
