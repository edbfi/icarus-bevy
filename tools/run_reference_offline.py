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
MODES = frozenset({"startup", "resume", "missing", "truncated-json", "save-load", "handshake",
                   "complete-stateless", "hello", "join", "initialise"})
# A saved character's player id as the probe logs in with it (F-858, F-949).
PLAYER_ID = re.compile(r"[A-Za-z0-9_-]{1,64}")
PROSPECT_FILE = re.compile(r"[A-Za-z0-9_-]{1,64}\.json")
PROSPECTS = Path("Icarus/Saved/PlayerData/DedicatedServer/Prospects")
PLAYER_DATA = Path("Icarus/Saved/PlayerData")
SERVER_SETTINGS = Path("Icarus/Saved/Config/WindowsServer/ServerSettings.ini")


class Arguments(argparse.Namespace):
    case_root: str = ""
    session: str = ""
    run_name: str = ""
    seconds: int = 120
    startup_probe: str = "none"
    load_fixture: str = "none"
    save_load: bool = False
    handshake_probe: bool = False
    complete_stateless: bool = False
    hello_probe: bool = False
    join_probe: bool = False
    initialise_probe: bool = False


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


def validate_offline_scope(scope: Path) -> None:
    """Check the project runtime contract in the case's Markdown scope file."""
    required = {("auth", "status"): "granted",
                ("network_profile", "mode"): "offline",
                ("signoff", "ready_for_act"): "true"}
    fields: dict[tuple[str, str], str] = {}
    section = ""
    for line in scope.read_text().splitlines():
        if line.startswith("## "):
            section = line[3:].strip().lower()
        elif match := re.fullmatch(r"\s*-\s+([a-z_]+):\s*(.*?)\s*", line):
            key = (section, match[1])
            if key in required:
                if key in fields:
                    raise ValueError(f"duplicate scope field: {section}.{match[1]}")
                fields[key] = match[2].lower()
    for key, expected in required.items():
        if fields.get(key) != expected:
            raise ValueError(f"scope {'.'.join(key)} must be {expected}")


def selected_modes(args: Arguments) -> frozenset[str]:
    if args.complete_stateless and not args.handshake_probe:
        raise ValueError("--complete-stateless requires --handshake-probe")
    if args.hello_probe and (not args.handshake_probe or args.complete_stateless):
        raise ValueError("--hello-probe requires --handshake-probe without --complete-stateless")
    if args.join_probe and not args.hello_probe:
        raise ValueError("--join-probe requires --hello-probe")
    if args.initialise_probe and not args.join_probe:
        raise ValueError("--initialise-probe requires --join-probe")
    if args.startup_probe not in {"none", "resume"} or args.load_fixture not in {"none", "missing", "truncated-json"}:
        raise ValueError("unknown experiment mode")
    if sum((args.load_fixture != "none", args.startup_probe != "none", args.save_load)) > 1:
        raise ValueError("select only one startup experiment")
    modes = {"save-load" if args.save_load else args.load_fixture if args.load_fixture != "none" else
             ("resume" if args.startup_probe == "resume" else "startup")}
    if args.handshake_probe:
        if args.seconds < 60:
            raise ValueError("handshake probe requires at least 60 observation seconds")
        modes.add("handshake")
    if args.complete_stateless:
        modes.add("complete-stateless")
    if args.hello_probe:
        modes.add("hello")
    if args.join_probe:
        modes.add("join")
    if args.initialise_probe:
        modes.add("initialise")
    return frozenset(modes)


@dataclass(frozen=True)
class ApprovedRun:
    case: Path
    session_path: Path
    session: JsonObject
    session_sha256: str
    reference: Path
    probe: Path | None
    save_input: Path | None
    shutdown_seconds: int | None
    hello_version: int | None
    join_seconds: int | None
    initialise: tuple[str, int] | None
    probe_delay_seconds: int
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
    # The preserved install or a fetched icarus-server-<buildid> sibling.
    reference = unredirected(Path(string_field(session, "reference_root")))
    if reference.parent != root or re.fullmatch(r"icarus-orig|icarus-server-[0-9]+", reference.name) is None:
        raise ValueError("session reference binding mismatch")
    executable = regular_path(reference / REFERENCE_EXE)
    if string_field(session, "reference_executable") != str(executable):
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
    save_input: Path | None = None
    shutdown: int | None = None
    if "save-load" in allowed:
        # A disposable copy of a supplied save, never the private original.
        save_input = regular_path(Path(string_field(session, "save_input")))
        if not save_input.is_relative_to(case) or PROSPECT_FILE.fullmatch(save_input.name) is None:
            raise ValueError("save input must be a case-local <prospect>.json file")
        checks.append(("save_input_sha256", save_input))
        shutdown = integer_field(session, "shutdown_if_not_joined_seconds")
        if not 0 <= shutdown <= 780:
            raise ValueError("shutdown_if_not_joined_seconds must be 0..780")
    elif any(key in session for key in ("save_input", "save_input_sha256", "shutdown_if_not_joined_seconds")):
        raise ValueError("save binding requires save-load authorization")
    hello_version: int | None = None
    if "hello" in allowed:
        # One Hello with an empty token: no identity material, no Login.
        if "handshake" not in allowed:
            raise ValueError("hello requires handshake authorization")
        hello_version = integer_field(session, "hello_version")
        if not 0 <= hello_version <= 0xFFFF_FFFF:
            raise ValueError("hello_version must be an unsigned 32-bit value")
    elif "hello_version" in session:
        raise ValueError("hello_version requires hello authorization")
    join_seconds: int | None = None
    if "join" in allowed:
        # Synthetic ids only; the probe holds the connection, then lets it lapse.
        if "hello" not in allowed:
            raise ValueError("join requires hello authorization")
        join_seconds = integer_field(session, "join_seconds")
        if not 30 <= join_seconds <= 600:
            raise ValueError("join_seconds must be 30..600")
    elif "join_seconds" in session:
        raise ValueError("join_seconds requires join authorization")
    initialise: tuple[str, int] | None = None
    if "initialise" in allowed:
        # The joined probe initialises as this saved character's player.
        if "join" not in allowed:
            raise ValueError("initialise requires join authorization")
        player_id = string_field(session, "initialise_player_id")
        chr_slot = integer_field(session, "initialise_chr_slot")
        if PLAYER_ID.fullmatch(player_id) is None or not 0 <= chr_slot <= 99:
            raise ValueError("initialise needs a 1..64 character player id and a 0..99 slot")
        initialise = (player_id, chr_slot)
    elif any(key in session for key in ("initialise_player_id", "initialise_chr_slot")):
        raise ValueError("initialise binding requires initialise authorization")
    probe_delay = 40
    if "probe_delay_seconds" in session:
        if "handshake" not in allowed:
            raise ValueError("probe_delay_seconds requires handshake authorization")
        probe_delay = integer_field(session, "probe_delay_seconds")
        if not 40 <= probe_delay <= 600:
            raise ValueError("probe_delay_seconds must be 40..600")
    for field, path in checks:
        expected = string_field(session, field)
        if re.fullmatch(r"[0-9a-f]{64}", expected) is None or expected != digest(path):
            raise ValueError(f"{field} mismatch")
    validate_offline_scope(scope)
    start = datetime.datetime.fromisoformat(string_field(session, "start_utc"))
    end = datetime.datetime.fromisoformat(string_field(session, "end_utc"))
    if start.utcoffset() is None or end.utcoffset() is None:
        raise ValueError("session timestamps must include timezone offsets")
    observation = integer_field(session, "observation_seconds")
    lifetime = integer_field(session, "run_timeout_seconds")
    max_runs = integer_field(session, "max_runs")
    if not 15 <= observation <= 780 or args.seconds != observation:
        raise ValueError("observation must match session and be 15..780 seconds")
    # A probe starts after its delay and needs its own cap inside the window.
    probe_window = probe_delay + (join_seconds + 60 if join_seconds is not None else 15)
    if args.handshake_probe and observation < probe_window:
        raise ValueError("observation must cover the probe delay and the probe (the join hold and 60 seconds, else 15)")
    if not observation + 120 <= lifetime <= 900:
        raise ValueError("lifetime must reserve 120 cleanup seconds and be <=900 seconds")
    if not 1 <= max_runs <= 12:
        raise ValueError("max_runs must be 1..12")
    if not start <= now < end or now + datetime.timedelta(seconds=lifetime) >= end:
        raise ValueError("session is future, expired or has insufficient approved time")
    return ApprovedRun(case, session_path, session, hashlib.sha256(raw_session).hexdigest(),
                       reference, probe, save_input, shutdown, hello_version, join_seconds, initialise,
                       probe_delay,
                       observation, lifetime, max_runs)


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
    if args.save_load:
        if approved.save_input is None or approved.shutdown_seconds is None:
            raise ValueError("missing authorized save input")
        relative = PROSPECTS / approved.save_input.name
        (home / relative).parent.mkdir(parents=True)
        _ = shutil.copyfile(approved.save_input, home / relative)
        if digest(home / relative) != string_field(approved.session, "save_input_sha256"):
            raise ValueError("save copy digest mismatch")
        settings = ("[/Script/Icarus.DedicatedServerSettings]\nResumeProspect=False\n"
                    + f"ShutdownIfNotJoinedFor={approved.shutdown_seconds}\n"
                    + f"ShutdownIfEmptyFor={approved.shutdown_seconds}\n")
        (home / SERVER_SETTINGS).parent.mkdir(parents=True)
        _ = (home / SERVER_SETTINGS).write_text(settings)
        fixture = {"kind": "save-load", "relative_path": str(relative), "exists_at_start": True,
                   "bytes": (home / relative).stat().st_size, "sha256": digest(home / relative),
                   "server_settings": settings, "server_settings_sha256": digest(home / SERVER_SETTINGS)}
    if fixture is not None:
        _ = (run / "fixture.json").write_text(json.dumps(fixture, indent=2) + "\n")
    launch = ('mkdir -p "$STEAM_COMPAT_DATA_PATH"; exec xvfb-run -a proton run '
              '/reference/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe '
              '-UserDir="Z:/home/container/Icarus" -Log '
              '-SteamServerName="icarus-research-offline" -PORT=17777 -QueryPort=27015')
    if args.startup_probe == "resume":
        launch += ' -ExecCmds="ResumeProspect"'
    if args.load_fixture != "none":
        launch += ' -LoadProspect=overnight-authored'
    if args.save_load and approved.save_input is not None:
        launch += ' -LoadProspect=' + approved.save_input.stem
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
                if args.handshake_probe and not probed and time.monotonic() - start >= approved.probe_delay_seconds:
                    probe_argv = ["docker", "exec", "--env", "ICARUS_OFFLINE_PROBE=1",
                                  container, "/probes/handshake_probe"]
                    if args.complete_stateless:
                        probe_argv.append("--complete-stateless")
                    if args.hello_probe:
                        if approved.hello_version is None:
                            raise ValueError("missing authorized hello version")
                        probe_argv += ["--hello", str(approved.hello_version)]
                    cap = 15
                    if args.join_probe:
                        if approved.join_seconds is None:
                            raise ValueError("missing authorized join hold")
                        probe_argv += ["--join", str(approved.join_seconds)]
                        cap = approved.join_seconds + 60
                    if args.initialise_probe:
                        if approved.initialise is None:
                            raise ValueError("missing authorized initialisation")
                        player_id, chr_slot = approved.initialise
                        probe_argv += ["--initialise", "--player-id", player_id, "--chr-slot", str(chr_slot)]
                    _ = (run / "probe-invocation.json").write_text(json.dumps(probe_argv) + "\n")
                    probe = observation.run(probe_argv, cap=cap, check=False)
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


def record_save_outputs(run: Path, approved: ApprovedRun) -> JsonObject:
    """Digest every file the server left under its PlayerData tree."""
    player_data = run / "home" / PLAYER_DATA
    files: list[JsonObject] = []
    if player_data.is_dir():
        for path in sorted(player_data.rglob("*")):
            if path.is_symlink() or not path.is_file():
                continue
            files.append({"relative_path": str(path.relative_to(run / "home")),
                          "bytes": path.stat().st_size, "sha256": digest(path)})
    input_path = str(PROSPECTS / approved.save_input.name) if approved.save_input is not None else None
    expected = string_field(approved.session, "save_input_sha256")
    unchanged = any(entry["relative_path"] == input_path and entry["sha256"] == expected for entry in files)
    outputs: JsonObject = {"input_relative_path": input_path, "input_sha256": expected,
                           "input_unchanged": unchanged, "files": list(files)}
    _ = (run / "save-outputs.json").write_text(json.dumps(outputs, indent=2) + "\n")
    return outputs


def observe_and_record(args: Arguments, approved: ApprovedRun, run: Path,
                       name: str, token: str, argv: list[str]) -> JsonObject:
    """Observe, then record save outputs; a recording failure never hides a run failure."""
    try:
        cleanup = observe(args, approved, run, name, token, argv)
    except BaseException as failure:
        if args.save_load:
            try:
                _ = record_save_outputs(run, approved)
            except Exception as recording:
                failure.add_note(f"save outputs not recorded: {recording}")
        raise
    if args.save_load:
        _ = record_save_outputs(run, approved)
    return cleanup


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--case-root", required=True)
    _ = parser.add_argument("--session", required=True)
    _ = parser.add_argument("--run-name", required=True)
    _ = parser.add_argument("--seconds", type=int, default=120)
    _ = parser.add_argument("--startup-probe", choices=["none", "resume"], default="none")
    _ = parser.add_argument("--load-fixture", choices=["none", "missing", "truncated-json"], default="none")
    _ = parser.add_argument("--save-load", action="store_true")
    _ = parser.add_argument("--handshake-probe", action="store_true")
    _ = parser.add_argument("--complete-stateless", action="store_true")
    _ = parser.add_argument("--hello-probe", action="store_true")
    _ = parser.add_argument("--join-probe", action="store_true")
    _ = parser.add_argument("--initialise-probe", action="store_true")
    args = parser.parse_args(namespace=Arguments())
    root = Path(__file__).resolve().parents[1]
    approved = validate_session(root, args, datetime.datetime.now(datetime.UTC))
    if available_memory() < 3 * 1024**3 or shutil.disk_usage(approved.case).free < 30 * 1024**3:
        raise RuntimeError("resource floor reached; defer original execution")
    with reserve_run(approved, args.run_name) as run:
        token = uuid.uuid4().hex
        name = "icarus-offline-" + token
        argv = prepare_launch(root, args, approved, run, name, token)
        revalidate(root, args, approved)
        cleanup = observe_and_record(args, approved, run, name, token, argv)
        print(json.dumps({"run": str(run), **cleanup}))


if __name__ == "__main__":
    main()
