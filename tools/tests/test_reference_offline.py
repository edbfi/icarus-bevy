"""Offline launcher authorization/lifetime regressions; never invokes Docker."""

from collections.abc import Sequence
from dataclasses import dataclass
import datetime
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import Mock, patch

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
