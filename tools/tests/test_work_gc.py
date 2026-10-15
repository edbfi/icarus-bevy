"""Authored fixtures for the work/ scratch lint; no repository files are read."""

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
from tools import work_gc  # noqa: E402

# main() reads the clock, so fixtures age relative to the real time.
NOW = float(int(time.time()))
OLD = NOW - 30 * 24 * 3600
COPY = b"\x5a" * work_gc.COPY_MIN_BYTES


def no_processes() -> set[Path]:
    return set()


def no_references(_root: Path, _name: str) -> list[str]:
    return []


class WorkGcTest(unittest.TestCase):
    root: Path = Path()

    @override
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        (self.root / ".git").mkdir()
        (self.root / "evidence" / "area").mkdir(parents=True)
        (self.root / "work").mkdir()
        (self.root / "icarus-orig").mkdir()
        _ = (self.root / "icarus-orig" / "server.exe").write_bytes(COPY)

    def write(self, rel: str, data: bytes = b"x") -> Path:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        _ = path.write_bytes(data)
        return path

    def evidence(self, *paths: str, status: str = "active") -> None:
        lines = [
            json.dumps({"id": f"M-{index:03d}", "path": path, "class": "local-private", "status": status})
            for index, path in enumerate(paths, start=1)
        ]
        _ = (self.root / "evidence" / "area" / "001.jsonl").write_text("\n".join(lines) + "\n")

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

    def test_fixity_manifest_pins_kept_files_and_frees_scratch(self) -> None:
        self.project("a")
        notes = self.write("work/a/notes.md", b"kept")
        _ = self.write("work/a/sub/out.json", b"{}")
        (self.root / "work/a/link").symlink_to(notes)
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
        _ = (self.root / "work/a/fixity.sha256").write_text("0" * 64 + "  ../escape\n")
        with self.assertRaises(SystemExit):
            _ = work_gc.protected_paths(self.root)

    def test_retire_checks_every_name_before_removing_any(self) -> None:
        for name in ("free", "indexed", "mentioned", "open"):
            _ = self.write(f"work/{name}/notes.md")
        _ = self.write("work/open/case.json", b'{"state": "open"}')
        _ = self.write("work/stray.log")
        self.evidence("work/indexed/notes.md")
        self.age()

        def mentions(_root: Path, name: str) -> list[str]:
            return ["docs/x.md"] if name == "mentioned" else []

        report = work_gc.scan(self.root, now=NOW, live=no_processes, references=mentions)
        self.assertEqual([case.name for case in report.retirable()], ["free", "open"])
        for name in ("indexed", "mentioned", "open", "missing"):
            with self.assertRaises(SystemExit):
                _ = work_gc.retire(report, ["free", name], NOW)
            self.assertTrue((self.root / "work/free").exists(), name)
        removed = work_gc.retire(report, ["free", "stray.log"], NOW)
        self.assertEqual({item.kind for item in removed}, {"retired-case", "retired-loose"})
        self.assertFalse((self.root / "work/free").exists())
        self.assertFalse((self.root / "work/stray.log").exists())
        self.assertEqual(len((self.root / "work/.gc/log.jsonl").read_text().splitlines()), 2)

    def test_failed_reference_search_counts_as_a_reference(self) -> None:
        self.assertNotEqual(work_gc.tracked_references(self.root, "anything"), [])

    def test_relative_arguments_resolve_against_the_process_directory(self) -> None:
        cwd = self.root
        found = work_gc.argument_paths(cwd, ["ghidraRun", "work/a/project/Case.gpr", "--data-dir=work/b", "/abs/c"])
        self.assertIn(self.root / "work/a/project/Case.gpr", found)
        self.assertIn(self.root / "work/b", found)
        self.assertIn(Path("/abs/c"), found)


if __name__ == "__main__":
    _ = unittest.main()
