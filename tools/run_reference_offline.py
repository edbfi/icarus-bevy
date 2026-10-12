#!/usr/bin/env python3
"""Bounded offline reference observation under an explicit current case/session.

No downloads, external network, identity override, extraction or reference writes.
The executable hash binds that file only, not the entire mounted installation.
Docker operations share an observation deadline and reserved cleanup lifetime;
this cannot guarantee daemon termination when the daemon or host is unavailable.
"""

import argparse
from collections.abc import Generator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import stat
import subprocess
import sys
import time
from types import FrameType
import uuid

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.runtime_types import JsonObject, integer_field, json_object, parse_container_identity, parse_container_state, parse_json, string_field

IMAGE = "ghcr.io/pelican-eggs/steamcmd@sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237"
REFERENCE_EXE = Path("Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe")
MODES = frozenset({"startup", "resume", "missing", "truncated-json", "handshake", "complete-stateless"})


class Arguments(argparse.Namespace):
    case_root: str = ""
    session: str = ""
    run_name: str = ""
    seconds: int = 120
    startup_probe: str = "none"
    load_fixture: str = "none"
    handshake_probe: bool = False
    complete_stateless: bool = False


def command(args: Sequence[str], timeout: float = 30, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout, check=check)


def available_memory() -> int:
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    raise RuntimeError("cannot determine available memory")


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        before = os.fstat(stream.fileno())
        result = hashlib.file_digest(stream, "sha256").hexdigest()
        after = os.fstat(stream.fileno())
    if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
        raise ValueError("input changed during hashing")
    return result


def unredirected(path: Path) -> Path:
    absolute = path.absolute()
    if absolute.resolve() != absolute or any(char in str(absolute) for char in ",\n\r"):
        raise ValueError("redirected or unsupported path")
    return absolute


def regular_path(path: Path) -> Path:
    absolute = unredirected(path)
    if not absolute.is_file():
        raise ValueError("missing regular input file")
    return absolute


def selected_modes(args: Arguments) -> frozenset[str]:
    if args.complete_stateless and not args.handshake_probe:
        raise ValueError("--complete-stateless requires --handshake-probe")
    if args.startup_probe not in {"none", "resume"} or args.load_fixture not in {"none", "missing", "truncated-json"}:
        raise ValueError("unknown experiment mode")
    if args.load_fixture != "none" and args.startup_probe != "none":
        raise ValueError("select only one startup experiment")
    modes = {args.load_fixture if args.load_fixture != "none" else
             ("resume" if args.startup_probe == "resume" else "startup")}
    if args.handshake_probe:
        if args.seconds < 60:
            raise ValueError("handshake probe requires at least 60 observation seconds")
        modes.add("handshake")
    if args.complete_stateless:
        modes.add("complete-stateless")
    return frozenset(modes)


@dataclass(frozen=True)
class ApprovedRun:
    case: Path
    session_path: Path
    session: JsonObject
    session_sha256: str
    reference: Path
    probe: Path | None
    observation_seconds: int
    lifetime_seconds: int
    max_runs: int


def validate_session(root: Path, args: Arguments, now: datetime.datetime) -> ApprovedRun:
    case = unredirected(Path(args.case_root))
    if not case.is_dir() or not case.is_relative_to(root / "work") or case == root / "work":
        raise ValueError("case must be an existing directory below work")
    session_path = regular_path(Path(args.session))
    scope = regular_path(case / "scope.md")
    if not session_path.is_relative_to(case):
        raise ValueError("session must be case-local")
    with session_path.open("rb") as stream:
        raw_session = stream.read(65537)
    if len(raw_session) > 65536:
        raise ValueError("session exceeds 64 KiB")
    session = json_object(parse_json(raw_session))
    if string_field(session, "scope") != "reference-offline":
        raise ValueError("session does not authorize reference-offline")
    if not string_field(session, "authorization_source").strip():
        raise ValueError("authorization_source must identify the current instruction")
    if string_field(session, "case_root") != str(case) or string_field(session, "image") != IMAGE:
        raise ValueError("session case/image binding mismatch")
    reference = unredirected(root / "icarus-orig")
    executable = regular_path(reference / REFERENCE_EXE)
    if (string_field(session, "reference_root") != str(reference)
            or string_field(session, "reference_executable") != str(executable)):
        raise ValueError("session reference binding mismatch")
    checks = [("scope_sha256", scope), ("reference_sha256", executable)]
    requested_modes = selected_modes(args)
    allowed = session.get("allowed_modes")
    if (not isinstance(allowed, list) or not allowed
            or any(not isinstance(mode, str) or mode not in MODES for mode in allowed)
            or not requested_modes.issubset(allowed)):
        raise ValueError("experiment mode is not authorized")
    probe: Path | None = None
    if "handshake" in allowed:
        probe = regular_path(Path(string_field(session, "probe")))
        if not probe.is_relative_to(case) or not os.access(probe, os.X_OK):
            raise ValueError("probe must be an executable case-local file")
        checks.append(("probe_sha256", probe))
    elif "probe" in session or "probe_sha256" in session:
        raise ValueError("probe binding requires handshake authorization")
    for field, path in checks:
        expected = string_field(session, field)
        if re.fullmatch(r"[0-9a-f]{64}", expected) is None or expected != digest(path):
            raise ValueError(f"{field} mismatch")
    start = datetime.datetime.fromisoformat(string_field(session, "start_utc"))
    end = datetime.datetime.fromisoformat(string_field(session, "end_utc"))
    if start.utcoffset() is None or end.utcoffset() is None:
        raise ValueError("session timestamps must include timezone offsets")
    observation = integer_field(session, "observation_seconds")
    lifetime = integer_field(session, "run_timeout_seconds")
    max_runs = integer_field(session, "max_runs")
    if not 15 <= observation <= 780 or args.seconds != observation:
        raise ValueError("observation must match session and be 15..780 seconds")
    if not observation + 120 <= lifetime <= 900:
        raise ValueError("lifetime must reserve 120 cleanup seconds and be <=900 seconds")
    if not 1 <= max_runs <= 12:
        raise ValueError("max_runs must be 1..12")
    if not start <= now < end or now + datetime.timedelta(seconds=lifetime) >= end:
        raise ValueError("session is future, expired or has insufficient approved time")
    return ApprovedRun(case, session_path, session, hashlib.sha256(raw_session).hexdigest(),
                       reference, probe, observation, lifetime, max_runs)


def revalidate(root: Path, args: Arguments, approved: ApprovedRun) -> None:
    if validate_session(root, args, datetime.datetime.now(datetime.UTC)) != approved:
        raise RuntimeError("session changed during preparation")


@contextmanager
def reserve_run(approved: ApprovedRun, run_name: str) -> Generator[Path]:
    if re.fullmatch(r"[a-z0-9][a-z0-9-]{0,39}", run_name) is None:
        raise ValueError("run-name must be short lowercase alphanumeric/hyphens")
    lock_path = approved.case / "reference-offline.lock"
    descriptor = os.open(lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError("case lock must be a regular file")
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        runs = unredirected(approved.case / "runtime")
        runs.mkdir(exist_ok=True)
        if sum(1 for _ in runs.glob("run-*")) >= approved.max_runs:
            raise RuntimeError("approved run count exhausted")
        run = runs / ("run-" + run_name)
        run.mkdir()  # Failed attempts count; evidence is never overwritten.
        yield run
    finally:
        os.close(descriptor)


@dataclass(frozen=True)
class Budget:
    deadline: float

    def run(self, argv: Sequence[str], *, check: bool = True, cap: float = 10) -> subprocess.CompletedProcess[str]:
        remaining = self.deadline - time.monotonic()
        if remaining <= 0 or cap <= 0:
            raise TimeoutError("reference run deadline exhausted")
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
    previous_term = signal.signal(signal.SIGTERM, signal.SIG_IGN)
    previous_int = signal.signal(signal.SIGINT, signal.SIG_IGN)
    try:
        inspected = budget.run(["docker", "inspect", name],
                               cap=min(10, (budget.deadline - time.monotonic()) / 6))
        container = owned_container(inspected.stdout, name, token, expected_id)
        cleanup["container_id"] = container
        try:
            _ = (run / "container-before-stop.json").write_text(inspected.stdout)
            logs = budget.run(["docker", "logs", container],
                              cap=min(10, (budget.deadline - time.monotonic()) / 6))
            _ = (run / "console-before.log").write_text(logs.stdout + logs.stderr)
            _ = budget.run(["docker", "stop", "--time", "10", container],
                           cap=min(15, (budget.deadline - time.monotonic()) / 3))
            logs = budget.run(["docker", "logs", container],
                              cap=min(10, (budget.deadline - time.monotonic()) / 4))
            _ = (run / "console-after.log").write_text(logs.stdout + logs.stderr)
        finally:
            # Log/stop failures cannot bypass removal of an already verified ID.
            _ = budget.run(["docker", "rm", "--force", container],
                           cap=min(15, (budget.deadline - time.monotonic()) / 2))
            remaining = budget.run(["docker", "ps", "--all", "--no-trunc", "--filter",
                                    f"id={container}", "--format", "{{.ID}}"])
            cleanup["removed"] = not remaining.stdout.strip()
            if cleanup["removed"] is not True:
                raise RuntimeError("owned container remains after removal")
    except (Exception, KeyboardInterrupt) as exc:
        cleanup["error"] = str(exc)
        raise
    finally:
        try:
            _ = (run / "cleanup.json").write_text(json.dumps(cleanup, indent=2) + "\n")
        finally:
            _ = signal.signal(signal.SIGTERM, previous_term)
            _ = signal.signal(signal.SIGINT, previous_int)
    return cleanup


def prepare_launch(root: Path, args: Arguments, approved: ApprovedRun,
                   run: Path, name: str, token: str) -> list[str]:
    home = run / "home"
    home.mkdir()
    fixture: JsonObject | None = None
    if args.load_fixture != "none":
        relative = Path("Icarus/Saved/PlayerData/DedicatedServer/Prospects/overnight-authored.json")
        fixture = {"kind": args.load_fixture, "relative_path": str(relative), "exists_at_start": False}
        if args.load_fixture == "truncated-json":
            payload = b'{"ProspectInfo":{},"ProspectBlob":'
            inputs = run / "inputs"
            inputs.mkdir()
            _ = (inputs / "overnight-authored.json").write_bytes(payload)
            (home / relative).parent.mkdir(parents=True)
            _ = (home / relative).write_bytes(payload)
            fixture.update(exists_at_start=True, bytes=len(payload), sha256=hashlib.sha256(payload).hexdigest())
        _ = (run / "fixture.json").write_text(json.dumps(fixture, indent=2) + "\n")
    launch = ('mkdir -p "$STEAM_COMPAT_DATA_PATH"; exec xvfb-run -a proton run '
              '/reference/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe '
              '-UserDir="Z:/home/container/Icarus" -Log '
              '-SteamServerName="icarus-research-offline" -PORT=17777 -QueryPort=27015')
    if args.startup_probe == "resume":
        launch += ' -ExecCmds="ResumeProspect"'
    if args.load_fixture != "none":
        launch += ' -LoadProspect=overnight-authored'
    argv = ["docker", "run", "--detach", "--pull", "never", "--name", name,
            "--label", "icarus.research.run=" + name,
            "--label", "icarus.research.owner=" + token, "--cidfile", str(run / "container.cid"),
            "--network", "none", "--read-only", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--memory", "6g", "--cpus", "2",
            "--pids-limit", "512", "--shm-size", "256m",
            "--tmpfs", "/tmp:rw,exec,nosuid,size=1g", "--tmpfs", "/run:rw,nosuid,size=16m",
            "--mount", f"type=bind,src={approved.reference},dst=/reference,readonly",
            "--mount", f"type=bind,src={home},dst=/home/container", "--workdir", "/reference",
            "--env", "SteamAppId=1149460", "--env", "SteamGameId=1149460",
            "--env", "STEAM_COMPAT_DATA_PATH=/home/container/proton-prefix",
            "--env", "STEAM_COMPAT_CLIENT_INSTALL_PATH=/home/container"]
    if args.handshake_probe:
        if approved.probe is None:
            raise ValueError("missing authorized probe")
        argv += ["--mount", f"type=bind,src={approved.probe},dst=/probes/handshake_probe,readonly"]
    argv += [IMAGE, "/bin/bash", "-lc", launch]
    helpers = [Path(__file__), root / "tools/run_reference_offline.sh", root / "tools/runtime_types.py"]
    record: JsonObject = {
        "time": datetime.datetime.now(datetime.UTC).isoformat(), "argv": list(argv),
        "timeout_seconds": approved.lifetime_seconds, "observation_seconds": approved.observation_seconds,
        "approval": approved.session, "session_sha256": approved.session_sha256,
        "reference_binding": "executable_only_not_full_tree", "fixture": fixture,
        "helpers": {str(path.relative_to(root)): digest(path) for path in helpers},
    }
    with (approved.case / "commands.jsonl").open("a") as stream:
        _ = stream.write(json.dumps(record) + "\n")
    _ = (run / "invocation.json").write_text(json.dumps(record, indent=2) + "\n")
    return argv


def observe(args: Arguments, approved: ApprovedRun, run: Path,
            name: str, token: str, argv: list[str]) -> JsonObject:
    # Hashing and preparation can consume the slack checked by revalidate().
    # Sample monotonic first so the conversion cannot extend the UTC expiry.
    start = time.monotonic()
    now = datetime.datetime.now(datetime.UTC)
    session_start = datetime.datetime.fromisoformat(string_field(approved.session, "start_utc"))
    session_end = datetime.datetime.fromisoformat(string_field(approved.session, "end_utc"))
    remaining = (session_end - now).total_seconds()
    if now < session_start or remaining <= approved.observation_seconds + 120:
        raise ValueError("session has insufficient approved time at launch")
    budget = Budget(start + min(approved.lifetime_seconds, remaining))
    observation = Budget(start + approved.observation_seconds)

    def interrupted(_signum: int, _frame: FrameType | None) -> None:
        raise KeyboardInterrupt("reference observation interrupted")

    previous = signal.signal(signal.SIGTERM, interrupted)
    previous_int = signal.signal(signal.SIGINT, interrupted)
    container: str | None = None
    try:
        result = observation.run(argv, cap=45)
        candidate = result.stdout.strip()
        if re.fullmatch(r"[0-9a-f]{64}", candidate) is None:
            raise RuntimeError("unexpected container ID")
        container = candidate
        with (run / "resource-samples.jsonl").open("x") as samples:
            probed = False
            while time.monotonic() < observation.deadline:
                if available_memory() < 3 * 1024**3 or shutil.disk_usage(approved.case).free < 30 * 1024**3:
                    raise RuntimeError("resource floor reached during observation")
                inspected = observation.run(["docker", "inspect", container])
                _ = owned_container(inspected.stdout, name, token, container)
                if not parse_container_state(inspected.stdout).running:
                    break
                if args.handshake_probe and not probed and time.monotonic() - start >= 40:
                    probe_argv = ["docker", "exec", "--env", "ICARUS_OFFLINE_PROBE=1",
                                  container, "/probes/handshake_probe"]
                    if args.complete_stateless:
                        probe_argv.append("--complete-stateless")
                    _ = (run / "probe-invocation.json").write_text(json.dumps(probe_argv) + "\n")
                    probe = observation.run(probe_argv, cap=15, check=False)
                    _ = (run / "probe.jsonl").write_text(probe.stdout)
                    _ = (run / "probe.stderr").write_text(probe.stderr)
                    _ = (run / "probe-exit.txt").write_text(str(probe.returncode) + "\n")
                    if probe.returncode != 0:
                        raise RuntimeError("handshake probe failed")
                    probed = True
                _ = samples.write(observation.run(["docker", "stats", "--no-stream", "--format",
                                                   "{{json .}}", container]).stdout)
                samples.flush()
                time.sleep(min(5, max(0, observation.deadline - time.monotonic())))
            if args.handshake_probe and not probed:
                raise RuntimeError("requested handshake probe did not run")
        _ = (run / "observed-seconds.txt").write_text(str(time.monotonic() - start) + "\n")
    finally:
        try:
            cleanup = cleanup_container(run, name, token, container, budget)
        finally:
            _ = signal.signal(signal.SIGTERM, previous)
            _ = signal.signal(signal.SIGINT, previous_int)
    return cleanup


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--case-root", required=True)
    _ = parser.add_argument("--session", required=True)
    _ = parser.add_argument("--run-name", required=True)
    _ = parser.add_argument("--seconds", type=int, default=120)
    _ = parser.add_argument("--startup-probe", choices=["none", "resume"], default="none")
    _ = parser.add_argument("--load-fixture", choices=["none", "missing", "truncated-json"], default="none")
    _ = parser.add_argument("--handshake-probe", action="store_true")
    _ = parser.add_argument("--complete-stateless", action="store_true")
    args = parser.parse_args(namespace=Arguments())
    root = Path(__file__).resolve().parents[1]
    approved = validate_session(root, args, datetime.datetime.now(datetime.UTC))
    _ = command(["bash", str(root / ".agents/vendor/reverse-skill/skills/scripts/case-guard.sh"),
                 "--case-root", str(approved.case)])
    if available_memory() < 3 * 1024**3 or shutil.disk_usage(approved.case).free < 30 * 1024**3:
        raise RuntimeError("resource floor reached; defer original execution")
    with reserve_run(approved, args.run_name) as run:
        token = uuid.uuid4().hex
        name = "icarus-offline-" + token
        argv = prepare_launch(root, args, approved, run, name, token)
        revalidate(root, args, approved)
        cleanup = observe(args, approved, run, name, token, argv)
        print(json.dumps({"run": str(run), **cleanup}))


if __name__ == "__main__":
    main()
