"""Offline launcher authorization/lifetime regressions; never invokes Docker."""

from collections.abc import Sequence
from contextlib import nullcontext
from dataclasses import dataclass
import datetime
import json
import os
from pathlib import Path
import signal
import socket
import stat
import struct
import subprocess
import tempfile
import time
import unittest
from unittest.mock import Mock, patch
from typing import final

from tools import reference_intake
from tools.run_reference_offline import (IMAGE, REFERENCE_EXE, ApprovedRun, Arguments, Budget,
    cleanup_container, digest, observe, observe_and_record, owned_container, prepare_launch,
    record_save_outputs, reserve_run, revalidate, validate_session)
from tools.runtime_types import JsonObject, json_object, parse_json

CONTAINER = "a" * 64
NAME = "icarus-offline-authored"
TOKEN = "authored-owner"
ROOT = Path(__file__).resolve().parents[2]


def inspection(*, name: str = NAME, token: str = TOKEN, container: str = CONTAINER,
               running: bool = False) -> str:
    return json.dumps([{"Id": container, "Name": "/" + name,
        "Config": {"Labels": {"icarus.research.run": NAME, "icarus.research.owner": token}},
        "State": {"Running": running, "ExitCode": 0}}])


class Fixture:
    def __init__(self, directory: str) -> None:
        self.root: Path = Path(directory).resolve()
        self.case: Path = self.root / "work/current-case"
        self.case.mkdir(parents=True)
        self.scope: Path = self.case / "scope.md"
        _ = self.scope.write_text("## auth\n- status: granted\n## network_profile\n- mode: offline\n"
                                 + "## signoff\n- ready_for_act: true\n")
        self.reference: Path = self.root / "icarus-orig" / REFERENCE_EXE
        self.reference.parent.mkdir(parents=True)
        _ = self.reference.write_bytes(b"authored non-executable reference fixture")
        self.probe: Path = self.case / "probe"
        _ = self.probe.write_bytes(b"authored probe fixture")
        self.probe.chmod(0o700)
        self.now: datetime.datetime = datetime.datetime.now(datetime.UTC)
        self.session: JsonObject = {
            "scope": "reference-offline", "authorization_source": "authored test authorization",
            "case_root": str(self.case), "scope_sha256": digest(self.scope), "image": IMAGE,
            "reference_root": str(self.root / "icarus-orig"), "reference_executable": str(self.reference),
            "reference_sha256": digest(self.reference), "max_runs": 2,
            "observation_seconds": 120, "run_timeout_seconds": 240,
            "start_utc": (self.now - datetime.timedelta(seconds=1)).isoformat(),
            "end_utc": (self.now + datetime.timedelta(hours=1)).isoformat(),
            "allowed_modes": ["startup"],
        }
        self.path: Path = self.case / "session.json"
        self.args: Arguments = Arguments()
        self.args.case_root = str(self.case)
        self.args.session = str(self.path)
        self.args.run_name = "authored"
        self.write()

    def write(self, update: JsonObject | None = None) -> None:
        _ = self.path.write_text(json.dumps({**self.session, **(update or {})}))

    def approve(self) -> ApprovedRun:
        return validate_session(self.root, self.args, self.now)


@dataclass(frozen=True)
class DiskSpace:
    free: int = 40 * 1024**3


def disk_space(_path: Path) -> DiskSpace:
    return DiskSpace()


class Clock:
    def __init__(self) -> None:
        self.now: float = 0

    def __call__(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += seconds


class DockerFixture:
    def __init__(self, *, failure: str = "", payload: str | None = None,
                 remaining: bool = False, handshake: bool = False, interrupt: bool = False) -> None:
        self.failure: str = failure
        self.payload: str | None = payload
        self.remaining: bool = remaining
        self.handshake: bool = handshake
        self.interrupt: bool = interrupt
        self.probed: bool = False
        self.calls: list[list[str]] = []
        self.timeouts: list[float] = []

    def __call__(self, args: Sequence[str], timeout: float = 30,
                 check: bool = True) -> subprocess.CompletedProcess[str]:
        argv = list(args)
        self.calls.append(argv)
        self.timeouts.append(timeout)
        action = argv[1]
        if action == self.failure:
            if check:
                raise subprocess.CalledProcessError(1, argv, stderr="authored failure")
            return subprocess.CompletedProcess(argv, 1, "", "authored failure")
        if action == "run":
            return subprocess.CompletedProcess(argv, 0, CONTAINER, "")
        if action == "inspect":
            payload = self.payload if self.payload is not None else inspection(running=self.handshake and not self.probed)
            return subprocess.CompletedProcess(argv, 0, payload, "")
        if action == "exec":
            self.probed = True
        if action == "logs" and self.interrupt:
            signal.raise_signal(signal.SIGTERM)
            signal.raise_signal(signal.SIGINT)
        if action == "ps":
            return subprocess.CompletedProcess(argv, 0, CONTAINER if self.remaining else "", "")
        return subprocess.CompletedProcess(argv, 0, '{"authored":true}\n', "")


class SessionTests(unittest.TestCase):
    def test_scope_contract_rejects_unsigned_online_and_misplaced_fields(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            valid = fixture.scope.read_text()
            invalid_scopes = [
                valid.replace("status: granted", "status: pending"),
                valid.replace("ready_for_act: true", "ready_for_act: false"),
                valid.replace("mode: offline", "mode: authorized_target_only"),
                valid.replace("## auth", "## notes"),
                valid.replace("- status: granted", "- status: granted\n- status: pending"),
                "Authored current offline scope.",
            ]
            for scope in invalid_scopes:
                with self.subTest(scope=scope):
                    _ = fixture.scope.write_text(scope)
                    fixture.session["scope_sha256"] = digest(fixture.scope)
                    fixture.write()
                    with self.assertRaisesRegex(ValueError, "scope"):
                        _ = fixture.approve()

    def test_current_session_rejects_invalid_authorization_and_bindings(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            self.assertEqual(fixture.approve().lifetime_seconds, 240)
            updates: list[JsonObject] = [
                {"scope": "native-io-faults"}, {"authorization_source": " "},
                {"case_root": str(fixture.root)}, {"image": "different-image"},
                {"reference_root": str(fixture.case)}, {"reference_executable": str(fixture.probe)},
                {"reference_root": str(fixture.root / "icarus-server-x")},
                {"reference_sha256": "0" * 64}, {"scope_sha256": "0" * 64},
                {"start_utc": (fixture.now + datetime.timedelta(seconds=1)).isoformat()},
                {"end_utc": fixture.now.isoformat()}, {"start_utc": "2026-10-12T00:00:00"},
                {"end_utc": (fixture.now + datetime.timedelta(seconds=240)).isoformat()},
                {"observation_seconds": 121}, {"observation_seconds": True},
                {"run_timeout_seconds": 239}, {"run_timeout_seconds": 901},
                {"max_runs": 0}, {"max_runs": 13}, {"max_runs": True},
                {"allowed_modes": ["resume"]}, {"allowed_modes": ["startup", "unknown"]},
            ]
            for update in updates:
                with self.subTest(update=update):
                    fixture.write(update)
                    with self.assertRaises(ValueError):
                        _ = fixture.approve()
            fixture.write()
            approved = fixture.approve()
            _ = fixture.path.write_text(fixture.path.read_text() + "\n")
            with self.assertRaisesRegex(RuntimeError, "changed"):
                revalidate(fixture.root, fixture.args, approved)
            fixture.write()
            _ = fixture.reference.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "reference_sha256"):
                revalidate(fixture.root, fixture.args, approved)

    def test_a_fetched_server_install_may_be_the_reference(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fetched = fixture.root / "icarus-server-25793607" / REFERENCE_EXE
            fetched.parent.mkdir(parents=True)
            _ = fetched.write_bytes(b"authored fetched fixture")
            fixture.write({"reference_root": str(fixture.root / "icarus-server-25793607"),
                           "reference_executable": str(fetched), "reference_sha256": digest(fetched)})
            self.assertEqual(fixture.approve().reference, fixture.root / "icarus-server-25793607")

    def test_probe_modes_redirects_and_scope_mutation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fixture.args.handshake_probe = True
            fixture.session.update(allowed_modes=["startup", "handshake"],
                                   probe=str(fixture.probe), probe_sha256=digest(fixture.probe))
            fixture.write()
            approved = fixture.approve()
            fixture.args.complete_stateless = True
            with self.assertRaisesRegex(ValueError, "not authorized"):
                _ = fixture.approve()
            fixture.args.complete_stateless = False
            fixture.write({"probe_sha256": "0" * 64})
            with self.assertRaisesRegex(ValueError, "probe_sha256"):
                _ = fixture.approve()
            alias = fixture.case / "alias"
            alias.symlink_to(fixture.probe)
            fixture.write({"probe": str(alias)})
            with self.assertRaisesRegex(ValueError, "redirected"):
                _ = fixture.approve()
            fixture.write()
            _ = fixture.scope.write_text("changed scope")
            with self.assertRaisesRegex(ValueError, "scope_sha256"):
                revalidate(fixture.root, fixture.args, approved)

    def test_quota_is_locked_and_failed_attempts_count(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            approved = fixture.approve()
            with reserve_run(approved, "first") as run:
                self.assertTrue(run.is_dir())
                with self.assertRaises(BlockingIOError):
                    with reserve_run(approved, "racing"):
                        self.fail("concurrent reservation acquired lock")
            with self.assertRaisesRegex(RuntimeError, "authored failure"):
                with reserve_run(approved, "failed"):
                    raise RuntimeError("authored failure")
            with self.assertRaisesRegex(RuntimeError, "count exhausted"):
                with reserve_run(approved, "third"):
                    self.fail("quota exceeded")
            self.assertEqual(sorted(path.name for path in (fixture.case / "runtime").iterdir()),
                             ["run-failed", "run-first"])

    def test_expired_budget_never_invokes_command_and_clips_timeout(self) -> None:
        docker = DockerFixture()
        with patch("tools.run_reference_offline.time.monotonic", return_value=10), \
             patch("tools.run_reference_offline.command", new=docker):
            with self.assertRaises(TimeoutError):
                _ = Budget(10).run(["docker", "inspect", NAME])
            self.assertEqual(docker.calls, [])
            _ = Budget(12).run(["docker", "inspect", NAME], cap=30)
        self.assertEqual(docker.timeouts, [2])


@final
class IntakeFixture(Fixture):
    def __init__(self, directory: str) -> None:
        super().__init__(directory)
        intake_case = self.root / "work/issue284-release-20261113"
        self.manifest: Path = intake_case / "intake/official-fixity.sha256"
        self.manifest.parent.mkdir(parents=True)
        self.install: Path = intake_case / "official"
        self.reference = self.install / REFERENCE_EXE
        self.reference.parent.mkdir(parents=True)
        _ = self.reference.write_bytes(b"authored server fixture")
        self.data: Path = self.install / "authored data.test"
        _ = self.data.write_bytes(b"authored package fixture")
        self.downloading: Path = self.install / "steamapps/downloading"
        self.downloading.mkdir(parents=True)
        self.steam_temp: Path = self.install / "steamapps/temp"
        self.steam_temp.mkdir()
        app_manifest = self.install / "steamapps/appmanifest_2089300.acf"
        _ = app_manifest.write_bytes(b"authored Steam app manifest fixture")
        self.manifest_body: str = "".join(f"{digest(path)}  {path.relative_to(self.root)}\n"
                                         for path in (self.reference, self.data, app_manifest))
        self.scope_base: str = self.scope.read_text()
        self.args.reference_input = reference_intake.EVIDENCE
        self.session.update(reference_root=str(self.install), reference_executable=str(self.reference),
                            reference_sha256=digest(self.reference))
        self.binding: JsonObject = {
            "evidence": reference_intake.EVIDENCE, "build_id": reference_intake.BUILD_ID,
            "release": reference_intake.RELEASE, "manifest": str(self.manifest),
            "authorization_source": self.session["authorization_source"],
        }
        self.write_manifest(self.manifest_body)

    def write_manifest(self, body: str) -> None:
        _ = self.manifest.write_text(body)
        self.binding["manifest_sha256"] = digest(self.manifest)
        self.write_binding()

    def write_binding(self) -> None:
        fields = {**self.binding, "status": "granted",
                  **{key: self.session[key] for key in ("reference_root", "reference_executable", "reference_sha256")}}
        _ = self.scope.write_text(self.scope_base + "## reference_input\n"
                                  + "".join(f"- {name}: {value}\n" for name, value in fields.items()))
        self.session.update(reference_input=self.binding, scope_sha256=digest(self.scope))
        self.write()

    def enable_handshake(self) -> None:
        self.args.handshake_probe = True
        self.session.update(allowed_modes=["startup", "handshake"], probe=str(self.probe), probe_sha256=digest(self.probe))
        self.write_binding()

    def get_intake(self) -> reference_intake.Intake:
        with patch.object(reference_intake, "MANIFEST_SHA256", self.binding["manifest_sha256"]):
            intake = self.approve().intake
        if intake is None:
            raise AssertionError("expected intake approval")
        return intake


class IntakeTests(unittest.TestCase):
    def test_intake_requires_opt_in_and_current_scope_and_session(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = IntakeFixture(directory)
            checksum = fixture.binding["manifest_sha256"]
            with patch.object(reference_intake, "MANIFEST_SHA256", checksum):
                self.assertEqual(fixture.approve().reference, fixture.install)
                for selector in (None, str(fixture.install), "M-0000"):
                    fixture.args.reference_input = selector
                    with self.assertRaisesRegex(ValueError, "reference input|reference intake"):
                        _ = fixture.approve()
                fixture.args.reference_input = reference_intake.EVIDENCE
                replacements: list[JsonObject] = [
                    {"manifest": str(fixture.case / fixture.manifest.name)}, {"manifest_sha256": "0" * 64},
                    {"release": "other"}, {"build_id": "26613688"}, {"build_id": 26613689},
                    {"authorization_source": " "}, {"authorization_source": "other scope source"},
                ]
                for replacement in replacements:
                    with self.subTest(replacement=replacement):
                        fixture.write({"reference_input": {**fixture.binding, **replacement}})
                        with self.assertRaisesRegex(ValueError, "intake|build_id|authorization_source"):
                            _ = fixture.approve()
                incomplete_binding = dict(fixture.binding)
                del incomplete_binding["authorization_source"]
                fixture.write({"reference_input": incomplete_binding})
                with self.assertRaisesRegex(ValueError, "authorization_source"):
                    _ = fixture.approve()
                fixture.write()
                valid = fixture.scope.read_text()
                for body in (fixture.scope_base, valid.replace("granted", "pending"),
                             valid + "- manifest_sha256: alternate\n"):
                    _ = fixture.scope.write_text(body)
                    fixture.write({"scope_sha256": digest(fixture.scope)})
                    with self.assertRaisesRegex(ValueError, "scope"):
                        _ = fixture.approve()
                fixture.write_binding()
                _ = fixture.path.write_text(fixture.path.read_text().replace('"evidence":', '"evidence": "other", "evidence":'))
                with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
                    _ = fixture.approve()
                for field, value in (("reference_root", str(fixture.case)),
                                     ("reference_root", str(fixture.install) + "/"),
                                     ("reference_executable", str(fixture.data)), ("reference_sha256", "0" * 64)):
                    previous = fixture.session[field]
                    fixture.session[field] = value
                    fixture.write_binding()
                    with self.assertRaisesRegex(ValueError, "executable/root"):
                        _ = fixture.approve()
                    fixture.session[field] = previous
                fixture.write_binding()
                for case in (fixture.install.parent, fixture.manifest.parent, fixture.install, fixture.downloading):
                    with self.subTest(case=case):
                        fixture.args.case_root = str(case)
                        with self.assertRaisesRegex(ValueError, "runtime case must be separate"):
                            _ = fixture.approve()

    def test_manifest_requires_pinned_repository_relative_install_entries(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = IntakeFixture(directory)
            self.assertEqual(fixture.get_intake().root, fixture.install)
            with patch.object(reference_intake, "MANIFEST_SHA256", fixture.binding["manifest_sha256"]):
                _ = fixture.manifest.write_text(fixture.manifest_body + "\n")
                with self.assertRaisesRegex(ValueError, "digest mismatch"):
                    _ = fixture.approve()
                other = fixture.manifest.rename(fixture.manifest.with_suffix(".other"))
                fixture.manifest.symlink_to(other)
                with self.assertRaisesRegex(ValueError, "redirected intake manifest"):
                    _ = fixture.approve()
                fixture.manifest.unlink()
            checksum = "a" * 64
            install = fixture.install.relative_to(fixture.root)
            for line in (fixture.manifest_body.splitlines()[0], f"{checksum}  {install}",
                         f"{checksum}  outside/{REFERENCE_EXE}", f"{checksum}  official/{REFERENCE_EXE}",
                         f"{checksum}  {install.parent}/intake/official/{REFERENCE_EXE}",
                         f"{checksum}  ./{install}/{REFERENCE_EXE}", f"{checksum}  {install}/../other",
                         f"{checksum}  {install}//data.test", f"1 {install}/data.test"):
                with self.subTest(line=line):
                    fixture.write_manifest(fixture.manifest_body + line + "\n")
                    with self.assertRaises(ValueError):
                        _ = fixture.get_intake()
            fixture.write_manifest("\n".join(fixture.manifest_body.splitlines()[1:]) + "\n")
            with self.assertRaisesRegex(ValueError, "does not name the reference executable"):
                _ = fixture.get_intake()
            fixture.write_manifest(fixture.manifest_body)
            moved = fixture.install.rename(fixture.install.with_name("alternate"))
            fixture.install.symlink_to(moved, target_is_directory=True)
            with self.assertRaisesRegex(ValueError, "redirected"):
                _ = fixture.get_intake()

    def test_tree_rejects_stale_missing_extra_and_redirected_entries(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = IntakeFixture(directory)
            intake = fixture.get_intake()
            with_steam_dirs = reference_intake.verify_install(intake, time.monotonic() + 10)
            fixture.downloading.rmdir()
            fixture.steam_temp.rmdir()
            without_steam_dirs = reference_intake.verify_install(intake, time.monotonic() + 10)
            self.assertGreater(with_steam_dirs.length, without_steam_dirs.length)
        for failure, expected in [("stale", "digest mismatch"), ("missing", "missing"),
                                  ("extra", "unlisted"), ("empty-directory", "unlisted"),
                                  ("steam-file", "empty Steam directory"), ("steam-dir", "empty Steam directory"),
                                  ("symlink", "symbolic link"), ("directory-symlink", "redirected"),
                                  ("hardlink", "one link"), ("fifo", "regular file"), ("deadline", "window")]:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                fixture = IntakeFixture(directory)
                intake = fixture.get_intake()
                if failure == "stale":
                    _ = fixture.data.write_bytes(b"X" * fixture.data.stat().st_size)
                elif failure == "missing":
                    fixture.data.unlink()
                elif failure == "extra":
                    _ = (fixture.install / "extra").touch()
                elif failure == "empty-directory":
                    (fixture.install / "extra").mkdir()
                elif failure == "steam-file":
                    (fixture.downloading / "unlisted").touch()
                elif failure == "steam-dir":
                    (fixture.steam_temp / "unlisted").mkdir()
                elif failure == "symlink":
                    fixture.data.unlink()
                    fixture.data.symlink_to(fixture.probe)
                elif failure == "directory-symlink":
                    moved = (fixture.install / "Icarus").rename(fixture.case / "Icarus")
                    (fixture.install / "Icarus").symlink_to(moved, target_is_directory=True)
                elif failure == "hardlink":
                    (fixture.case / "other").hardlink_to(fixture.data)
                elif failure == "fifo":
                    fixture.data.unlink()
                    os.mkfifo(fixture.data)
                deadline = time.monotonic() + (0 if failure == "deadline" else 10)
                with self.assertRaisesRegex((OSError, ValueError), expected):
                    _ = reference_intake.verify_install(intake, deadline)

    def test_actual_mount_guard_fails_before_launch_for_changed_bytes_or_mounts(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = IntakeFixture(directory)
            fixture.enable_handshake()
            with patch.object(reference_intake, "MANIFEST_SHA256", fixture.binding["manifest_sha256"]):
                approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            tree = reference_intake.verify_install(fixture.get_intake(), time.monotonic() + 10)
            argv = prepare_launch(ROOT, fixture.args, approved, run, NAME, TOKEN, tree)
            self.assertIn(f"type=bind,src={fixture.install},dst=/reference,readonly,bind-recursive=disabled", argv)
            self.assertEqual(argv[argv.index("--network") + 1], "none")
            self.assertNotIn("--entrypoint", argv)
            self.assertEqual(argv[-3:-1], ["/bin/bash", "-c"])
            self.assertIn("--no-healthcheck", argv)
            home = run / "home"
            self.assertEqual(stat.S_IMODE(home.stat().st_mode), 0o700)
            self.assertIn(f"type=bind,src={home},dst=/home/container,bind-recursive=disabled", argv)
            mountinfo = fixture.case / "mountinfo"
            # Exercise the real guard with authored mountinfo and a shell builtin in place of Proton.
            self.assertIn(f"type=bind,src={fixture.probe},dst=/probes/handshake_probe,readonly", argv)
            guard = reference_intake.guard_launch(tree, home, fixture.probe, TOKEN)
            self.assertTrue(argv[-1].startswith(guard + "exec /bin/bash -lc "))
            script = (guard.replace("/proc/self/mountinfo", str(mountinfo))
                      .replace("/home/container", str(home)).replace("/reference", str(fixture.install))
                      .replace("/probes/handshake_probe", str(fixture.probe)) + "printf reached")
            mounts = f"1 0 0:0 / {fixture.install} ro - test test ro\n2 0 0:0 / {home} rw - test test rw\n"
            probe_mount = f"3 0 0:0 / {fixture.probe} ro - test test ro\n"
            mounts += probe_mount
            receipt = home / reference_intake.MOUNT_RECEIPT
            nested_mount = f"3 2 0:0 / {home}/unexpected rw - test test rw\n"
            for bad_mounts in (mounts.replace(" ro ", " rw "), mounts + mounts.splitlines()[0] + "\n",
                               mounts + nested_mount, mounts.replace(probe_mount, probe_mount.replace(" ro ", " rw "))):
                _ = mountinfo.write_text(bad_mounts)
                result = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=10, check=False)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")
                self.assertRegex(result.stderr, r"reference guard failed at line [0-9]+")
                self.assertFalse(receipt.exists())
            _ = mountinfo.write_text(mounts)
            result = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=10, check=True)
            self.assertEqual(result.stdout, "reached")
            self.assertTrue(reference_intake.mount_verified(receipt, TOKEN))
            receipt.unlink()
            original = fixture.probe.rename(fixture.case / "original-probe")
            _ = fixture.probe.write_bytes(original.read_bytes())
            result = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=10, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(receipt.exists())
            _ = original.replace(fixture.probe)
            previous = fixture.data.stat()
            _ = fixture.data.write_bytes(b"X" * previous.st_size)
            os.utime(fixture.data, ns=(previous.st_atime_ns, previous.st_mtime_ns))
            result = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=10, check=False)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stdout, "")
            self.assertFalse(receipt.exists())

    def test_unverified_daemon_fails_before_any_docker_command(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = IntakeFixture(directory)
            with patch.object(reference_intake, "MANIFEST_SHA256", fixture.binding["manifest_sha256"]):
                approved = fixture.approve()
            for key in ("DOCKER_CONTEXT", "DOCKER_HOST"):
                docker = DockerFixture()
                with (patch.dict(os.environ, {key: "unverified"}),
                      patch("tools.run_reference_offline.command", new=docker),
                      self.assertRaisesRegex(ValueError, "host/context")):
                    _ = observe(fixture.args, approved, fixture.case, NAME, TOKEN, ["docker", "run"])
                self.assertEqual(docker.calls, [])

    def test_resource_floor_rechecked_before_starting_verified_intake(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = IntakeFixture(directory)
            with patch.object(reference_intake, "MANIFEST_SHA256", fixture.binding["manifest_sha256"]):
                approved = fixture.approve()
            docker = DockerFixture()
            with (patch.object(reference_intake, "verify_local_dockerd"),
                  patch("tools.run_reference_offline.available_memory", return_value=0),
                  patch("tools.run_reference_offline.command", new=docker),
                  self.assertRaisesRegex(RuntimeError, "resource floor")):
                _ = observe(fixture.args, approved, fixture.case, NAME, TOKEN, ["docker", "run"])
            self.assertEqual(docker.calls, [])

    def test_local_daemon_requires_kernel_sender_credentials(self) -> None:
        info = os.stat_result((stat.S_IFSOCK | 0o660, 2, 3, 1, 0, 0, 0, 0, 0, 0))
        reply = b"HTTP/1.1 200 OK\r\n"
        recvmsg = Mock()
        connect = Mock()
        setsockopt = Mock()
        peer = Mock(connect=connect, recvmsg=recvmsg, setsockopt=setsockopt)
        with (patch.dict(os.environ, {}, clear=True),
              patch.object(Path, "resolve", return_value=reference_intake.DOCKER_SOCKET),
              patch.object(Path, "stat", return_value=info),
              patch.object(Path, "read_text", return_value="dockerd\n") as process_name,
              patch.object(socket, "socket", return_value=nullcontext(peer))):
            credentials = (socket.SOL_SOCKET, socket.SCM_CREDENTIALS, struct.pack("iII", 123, 0, 0))
            recvmsg.return_value = (reply, [credentials], 0, None)
            reference_intake.verify_local_dockerd()
            connect.assert_called_once_with(str(reference_intake.DOCKER_SOCKET))
            setsockopt.assert_called_once_with(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
            for payload, ancillary, flags in [(reply, [], 0), (reply, [credentials], socket.MSG_CTRUNC),
                                              (b"HTTP/1.1 404 Not Found\r\n", [credentials], 0)]:
                recvmsg.return_value = (payload, ancillary, flags, None)
                with self.assertRaisesRegex(ValueError, "authenticate"):
                    reference_intake.verify_local_dockerd()
            recvmsg.return_value = (reply, [credentials], 0, None)
            process_name.return_value = "forwarder\n"
            with self.assertRaisesRegex(ValueError, "local root dockerd"):
                reference_intake.verify_local_dockerd()

    def test_bound_docker_commands_pin_the_socket_and_require_the_guard_receipt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = IntakeFixture(directory)
            fixture.enable_handshake()
            with patch.object(reference_intake, "MANIFEST_SHA256", fixture.binding["manifest_sha256"]):
                approved = fixture.approve()
            for receipt_at in (None, 0, approved.probe_delay_seconds + 5):
                run = fixture.case / f"run-{receipt_at}"
                home = run / "home"
                home.mkdir(parents=True)
                receipt = home / reference_intake.MOUNT_RECEIPT
                if receipt_at == 0:
                    _ = receipt.write_text(TOKEN + "\n")
                docker = DockerFixture(handshake=True)
                clock = Clock()

                def command_on_socket(args: Sequence[str], timeout: float = 30,
                                      check: bool = True) -> subprocess.CompletedProcess[str]:
                    self.assertEqual(list(args[:3]), ["docker", "--host", f"unix://{reference_intake.DOCKER_SOCKET}"])
                    if args[3] == "inspect" and receipt_at is not None and clock.now >= receipt_at and not receipt.exists():
                        _ = receipt.write_text(TOKEN + "\n")
                    if args[3] == "exec":
                        self.assertTrue(receipt.exists(), "probe must wait for the mount receipt")
                    return docker([args[0], *args[3:]], timeout, check)

                with (patch.object(reference_intake, "verify_local_dockerd"),
                      patch("tools.run_reference_offline.command", new=command_on_socket),
                      patch("tools.run_reference_offline.time.monotonic", new=clock),
                      patch("tools.run_reference_offline.time.sleep", new=clock.sleep),
                      patch("tools.run_reference_offline.available_memory", return_value=4 * 1024**3),
                      patch("tools.run_reference_offline.shutil.disk_usage", new=disk_space)):
                    if receipt_at is not None:
                        self.assertIs(observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])["removed"], True)
                    else:
                        with self.assertRaisesRegex(RuntimeError, "did not verify"):
                            _ = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
                probe_calls = [call for call in docker.calls if call[1] == "exec"]
                self.assertEqual(len(probe_calls), 0 if receipt_at is None else 1)
                self.assertIn(["docker", "rm", "--force", CONTAINER], docker.calls)


class CleanupTests(unittest.TestCase):
    def test_exact_identity_and_single_result_required(self) -> None:
        self.assertEqual(owned_container(inspection(), NAME, TOKEN, CONTAINER), CONTAINER)
        for payload in [inspection(name="foreign"), inspection(token="foreign"),
                        inspection(container="b" * 64), inspection(container="short"),
                        "[]", "[{},{}]"]:
            with self.subTest(payload=payload), self.assertRaises(RuntimeError):
                _ = owned_container(payload, NAME, TOKEN, CONTAINER)

    def test_cleanup_handles_partial_creation_and_repeated_interrupts(self) -> None:
        previous_term = signal.getsignal(signal.SIGTERM)
        previous_int = signal.getsignal(signal.SIGINT)
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            docker = DockerFixture(interrupt=True)
            with patch("tools.run_reference_offline.command", new=docker), \
                 patch("tools.run_reference_offline.time.monotonic", return_value=0):
                outcome = cleanup_container(run, NAME, TOKEN, None, Budget(120))
            self.assertIs(outcome["removed"], True)
            self.assertIn(["docker", "rm", "--force", CONTAINER], docker.calls)
            self.assertTrue(all(0 < timeout <= 20 for timeout in docker.timeouts))
            self.assertIn("--no-trunc", docker.calls[-1])
        self.assertEqual(signal.getsignal(signal.SIGTERM), previous_term)
        self.assertEqual(signal.getsignal(signal.SIGINT), previous_int)

    def test_failures_always_record_receipt_and_never_return_success(self) -> None:
        for failure, remaining in [("inspect", False), ("logs", False), ("stop", False),
                                   ("rm", False), ("ps", False), ("", True)]:
            with self.subTest(failure=failure, remaining=remaining), tempfile.TemporaryDirectory() as directory:
                run = Path(directory)
                docker = DockerFixture(failure=failure, remaining=remaining)
                with patch("tools.run_reference_offline.command", new=docker), \
                     patch("tools.run_reference_offline.time.monotonic", return_value=0):
                    with self.assertRaises((RuntimeError, subprocess.CalledProcessError)):
                        _ = cleanup_container(run, NAME, TOKEN, CONTAINER, Budget(120))
                receipt = json_object(parse_json((run / "cleanup.json").read_text()))
                self.assertIn("error", receipt)
                if failure in {"logs", "stop"}:
                    self.assertIs(receipt["removed"], True)
                    self.assertIn(["docker", "rm", "--force", CONTAINER], docker.calls)
                else:
                    self.assertIs(receipt["removed"], False)

    def test_foreign_container_and_exhausted_cleanup_never_act(self) -> None:
        for deadline, payload in [(120, inspection(token="foreign")), (0, inspection())]:
            with self.subTest(deadline=deadline), tempfile.TemporaryDirectory() as directory:
                run = Path(directory)
                docker = DockerFixture(payload=payload)
                with patch("tools.run_reference_offline.command", new=docker), \
                     patch("tools.run_reference_offline.time.monotonic", return_value=0):
                    with self.assertRaises((RuntimeError, TimeoutError)):
                        _ = cleanup_container(run, NAME, TOKEN, None, Budget(deadline))
                self.assertFalse(any(call[1] in {"logs", "stop", "rm"} for call in docker.calls))
                self.assertIs(json_object(parse_json((run / "cleanup.json").read_text()))["removed"], False)


class InvocationTests(unittest.TestCase):
    def test_launch_rechecks_time_after_hashing_and_preparation(self) -> None:
        for delay_in_hash in [False, True]:
            with self.subTest(delay_in_hash=delay_in_hash), tempfile.TemporaryDirectory() as directory:
                fixture = Fixture(directory)
                fixture.write({"end_utc": (fixture.now + datetime.timedelta(seconds=241)).isoformat()})
                approved = fixture.approve()
                clock = Clock()
                docker = DockerFixture()
                run = fixture.case / "run"
                run.mkdir()
                previous_term = signal.getsignal(signal.SIGTERM)
                previous_int = signal.getsignal(signal.SIGINT)

                def slow_digest(path: Path) -> str:
                    clock.now += 1
                    return digest(path)

                def current_time(_tz: datetime.tzinfo | None = None) -> datetime.datetime:
                    return fixture.now + datetime.timedelta(seconds=clock.now)

                wall = Mock(wraps=datetime.datetime, now=Mock(side_effect=current_time))
                with patch("tools.run_reference_offline.datetime.datetime", new=wall), \
                     patch("tools.run_reference_offline.time.monotonic", new=clock), \
                     patch("tools.run_reference_offline.command", new=docker):
                    if delay_in_hash:
                        with patch("tools.run_reference_offline.digest", new=slow_digest):
                            revalidate(fixture.root, fixture.args, approved)
                    else:
                        revalidate(fixture.root, fixture.args, approved)
                        clock.now = 2  # Scheduling/preparation delay after revalidation.
                    with self.assertRaisesRegex(ValueError, "at launch"):
                        _ = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
                self.assertEqual(docker.calls, [])
                self.assertFalse((run / "cleanup.json").exists())
                self.assertEqual(signal.getsignal(signal.SIGTERM), previous_term)
                self.assertEqual(signal.getsignal(signal.SIGINT), previous_int)

    def test_launch_clamps_lifetime_to_fresh_session_expiry(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fixture.write({"run_timeout_seconds": 300,
                           "end_utc": (fixture.now + datetime.timedelta(seconds=301)).isoformat()})
            approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            clock = Clock()
            clock.now = 50  # 251 seconds remain: observation plus cleanup still fit.
            docker = DockerFixture()
            deadlines: list[float] = []

            def checked_cleanup(run: Path, name: str, token: str, expected_id: str | None,
                                budget: Budget) -> JsonObject:
                deadlines.append(budget.deadline)
                return cleanup_container(run, name, token, expected_id, budget)

            wall = Mock(wraps=datetime.datetime,
                        now=Mock(return_value=fixture.now + datetime.timedelta(seconds=50)))
            with patch("tools.run_reference_offline.datetime.datetime", new=wall), \
                 patch("tools.run_reference_offline.time.monotonic", new=clock), \
                 patch("tools.run_reference_offline.command", new=docker), \
                 patch("tools.run_reference_offline.cleanup_container", new=checked_cleanup), \
                 patch("tools.run_reference_offline.available_memory", return_value=4 * 1024**3), \
                 patch("tools.run_reference_offline.shutil.disk_usage", new=disk_space):
                result = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
            self.assertEqual(deadlines, [301])
            self.assertIs(result["removed"], True)

    def test_observation_cannot_spend_reserved_cleanup_time(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            docker = DockerFixture(handshake=True)
            clock = Clock()

            def stalled_inspect(args: Sequence[str], timeout: float = 30,
                                check: bool = True) -> subprocess.CompletedProcess[str]:
                result = docker(args, timeout, check)
                if args[1] == "inspect" and clock.now == 0:
                    clock.now = 120  # Observation budget exhausted after a valid reply.
                return result

            with patch("tools.run_reference_offline.command", new=stalled_inspect), \
                 patch("tools.run_reference_offline.time.monotonic", new=clock), \
                 patch("tools.run_reference_offline.available_memory", return_value=4 * 1024**3), \
                 patch("tools.run_reference_offline.shutil.disk_usage", new=disk_space):
                with self.assertRaises(TimeoutError):
                    _ = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
            self.assertFalse(any(call[1] == "stats" for call in docker.calls))
            self.assertIn(["docker", "rm", "--force", CONTAINER], docker.calls)
            self.assertIs(json_object(parse_json((run / "cleanup.json").read_text()))["removed"], True)

    def test_probe_uses_verified_id_and_failed_launch_cleans_partial_creation(self) -> None:
        for failure in ["", "run"]:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                fixture = Fixture(directory)
                fixture.args.handshake_probe = True
                fixture.session.update(allowed_modes=["startup", "handshake"],
                                       probe=str(fixture.probe), probe_sha256=digest(fixture.probe))
                fixture.write()
                approved = fixture.approve()
                run = fixture.case / "run"
                run.mkdir()
                docker = DockerFixture(handshake=True, failure=failure)
                clock = Clock()
                with patch("tools.run_reference_offline.command", new=docker), \
                     patch("tools.run_reference_offline.time.monotonic", new=clock), \
                     patch("tools.run_reference_offline.time.sleep", new=clock.sleep), \
                     patch("tools.run_reference_offline.available_memory", return_value=4 * 1024**3), \
                     patch("tools.run_reference_offline.shutil.disk_usage", new=disk_space):
                    if failure:
                        with self.assertRaises(subprocess.CalledProcessError):
                            _ = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
                    else:
                        result = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
                        self.assertIs(result["removed"], True)
                        probe = next(call for call in docker.calls if call[1] == "exec")
                        self.assertEqual(probe[4], CONTAINER)
                        self.assertNotIn(NAME, probe)
                self.assertIn(["docker", "rm", "--force", CONTAINER], docker.calls)

    def test_launch_retains_offline_bounds_and_binds_only_probe_file(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fixture.args.handshake_probe = True
            fixture.session.update(allowed_modes=["startup", "handshake"],
                                   probe=str(fixture.probe), probe_sha256=digest(fixture.probe))
            fixture.write()
            approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            # Source helper hashes come from the real repository; inputs remain authored.
            argv = prepare_launch(ROOT, fixture.args, approved, run, NAME, TOKEN)
            self.assertEqual(argv[argv.index("--network") + 1], "none")
            self.assertEqual(argv[argv.index("--pull") + 1], "never")
            self.assertIn(f"type=bind,src={fixture.probe},dst=/probes/handshake_probe,readonly", argv)
            self.assertIn(f"type=bind,src={approved.reference},dst=/reference,readonly", argv)
            record = json_object(parse_json((run / "invocation.json").read_text()))
            self.assertEqual(record["session_sha256"], approved.session_sha256)
            self.assertEqual(record["reference_binding"], "executable_only_not_full_tree")

    def test_hello_mode_binds_one_u32_version_behind_handshake(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fixture.args.handshake_probe = True
            fixture.args.hello_probe = True
            fixture.session.update(allowed_modes=["startup", "handshake", "hello"], hello_version=2406501229,
                                   probe=str(fixture.probe), probe_sha256=digest(fixture.probe))
            fixture.write()
            self.assertEqual(fixture.approve().hello_version, 2406501229)
            updates: list[JsonObject] = [
                {"hello_version": -1}, {"hello_version": 2**32}, {"hello_version": True},
                {"hello_version": "2406501229"}, {"allowed_modes": ["startup", "hello"]},
                {"allowed_modes": ["startup", "handshake"]},
                {"probe_delay_seconds": 106},  # 106 + 15 exceeds the 120-second observation
            ]
            for update in updates:
                with self.subTest(update=update):
                    fixture.write(update)
                    with self.assertRaises(ValueError):
                        _ = fixture.approve()
            fixture.write({"probe_delay_seconds": 105})
            self.assertEqual(fixture.approve().probe_delay_seconds, 105)
            fixture.args.hello_probe = False
            fixture.write({"allowed_modes": ["startup", "handshake"]})
            with self.assertRaisesRegex(ValueError, "requires hello authorization"):
                _ = fixture.approve()
            fixture.args.hello_probe = True
            fixture.write()
            fixture.args.complete_stateless = True
            with self.assertRaisesRegex(ValueError, "--hello-probe"):
                _ = fixture.approve()
            fixture.args.complete_stateless = False
            fixture.args.handshake_probe = False
            with self.assertRaisesRegex(ValueError, "--hello-probe"):
                _ = fixture.approve()

    def test_hello_probe_receives_only_the_bound_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fixture.args.handshake_probe = True
            fixture.args.hello_probe = True
            fixture.session.update(allowed_modes=["startup", "handshake", "hello"], hello_version=1379612455,
                                   probe=str(fixture.probe), probe_sha256=digest(fixture.probe))
            fixture.write()
            approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            docker = DockerFixture(handshake=True)
            clock = Clock()
            with patch("tools.run_reference_offline.command", new=docker), \
                 patch("tools.run_reference_offline.time.monotonic", new=clock), \
                 patch("tools.run_reference_offline.time.sleep", new=clock.sleep), \
                 patch("tools.run_reference_offline.available_memory", return_value=4 * 1024**3), \
                 patch("tools.run_reference_offline.shutil.disk_usage", new=disk_space):
                result = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
            self.assertIs(result["removed"], True)
            probe = next(call for call in docker.calls if call[1] == "exec")
            self.assertEqual(probe[4:], [CONTAINER, "/probes/handshake_probe", "--hello", "1379612455"])

    def test_join_mode_binds_a_bounded_hold_after_the_probe_delay(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fixture.args.handshake_probe = True
            fixture.args.hello_probe = True
            fixture.args.join_probe = True
            fixture.args.seconds = 600
            fixture.session.update(allowed_modes=["startup", "handshake", "hello", "join"], hello_version=7,
                                   join_seconds=300, probe_delay_seconds=150, observation_seconds=600,
                                   run_timeout_seconds=720, probe=str(fixture.probe),
                                   probe_sha256=digest(fixture.probe))
            fixture.write()
            approved = fixture.approve()
            self.assertEqual((approved.join_seconds, approved.probe_delay_seconds), (300, 150))
            updates: list[JsonObject] = [
                {"join_seconds": 29}, {"join_seconds": 601}, {"probe_delay_seconds": 39},
                {"allowed_modes": ["startup", "handshake", "join"]},
                {"join_seconds": 400},  # 150 + 400 + 60 exceeds the 600-second observation
            ]
            for update in updates:
                with self.subTest(update=update):
                    fixture.write(update)
                    with self.assertRaises(ValueError):
                        _ = fixture.approve()
            fixture.write()
            run = fixture.case / "run"
            run.mkdir()
            docker = DockerFixture(handshake=True)
            clock = Clock()
            with patch("tools.run_reference_offline.command", new=docker), \
                 patch("tools.run_reference_offline.time.monotonic", new=clock), \
                 patch("tools.run_reference_offline.time.sleep", new=clock.sleep), \
                 patch("tools.run_reference_offline.available_memory", return_value=4 * 1024**3), \
                 patch("tools.run_reference_offline.shutil.disk_usage", new=disk_space):
                _ = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
            probe_index = next(i for i, call in enumerate(docker.calls) if call[1] == "exec")
            self.assertEqual(docker.calls[probe_index][6:], ["--hello", "7", "--join", "300"])
            self.assertEqual(docker.timeouts[probe_index], 360)
            fixture.args.hello_probe = False
            with self.assertRaisesRegex(ValueError, "--join-probe"):
                _ = fixture.approve()

    def test_initialise_mode_binds_one_player_id_and_slot(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fixture.args.handshake_probe = True
            fixture.args.hello_probe = True
            fixture.args.join_probe = True
            fixture.args.initialise_probe = True
            fixture.args.seconds = 600
            fixture.session.update(allowed_modes=["startup", "handshake", "hello", "join", "initialise"],
                                   hello_version=7, join_seconds=300, initialise_player_id="7656119800000001",
                                   initialise_chr_slot=2, observation_seconds=600, run_timeout_seconds=720,
                                   probe=str(fixture.probe), probe_sha256=digest(fixture.probe))
            fixture.write()
            approved = fixture.approve()
            self.assertEqual(approved.initialise, ("7656119800000001", 2))
            updates: list[JsonObject] = [
                {"initialise_player_id": ""}, {"initialise_player_id": "a b"},
                {"initialise_player_id": "-7656119800000001"},
                {"initialise_player_id": "x" * 65}, {"initialise_chr_slot": -1},
                {"initialise_chr_slot": 100},
                {"allowed_modes": ["startup", "handshake", "hello", "initialise"]},
            ]
            for update in updates:
                with self.subTest(update=update):
                    fixture.write(update)
                    with self.assertRaises(ValueError):
                        _ = fixture.approve()
            fixture.write()
            run = fixture.case / "run"
            run.mkdir()
            docker = DockerFixture(handshake=True)
            clock = Clock()
            with patch("tools.run_reference_offline.command", new=docker), \
                 patch("tools.run_reference_offline.time.monotonic", new=clock), \
                 patch("tools.run_reference_offline.time.sleep", new=clock.sleep), \
                 patch("tools.run_reference_offline.available_memory", return_value=4 * 1024**3), \
                 patch("tools.run_reference_offline.shutil.disk_usage", new=disk_space):
                _ = observe(fixture.args, approved, run, NAME, TOKEN, ["docker", "run"])
            probe_index = next(i for i, call in enumerate(docker.calls) if call[1] == "exec")
            self.assertEqual(docker.calls[probe_index][6:], [
                "--hello", "7", "--join", "300", "--initialise", "--player-id", "7656119800000001",
                "--chr-slot", "2"])
            fixture.args.join_probe = False
            with self.assertRaisesRegex(ValueError, "--initialise-probe"):
                _ = fixture.approve()
            fixture.args.join_probe = True
            fixture.args.initialise_probe = False
            fixture.write({"allowed_modes": ["startup", "handshake", "hello", "join"]})
            with self.assertRaisesRegex(ValueError, "initialise binding"):
                _ = fixture.approve()

    def test_save_load_binds_one_case_local_prospect_copy(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            save = fixture.case / "inputs" / "authored_prospect.json"
            save.parent.mkdir()
            _ = save.write_text('{"ProspectInfo":{},"ProspectBlob":{}}')
            binding: JsonObject = {"allowed_modes": ["save-load"], "save_input": str(save),
                                   "save_input_sha256": digest(save), "shutdown_if_not_joined_seconds": 60}
            fixture.write({**binding, "allowed_modes": ["startup"]})
            with self.assertRaisesRegex(ValueError, "requires save-load authorization"):
                _ = fixture.approve()
            fixture.args.save_load = True
            fixture.write(binding)
            approved = fixture.approve()
            self.assertEqual(approved.save_input, save)
            self.assertEqual(approved.shutdown_seconds, 60)
            fixture.args.load_fixture = "missing"
            with self.assertRaisesRegex(ValueError, "only one startup experiment"):
                _ = fixture.approve()
            fixture.args.load_fixture = "none"
            fixture.write({**binding, "save_input_sha256": "0" * 64})
            with self.assertRaisesRegex(ValueError, "save_input_sha256"):
                _ = fixture.approve()
            fixture.write({**binding, "shutdown_if_not_joined_seconds": 781})
            with self.assertRaisesRegex(ValueError, "0..780"):
                _ = fixture.approve()
            outside = fixture.root / "outside.json"
            _ = outside.write_text("{}")
            fixture.write({**binding, "save_input": str(outside), "save_input_sha256": digest(outside)})
            with self.assertRaisesRegex(ValueError, "case-local"):
                _ = fixture.approve()
            renamed = fixture.case / "inputs" / "bad name.json"
            _ = renamed.write_text("{}")
            fixture.write({**binding, "save_input": str(renamed), "save_input_sha256": digest(renamed)})
            with self.assertRaisesRegex(ValueError, "case-local"):
                _ = fixture.approve()

    def test_save_load_launch_copies_the_save_and_records_outputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            save = fixture.case / "authored_prospect.json"
            _ = save.write_text('{"ProspectInfo":{},"ProspectBlob":{}}')
            fixture.args.save_load = True
            fixture.write({"allowed_modes": ["save-load"], "save_input": str(save),
                           "save_input_sha256": digest(save), "shutdown_if_not_joined_seconds": 45})
            approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            argv = prepare_launch(ROOT, fixture.args, approved, run, NAME, TOKEN)
            self.assertEqual(argv[argv.index("--network") + 1], "none")
            self.assertTrue(argv[-1].endswith(" -LoadProspect=authored_prospect"))
            prospects = run / "home/Icarus/Saved/PlayerData/DedicatedServer/Prospects"
            self.assertEqual((prospects / "authored_prospect.json").read_bytes(), save.read_bytes())
            settings = (run / "home/Icarus/Saved/Config/WindowsServer/ServerSettings.ini").read_text()
            self.assertIn("ResumeProspect=False\nShutdownIfNotJoinedFor=45\nShutdownIfEmptyFor=45\n", settings)
            recorded = json_object(parse_json((run / "fixture.json").read_text()))
            self.assertEqual(recorded["kind"], "save-load")
            self.assertEqual(recorded["sha256"], digest(save))
            self.assertTrue(record_save_outputs(run, approved)["input_unchanged"])
            _ = (prospects / "authored_prospect.json").write_text('{"ProspectInfo":{"Changed":1}}')
            _ = (prospects / "authored_prospect.json.backup").write_text("{}")
            outputs = record_save_outputs(run, approved)
            self.assertFalse(outputs["input_unchanged"])
            files = outputs["files"]
            self.assertIsInstance(files, list)
            self.assertEqual(len(files) if isinstance(files, list) else 0, 2)
            self.assertTrue((run / "save-outputs.json").is_file())

    def test_save_output_recording_never_hides_a_run_failure(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            save = fixture.case / "authored_prospect.json"
            _ = save.write_text("{}")
            fixture.args.save_load = True
            fixture.write({"allowed_modes": ["save-load"], "save_input": str(save),
                           "save_input_sha256": digest(save), "shutdown_if_not_joined_seconds": 0})
            approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            run_failure = RuntimeError("authored run failure")
            with (patch("tools.run_reference_offline.observe", side_effect=run_failure),
                  patch("tools.run_reference_offline.record_save_outputs",
                        side_effect=PermissionError("authored unreadable output"))):
                with self.assertRaises(RuntimeError) as raised:
                    _ = observe_and_record(fixture.args, approved, run, NAME, TOKEN, [])
            self.assertIs(raised.exception, run_failure)
            self.assertIn("save outputs not recorded: authored unreadable output",
                          getattr(raised.exception, "__notes__", []))
            recorder = Mock(return_value={})
            with (patch("tools.run_reference_offline.observe", return_value={"removed": True}),
                  patch("tools.run_reference_offline.record_save_outputs", recorder)):
                self.assertEqual(observe_and_record(fixture.args, approved, run, NAME, TOKEN, []),
                                 {"removed": True})
            recorder.assert_called_once_with(run, approved)

    def test_shell_forwards_exact_arguments_and_requires_explicit_session(self) -> None:
        wrapper = ROOT / "tools/run_reference_offline.sh"
        result = subprocess.run(["bash", str(wrapper)], capture_output=True, text=True, check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("--case-root", result.stderr)
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            interpreter = base / "python3"
            _ = interpreter.write_text('#!/bin/sh\nprintf "%s\\0" "$@" > "$WRAPPER_ARGS"\n')
            interpreter.chmod(0o700)
            output = base / "args"
            args = ["--case-root", "space and $(literal)", "--session", "semi;colon", "*"]
            _ = subprocess.run(["bash", str(wrapper), *args], cwd=base,
                               env={**os.environ, "PATH": str(base) + os.pathsep + os.environ["PATH"],
                                    "WRAPPER_ARGS": str(output)}, check=True, capture_output=True)
            self.assertEqual(output.read_bytes().split(b"\0")[:-1],
                             [str(ROOT / "tools/run_reference_offline.py").encode(),
                              *(arg.encode() for arg in args)])


if __name__ == "__main__":
    _ = unittest.main()
