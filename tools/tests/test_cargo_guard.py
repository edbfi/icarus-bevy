"""Synthetic fixtures and small Python subprocesses; never run Cargo/real targets."""
from __future__ import annotations

from collections.abc import Sequence
import fcntl
import json
import os
import io
import shutil
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
from typing import NamedTuple, TextIO, override
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools import cargo_guard as guard
from tools import docs_lint


class CargoGuardTests(unittest.TestCase):
    root: Path = Path()
    info: guard.Identity = guard.Identity(Path(), Path())
    env: dict[str, str] = {}

    @override
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name) / "checkout"
        self.root.mkdir()
        common = Path(directory.name) / "git-common"
        common.mkdir()
        self.info = guard.Identity(self.root, common)
        guard.state_directory(self.info)
        home = Path(directory.name) / "cargo-home"
        home.mkdir()
        self.env = {"CARGO_HOME": str(home)}
        _ = self.write("Cargo.toml", '[package]\nname="fixture"\nversion="0.1.0"\n')
        _ = self.write("Cargo.lock", "version = 4\n")
        _ = self.write("rust-toolchain.toml", '[toolchain]\nchannel="fixture"\n')
        _ = self.write("src/lib.rs", "// authored fixture\n")
        for name, value in (("git_text", f"worktree {self.root}\0"), ("group_memory", 0)):
            mock = patch.object(guard, name, return_value=value)
            _ = mock.start()
            self.addCleanup(mock.stop)

    def write(self, path: str, text: str = "x") -> Path:
        destination = self.root / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        _ = destination.write_text(text)
        return destination

    def own(self, current: dict[str, str] | None = None) -> None:
        self.info.target.mkdir(mode=0o700, exist_ok=True)
        guard.write_state(self.info, current)

    def state(self) -> dict[str, object]:
        result = guard.read_state(self.info)
        self.assertIsNotNone(result)
        assert result is not None
        return result

    def execute(self, args: Sequence[str] = ("build", "--locked"), *, adopt: str | None = None, retire: bool = False) -> None:
        with guard.build_lock(self.info, 0) as fd:
            guard.execute(self.info, args, self.env, build_fd=fd, adopt=adopt, retire=retire)

    def fake_cargo(self, command: Sequence[str], _info: guard.Identity,
                   _env: dict[str, str], **_kwargs: object) -> str:
        if command == ["cargo", "--version"]:
            return "cargo fixture"
        if command == ["rustc", "-vV"]:
            return "rustc fixture\nhost: fixture-host\n"
        if command[:2] == ["cargo", "clean"]:
            # Simulate Cargo deleting only an authored fixture, never a real cache.
            (self.info.target / "artifact").unlink(missing_ok=True)
        else:
            _ = self.write("target/artifact", "authored build fixture")
        return ""

    def evidence(self, path: str, *, status: str = "active") -> None:
        row = {"id": "M-001", "path": path, "sha256": None, "class": "local-private",
               "kind": "input", "date": None, "label": "authored metadata fixture",
               "status": status, "legacy": True}
        _ = self.write("evidence/fixture/001.jsonl", json.dumps(row) + "\n")

    def test_common_lock_serializes_worktrees_and_keeps_inode(self) -> None:
        other = guard.Identity(self.root.parent / "other", self.info.common)
        with guard.build_lock(self.info, 0):
            inode = (self.info.state_dir / "build.lock").stat().st_ino
            with self.assertRaisesRegex(guard.GuardError, "timed out"):
                with guard.build_lock(other, 0):
                    self.fail("second checkout entered")
        with guard.build_lock(other, 0):
            self.assertEqual((self.info.state_dir / "build.lock").stat().st_ino, inode)

    def test_shared_runtime_leases_exclude_cleanup(self) -> None:
        with guard.target_lease(self.info, exclusive=False):
            with guard.target_lease(self.info, exclusive=False):
                with self.assertRaisesRegex(guard.GuardError, "timed out"):
                    with guard.target_lease(self.info, exclusive=True, timeout=0):
                        self.fail("cleanup entered")
        with guard.target_lease(self.info, exclusive=True, timeout=0):
            pass

    def test_lock_wait_announces_and_normal_default_covers_command_deadline(self) -> None:
        with guard.build_lock(self.info, 0):
            with patch.object(guard, "TIMEOUT", 0), patch("sys.stderr", new_callable=io.StringIO) as stderr:
                with self.assertRaises(guard.GuardError):
                    with guard.build_lock(self.info, 0):
                        pass
                self.assertIn("waiting for", stderr.getvalue())

    def test_aborted_supervisor_child_keeps_both_inherited_locks(self) -> None:
        """Actual subprocesses prove lifetime, including supervisor SIGKILL."""
        ready = self.root / "ready"
        release = self.root / "release"
        child_code = f"""import os,pathlib,time
pathlib.Path({str(ready)!r}).write_text(str(os.getpid()))
p=pathlib.Path({str(release)!r})
while not p.exists():
    time.sleep(0.01)
"""
        supervisor_code = f"""import os,pathlib,sys
from tools import cargo_guard as g
i=g.Identity(pathlib.Path({str(self.root)!r}),pathlib.Path({str(self.info.common)!r}))
with g.build_lock(i) as b:
    with g.target_lease(i,exclusive=False) as l:
        g.run_owned([sys.executable,"-c",{child_code!r}],i,os.environ,pass_fds=(b,l),monitor_space=False)
"""
        supervisor = subprocess.Popen([sys.executable, "-c", supervisor_code], cwd=Path(__file__).resolve().parents[2],
                                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        child_pid: int | None = None
        try:
            deadline = time.monotonic() + 8
            while not ready.exists() and time.monotonic() < deadline and supervisor.poll() is None:
                time.sleep(0.01)
            self.assertTrue(ready.exists(), "child did not start")
            child_pid = int(ready.read_text())
            supervisor.kill()
            _ = supervisor.wait(timeout=5)
            for path in (self.info.state_dir / "build.lock", self.info.lease):
                fd = os.open(path, os.O_RDWR)
                try:
                    with self.assertRaises(BlockingIOError):
                        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                finally:
                    os.close(fd)
            _ = release.write_text("done")
            deadline = time.monotonic() + 5
            while True:
                try:
                    with guard.build_lock(self.info, 0):
                        with guard.target_lease(self.info, exclusive=True, timeout=0):
                            child_pid = None
                            break
                except guard.GuardError:
                    if time.monotonic() >= deadline:
                        self.fail("child did not release locks")
                    time.sleep(0.02)
        finally:
            if supervisor.poll() is None:
                supervisor.kill()
            _ = supervisor.wait(timeout=5)
            if child_pid is not None:
                try:
                    os.kill(child_pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_real_lightweight_subprocess_inherits_descriptors(self) -> None:
        with guard.build_lock(self.info, 0) as build, guard.target_lease(self.info, exclusive=True) as lease:
            code = f"import os; os.fstat({build}); os.fstat({lease}); print('inherited')"
            output = guard.run_owned([sys.executable, "-c", code], self.info, os.environ,
                                     pass_fds=(build, lease), monitor_space=False, capture=True)
        self.assertEqual(output.strip(), "inherited")

    def test_catchable_signals_stop_real_owned_group(self) -> None:
        for sig in (signal.SIGHUP, signal.SIGTERM, signal.SIGINT):
            ready = self.root / f"ready-{sig}"
            code = f"import os,pathlib,time; pathlib.Path({str(ready)!r}).write_text(str(os.getpid())); time.sleep(30)"
            wrapper = (
                "import os,pathlib,signal,sys; from tools import cargo_guard as g; "
                f"i=g.Identity(pathlib.Path({str(self.root)!r}),pathlib.Path({str(self.info.common)!r})); "
                "[signal.signal(s,g.interrupted) for s in g.TERMINATION]; "
                f"g.run_owned([sys.executable,'-c',{code!r}],i,os.environ,pass_fds=(),monitor_space=False)"
            )
            process = subprocess.Popen([sys.executable, "-c", wrapper], cwd=Path(__file__).resolve().parents[2],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                deadline = time.monotonic() + 5
                while not ready.exists() and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(ready.exists())
                process.send_signal(sig)
                self.assertNotEqual(process.wait(timeout=5), 0)
                with self.assertRaises(ProcessLookupError):
                    os.kill(int(ready.read_text()), 0)
            finally:
                if process.poll() is None:
                    process.kill()
                _ = process.wait(timeout=5)

    def test_unknown_even_empty_target_is_not_silently_adopted(self) -> None:
        self.info.target.mkdir()
        with patch.object(guard, "run_owned") as run, self.assertRaisesRegex(guard.GuardError, "unowned target"):
            self.execute()
        run.assert_not_called()

    def test_fresh_target_private_and_stamped_before_cargo(self) -> None:
        seen: list[bool] = []
        def cargo(command: Sequence[str], info: guard.Identity, env: dict[str, str], **kwargs: object) -> str:
            if command[:2] == ["cargo", "build"]:
                seen.append(self.state()["generation"] is not None and self.state()["validated"] is False)
                self.assertEqual(info.target.stat().st_mode & 0o777, 0o700)
            return self.fake_cargo(command, info, env, **kwargs)
        with patch.object(guard, "run_owned", side_effect=cargo), patch.object(guard, "check_space"):
            self.execute()
        self.assertEqual(seen, [True])
        self.assertIs(self.state()["validated"], True)

    def test_explicit_exact_adoption_is_private_and_requires_full_retirement(self) -> None:
        _ = self.write("target/artifact")
        with patch.object(guard, "cleanup_veto"), patch.object(guard, "run_owned") as run:
            with self.assertRaisesRegex(guard.GuardError, "canonical absolute"):
                self.execute(adopt="target")
            self.execute(adopt=str(self.info.target))
        run.assert_not_called()
        self.assertTrue((self.info.target / "artifact").exists())
        self.assertIs(self.state()["needs_clean"], True)
        self.assertEqual(self.info.target.stat().st_mode & 0o777, 0o700)

    def test_inode_replacement_and_public_permissions_are_refused(self) -> None:
        self.own()
        _ = self.info.target.rename(self.root / "old-target")
        self.info.target.mkdir(mode=0o700)
        with self.assertRaisesRegex(guard.GuardError, "replaced"):
            _ = self.state()
        guard.write_state(self.info, None)
        self.info.target.chmod(0o755)
        with self.assertRaisesRegex(guard.GuardError, "private"):
            _ = self.state()

    def test_own_symlink_and_nested_mount_are_rejected(self) -> None:
        destination = self.root.parent / "shared"
        destination.mkdir()
        self.info.target.symlink_to(destination, target_is_directory=True)
        with self.assertRaisesRegex(guard.GuardError, "canonical"):
            _ = guard.check_configuration(self.info, self.env)
        metadata = self.write("mountinfo", f"1 2 0:1 / {self.info.target}/debug rw - ext4 /dev/fixture rw\n")
        with self.assertRaisesRegex(guard.GuardError, "bind mount"):
            guard.mount_veto(self.info.target, metadata)

    def test_allocated_snapshot_deduplicates_hardlinks_without_live_veto(self) -> None:
        source = self.write("target/artifact", "authored fixture")
        os.link(source, self.info.target / "alias")
        self.assertEqual(guard.target_inventory(self.info.target)[0], source.stat().st_blocks * 512)
        os.link(source, self.root / "outside-alias")
        _ = guard.target_inventory(self.info.target)
        with self.assertRaisesRegex(guard.GuardError, "hardlinks outside"):
            _ = guard.target_inventory(self.info.target, strict=True)

    def test_cleanup_vetoes_nested_symlinks_only_in_strict_mode(self) -> None:
        self.info.target.mkdir()
        (self.info.target / "alias").symlink_to(self.root / "elsewhere")
        _ = guard.target_inventory(self.info.target)
        with self.assertRaisesRegex(guard.GuardError, "non-regular"):
            _ = guard.target_inventory(self.info.target, strict=True)

    def test_unrelated_symlink_missing_prunable_worktrees_are_reporting_only(self) -> None:
        self.own()
        other = self.root.parent / "other"
        other.mkdir()
        (other / "target").symlink_to(self.info.target, target_is_directory=True)
        alias = self.root.parent / "alias"
        alias.symlink_to(other, target_is_directory=True)
        records = f"worktree {self.root}\0worktree {other}\0worktree {alias}\0worktree {self.root.parent / 'missing'}\0prunable stale\0"
        with patch.object(guard, "git_text", return_value=records):
            self.assertEqual(len(guard.checkout_roots(self.info)), 2)
            with patch.object(guard, "target_inventory", wraps=guard.target_inventory) as inventory:
                guard.report_caches(self.info)
                self.assertEqual(inventory.call_count, 1)
            guard.evidence_veto(self.info)
        self.assertTrue((other / "target").is_symlink())

    def test_network_registry_user_config_is_read_only_and_accepted(self) -> None:
        config = Path(self.env["CARGO_HOME"]) / "config.toml"
        text = '[net]\nretry=2\n[registries.fixture]\nindex="https://example.invalid/index"\n'
        _ = config.write_text(text)
        _ = guard.check_configuration(self.info, self.env)
        self.assertEqual(config.read_text(), text)
        _ = config.write_text('[build]\nbuild-dir="elsewhere"\n')
        with self.assertRaisesRegex(guard.GuardError, "only build.jobs"):
            _ = guard.check_configuration(self.info, self.env)

    def test_environment_redirections_and_compiler_overrides_are_refused(self) -> None:
        for key, value in (("CARGO_TARGET_DIR", "/elsewhere"), ("CARGO_BUILD_BUILD_DIR", "/elsewhere"),
                           ("CARGO_PROFILE_DEV_DEBUG", "0"), ("RUSTFLAGS", "-C debuginfo=0"),
                           ("CARGO_INCREMENTAL", "0"), ("RUSTC_WRAPPER", "/wrapper"),
                           ("CARGO_BUILD_JOBS", "8"), ("RUSTUP_TOOLCHAIN", "nightly")):
            with self.subTest(key=key), self.assertRaises(guard.GuardError):
                _ = guard.check_configuration(self.info, {**self.env, key: value})
        _ = guard.check_configuration(self.info, {**self.env, "CARGO_BUILD_JOBS": "2", "RUSTUP_TOOLCHAIN": "fixture"})

    def test_active_pinned_toolchain_host_suffix_is_accepted_and_wrong_host_refused(self) -> None:
        with patch.object(guard, "run_owned", side_effect=self.fake_cargo), patch.object(guard, "check_space"):
            self.env["RUSTUP_TOOLCHAIN"] = "fixture-fixture-host"
            self.execute()
            self.env["RUSTUP_TOOLCHAIN"] = "fixture-other-host"
            with self.assertRaisesRegex(guard.GuardError, "active pinned"):
                self.execute()

    def test_normal_arguments_filters_json_quiet_test_trailers(self) -> None:
        for args in (
            ["check", "--locked", "-q", "--message-format", "json"],
            ["build", "--locked", "--release", "--bin=fixture"],
            ["test", "--locked", "--no-run", "--test", "protocol"],
            ["test", "--locked", "filter", "--", "--list", "--ignored", "--include-ignored", "--exact", "--nocapture", "--test-threads=2", "--format", "terse"],
            ["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
            ["test", "--locked", "--config", "net.offline=true", "--", "filter", "--skip=other"],
        ):
            with self.subTest(args=args):
                _ = guard.check_arguments(args, "fixture")
        for args in (["build", "--locked", "--target-dir", "/elsewhere"],
                     ["check", "--locked", "--config=build.jobs=8"],
                     ["test", "--locked", "--", "--logfile=private"],
                     ["test", "--locked", "--", ">", "output"],
                     ["clippy", "--locked", "--", "-C", "debuginfo=0"],
                     ["build", "--locked", "--profile", "custom"], ["test", "--no-run"]):
            with self.subTest(args=args), self.assertRaises(guard.GuardError):
                _ = guard.check_arguments(args, "fixture")

    def test_feature_variants_reject_before_cleanup_but_target_selections_do_not(self) -> None:
        base = guard.check_arguments(["test", "--locked"], "fixture")
        self.assertEqual(base, guard.check_arguments(["test", "--locked", "--test", "protocol", "filter"], "fixture"))
        for flag in ("--features=typo", "-F", "--all-features", "--no-default-features"):
            with self.assertRaisesRegex(guard.GuardError, "feature flags"):
                _ = guard.check_arguments(["test", "--locked", flag], "fixture")

    def test_evidence_lexical_resolved_overlap_and_fixity_parent_veto(self) -> None:
        for path in (".", "target", "target/debug/artifact", "target/fixity.sha256", "fixity.sha256"):
            self.evidence(path)
            with self.subTest(path=path), self.assertRaisesRegex(guard.GuardError, "active evidence"):
                guard.evidence_veto(self.info)
        alias = self.root / "work/alias"
        alias.parent.mkdir()
        alias.symlink_to(self.info.target, target_is_directory=True)
        self.evidence("work/alias/file")
        with self.assertRaisesRegex(guard.GuardError, "active evidence"):
            guard.evidence_veto(self.info)

    def test_unlisted_proton_root_alias_does_not_veto_fixity(self) -> None:
        _ = self.write("work/case/fixity.sha256", "a" * 64 + "  kept.log\n")
        _ = self.write("work/case/kept.log", "authored artifact; do not read")
        dosdevices = self.root / "work/case/pfx/dosdevices"
        dosdevices.mkdir(parents=True)
        (dosdevices / "z:").symlink_to(Path("/"), target_is_directory=True)
        self.evidence("work/case/fixity.sha256")
        guard.evidence_veto(self.info)

    def test_listed_fixity_aliases_to_target_ancestors_and_descendants_veto(self) -> None:
        manifest = self.write("work/case/fixity.sha256", "a" * 64 + "  alias\n")
        alias = manifest.parent / "alias"
        self.evidence("work/case/fixity.sha256")
        for destination in (self.info.target, self.root, self.info.target / "debug"):
            with self.subTest(destination=destination):
                alias.symlink_to(destination, target_is_directory=True)
                with self.assertRaisesRegex(guard.GuardError, "active fixity entry"):
                    guard.evidence_veto(self.info)
                alias.unlink()
        _ = manifest.write_text("a" * 64 + "  alias/artifact\n")
        alias.symlink_to(self.info.target, target_is_directory=True)
        with self.assertRaisesRegex(guard.GuardError, "active fixity entry"):
            guard.evidence_veto(self.info)
        alias.unlink()
        _ = manifest.write_text("a" * 64 + "  alias\n")
        alias.symlink_to(Path("/"), target_is_directory=True)
        with self.assertRaisesRegex(guard.GuardError, "active fixity entry"):
            guard.evidence_veto(self.info)

    def test_manifest_itself_resolving_into_target_vetoes(self) -> None:
        manifest = self.root / "work/case/fixity.sha256"
        manifest.parent.mkdir(parents=True)
        manifest.symlink_to(self.info.target / "artifact")
        self.evidence("work/case/fixity.sha256")
        with self.assertRaisesRegex(guard.GuardError, "active evidence"):
            guard.evidence_veto(self.info)

    def test_absent_fixity_dirs_and_evidence_dirs_do_not_crash_or_read_artifacts(self) -> None:
        guard.evidence_veto(self.info)
        self.evidence("reference-data/absent/fixity.sha256")
        original = Path.open
        def metadata_only(path: Path, mode: str = "r", *, encoding: str | None = None) -> TextIO:
            self.assertTrue(path.suffix == ".jsonl", f"artifact opened: {path}")
            self.assertEqual(mode, "r")
            return original(path, "r", encoding=encoding)
        # Absent manifests need only index metadata; no referenced artifact reads.
        with patch.object(Path, "open", autospec=True, side_effect=metadata_only):
            guard.evidence_veto(self.info)
        (self.root / "reference-data/absent").mkdir(parents=True)
        guard.evidence_veto(self.info)
        records = f"worktree {self.root}\0worktree {self.root.parent / 'missing'}\0"
        with patch.object(guard, "git_text", return_value=records):
            guard.evidence_veto(self.info)

    def test_fixity_reads_hash_path_metadata_only(self) -> None:
        manifest = self.write("work/case/fixity.sha256", "a" * 64 + "  kept.log\n")
        _ = self.write("work/case/kept.log", "authored artifact; do not read")
        self.evidence("work/case/fixity.sha256")
        original = Path.read_text
        def metadata_only(path: Path, encoding: str | None = None, errors: str | None = None) -> str:
            self.assertEqual(path, manifest, f"artifact read: {path}")
            return original(path, encoding=encoding, errors=errors)
        with patch.object(Path, "read_text", autospec=True, side_effect=metadata_only) as read:
            guard.evidence_veto(self.info)
        read.assert_called_once_with(manifest, encoding="utf-8")

    def test_present_malformed_unreadable_and_dangling_fixity_fail_closed(self) -> None:
        manifest = self.write("work/case/fixity.sha256", "malformed\n")
        self.evidence("work/case/fixity.sha256")
        for content in ("malformed\n", "a" * 64 + "  ../escape\n", "a" * 64 + "  /absolute\n"):
            _ = manifest.write_text(content)
            with self.subTest(content=content), self.assertRaisesRegex(guard.GuardError, "unreadable fixity"):
                guard.evidence_veto(self.info)
        _ = manifest.write_text("a" * 64 + "  kept.log\n")
        with patch.object(Path, "read_text", side_effect=PermissionError("authored denial")):
            with self.assertRaisesRegex(guard.GuardError, "unreadable fixity"):
                guard.evidence_veto(self.info)
        _ = manifest.write_bytes(b"\xff")
        with self.assertRaisesRegex(guard.GuardError, "unreadable fixity"):
            guard.evidence_veto(self.info)
        manifest.unlink()
        manifest.symlink_to(manifest.parent / "missing")
        with self.assertRaisesRegex(guard.GuardError, "unreadable fixity"):
            guard.evidence_veto(self.info)
        manifest.unlink()
        manifest.mkdir()
        with self.assertRaisesRegex(guard.GuardError, "invalid or oversized fixity"):
            guard.evidence_veto(self.info)

    def test_evidence_overlap_uses_components_not_string_prefixes(self) -> None:
        base = Path("/checkout/target")
        for other in (base, base / "deps", base.parent, Path("/")):
            self.assertTrue(guard.paths_overlap(base, other))
            self.assertTrue(guard.paths_overlap(other, base))
        for other in (Path("/checkout/target-old"), Path("/other/target")):
            self.assertFalse(guard.paths_overlap(base, other))

    def test_shared_manifest_checks_each_lexical_parent(self) -> None:
        manifest = self.write("work/a/fixity.sha256", "a" * 64 + "  alias/payload\n")
        self.evidence("work/a/fixity.sha256")
        other = self.root / "work/b"
        other.mkdir()
        (other / "fixity.sha256").symlink_to(manifest)
        (other / "alias").symlink_to(self.info.target, target_is_directory=True)
        shard = self.root / "evidence/fixture/001.jsonl"
        row = shard.read_text()
        _ = shard.write_text(row + row.replace("work/a/", "work/b/"))
        with patch.object(guard, "FIXITY_METADATA_BYTES", manifest.stat().st_size):
            with self.assertRaisesRegex(guard.GuardError, "active fixity entry"):
                guard.evidence_veto(self.info)

    def test_signal_during_child_registration_still_reaps_child(self) -> None:
        original = subprocess.Popen
        children: list[subprocess.Popen[str]] = []

        def launch(*_args: object, **_kwargs: object) -> subprocess.Popen[str]:
            # Deliberately signal after real creation but before assigning caller's process.
            child = original([sys.executable, "-c", "import time; time.sleep(30)"],
                             start_new_session=True, text=True)
            children.append(child)
            os.kill(os.getpid(), signal.SIGTERM)
            return child

        with patch.object(subprocess, "Popen", side_effect=launch):
            with self.assertRaisesRegex(guard.GuardError, "interrupted"):
                _ = guard.run_owned(["unused"], self.info, self.env, pass_fds=(), monitor_space=False)
        self.assertEqual(len(children), 1)
        with self.assertRaises(ProcessLookupError):
            os.kill(children[0].pid, 0)

    def test_fixity_metadata_bytes_and_entries_are_bounded_across_rows(self) -> None:
        manifest = self.write("work/case/fixity.sha256", "a" * 64 + "  first\n" + "b" * 64 + "  second\n")
        self.evidence("work/case/fixity.sha256")
        with patch.object(guard, "FIXITY_METADATA_BYTES", 1), patch.object(docs_lint, "read_fixity") as read:
            with self.assertRaisesRegex(guard.GuardError, "oversized fixity"):
                guard.evidence_veto(self.info)
        read.assert_not_called()
        with patch.object(guard, "FIXITY_METADATA_ENTRIES", 1):
            with self.assertRaisesRegex(guard.GuardError, "entry budget"):
                guard.evidence_veto(self.info)
        shard = self.root / "evidence/fixture/001.jsonl"
        row = shard.read_text()
        _ = shard.write_text(row + row)
        # Repeated rows/worktree aliases consume the resolved manifest budget once.
        with patch.object(guard, "FIXITY_METADATA_BYTES", manifest.stat().st_size), patch.object(guard, "FIXITY_METADATA_ENTRIES", 2):
            guard.evidence_veto(self.info)
        _ = self.write("work/second/fixity.sha256", manifest.read_text())
        _ = shard.write_text(row + row.replace("work/case/fixity.sha256", "work/second/fixity.sha256"))
        with patch.object(guard, "FIXITY_METADATA_BYTES", manifest.stat().st_size):
            with self.assertRaisesRegex(guard.GuardError, "oversized fixity"):
                guard.evidence_veto(self.info)
        with patch.object(guard, "FIXITY_METADATA_ENTRIES", 2):
            with self.assertRaisesRegex(guard.GuardError, "entry budget"):
                guard.evidence_veto(self.info)

    def test_other_worktree_evidence_veto_and_malformed_metadata(self) -> None:
        other = self.root.parent / "other"
        (other / "evidence").mkdir(parents=True)
        self.evidence(str(self.info.target))
        _ = (other / "evidence/001.jsonl").write_text((self.root / "evidence/fixture/001.jsonl").read_text())
        self.evidence("target", status="retired")
        with patch.object(guard, "git_text", return_value=f"worktree {other}\0"), self.assertRaisesRegex(guard.GuardError, "active evidence"):
            guard.evidence_veto(self.info)
        _ = self.write("evidence/fixture/001.jsonl", '{"status":"active"}\n')
        with self.assertRaises(ValueError):
            guard.evidence_veto(self.info)

    def test_process_readable_matches_veto_despite_unreadable_neighbors(self) -> None:
        proc = self.root / "proc"
        (proc / "123/fd").mkdir(parents=True)
        (proc / "122").mkdir()
        (proc / "123/cwd").symlink_to(self.info.target)
        with self.assertRaisesRegex(guard.GuardError, "uses target"):
            _ = guard.process_veto(self.info, proc)
        (proc / "123/cwd").unlink()
        _ = self.write("proc/123/maps", f"1-2 r--p 0 0:0 0 {self.info.target}/mapped\n")
        with self.assertRaisesRegex(guard.GuardError, "maps target"):
            _ = guard.process_veto(self.info, proc)

    def test_process_exe_fd_and_checkout_compiler_veto(self) -> None:
        (self.root / "proc/123/fd").mkdir(parents=True)
        for location in ("exe", "fd/7"):
            link = self.root / "proc/123" / location
            link.symlink_to(self.info.target / "artifact")
            with self.subTest(location=location), self.assertRaisesRegex(guard.GuardError, "uses target"):
                _ = guard.process_veto(self.info, self.root / "proc")
            link.unlink()
        (self.root / "proc/123/cwd").symlink_to(self.root)
        # Even absent maps must not hide a readable compiler/cwd match.
        _ = self.write("proc/123/cmdline", "cargo\0build\0")
        with self.assertRaisesRegex(guard.GuardError, "unguarded compiler"):
            _ = guard.process_veto(self.info, self.root / "proc")

    def test_process_denied_vanished_and_missing_proc_are_limitations(self) -> None:
        (self.root / "proc/123").mkdir(parents=True)
        self.assertGreater(guard.process_veto(self.info, self.root / "proc"), 0)
        self.assertEqual(guard.process_veto(self.info, self.root / "missing-proc"), 1)
        with patch.object(os, "readlink", side_effect=PermissionError("denied")):
            self.assertGreater(guard.process_veto(self.info, self.root / "proc"), 0)

    def test_cargo_profile_lock_probe_vetoes_then_releases_before_cargo(self) -> None:
        lock = self.write("target/debug/.cargo-lock", "")
        fd = os.open(lock, os.O_RDWR)
        try:
            fcntl.flock(fd, fcntl.LOCK_SH)
            with self.assertRaisesRegex(guard.GuardError, "Cargo lock is busy"):
                guard.cargo_lock_probe(self.info)
            fcntl.flock(fd, fcntl.LOCK_UN)
            guard.cargo_lock_probe(self.info)
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(fd)

    def test_source_edits_and_nested_module_addition_keep_layout_generation(self) -> None:
        before = guard.generation(self.info, "config", "tools")
        _ = self.write("src/lib.rs", "mod added;")
        _ = self.write("src/added.rs", "// module")
        _ = self.write("tests/protocol/nested.rs", "// nested module")
        self.assertEqual(before, guard.generation(self.info, "config", "tools"))
        _ = self.write("tests/protocol.rs", "// new target root")
        self.assertNotEqual(before, guard.generation(self.info, "config", "tools"))

    def test_auto_directory_main_explicit_targets_and_removals_change_generation(self) -> None:
        before = guard.generation(self.info, "config", "tools")
        root = self.write("tests/one/main.rs", "// auto root")
        self.assertNotEqual(before, guard.generation(self.info, "config", "tools"))
        root.unlink()
        self.assertEqual(before, guard.generation(self.info, "config", "tools"))
        _ = self.write("Cargo.toml", '[package]\nname="fixture"\nversion="0.1.0"\n[[test]]\nname="explicit"\npath="custom/root.rs"\n')
        self.assertNotEqual(before, guard.generation(self.info, "config", "tools"))

    def test_lock_toolchain_profile_target_changes_always_full_clean(self) -> None:
        with patch.object(guard, "run_owned", side_effect=self.fake_cargo) as cargo, patch.object(guard, "check_space"), patch.object(guard, "cleanup_veto"):
            self.execute()
            for path, content in (("Cargo.lock", "version=4\n# changed"),
                                  ("rust-toolchain.toml", '[toolchain]\nchannel="fixture"\nprofile="minimal"'),
                                  ("Cargo.toml", '[package]\nname="fixture"\nversion="0.1.0"\n[profile.dev]\ndebug=1'),
                                  ("tests/new.rs", "// root")):
                cargo.reset_mock()
                _ = self.write(path, content)
                self.execute()
                commands = [call.args[0] for call in cargo.call_args_list]
                self.assertIn(["cargo", "clean", "--locked"], commands)
                self.assertNotIn(["cargo", "clean", "--locked", "--package", "fixture"], commands)

    def test_cold_failure_then_warm_retry_preserves_prepared_generation(self) -> None:
        attempts = 0
        def cargo(command: Sequence[str], info: guard.Identity, env: dict[str, str], **kwargs: object) -> str:
            nonlocal attempts
            result = self.fake_cargo(command, info, env, **kwargs)
            if command[:2] == ["cargo", "test"]:
                attempts += 1
                if attempts == 1:
                    raise guard.GuardError("test failed")
            return result
        with patch.object(guard, "run_owned", side_effect=cargo) as run, patch.object(guard, "check_space"):
            with self.assertRaisesRegex(guard.GuardError, "test failed"):
                self.execute(["test", "--locked"])
            prepared = self.state()["generation"]
            self.assertIsNotNone(prepared)
            self.assertIs(self.state()["validated"], False)
            self.execute(["test", "--locked"])
            self.assertEqual(self.state()["generation"], prepared)
            self.assertIs(self.state()["validated"], True)
            self.assertNotIn(["cargo", "clean", "--locked"], [call.args[0] for call in run.call_args_list])

    def test_failed_clippy_and_timeout_keep_same_generation_artifacts(self) -> None:
        for command in ("clippy", "build"):
            with patch.object(guard, "run_owned", side_effect=self.fake_cargo), patch.object(guard, "check_space"):
                self.execute([command, "--locked"])
            prepared = self.state()["generation"]
            def failed(args: Sequence[str], info: guard.Identity, env: dict[str, str], **kwargs: object) -> str:
                if args[:2] == ["cargo", command]:
                    raise guard.GuardError("timeout" if command == "build" else "clippy failure")
                return self.fake_cargo(args, info, env, **kwargs)
            with patch.object(guard, "run_owned", side_effect=failed), patch.object(guard, "check_space"):
                with self.assertRaises(guard.GuardError):
                    self.execute([command, "--locked"])
            self.assertEqual(self.state()["generation"], prepared)
            self.assertIs(self.state()["validated"], False)
            with patch.object(guard, "run_owned", side_effect=self.fake_cargo) as run, patch.object(guard, "check_space"):
                self.execute([command, "--locked"])
            self.assertNotIn(["cargo", "clean", "--locked"], [call.args[0] for call in run.call_args_list])

    def test_hold_target_cli_uses_shared_lease_and_never_global_build_lock(self) -> None:
        self.own()
        old_handlers = {sig: signal.getsignal(sig) for sig in guard.TERMINATION}
        def runtime(command: Sequence[str], _info: guard.Identity, _env: dict[str, str], **kwargs: object) -> str:
            self.assertEqual(command, [sys.executable, "-c", "pass"])
            self.assertIn("pass_fds", kwargs)
            with guard.target_lease(self.info, exclusive=False, timeout=0):
                pass
            with self.assertRaises(guard.GuardError):
                with guard.target_lease(self.info, exclusive=True, timeout=0):
                    pass
            return ""
        try:
            with patch.object(guard, "identity", return_value=self.info), patch.object(guard, "build_lock", side_effect=AssertionError("runtime took build lock")), patch.object(guard, "check_configuration"), patch.object(guard, "run_owned", side_effect=runtime):
                self.assertEqual(guard.main(["--hold-target", "--", sys.executable, "-c", "pass"]), 0)
        finally:
            for sig, handler in old_handlers.items():
                _ = signal.signal(sig, handler)

    def test_interrupted_clean_marks_needs_clean_and_retries_full_clean(self) -> None:
        self.own({"old": "generation"})
        with patch.object(guard, "cleanup_veto"), patch.object(guard, "run_owned", side_effect=guard.GuardError("interrupted")):
            with guard.build_lock(self.info, 0) as build, guard.target_lease(self.info, exclusive=True) as lease:
                with self.assertRaisesRegex(guard.GuardError, "interrupted"):
                    guard.clean(self.info, self.env, pass_fds=(build, lease))
        self.assertIs(self.state()["needs_clean"], True)
        with patch.object(guard, "cleanup_veto"), patch.object(guard, "check_space"), patch.object(guard, "run_owned", side_effect=self.fake_cargo) as run:
            self.execute()
        self.assertIn(["cargo", "clean", "--locked"], [call.args[0] for call in run.call_args_list])
        self.assertIs(self.state()["needs_clean"], False)

    def test_pending_state_recovers_under_locks_and_rejects_aliases(self) -> None:
        self.own()
        pending = self.info.state.with_suffix(".pending")
        _ = pending.write_text("partial json")
        with guard.build_lock(self.info, 0), guard.target_lease(self.info, exclusive=True):
            guard.write_state(self.info, {"prepared": "new"})
        self.assertFalse(pending.exists())
        self.assertEqual(self.state()["generation"], {"prepared": "new"})
        pending.symlink_to(self.root / "do-not-touch")
        with self.assertRaisesRegex(guard.GuardError, "unsafe pending"):
            guard.write_state(self.info, None)
        self.assertTrue(pending.is_symlink())

    def test_pending_hardlink_never_truncates_external_alias(self) -> None:
        self.own()
        source = self.write("external-state", "keep fixture")
        pending = self.info.state.with_suffix(".pending")
        os.link(source, pending)
        with self.assertRaisesRegex(guard.GuardError, "unsafe pending"):
            guard.write_state(self.info, None)
        self.assertEqual(source.read_text(), "keep fixture")

    def test_stale_cleanup_precedes_admission_and_holds_both_locks(self) -> None:
        self.own({"old": "generation"})
        order: list[str] = []
        def cargo(command: Sequence[str], info: guard.Identity, env: dict[str, str], **kwargs: object) -> str:
            if command[:2] == ["cargo", "clean"]:
                order.append("clean")
                for path in (info.state_dir / "build.lock", info.lease):
                    fd = os.open(path, os.O_RDWR)
                    try:
                        with self.assertRaises(BlockingIOError):
                            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    finally:
                        os.close(fd)
            return self.fake_cargo(command, info, env, **kwargs)
        def admission(_root: Path, reserve: int = 0, *, policy: guard.Policy = guard.LOCAL) -> None:
            _ = reserve, policy
            order.append("admission")
        with patch.object(guard, "run_owned", side_effect=cargo), patch.object(guard, "check_space", side_effect=admission), patch.object(guard, "cleanup_veto"):
            self.execute()
        self.assertLess(order.index("clean"), order.index("admission"))

    def test_cold_warm_reserve_math_and_absolute_floor(self) -> None:
        self.assertEqual(guard.admission_reserve(0), 8 * guard.GIB)
        self.assertEqual(guard.admission_reserve(4 * guard.GIB), 6 * guard.GIB)
        self.assertEqual(guard.admission_reserve(8 * guard.GIB), 2 * guard.GIB)
        for size in (8 * guard.GIB + 1, 10 * guard.GIB, 11 * guard.GIB):
            with self.assertRaisesRegex(guard.GuardError, "transient headroom"):
                _ = guard.admission_reserve(size)
        with patch.object(guard, "available_ram", return_value=4 * guard.GIB):
            for policy in (guard.LOCAL, guard.CI):
                with patch.object(shutil, "disk_usage", return_value=disk_usage(policy.disk_floor + 8 * guard.GIB)):
                    guard.check_space(self.root, guard.admission_reserve(0), policy=policy)
                with patch.object(shutil, "disk_usage", return_value=disk_usage(policy.disk_floor - 1)), self.assertRaisesRegex(guard.GuardError, "disk floor"):
                    guard.check_space(self.root, policy=policy)
        with patch.object(guard, "available_ram", return_value=guard.RAM_FLOOR - 1), self.assertRaisesRegex(guard.GuardError, "available RAM"):
            guard.check_space(self.root)

    def test_ci_env_is_explicit_and_no_generated_config(self) -> None:
        self.assertEqual(guard.selected_policy({"CI": "true"}, None), guard.LOCAL)
        self.assertEqual(guard.selected_policy({"ICARUS_CARGO_GUARD_POLICY": "ci"}, None), guard.CI)
        self.assertEqual(guard.selected_policy({"ICARUS_CARGO_GUARD_POLICY": "ci"}, "local"), guard.LOCAL)
        self.assertFalse((self.root / ".cargo/guard.toml").exists())

    def test_other_cache_size_does_not_consume_own_cap(self) -> None:
        other = self.root.parent / "other"
        (other / "target").mkdir(parents=True)
        def inventory(path: Path, *, strict: bool = False) -> tuple[int, int]:
            _ = strict
            return (20 * guard.GIB, 1) if path == other / "target" else (0, 0)
        records = f"worktree {self.root}\0worktree {other}\0"
        with patch.object(guard, "git_text", return_value=records), patch.object(guard, "target_inventory", side_effect=inventory), patch.object(guard, "check_space"), patch.object(guard, "run_owned", side_effect=self.fake_cargo):
            self.execute()
        self.assertIs(self.state()["validated"], True)
        self.assertFalse(guard.Identity(other, self.info.common).state.exists())

    def test_live_monitor_never_inventories_cache_or_attributes_free_space_delta(self) -> None:
        with patch.object(guard, "target_inventory", side_effect=AssertionError("live census")), patch.object(guard, "report_caches", side_effect=AssertionError("live report")), patch.object(guard, "check_space") as space:
            _ = guard.run_owned([sys.executable, "-c", "import time;time.sleep(0.1)"], self.info,
                                os.environ, pass_fds=(), timeout=5)
        self.assertGreater(space.call_count, 0)
        self.assertTrue(all(call.args == (self.root,) for call in space.call_args_list))

    def test_timeout_ram_floor_failures_stop_owned_group(self) -> None:
        for kind in ("timeout", "ram", "floor"):
            process = FakeProcess()
            with patch.object(subprocess, "Popen", return_value=process), patch.object(guard, "child_status", return_value=None), patch.object(guard, "group_memory", return_value=guard.OWNED_RAM_LIMIT + 1 if kind == "ram" else 0), patch.object(guard, "check_space", side_effect=guard.GuardError("floor") if kind == "floor" else None), patch.object(os, "killpg") as kill, patch.object(time, "sleep"):
                with self.subTest(kind=kind), self.assertRaisesRegex(guard.GuardError, {"timeout": "timed out", "ram": "resident RAM", "floor": "floor"}[kind]):
                    _ = guard.run_owned(["authored-fixture"], self.info, self.env, pass_fds=(), timeout=0 if kind == "timeout" else 5)
                kill.assert_any_call(process.pid, signal.SIGKILL)
                self.assertEqual(process.wait_timeouts, [5])

    def test_success_stops_leftover_descendants_and_capture_is_bounded(self) -> None:
        process = FakeProcess()
        with patch.object(subprocess, "Popen", return_value=process), patch.object(guard, "child_status", return_value=0), patch.object(os, "killpg") as kill, patch.object(time, "sleep"):
            _ = guard.run_owned(["authored-fixture"], self.info, self.env, pass_fds=(), monitor_space=False)
        kill.assert_any_call(process.pid, signal.SIGKILL)
        with self.assertRaisesRegex(guard.GuardError, "version output exceeds"):
            _ = guard.run_owned([sys.executable, "-c", "print('x'*65537)"], self.info, os.environ,
                                pass_fds=(), capture=True, monitor_space=False)

    def test_child_status_keeps_leader_waitable(self) -> None:
        status = os.waitid_result((123456, os.getuid(), signal.SIGCHLD, 0, os.CLD_EXITED))
        with patch.object(os, "waitid", return_value=status) as wait:
            self.assertEqual(guard.child_status(123456), 0)
        wait.assert_called_once_with(os.P_PID, 123456, os.WEXITED | os.WNOHANG | os.WNOWAIT)


class FakeProcess:
    pid: int = 123456

    def __init__(self) -> None:
        self.wait_timeouts: list[float | None] = []

    def wait(self, timeout: float | None = None) -> int:
        self.wait_timeouts.append(timeout)
        return -15


class DiskUsage(NamedTuple):
    total: int
    used: int
    free: int


def disk_usage(free: int) -> DiskUsage:
    return DiskUsage(100 * guard.GIB, 100 * guard.GIB - free, free)


class GroupMemoryTests(unittest.TestCase):
    def test_process_exit_during_stat_or_statm_keeps_other_group_members(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            proc = Path(directory)
            for pid in (123, 124):
                entry = proc / str(pid)
                entry.mkdir()
                _ = (entry / "stat").write_text(f"{pid} (worker) S 1 77")
                _ = (entry / "statm").write_text("10 3")
            original = Path.read_text
            for filename in ("stat", "statm"):
                vanished = proc / "123" / filename

                def read(path: Path) -> str:
                    if path == vanished:
                        raise ProcessLookupError(3, "No such process")
                    return original(path)

                with self.subTest(filename=filename), patch.object(guard, "PROC", proc), patch.object(Path, "read_text", new=read):
                    self.assertEqual(guard.group_memory(77), 3 * os.sysconf("SC_PAGE_SIZE"))

            with patch.object(guard, "PROC", proc), patch.object(Path, "read_text", side_effect=OSError(5, "I/O error")):
                with self.assertRaises(OSError):
                    _ = guard.group_memory(77)


if __name__ == "__main__":
    _ = unittest.main()
