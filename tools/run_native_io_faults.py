#!/usr/bin/env python3
"""Run the authored ENOSPC/EACCES probe under an explicit, current case scope.

The session binds the case, scope and probe hashes and grants only this native
experiment. All Docker operations share a <=120-second budget including cleanup.
No original execution, downloads, networking or host filesystem exhaustion.
"""
import argparse
from collections.abc import Sequence
from dataclasses import dataclass
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import time
from types import FrameType
import uuid

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.run_reference_offline import IMAGE, available_memory, command, validate_offline_scope
from tools.runtime_types import JsonObject, integer_field, json_object, parse_container_identity, parse_container_state, parse_json, string_field


class Arguments(argparse.Namespace):
    case_root: str = ""
    session: str = ""
    probe: str = ""


@dataclass(frozen=True)
class ApprovedRun:
    case: Path
    probe: Path
    session: JsonObject
    end: datetime.datetime
    seconds: int


def regular_path(path: Path) -> Path:
    absolute = path.absolute()
    if absolute.resolve() != absolute or not absolute.is_file():
        raise ValueError(f"missing or redirected file: {absolute}")
    return absolute


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def validate_session(root: Path, case: Path, session_path: Path, probe: Path,
                     now: datetime.datetime) -> ApprovedRun:
    case = case.absolute()
    if case.resolve() != case or not case.is_dir() or not case.is_relative_to(root / "work"):
        raise ValueError("case must be an existing unredirected directory under work")
    session_path = regular_path(session_path)
    probe = regular_path(probe)
    scope = regular_path(case / "scope.md")
    if not session_path.is_relative_to(case) or not probe.is_relative_to(case):
        raise ValueError("session and probe must be case-local")
    if not os.access(probe, os.X_OK):
        raise ValueError("probe must be executable")
    session = json_object(parse_json(session_path.read_text()))
    if string_field(session, "scope") != "native-io-faults":
        raise ValueError("session does not authorize native-io-faults")
    if not string_field(session, "authorization_source").strip():
        raise ValueError("authorization_source must identify the current user instruction")
    if string_field(session, "case_root") != str(case) or string_field(session, "probe") != str(probe):
        raise ValueError("session case/probe binding mismatch")
    for field, path in (("scope_sha256", scope), ("probe_sha256", probe)):
        expected = string_field(session, field)
        if not re.fullmatch(r"[0-9a-f]{64}", expected) or expected != digest(path):
            raise ValueError(f"{field} mismatch")
    validate_offline_scope(scope)
    start = datetime.datetime.fromisoformat(string_field(session, "start_utc"))
    end = datetime.datetime.fromisoformat(string_field(session, "end_utc"))
    if start.utcoffset() is None or end.utcoffset() is None:
        raise ValueError("session timestamps must include timezone offsets")
    seconds = integer_field(session, "run_timeout_seconds")
    if not 1 <= seconds <= 120:
        raise ValueError("run_timeout_seconds must be 1..120")
    if not start <= now < end or now + datetime.timedelta(seconds=seconds) >= end:
        raise ValueError("session is future, expired or has insufficient approved time")
    return ApprovedRun(case, probe, session, end, seconds)


@dataclass(frozen=True)
class Budget:
    deadline: float

    def run(self, argv: Sequence[str], *, check: bool = True, cap: float = 10) -> subprocess.CompletedProcess[str]:
        remaining = self.deadline - time.monotonic()
        if remaining <= 0:
            raise TimeoutError("native I/O run deadline exhausted")
        return command(argv, timeout=min(cap, remaining), check=check)


def owned_container(text: str, name: str, token: str, expected_id: str | None) -> str:
    values = parse_json(text)
    if not isinstance(values, list) or len(values) != 1:
        raise RuntimeError("container ownership requires exactly one inspect result")
    obj = json_object(values[0])
    identity = parse_container_identity(text)
    if (string_field(obj, "Name") != "/" + name
            or identity.labels.get("icarus.research.run") != name
            or identity.labels.get("icarus.research.owner") != token
            or re.fullmatch(r"[0-9a-f]{64}", identity.container_id) is None
            or (expected_id is not None and identity.container_id != expected_id)):
        raise RuntimeError("container ownership mismatch")
    return identity.container_id


def cleanup_container(run: Path, name: str, token: str, expected_id: str | None,
                      budget: Budget) -> JsonObject:
    cleanup: JsonObject = {"removed": False}
    try:
        inspected = budget.run(["docker", "inspect", name],
                               cap=min(10, (budget.deadline - time.monotonic()) / 4))
        container = owned_container(inspected.stdout, name, token, expected_id)
        cleanup["container_id"] = container
        try:
            _ = (run / "container.json").write_text(inspected.stdout)
            # Leave most cleanup time for removal and its independent verification.
            logs = budget.run(["docker", "logs", container],
                              cap=min(5, (budget.deadline - time.monotonic()) / 4))
            _ = (run / "probe.jsonl").write_text(logs.stdout)
            _ = (run / "probe.stderr").write_text(logs.stderr)
        finally:
            _ = budget.run(["docker", "rm", "--force", container],
                           cap=min(10, (budget.deadline - time.monotonic()) / 2))
            remaining = budget.run(["docker", "ps", "--all", "--no-trunc", "--filter",
                                    f"id={container}", "--format", "{{.ID}}"])
            cleanup["removed"] = not remaining.stdout.strip()
            if not cleanup["removed"]:
                raise RuntimeError("owned container remains after removal")
    except (Exception, KeyboardInterrupt) as exc:
        cleanup["error"] = str(exc)
        raise
    finally:
        _ = (run / "cleanup.json").write_text(json.dumps(cleanup, indent=2) + "\n")
    return cleanup


def validate_probe_events(text: str) -> None:
    events = [json_object(parse_json(line)) for line in text.splitlines() if line.strip()]
    if [string_field(event, "event") for event in events] != [
            "enospc_verified", "eacces_verified", "io_fault_probe_complete"]:
        raise RuntimeError("missing or unexpected native I/O probe events")
    if any(event.get("state_verified") is not True for event in events):
        raise RuntimeError("native I/O probe did not verify preserved state")
    if any(event.get("manifest_unchanged") is not True for event in events[:2]):
        raise RuntimeError("native I/O probe did not verify unchanged manifests")
    complete = events[-1]
    if (integer_field(complete, "generation"), integer_field(complete, "tick"),
            integer_field(complete, "entities")) != (4, 40, 1100):
        raise RuntimeError("native I/O probe completion state mismatch")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--case-root", required=True)
    _ = parser.add_argument("--session", required=True)
    _ = parser.add_argument("--probe", required=True)
    args = parser.parse_args(namespace=Arguments())
    root = Path(__file__).resolve().parents[1]
    approved = validate_session(root, Path(args.case_root), Path(args.session), Path(args.probe),
                                datetime.datetime.now(datetime.UTC))
    if available_memory() < 3 * 1024**3 or shutil.disk_usage(approved.case).free < 30 * 1024**3:
        raise RuntimeError("resource floor reached")
    run = approved.case / "native-io-faults"
    run.mkdir()  # Preserve prior evidence; reruns need a fresh case.
    home = run / "home"
    home.mkdir()
    token = uuid.uuid4().hex
    name = "icarus-native-io-faults-" + token
    argv = ["docker", "run", "--detach", "--pull", "never", "--name", name,
            "--label", "icarus.research.run=" + name,
            "--label", "icarus.research.owner=" + token,
            "--network", "none", "--read-only", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--memory", "6g", "--cpus", "2",
            "--pids-limit", "512", "--shm-size", "256m",
            "--tmpfs", "/tmp:rw,noexec,nosuid,size=256k", "--tmpfs", "/run:rw,nosuid,size=16m",
            "--mount", f"type=bind,src={approved.probe},dst=/probes/io_fault_probe,readonly",
            "--mount", f"type=bind,src={home},dst=/home/container",
            "--workdir", "/tmp", "--env", "ICARUS_BOUNDED_TMPFS_BYTES=262144",
            IMAGE, "/probes/io_fault_probe"]
    helpers = [Path(__file__), root / "tools/run_reference_offline.py", root / "tools/runtime_types.py",
               approved.probe, root / "examples/io_fault_probe.rs", root / "src/persistence.rs", root / "Cargo.lock"]
    record = {"time": datetime.datetime.now(datetime.UTC).isoformat(), "argv": argv,
              "timeout_seconds": approved.seconds, "approval": approved.session,
              "helpers": {str(p.relative_to(root)): digest(p) for p in helpers}}
    with (approved.case / "commands.jsonl").open("a") as stream:
        _ = stream.write(json.dumps(record) + "\n")
    _ = (run / "invocation.json").write_text(json.dumps(record, indent=2) + "\n")
    # Recheck immediately before launch; preparation must not consume authorization.
    if validate_session(root, approved.case, Path(args.session), approved.probe,
                        datetime.datetime.now(datetime.UTC)) != approved:
        raise RuntimeError("session changed during preparation; refuse unrecorded authorization")

    def interrupted(_signum: int, _frame: FrameType | None) -> None:
        raise KeyboardInterrupt("native I/O probe interrupted")

    previous = signal.signal(signal.SIGTERM, interrupted)
    previous_interrupt = signal.getsignal(signal.SIGINT)
    start = time.monotonic()
    budget = Budget(start + approved.seconds)
    observation = Budget(start + approved.seconds * 2 / 3)
    container: str | None = None
    outcome: int | None = None
    try:
        result = observation.run(argv, cap=min(25, approved.seconds / 3))
        container = result.stdout.strip()
        if re.fullmatch(r"[0-9a-f]{64}", container) is None:
            container = None
            raise RuntimeError("unexpected container ID")
        while time.monotonic() < observation.deadline:
            inspected = observation.run(["docker", "inspect", container])
            _ = owned_container(inspected.stdout, name, token, container)
            info = parse_container_state(inspected.stdout)
            if not info.running:
                outcome = info.exit_code
                break
            time.sleep(min(1, max(0, observation.deadline - time.monotonic())))
        if outcome is None:
            raise RuntimeError("native probe exceeded observation deadline")
    finally:
        try:
            # Repeated termination must not interrupt removal of an owned container.
            _ = signal.signal(signal.SIGTERM, signal.SIG_IGN)
            _ = signal.signal(signal.SIGINT, signal.SIG_IGN)
            cleanup = cleanup_container(run, name, token, container, budget)
        finally:
            _ = signal.signal(signal.SIGTERM, previous)
            _ = signal.signal(signal.SIGINT, previous_interrupt)
    if outcome != 0 or cleanup["removed"] is not True:
        raise RuntimeError(f"native probe or cleanup failed: exit={outcome}, cleanup={cleanup}")
    probe_events = (run / "probe.jsonl").read_text()
    validate_probe_events(probe_events)
    print(probe_events)
    print(json.dumps(cleanup))


if __name__ == "__main__":
    main()
