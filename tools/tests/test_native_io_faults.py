"""Session and exact-owned cleanup regressions; these tests never invoke Docker."""

from collections.abc import Sequence
import datetime
import json
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

from tools.run_native_io_faults import Budget, cleanup_container, digest, owned_container, validate_probe_events, validate_session
from tools.runtime_types import JsonObject, json_object, parse_json


CONTAINER = "a" * 64
NAME = "icarus-native-io-faults-test"
TOKEN = "test-owner"


def inspection(*, name: str = NAME, owner: str = TOKEN, container: str = CONTAINER) -> str:
    return json.dumps([{"Id": container, "Name": "/" + name,
                        "Config": {"Labels": {"icarus.research.run": NAME,
                                               "icarus.research.owner": owner}}}])


class DockerFixture:
    def __init__(self, *, inspect: str | None = None, failure: str = "", remaining: bool = False) -> None:
        self.inspect: str = inspection() if inspect is None else inspect
        self.failure: str = failure
        self.remaining: bool = remaining
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
                raise subprocess.CalledProcessError(1, argv, stderr="fixture failure")
            return subprocess.CompletedProcess(argv, 1, "", "fixture failure")
        if action == "inspect":
            return subprocess.CompletedProcess(argv, 0, self.inspect, "")
        if action == "ps":
            return subprocess.CompletedProcess(argv, 0, CONTAINER if self.remaining else "", "")
        return subprocess.CompletedProcess(argv, 0, '{"event":"fixture"}\n', "")


class NativeIoSessionTest(unittest.TestCase):
    def test_exit_success_requires_all_fault_and_recovery_evidence(self) -> None:
        events: list[JsonObject] = [
            {"event": "enospc_verified", "state_verified": True, "manifest_unchanged": True},
            {"event": "eacces_verified", "state_verified": True, "manifest_unchanged": True},
            {"event": "io_fault_probe_complete", "state_verified": True,
             "generation": 4, "tick": 40, "entities": 1100},
        ]
        validate_probe_events("\n".join(json.dumps(event) for event in events))
        for invalid in [[], events[:1], list(reversed(events)),
                        [events[0], events[1], {**events[2], "entities": 1}],
                        [{**events[0], "manifest_unchanged": False}, *events[1:]]]:
            with self.subTest(events=invalid):
                with self.assertRaises(RuntimeError):
                    validate_probe_events("\n".join(json.dumps(event) for event in invalid))

    def test_valid_session_and_rejected_scope_deadlines_and_bindings(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            case = root / "work/native-case"
            case.mkdir(parents=True)
            scope = case / "scope.md"
            _ = scope.write_text("## auth\n- status: granted\n## network_profile\n- mode: offline\n"
                                 + "## signoff\n- ready_for_act: true\n")
            probe = case / "probe"
            _ = probe.write_bytes(b"authored executable fixture")
            probe.chmod(0o700)
            session_path = case / "session.json"
            now = datetime.datetime(2026, 10, 12, tzinfo=datetime.UTC)
            session: JsonObject = {
                "scope": "native-io-faults", "authorization_source": "current execution instruction",
                "case_root": str(case), "probe": str(probe), "probe_sha256": digest(probe),
                "scope_sha256": digest(scope), "start_utc": (now - datetime.timedelta(seconds=1)).isoformat(),
                "end_utc": (now + datetime.timedelta(minutes=5)).isoformat(), "run_timeout_seconds": 120,
            }
            _ = session_path.write_text(json.dumps(session))
            approved = validate_session(root, case, session_path, probe, now)
            self.assertEqual(approved.seconds, 120)
            cases: list[tuple[str, JsonObject]] = [
                ("future", {"start_utc": (now + datetime.timedelta(seconds=1)).isoformat()}),
                ("expired", {"end_utc": (now - datetime.timedelta(seconds=1)).isoformat()}),
                ("no cleanup time", {"end_utc": (now + datetime.timedelta(seconds=120)).isoformat()}),
                ("naive timestamp", {"start_utc": "2026-10-11T00:00:00"}),
                ("bad timestamp", {"end_utc": "invalid"}),
                ("boolean duration", {"run_timeout_seconds": True}),
                ("unbounded duration", {"run_timeout_seconds": 121}),
                ("zero duration", {"run_timeout_seconds": 0}),
                ("wrong scope", {"scope": "reference"}),
                ("absent authorization", {"authorization_source": " "}),
                ("wrong case", {"case_root": str(root)}),
                ("wrong probe", {"probe": str(scope)}),
                ("changed scope", {"scope_sha256": "0" * 64}),
                ("changed binary", {"probe_sha256": "0" * 64}),
            ]
            for label, update in cases:
                with self.subTest(label=label):
                    _ = session_path.write_text(json.dumps({**session, **update}))
                    with self.assertRaises(ValueError):
                        _ = validate_session(root, case, session_path, probe, now)
            _ = session_path.write_text(json.dumps(session))
            alias = case / "redirected-probe"
            alias.symlink_to(probe)
            with self.assertRaisesRegex(ValueError, "redirected"):
                _ = validate_session(root, case, session_path, alias, now)
            external_session = root / "session.json"
            _ = external_session.write_text(json.dumps(session))
            with self.assertRaisesRegex(ValueError, "case-local"):
                _ = validate_session(root, case, external_session, probe, now)
            _ = session_path.write_text(json.dumps({**session, "padding": "x" * 65536}))
            with self.assertRaisesRegex(ValueError, "64 KiB"):
                _ = validate_session(root, case, session_path, probe, now)
            mount_option_case = root / "work/native,readonly=false"
            mount_option_case.mkdir()
            with self.assertRaisesRegex(ValueError, "unsupported path"):
                _ = validate_session(root, mount_option_case, session_path, probe, now)

    def test_session_rejects_unsigned_scope_even_with_matching_hash(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            case = root / "work/native-case"
            case.mkdir(parents=True)
            scope = case / "scope.md"
            _ = scope.write_text("## auth\n- status: pending\n")
            probe = case / "probe"
            _ = probe.write_bytes(b"authored fixture")
            probe.chmod(0o700)
            session_path = case / "session.json"
            _ = session_path.write_text(json.dumps({
                "scope": "native-io-faults", "authorization_source": "test instruction",
                "case_root": str(case), "probe": str(probe),
                "scope_sha256": digest(scope), "probe_sha256": digest(probe),
            }))
            with self.assertRaisesRegex(ValueError, "scope auth.status"):
                _ = validate_session(root, case, session_path, probe, datetime.datetime.now(datetime.UTC))

    def test_expired_operation_budget_never_invokes_command(self) -> None:
        fixture = DockerFixture()
        with patch("tools.run_native_io_faults.command", new=fixture):
            with self.assertRaises(TimeoutError):
                _ = Budget(time.monotonic() - 1).run(["docker", "run"])
        self.assertEqual(fixture.calls, [])


class NativeIoCleanupTest(unittest.TestCase):
    def test_identity_requires_full_id_name_unique_label_and_single_result(self) -> None:
        self.assertEqual(owned_container(inspection(), NAME, TOKEN, CONTAINER), CONTAINER)
        invalid = [inspection(name="other"), inspection(owner="other"),
                   inspection(container="b" * 64), inspection(container="short"), "[]",
                   json.dumps([{}, {}])]
        for payload in invalid:
            with self.subTest(payload=payload):
                with self.assertRaises(RuntimeError):
                    _ = owned_container(payload, NAME, TOKEN, CONTAINER)

    def test_cleanup_after_launch_failure_resolves_only_exact_owned_container(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            fixture = DockerFixture()
            with patch("tools.run_native_io_faults.command", new=fixture):
                result = cleanup_container(run, NAME, TOKEN, None, Budget(time.monotonic() + 40))
            self.assertIs(result["removed"], True)
            self.assertEqual(fixture.calls[0], ["docker", "inspect", NAME])
            self.assertIn(["docker", "rm", "--force", CONTAINER], fixture.calls)
            self.assertIn("--no-trunc", fixture.calls[-1])
            self.assertEqual(json_object(parse_json((run / "cleanup.json").read_text()))["removed"], True)
            self.assertTrue(all(0 < value <= 10 for value in fixture.timeouts))

    def test_log_failure_still_removes_and_verifies_owned_container(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            fixture = DockerFixture(failure="logs")
            with patch("tools.run_native_io_faults.command", new=fixture):
                with self.assertRaises(subprocess.CalledProcessError):
                    _ = cleanup_container(run, NAME, TOKEN, CONTAINER, Budget(time.monotonic() + 40))
            self.assertIn(["docker", "rm", "--force", CONTAINER], fixture.calls)
            report = json_object(parse_json((run / "cleanup.json").read_text()))
            self.assertIs(report["removed"], True)
            self.assertIn("error", report)

    def test_cleanup_refuses_foreign_container_and_records_failure(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory)
            fixture = DockerFixture(inspect=inspection(owner="unrelated"))
            with patch("tools.run_native_io_faults.command", new=fixture):
                with self.assertRaisesRegex(RuntimeError, "ownership mismatch"):
                    _ = cleanup_container(run, NAME, TOKEN, None, Budget(time.monotonic() + 40))
            self.assertEqual(fixture.calls, [["docker", "inspect", NAME]])
            report = json_object(parse_json((run / "cleanup.json").read_text()))
            self.assertIs(report["removed"], False)

    def test_daemon_removal_and_verification_failures_never_claim_cleanup(self) -> None:
        for failure, remaining in [("inspect", False), ("rm", False), ("ps", False), ("", True)]:
            with self.subTest(failure=failure, remaining=remaining), tempfile.TemporaryDirectory() as directory:
                run = Path(directory)
                fixture = DockerFixture(failure=failure, remaining=remaining)
                with patch("tools.run_native_io_faults.command", new=fixture):
                    with self.assertRaises((subprocess.CalledProcessError, RuntimeError)):
                        _ = cleanup_container(run, NAME, TOKEN, CONTAINER, Budget(time.monotonic() + 40))
                report = json_object(parse_json((run / "cleanup.json").read_text()))
                self.assertIs(report["removed"], False)
                self.assertIn("error", report)


if __name__ == "__main__":
    _ = unittest.main()
