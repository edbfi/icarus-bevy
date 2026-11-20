"""Authored fixtures for the work/ scratch lint, in temporary Git repositories; no repository files are read.

Pins come from ``memory protected-paths`` (``tools/memory/paths.py``): invented legacy shards, invented
journal segments, and linked worktrees of the fixture repository.
"""

import contextlib
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import time
from typing import override
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools import sha256_manifest, work_gc  # noqa: E402
from tools.memory.journal import Revision, write_change_set  # noqa: E402
from tools.memory.model import Record  # noqa: E402
from tools.memory.paths import ProtectedPathsError  # noqa: E402
from tools.tests.memory_fixtures import CREATED, clean_git_environment, evidence, new_repository, run_git  # noqa: E402

# main() reads the clock, so fixtures age relative to the real time.
NOW = float(int(time.time()))
OLD = NOW - 30 * 24 * 3600
COPY = b"\x5a" * work_gc.COPY_MIN_BYTES


def no_processes() -> set[Path]:
    return set()


def no_references(_root: Path, _name: str) -> list[str]:
    return []


class WorkGcTest(unittest.TestCase):
    top: Path = Path()
    root: Path = Path()

    @override
    def setUp(self) -> None:
        self.top = Path(self.enterContext(tempfile.TemporaryDirectory())).resolve()
        self.root = self.top / "primary"
        self.root.mkdir()
        self.enterContext(clean_git_environment())
        new_repository(self.root)
        # An empty first commit, so linked worktrees check out nothing of the fixture.
        _ = run_git(self.root, "commit", "-q", "--allow-empty", "-m", "fixture")
        (self.root / "evidence" / "area").mkdir(parents=True)
        (self.root / "work").mkdir()
        (self.root / "icarus-orig").mkdir()
        _ = (self.root / "icarus-orig" / "server.exe").write_bytes(COPY)

    def write(self, rel: str, data: bytes = b"x") -> Path:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        _ = path.write_bytes(data)
        return path

    def evidence(self, *paths: str, status: str = "active", checkout: Path | None = None) -> None:
        """One legacy shard in ``checkout`` (default the primary) listing ``paths`` as local-private rows."""
        lines = [
            json.dumps({"id": f"M-{index:03d}", "path": path, "class": "local-private", "status": status})
            for index, path in enumerate(paths, start=1)
        ]
        shard = (checkout or self.root) / "evidence" / "area" / "001.jsonl"
        shard.parent.mkdir(parents=True, exist_ok=True)
        _ = shard.write_text("\n".join(lines) + "\n")

    def journal(self, checkout: Path, *paths: str) -> None:
        """One journal segment in ``checkout`` recording ``paths`` as active local-private evidence."""
        records = [Record.from_json(evidence(f"M-{index:03d}", path)) for index, path in enumerate(paths, start=101)]
        _ = write_change_set(checkout, CREATED, [Revision.of(record) for record in records])

    def linked(self, name: str = "linked") -> Path:
        checkout = self.top / name
        _ = run_git(self.root, "worktree", "add", "-q", "--detach", str(checkout))
        return checkout

    def project(self, case: str) -> None:
        _ = self.write(f"work/{case}/project/Case.gpr")
        _ = self.write(f"work/{case}/project/Case.rep/idata/00/data.gbf", b"g" * 4096)
        _ = self.write(f"work/{case}/project/Case.lock")

    def age(self) -> None:
        for path in (self.root / "work").rglob("*"):
            os.utime(path, (OLD, OLD), follow_symlinks=False)
        os.utime(self.root / "work", (OLD, OLD))

    def scan(self, **kwargs: float) -> work_gc.Report:
        return work_gc.scan(self.root, now=NOW, live=no_processes, references=no_references, **kwargs)

    def case(self, report: work_gc.Report, name: str) -> work_gc.Case:
        return next(case for case in report.cases if case.name == name)

    def kinds(self, items: list[work_gc.Candidate]) -> set[tuple[str, str]]:
        return {(item.path.relative_to(self.root).as_posix(), item.kind) for item in items}

    def test_finds_every_reproducible_kind(self) -> None:
        self.project("a")
        _ = self.write("work/a/home/.cache/fontconfig/x")
        _ = self.write("work/a/tagged/CACHEDIR.TAG", work_gc.CACHEDIR_SIGNATURE + b"\n")
        _ = self.write("work/a/prefix/version")
        _ = self.write("work/a/prefix/pfx/system.reg")
        _ = self.write("work/a/copy/server.exe", COPY)
        _ = self.write("work/a/notes.md")
        self.age()
        found = self.kinds(self.case(self.scan(), "a").candidates)
        self.assertEqual(
            found,
            {
                ("work/a/project/Case.gpr", "ghidra-project"),
                ("work/a/project/Case.rep", "ghidra-project"),
                ("work/a/project/Case.lock", "ghidra-project"),
                ("work/a/home/.cache", "cache"),
                ("work/a/tagged", "cache"),
                ("work/a/prefix", "wine-prefix"),
                ("work/a/copy/server.exe", "original-copy"),
            },
        )

    def test_different_bytes_of_original_size_are_kept(self) -> None:
        _ = self.write("work/a/build.exe", b"\x00" * work_gc.COPY_MIN_BYTES)
        self.age()
        self.assertEqual(self.case(self.scan(), "a").candidates, [])

    def test_active_evidence_holds_overlapping_scratch(self) -> None:
        self.project("inside")
        self.project("parent")
        self.project("retired")
        self.evidence("work/inside/project/Case.rep/idata/00/data.gbf", "work/parent/")
        self.age()
        report = self.scan()
        inside = self.case(report, "inside")
        self.assertIn(("work/inside/project/Case.rep", "ghidra-project"), self.kinds(inside.held))
        self.assertIn(("work/inside/project/Case.gpr", "ghidra-project"), self.kinds(inside.candidates))
        self.assertEqual(self.case(report, "parent").candidates, [])
        self.assertTrue(self.case(report, "parent").indexed)
        self.evidence("work/retired/", status="retired")
        self.assertEqual(len(self.case(self.scan(), "retired").candidates), 3)

    def test_open_live_recent_and_invalid_cases_are_deferred(self) -> None:
        for name in ("open", "live", "recent", "invalid", "closed"):
            self.project(name)
        _ = self.write("work/open/case.json", b'{"state": "open"}')
        _ = self.write("work/invalid/case.json", b'{"state": "done"}')
        _ = self.write("work/closed/case.json", b'{"state": "closed"}')
        self.age()
        os.utime(self.root / "work/recent/project/Case.gpr", (NOW, NOW))
        report = work_gc.scan(self.root, now=NOW, live=lambda: {self.root / "work/live/run.log"}, references=no_references)
        for name in ("open", "live", "recent", "invalid"):
            self.assertEqual(self.case(report, name).candidates, [], name)
            self.assertEqual(len(self.case(report, name).deferred), 3, name)
        self.assertEqual(len(self.case(report, "closed").candidates), 3)
        self.assertIsNotNone(self.case(report, "invalid").state_error)

    def test_unknown_processes_defer_everything(self) -> None:
        self.project("a")
        self.age()
        report = work_gc.scan(self.root, now=NOW, live=lambda: None, references=no_references)
        self.assertFalse(report.live_known)
        self.assertEqual(self.case(report, "a").candidates, [])

    def test_prune_removes_only_eligible_scratch_and_logs_it(self) -> None:
        self.project("a")
        self.project("held")
        notes = self.write("work/a/notes.md")
        self.evidence("work/held/project/")
        self.age()
        removed = work_gc.prune(self.scan(), NOW)
        self.assertEqual(len(removed), 3)
        self.assertFalse((self.root / "work/a/project/Case.rep").exists())
        self.assertTrue(notes.exists())
        self.assertTrue((self.root / "work/held/project/Case.rep").exists())
        log = (self.root / "work/.gc/log.jsonl").read_text().splitlines()
        self.assertEqual(len(log), 3)
        self.assertEqual(self.case(self.scan(), "a").candidates, [])

    def test_symlinks_are_not_followed(self) -> None:
        outside = self.root / "outside"
        outside.mkdir()
        _ = (outside / "Case.gpr").write_bytes(b"x")
        _ = self.write("work/a/notes.md")
        (self.root / "work/a/link").symlink_to(outside, target_is_directory=True)
        (self.root / "work/loose").symlink_to(outside, target_is_directory=True)
        self.age()
        report = self.scan()
        self.assertEqual(self.case(report, "a").candidates, [])
        self.assertIn(self.root / "work/loose", [item.path for item in report.loose])

    def test_check_fails_while_eligible_scratch_remains(self) -> None:
        self.project("a")
        self.age()
        root = ["--root", str(self.root), "--settle-hours", "0"]
        output = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(work_gc.main([*root, "--check"]), 1)
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(work_gc.main([*root, "--prune", "--json"]), 0)
        report: object = work_gc.load_object(output.getvalue())
        self.assertIsInstance(report, dict)
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(work_gc.main([*root, "--check"]), 0)

    def test_fixity_manifest_keeps_files_beside_nested_scratch(self) -> None:
        _ = self.write("work/c/runtime/run-1/home/.cache/CACHEDIR.TAG", b"Signature: 8a477f597d28d172789f06886806bc55")
        _ = self.write("work/c/runtime/run-1/home/.cache/blob", b"c" * 4096)
        _ = self.write("work/c/runtime/run-1/home/Saved/Logs/server.log", b"kept log")
        _ = self.write("work/c/runtime/run-1/cleanup.json", b"{}")
        _ = self.write("work/c/scope.md", b"scope")
        manifest, entries, _symlinks = work_gc.write_fixity(self.root, self.root / "work/c")
        self.assertEqual(entries, 3)
        self.assertEqual(
            [line.split("  ")[1] for line in manifest.read_text().splitlines()],
            ["runtime/run-1/cleanup.json", "runtime/run-1/home/Saved/Logs/server.log", "scope.md"],
        )

    def test_fixity_manifest_pins_kept_files_and_frees_scratch(self) -> None:
        self.project("a")
        notes = self.write("work/a/notes.md", b"kept")
        _ = self.write("work/a/sub/out.json", b"{}")
        (self.root / "work/a/link").symlink_to(notes)
        _ = self.write("work/a/.fixity.sha256.partial", b"crashed run")
        self.evidence("work/a/")
        self.age()
        self.assertEqual(self.case(self.scan(), "a").candidates, [])
        manifest, entries, symlinks = work_gc.write_fixity(self.root, self.root / "work/a")
        self.assertEqual((entries, symlinks), (2, 1))
        self.assertEqual(
            [line.split("  ")[1] for line in manifest.read_text().splitlines()],
            ["notes.md", "sub/out.json"],
        )
        with self.assertRaises(SystemExit):
            _ = work_gc.write_fixity(self.root, self.root / "work/a")
        with self.assertRaises(SystemExit):
            _ = work_gc.write_fixity(self.root, self.root / "icarus-orig")
        self.evidence("work/a/fixity.sha256")
        self.age()
        report = self.scan()
        self.assertIn(notes, work_gc.protected_paths(self.root))
        self.assertTrue(self.case(report, "a").indexed)
        self.assertEqual(len(self.case(report, "a").candidates), 3)
        _ = self.write("work/b/odd\x1cname")
        with self.assertRaises(SystemExit):
            _ = work_gc.write_fixity(self.root, self.root / "work/b")
        self.assertFalse((self.root / "work/b/fixity.sha256").exists())
        _ = (self.root / "work/a/fixity.sha256").write_text("0" * 64 + "  ../escape\n")
        # Only listed files that exist are protected: nothing absent can be deleted.
        self.assertNotIn(self.root / "work/escape", work_gc.protected_paths(self.root))
        _ = self.write("work/escape")
        self.assertIn(self.root / "work/escape", work_gc.protected_paths(self.root))

    def official_copy(self, case: str = "c") -> Path:
        """A case like issue218: an installation whose files byte-match an original input."""
        _ = self.write(f"work/{case}/case.json", b'{"state": "closed"}')
        _ = self.write(f"work/{case}/notes.md", b"kept")
        return self.write(f"work/{case}/official/Bin/server.exe", COPY)

    def manifest(self, rel: str, *entries: str) -> Path:
        return self.write(rel, "".join(f"{'0' * 64}  {entry}\n" for entry in entries).encode())

    def assert_held(self, path: Path, case: str = "c") -> None:
        report = self.scan()
        found = self.case(report, case)
        self.assertNotIn(path, [item.path for item in found.candidates])
        self.assertIn(path, [item.path for item in found.held + found.deferred])
        if report.pin_problem is None:
            self.assertEqual(work_gc.prune(report, NOW), [])
        else:
            self.assert_refuses(report)
        self.assertTrue(path.exists())

    def assert_refuses(self, report: work_gc.Report) -> None:
        """Unknown protected paths: prune and retire delete nothing, and every mode exits non-zero."""
        self.assertIsNotNone(report.pin_problem)
        with self.assertRaisesRegex(SystemExit, "nothing pruned"):
            _ = work_gc.prune(report, NOW)
        with self.assertRaisesRegex(SystemExit, "nothing retired"):
            _ = work_gc.retire(report, [case.name for case in report.cases], NOW)
        for mode in ([], ["--check"], ["--json"], ["--prune"], ["--retire", *(case.name for case in report.cases)]):
            with self.subTest(mode=mode), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                try:
                    status = work_gc.main(["--root", str(self.root), "--settle-hours", "0", *mode])
                except SystemExit as error:
                    status = error.code
                self.assertNotIn(status, (0, None))
        self.assertFalse((self.root / "work/.gc/log.jsonl").exists())

    def test_any_named_manifest_pins_what_it_lists(self) -> None:
        copy = self.official_copy()
        _ = self.manifest("work/c/official-fixity.sha256", "official/Bin/server.exe")
        self.evidence("work/c/official-fixity.sha256")
        self.age()
        self.assert_held(copy)

    def test_manifest_entries_may_be_repository_relative(self) -> None:
        copy = self.official_copy()
        _ = self.manifest("work/c/fixity.sha256", "work/c/official/Bin/server.exe")
        self.evidence("work/c/fixity.sha256")
        self.age()
        self.assert_held(copy)

    def test_issue218_layout_keeps_the_pinned_installation(self) -> None:
        copy = self.official_copy()
        _ = self.write("work/c/official-fixity.sha256", f"{'1' * 64} *work/c/official/Bin/server.exe\n".encode())
        self.evidence("work/c/official-fixity.sha256")
        self.age()
        self.assert_held(copy)
        self.assertTrue(self.case(self.scan(), "c").indexed)

    def test_dot_relative_entries_are_pinned(self) -> None:
        copy = self.official_copy()
        _ = self.manifest("work/c/capture-fixity.sha256", "./official/Bin/server.exe")
        self.evidence("work/c/capture-fixity.sha256")
        self.age()
        self.assert_held(copy)

    def test_manifest_entries_outside_the_case_pin_the_other_case(self) -> None:
        first = self.official_copy("b")
        second = self.write("work/b/second/server.exe", COPY)
        _ = self.write("work/a/notes.md")
        _ = self.manifest("work/a/outputs.sha256", "work/b/official/Bin/server.exe", "../b/second/server.exe")
        self.evidence("work/a/outputs.sha256")
        self.age()
        self.assert_held(first, "b")
        self.assert_held(second, "b")
        self.assertTrue(self.case(self.scan(), "b").indexed)

    def test_unusable_pin_manifest_defers_every_case_and_says_why(self) -> None:
        bodies = (
            ("missing", None),
            ("malformed", b"not a manifest line\n"),
            ("binary", b"\xff\xfe\x00"),
            ("truncated", f"{'0' * 64}  official/Bin/server.exe\n{'0' * 30}".encode()),
            ("absolute", f"{'0' * 64}  /official/Bin/server.exe\n".encode()),
            ("escaping", f"{'0' * 64}  ../../../official/Bin/server.exe\n".encode()),
        )
        for name, body in bodies:
            with self.subTest(name):
                copy = self.official_copy(name)
                if body is not None:
                    _ = self.write(f"work/{name}/official-fixity.sha256", body)
                free = self.write("work/free/copy/server.exe", COPY)
                _ = self.write("work/loose.log")
                self.evidence(f"work/{name}/official-fixity.sha256")
                self.age()
                report = self.scan()
                for case_name, path in ((name, copy), ("free", free)):
                    case = self.case(report, case_name)
                    self.assertEqual(case.candidates, [], case_name)
                    self.assertIn(path, [item.path for item in case.deferred], case_name)
                    self.assertIn("official-fixity.sha256", case.guard or "", case_name)
                self.assertTrue(all(item.guard for item in report.loose))
                self.assertTrue(any("official-fixity.sha256" in line for line in work_gc.render(report)))
                with self.assertRaises(SystemExit):
                    _ = work_gc.retire(report, [name], NOW)
                with self.assertRaises(SystemExit):
                    _ = work_gc.retire(report, ["loose.log"], NOW)
                self.assertIn(f"work/{name}/official-fixity.sha256", report.pin_problem or "")
                self.assert_refuses(report)
                self.assertTrue(copy.exists() and free.exists())

    def test_unusable_manifest_in_one_case_defers_the_case_it_would_pin(self) -> None:
        copy = self.official_copy("b")
        # Would pin work/b/official/..., but a truncated last line makes its entries unknown.
        _ = self.write("work/a/outputs.sha256", f"{'0' * 64}  work/b/official/Bin/server.exe\n{'0' * 30}".encode())
        self.evidence("work/a/outputs.sha256")
        self.age()
        self.assert_held(copy, "b")

    def test_size_lines_after_digest_lines_are_entries(self) -> None:
        # The shape of a removal receipt: digest lines, then '<bytes> <path>' lines for the same files.
        copy = self.official_copy()
        sized = self.write("work/c/sized/server.exe", COPY)
        _ = self.write(
            "work/c/removed.sha256",
            f"{'0' * 64}  official/Bin/server.exe\n{len(COPY)} official/Bin/server.exe\n{len(COPY)} sized/server.exe\n".encode(),
        )
        self.evidence("work/c/removed.sha256")
        self.age()
        report = self.scan()
        self.assertIsNone(report.pin_problem)
        self.assertIsNone(self.case(report, "c").guard)
        self.assert_held(copy)
        self.assert_held(sized)
        self.assertEqual(
            [entry.path for entry in sha256_manifest.read_manifest(self.root, self.root / "work/c/removed.sha256") or []],
            ["official/Bin/server.exe", "official/Bin/server.exe", "sized/server.exe"],
        )
        for line in (f"{'1' * 21} too-long-size", "12  two-spaces", "12 trailing-space ", "-1 negative"):
            _ = self.write("work/c/bad.sha256", f"{'0' * 64}  official/Bin/server.exe\n{line}\n".encode())
            with self.assertRaises(sha256_manifest.ManifestError, msg=line):
                _ = sha256_manifest.read_manifest(self.root, self.root / "work/c/bad.sha256")

    def test_unusable_pin_manifest_outside_a_case_defers_every_case(self) -> None:
        copy = self.official_copy()
        _ = self.write("work/loose-fixity.sha256", b"junk\n")
        self.evidence("work/loose-fixity.sha256")
        self.age()
        self.assert_held(copy)

    def test_manifest_detected_by_content_whatever_its_name(self) -> None:
        copy = self.official_copy()
        _ = self.manifest("work/c/hashes.txt", "official/Bin/server.exe")
        self.evidence("work/c/hashes.txt")
        self.age()
        self.assert_held(copy)

    def test_row_paths_are_normalised(self) -> None:
        copy = self.official_copy()
        self.evidence("./work/c/official//Bin/")
        self.age()
        self.assert_held(copy)

    def test_retire_checks_every_name_before_removing_any(self) -> None:
        for name in ("free", "indexed", "mentioned", "open"):
            _ = self.write(f"work/{name}/notes.md")
        _ = self.write("work/open/case.json", b'{"state": "open"}')
        _ = self.write("work/stray.log")
        _ = self.write("work/receipt.log")
        _ = self.write("work/busy.log")
        self.evidence("work/indexed/notes.md", "work/receipt.log")
        self.age()
        _ = self.write("work/fresh.log")

        def mentions(_root: Path, name: str) -> list[str]:
            return ["docs/x.md"] if name == "mentioned" else []

        report = work_gc.scan(self.root, now=NOW, live=lambda: {self.root / "work/busy.log"}, references=mentions)
        self.assertEqual([case.name for case in report.retirable()], ["free", "open"])
        for name in ("indexed", "mentioned", "open", "missing", "receipt.log", "busy.log", "fresh.log"):
            with self.assertRaises(SystemExit):
                _ = work_gc.retire(report, ["free", name], NOW)
            self.assertTrue((self.root / "work/free").exists(), name)
        removed = work_gc.retire(report, ["free", "stray.log"], NOW)
        self.assertEqual({item.kind for item in removed}, {"retired-case", "retired-loose"})
        self.assertFalse((self.root / "work/free").exists())
        self.assertFalse((self.root / "work/stray.log").exists())
        self.assertEqual(len((self.root / "work/.gc/log.jsonl").read_text().splitlines()), 2)

    def test_path_index_matches_pairwise_containment(self) -> None:
        pinned = [Path("/r/work/a/x"), Path("/r/work/b/"), Path("/r/work/c/d/e")]
        index = work_gc.PathIndex(pinned)
        for probe in ("/r", "/r/work/a", "/r/work/a/x", "/r/work/a/x/y", "/r/work/a/xy", "/r/work/b/z", "/r/work/c/d", "/r/work/c/q"):
            path = Path(probe)
            self.assertEqual(index.covers(path), any(work_gc.is_within(path, pin) for pin in pinned), probe)
            self.assertEqual(index.inside(path), any(work_gc.is_within(pin, path) for pin in pinned), probe)

    def test_failed_reference_search_counts_as_a_reference(self) -> None:
        outside = self.top / "not-a-repository"
        outside.mkdir()
        self.assertNotEqual(work_gc.tracked_references(outside, "anything"), [])
        _ = self.write("notes.md", b"see work/mentioned-20261120\n")
        self.assertEqual(work_gc.tracked_references(self.root, "mentioned-20261120"), ["notes.md"])
        self.assertEqual(work_gc.tracked_references(self.root, "absent-20261120"), [])

    def test_linked_worktree_branch_only_legacy_shard_pins_a_path(self) -> None:
        self.project("branch")
        self.project("free")
        linked = self.linked()
        self.evidence("work/branch/project/", checkout=linked)
        self.age()
        report = self.scan()
        self.assertEqual(self.case(report, "branch").candidates, [])
        self.assertEqual(len(self.case(report, "branch").held), 3)
        self.assertTrue(self.case(report, "branch").indexed)
        self.assertEqual(len(self.case(report, "free").candidates), 3)
        with self.assertRaisesRegex(SystemExit, "pins a path"):
            _ = work_gc.retire(report, ["branch"], NOW)

    def test_journal_only_pin_after_cut_over(self) -> None:
        # The cut-over state: no legacy index in the primary, a journal there, legacy rows only in an old worktree.
        (self.root / "evidence" / "area").rmdir()
        (self.root / "evidence").rmdir()
        for name in ("journal", "old", "free"):
            self.project(name)
        self.journal(self.root, "work/journal/project/Case.rep/idata/00/data.gbf")
        self.evidence("work/old/", checkout=self.linked("old-worktree"))
        self.age()
        report = self.scan()
        self.assertIsNone(report.pin_problem)
        self.assertEqual(self.kinds(self.case(report, "journal").held), {("work/journal/project/Case.rep", "ghidra-project")})
        self.assertEqual(self.case(report, "old").candidates, [])
        self.assertTrue(self.case(report, "old").indexed)
        self.assertEqual(len(self.case(report, "free").candidates), 3)
        removed = work_gc.prune(report, NOW)
        self.assertEqual({item.path.parent.parent.name for item in removed}, {"journal", "free"})
        self.assertTrue((self.root / "work/journal/project/Case.rep/idata/00/data.gbf").exists())
        self.assertTrue((self.root / "work/old/project/Case.rep").exists())

    def test_malformed_journal_or_no_source_refuses_to_prune(self) -> None:
        self.project("a")
        _ = self.write("memory/journal/2026/11/broken.jsonl", b"{}\n")
        self.age()
        report = self.scan()
        self.assertIn("not a segment path", report.pin_problem or "")
        self.assertEqual(self.case(report, "a").candidates, [])
        self.assert_refuses(report)
        self.assertTrue((self.root / "work/a/project/Case.rep").exists())
        (self.root / "memory/journal/2026/11/broken.jsonl").unlink()
        _ = self.write("evidence/area/001.jsonl", b'{"id": "M-001", "path": "work/x"}\n')
        self.assertIn("missing keys", self.scan().pin_problem or "")
        for directory in ("memory", "evidence"):
            for path in sorted((self.root / directory).rglob("*"), reverse=True):
                _ = path.rmdir() if path.is_dir() else path.unlink()
            (self.root / directory).rmdir()
        report = self.scan()
        self.assertIn("no worktree has a journal", report.pin_problem or "")
        self.assert_refuses(report)

    def test_runs_against_the_primary_checkout_from_a_linked_worktree(self) -> None:
        self.project("a")
        linked = self.linked()
        self.age()
        self.assertEqual(work_gc.find_root(linked), self.root)
        output = io.StringIO()
        with contextlib.chdir(linked), contextlib.redirect_stdout(output):
            self.assertEqual(work_gc.main(["--json", "--settle-hours", "0"]), 0)
        report = work_gc.load_object(output.getvalue())
        self.assertIsNotNone(report)
        self.assertIn('"name": "a"', output.getvalue())
        with self.assertRaises(ProtectedPathsError):
            _ = work_gc.protected_paths(linked)

    def test_relative_arguments_resolve_against_the_process_directory(self) -> None:
        cwd = self.root
        found = work_gc.argument_paths(cwd, ["ghidraRun", "work/a/project/Case.gpr", "--data-dir=work/b", "/abs/c"])
        self.assertIn(self.root / "work/a/project/Case.gpr", found)
        self.assertIn(self.root / "work/b", found)
        self.assertIn(Path("/abs/c"), found)


if __name__ == "__main__":
    _ = unittest.main()
