# SPDX-License-Identifier: AGPL-3.0-or-later
"""``tools/memory/paths.py``: its own legacy shard reader and the sources rule, with invented rows."""

from __future__ import annotations

import json
import shutil
import tempfile
import unittest
from pathlib import Path
from typing import override
from unittest.mock import patch

from tools.memory import paths
from tools.memory.journal import Revision, parse_segment, write_change_set
from tools.memory.state import replay
from tools.memory.model import JSON, Record
from tools.memory.paths import LEGACY_SHARD_LIMIT, ProtectedPathsError, Reader, checkout_of, legacy_rows, protected_paths, sources
from tools.tests.memory_fixtures import CREATED, DIGEST, clean_git_environment, commit_all, evidence, new_repository, run_git


def legacy(identifier: str, path: str, *, klass: str = "local-private", status: str = "active", **extra: JSON) -> dict[str, JSON]:
    row: dict[str, JSON] = {"id": identifier, "path": path, "sha256": DIGEST, "class": klass, "kind": "receipt",
                            "date": "2026-11-20", "label": "Invented", "status": status}
    row.update(extra)
    return row


ACCEPTED = [
    legacy("M-001", "work/case/fixity.sha256"),
    legacy("M-002", "work/old/", status="retired", note="Invented: superseded", superseded_by="M-003"),
    legacy("M-003", "work/case/report.md", klass="tracked"),
    legacy("M-004", "reference-data/", sha256=None, date=None, legacy=True),
    legacy("M-005", "work/gone.log", status="missing", note="Invented: lost"),
    legacy("M-006", "https://example.invalid/x", klass="external"),
    legacy("M-007", "work/case/rea-bundle.json", kind="rea-bundle", records=2, ev=["ev_a1b2c3d4e5f6"]),
]
# Lines the legacy lint's parser rejected for a reason that makes protection unknowable.
REJECTED = [
    "",
    "   ",
    "not json",
    "[]",
    '{"id": "M-008", "id": "M-009", "path": "work/x", "sha256": null, "class": "local-private", "kind": "input", '
    + '"date": null, "label": "Invented", "status": "active"}',
    json.dumps({key: value for key, value in legacy("M-010", "work/x").items() if key != "path"}),
    json.dumps({key: value for key, value in legacy("M-011", "work/x").items() if key != "status"}),
    json.dumps(legacy("", "work/x")),
    json.dumps(legacy("M-012", "")),
    json.dumps({**legacy("M-013", "work/x"), "path": ["work/x"]}),
    json.dumps({**legacy("M-014", "work/x"), "class": None}),
]


class LegacyReaderTests(unittest.TestCase):
    # Delete at cut-over, with tools/docs_lint.py: it compares the old parser with legacy_rows.
    def test_matches_the_legacy_lint_parser(self) -> None:
        from tools.docs_lint import parse_shard

        text = "".join(json.dumps(row) + "\n" for row in ACCEPTED)
        old, problems = parse_shard("evidence/area/001.jsonl", text)
        self.assertEqual([], problems)
        self.assertEqual([(row.identifier, row.klass, row.status, row.path) for row in old], legacy_rows("001.jsonl", text))
        for line in REJECTED:
            with self.subTest(line=line):
                self.assertEqual(1, len(parse_shard("x", line + "\n")[1]))
                with self.assertRaises(ProtectedPathsError):
                    _ = legacy_rows("x", line + "\n")

    def test_reads_only_what_protection_needs_and_rejects_unknown_vocabulary(self) -> None:
        self.assertEqual([("M-001", "local-private", "active", "work/a")],
                         legacy_rows("x", json.dumps({"id": "M-001", "path": "work/a", "class": "local-private", "status": "active"})))
        for row in (legacy("M-001", "work/a", status="Active"), legacy("M-001", "work/a", klass="private"),
                    {**legacy("M-001", "work/a"), "size": 1.5}):
            with self.subTest(row=row), self.assertRaises(ProtectedPathsError):
                _ = legacy_rows("x", json.dumps(row))


class SourcesTests(unittest.TestCase):
    top: Path = Path()
    root: Path = Path()

    @override
    def setUp(self) -> None:
        self.top = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.root = self.top / "primary"
        self.root.mkdir()
        self.enterContext(clean_git_environment())
        new_repository(self.root)
        commit_all(self.root)

    def test_no_journal_and_no_legacy_index_anywhere_is_a_refusal(self) -> None:
        self.assertEqual((), sources(self.root))
        with self.assertRaisesRegex(ProtectedPathsError, "no worktree has a journal"):
            _ = protected_paths(self.root)
        linked = self.top / "linked"
        _ = run_git(self.root, "worktree", "add", "-q", "--detach", str(linked))
        (linked / "evidence").mkdir()
        self.assertEqual(("evidence",), sources(linked))
        self.assertEqual((), protected_paths(self.root).paths)

    def test_cut_over_state_journal_on_main_legacy_rows_in_an_old_worktree(self) -> None:
        old = self.top / "old"
        _ = run_git(self.root, "worktree", "add", "-q", "--detach", str(old))
        (old / "evidence/area").mkdir(parents=True)
        _ = (old / "evidence/area/001.jsonl").write_text(json.dumps(legacy("M-002", "work/branch-only/")) + "\n")
        _ = write_change_set(self.root, CREATED, [Revision.of(Record.from_json(evidence("M-001", "work/journal-case/receipt.txt")))])
        self.assertEqual(("memory/journal",), sources(self.root))
        self.assertEqual(("work/branch-only/", "work/journal-case/receipt.txt"), protected_paths(old).paths)

    def test_absent_and_removal_receipt_entries_are_listed_but_not_printed(self) -> None:
        (self.root / "evidence").mkdir()
        rows = [legacy("M-001", "work/case/fixity.sha256"), legacy("M-1079", "work/case/removed.sha256")]
        _ = (self.root / "evidence/001.jsonl").write_text("".join(json.dumps(row) + "\n" for row in rows))
        (self.root / "work/case").mkdir(parents=True)
        _ = (self.root / "work/case/fixity.sha256").write_text(f"{DIGEST}  kept.bin\n{DIGEST}  absent.bin\n")
        _ = (self.root / "work/case/removed.sha256").write_text(f"{DIGEST}  gone.bin\n")
        _ = (self.root / "work/case/kept.bin").write_text("kept")
        _ = (self.root / "work/case/gone.bin").write_text("back again")
        result = protected_paths(self.root)
        self.assertEqual(("work/case/fixity.sha256", "work/case/kept.bin", "work/case/removed.sha256"), result.paths)
        # Both places an entry may live: beside the manifest and at the repository root.
        self.assertEqual(("absent.bin", "gone.bin", "kept.bin", "work/case/absent.bin", "work/case/gone.bin", "work/case/kept.bin"),
                         result.listed)
        self.assertEqual(("M-001: manifest entry missing: absent.bin",), result.notices)

    def test_reader_parses_each_distinct_segment_once_and_refuses_a_changed_one(self) -> None:
        records = [Record.from_json(evidence(f"M-{number}", f"work/case/{number}.txt")) for number in range(101, 104)]
        segments = write_change_set(self.root, CREATED, [Revision.of(record) for record in records])
        (self.root / "evidence").mkdir()
        _ = (self.root / "evidence/001.jsonl").write_text(json.dumps(legacy("M-001", "work/legacy/")) + "\n")
        copies = [self.top / f"copy{number}" for number in range(3)]
        for copy in copies:
            _ = shutil.copytree(self.root / "memory", copy / "memory")
            _ = shutil.copytree(self.root / "evidence", copy / "evidence")
        reader = Reader()
        with patch("tools.memory.paths.parse_segment", wraps=parse_segment) as parse, \
                patch.object(paths, "legacy_rows", wraps=legacy_rows) as shard, patch("tools.memory.paths.replay", wraps=replay) as replayed:
            results = [reader.evidence_rows(checkout) for checkout in (self.root, *copies)]
        self.assertEqual((len(segments), 1, 1), (parse.call_count, shard.call_count, replayed.call_count))
        self.assertEqual(1, len({frozenset(result) for result in results}))
        self.assertIn(("M-102", "work/case/102.txt"), results[0])
        changed = copies[0] / segments[0].relative_to(self.root)
        _ = changed.write_bytes(changed.read_bytes().replace(b"work/case/102.txt", b"work/case/999.txt"))
        with self.assertRaisesRegex(ProtectedPathsError, "differs from the segment of the same name in another worktree"):
            _ = reader.evidence_rows(copies[0])
        # On its own the edited segment no longer matches its name either.
        with self.assertRaisesRegex(ProtectedPathsError, "does not match its name"):
            _ = Reader().evidence_rows(copies[0])

    def test_reader_ticks_inside_the_journal_read(self) -> None:
        _ = write_change_set(self.root, CREATED, [Revision.of(Record.from_json(evidence("M-101", "work/case/a.txt")))])

        class Expired(Exception):
            pass

        def expire() -> None:
            raise Expired

        with patch("tools.memory.paths.parse_segment") as parse, self.assertRaises(Expired):
            _ = Reader(expire).evidence_rows(self.root)
        parse.assert_not_called()

    def test_only_a_checkout_git_directory_has_a_primary_checkout(self) -> None:
        self.assertEqual(self.top / "x", checkout_of(self.top / "x/.git"))
        for common in (self.top / "x.git", self.top / "x/.git/worktrees/y"):
            with self.subTest(common=common), self.assertRaisesRegex(ProtectedPathsError, "does not belong to a checkout"):
                _ = checkout_of(common)

    def test_symlinked_or_oversized_legacy_shards_fail_closed(self) -> None:
        (self.root / "evidence").mkdir()
        shard = self.root / "evidence/001.jsonl"
        _ = shard.write_text(json.dumps(legacy("M-001", "work/a")) + "\n" * LEGACY_SHARD_LIMIT)
        with self.assertRaisesRegex(ProtectedPathsError, "legacy shard size"):
            _ = protected_paths(self.root)
        shard.unlink()
        shard.symlink_to(self.top / "elsewhere.jsonl")
        with self.assertRaisesRegex(ProtectedPathsError, "symlink"):
            _ = protected_paths(self.root)


if __name__ == "__main__":
    _ = unittest.main()
