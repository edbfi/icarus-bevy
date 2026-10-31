#!/usr/bin/env python3
"""Bounded connected reference run: a matched official server on this host that
the user's stock client joins over the internet (connected-scenarios route B).

The server runs in the pinned Proton image on a bridged network with only the
game and query UDP ports published, behind a generated join password. It loads
one case-local save copy, and the run ends once the server has saved and
returned to its lobby after the last player left, or at the session's deadline.
Container ownership, budgets and cleanup are the offline launcher's.

A session may bind a local capture image (`capture_image`, an image ID built
from `tools/capture.Dockerfile`) and a byte cap (`capture_max_bytes`). A
sidecar then shares only the server container's network namespace, records
the game and query UDP ports with tcpdump as an unprivileged user, and is
stopped before the server; a capture reaching its cap is stopped early.

`connection_method: "direct"` writes an Engine.ini turning off
`bUseSteamNetworking` and `bAllowP2PPacketRelay`, with which the server
advertises `CONMETHOD=Direct` and clients join over the game UDP port instead
of Steam P2P; without it the server keeps its packaged configuration.
"""

import argparse
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import stat
import sys
import time
from types import FrameType
import uuid

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.run_reference_offline import (IMAGE, PLAYER_DATA, PROSPECT_FILE, PROSPECTS, REFERENCE_EXE,
    SERVER_SETTINGS, Budget, available_memory, cleanup_container, digest, owned_container, regular_path,
    unredirected)
from tools.runtime_types import JsonObject, integer_field, json_object, parse_container_state, parse_json, string_field

GAME_PORT = 17777
QUERY_PORT = 27015
MAX_OBSERVATION_SECONDS = 2700
CLEANUP_SECONDS = 120
CAPTURE_IMAGE = re.compile(r"sha256:[0-9a-f]{64}")
MIN_CAPTURE_BYTES = 1024**2
MAX_CAPTURE_BYTES = 2 * 1024**3
CAPTURE_FILE = "game.pcap"
CAPTURE_FILTER = f"udp port {GAME_PORT} or udp port {QUERY_PORT}"
CAPTURE_START_SECONDS = 15
ENGINE_INI = Path("Icarus/Saved/Config/WindowsServer/Engine.ini")
DIRECT_ENGINE_INI = "[OnlineSubsystemSteam]\nbUseSteamNetworking=False\nbAllowP2PPacketRelay=False\n"
SCENARIO_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,23}")
SERVER_LOG = Path("Icarus/Saved/Logs/Icarus.log")
PROSPECT_LOADED = "Prospect file loaded"
SAVING = "Saving and shutting down"
LOBBY_LOADED = "LoadMap(/Game/Maps/DedicatedServerEntry)"
# Proton's Steam bridge loads the Linux client library from $HOME/.steam/sdk*,
# where the user's Pelican egg installer copies SteamCMD's copies.
STEAMCLIENT_ROOT = Path.home() / ".local/share/steamcmd"


class Arguments(argparse.Namespace):
    case_root: str = ""
    session: str = ""
    run_name: str = ""


def validate_connected_scope(scope: Path) -> None:
    required = {("auth", "status"): "granted", ("network_profile", "mode"): "connected",
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


@dataclass(frozen=True)
class ApprovedRun:
    case: Path
    session: JsonObject
    session_sha256: str
    reference: Path
    save_input: Path
    scenario: str
    not_joined_seconds: int
    empty_seconds: int
    max_players: int
    observation_seconds: int
    lifetime_seconds: int
    max_runs: int
    capture_image: str | None = None
    capture_max_bytes: int = 0
    direct: bool = False


def validate_session(root: Path, args: Arguments, now: datetime.datetime) -> ApprovedRun:
    case = unredirected(Path(args.case_root))
    if not case.is_dir() or not case.is_relative_to(root / "work") or case == root / "work":
        raise ValueError("case must be an existing directory below work")
    session_path = regular_path(Path(args.session))
    scope = regular_path(case / "scope.md")
    if not session_path.is_relative_to(case):
        raise ValueError("session must be case-local")
    with session_path.open("rb") as stream:
        raw = stream.read(65537)
    if len(raw) > 65536:
        raise ValueError("session exceeds 64 KiB")
    session = json_object(parse_json(raw))
    if string_field(session, "scope") != "reference-connected":
        raise ValueError("session does not authorize reference-connected")
    if not string_field(session, "authorization_source").strip():
        raise ValueError("authorization_source must identify the current instruction")
    if string_field(session, "case_root") != str(case) or string_field(session, "image") != IMAGE:
        raise ValueError("session case/image binding mismatch")
    reference = unredirected(Path(string_field(session, "reference_root")))
    if reference.parent != root or re.fullmatch(r"icarus-server-[0-9]+", reference.name) is None:
        raise ValueError("reference_root must be a fetched icarus-server-<buildid> install")
    executable = regular_path(reference / REFERENCE_EXE)
    if string_field(session, "reference_executable") != str(executable):
        raise ValueError("session reference binding mismatch")
    save_input = regular_path(Path(string_field(session, "save_input")))
    if not save_input.is_relative_to(case) or PROSPECT_FILE.fullmatch(save_input.name) is None:
        raise ValueError("save input must be a case-local <prospect>.json file")
    for field, path in (("scope_sha256", scope), ("reference_sha256", executable), ("save_input_sha256", save_input)):
        expected = string_field(session, field)
        if re.fullmatch(r"[0-9a-f]{64}", expected) is None or expected != digest(path):
            raise ValueError(f"{field} mismatch")
    validate_connected_scope(scope)
    scenario = string_field(session, "scenario_id")
    if SCENARIO_ID.fullmatch(scenario) is None:
        raise ValueError("scenario_id must be short lowercase alphanumeric/hyphens")
    not_joined = integer_field(session, "shutdown_if_not_joined_seconds")
    empty = integer_field(session, "shutdown_if_empty_seconds")
    players = integer_field(session, "max_players")
    observation = integer_field(session, "observation_seconds")
    lifetime = integer_field(session, "run_timeout_seconds")
    max_runs = integer_field(session, "max_runs")
    if not 60 <= observation <= MAX_OBSERVATION_SECONDS:
        raise ValueError(f"observation must be 60..{MAX_OBSERVATION_SECONDS} seconds")
    if not observation + CLEANUP_SECONDS <= lifetime <= MAX_OBSERVATION_SECONDS + CLEANUP_SECONDS:
        raise ValueError("lifetime must reserve 120 cleanup seconds within the connected bound")
    if not 30 <= not_joined <= observation or not 0 <= empty <= 600 or not 1 <= players <= 8:
        raise ValueError("server timers or player cap out of bounds")
    if not 1 <= max_runs <= 4:
        raise ValueError("max_runs must be 1..4")
    capture_image: str | None = None
    capture_max_bytes = 0
    if ("capture_image" in session) != ("capture_max_bytes" in session):
        raise ValueError("capture_image and capture_max_bytes are bound together")
    if "capture_image" in session:
        capture_image = string_field(session, "capture_image")
        capture_max_bytes = integer_field(session, "capture_max_bytes")
        if CAPTURE_IMAGE.fullmatch(capture_image) is None:
            raise ValueError("capture_image must be a local image ID (sha256:<64 hex>)")
        if not MIN_CAPTURE_BYTES <= capture_max_bytes <= MAX_CAPTURE_BYTES:
            raise ValueError("capture_max_bytes must be 1 MiB..2 GiB")
    direct = False
    if "connection_method" in session:
        method = string_field(session, "connection_method")
        if method not in ("default", "direct"):
            raise ValueError("connection_method must be default or direct")
        direct = method == "direct"
    start = datetime.datetime.fromisoformat(string_field(session, "start_utc"))
    end = datetime.datetime.fromisoformat(string_field(session, "end_utc"))
    if start.utcoffset() is None or end.utcoffset() is None:
        raise ValueError("session timestamps must include timezone offsets")
    if not start <= now < end or now + datetime.timedelta(seconds=lifetime) >= end:
        raise ValueError("session is future, expired or has insufficient approved time")
    return ApprovedRun(case, session, hashlib.sha256(raw).hexdigest(), reference, save_input, scenario,
                       not_joined, empty, players, observation, lifetime, max_runs, capture_image,
                       capture_max_bytes, direct)


@contextmanager
def reserve_run(approved: ApprovedRun, run_name: str) -> Generator[Path]:
    if re.fullmatch(r"[a-z0-9][a-z0-9-]{0,39}", run_name) is None:
        raise ValueError("run-name must be short lowercase alphanumeric/hyphens")
    descriptor = os.open(approved.case / "reference-connected.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError("case lock must be a regular file")
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        runs = unredirected(approved.case / "runtime")
        runs.mkdir(exist_ok=True)
        if sum(1 for _ in runs.glob("run-*")) >= approved.max_runs:
            raise RuntimeError("approved run count exhausted")
        run = runs / ("run-" + run_name)
        run.mkdir()
        yield run
    finally:
        os.close(descriptor)


def server_settings(approved: ApprovedRun, password: str) -> str:
    return ("[/Script/Icarus.DedicatedServerSettings]\n"
            + f"JoinPassword={password}\nMaxPlayers={approved.max_players}\nResumeProspect=False\n"
            + f"ShutdownIfNotJoinedFor={approved.not_joined_seconds}\n"
            + f"ShutdownIfEmptyFor={approved.empty_seconds}\n"
            + "AllowNonAdminsToLaunchProspects=False\nAllowNonAdminsToDeleteProspects=False\n")


def prepare_launch(root: Path, approved: ApprovedRun, run: Path, name: str, token: str,
                   password: str) -> list[str]:
    home = run / "home"
    relative = PROSPECTS / approved.save_input.name
    (home / relative).parent.mkdir(parents=True)
    _ = shutil.copyfile(approved.save_input, home / relative)
    if digest(home / relative) != string_field(approved.session, "save_input_sha256"):
        raise ValueError("save copy digest mismatch")
    (home / SERVER_SETTINGS).parent.mkdir(parents=True)
    _ = (home / SERVER_SETTINGS).write_text(server_settings(approved, password))
    engine: str | None = None
    if approved.direct:
        _ = (home / ENGINE_INI).write_text(DIRECT_ENGINE_INI)
        engine = digest(home / ENGINE_INI)
    steam: JsonObject = {}
    for bits in ("32", "64"):
        source = regular_path(STEAMCLIENT_ROOT / f"linux{bits}/steamclient.so")
        target = home / f".steam/sdk{bits}/steamclient.so"
        target.parent.mkdir(parents=True)
        _ = shutil.copyfile(source, target)
        steam[f"sdk{bits}"] = {"source": str(source), "sha256": digest(target)}
    secret = run / "join-password.txt"
    _ = secret.write_text(password + "\n")
    secret.chmod(0o600)
    server_name = "icarus-research-" + approved.scenario
    launch = ('mkdir -p "$STEAM_COMPAT_DATA_PATH"; exec xvfb-run -a proton run '
              '/reference/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe '
              '-UserDir="Z:/home/container/Icarus" -Log '
              f'-SteamServerName="{server_name}" -PORT={GAME_PORT} -QueryPort={QUERY_PORT} '
              f'-LoadProspect={approved.save_input.stem}')
    argv = ["docker", "run", "--detach", "--pull", "never", "--name", name,
            "--label", "icarus.research.run=" + name,
            "--label", "icarus.research.owner=" + token, "--cidfile", str(run / "container.cid"),
            "--network", "bridge",
            "--publish", f"{GAME_PORT}:{GAME_PORT}/udp", "--publish", f"{QUERY_PORT}:{QUERY_PORT}/udp",
            "--read-only", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--memory", "6g", "--cpus", "2",
            "--pids-limit", "512", "--shm-size", "256m",
            "--tmpfs", "/tmp:rw,exec,nosuid,size=1g", "--tmpfs", "/run:rw,nosuid,size=16m",
            "--mount", f"type=bind,src={approved.reference},dst=/reference,readonly",
            "--mount", f"type=bind,src={home},dst=/home/container", "--workdir", "/reference",
            "--env", "SteamAppId=1149460", "--env", "SteamGameId=1149460",
            "--env", "STEAM_COMPAT_DATA_PATH=/home/container/proton-prefix",
            "--env", "STEAM_COMPAT_CLIENT_INSTALL_PATH=/home/container",
            IMAGE, "/bin/bash", "-lc", launch]
    helpers = [Path(__file__), root / "tools/run_reference_offline.py", root / "tools/runtime_types.py"]
    record: JsonObject = {
        "time": datetime.datetime.now(datetime.UTC).isoformat(), "argv": list(argv),
        "server_name": server_name, "join_password_sha256": hashlib.sha256(password.encode()).hexdigest(),
        "timeout_seconds": approved.lifetime_seconds, "observation_seconds": approved.observation_seconds,
        "approval": approved.session, "session_sha256": approved.session_sha256,
        "save_copy_sha256": digest(home / relative), "steamclient": steam, "server_settings_sha256": digest(home / SERVER_SETTINGS),
        "engine_ini_sha256": engine,
        "helpers": {str(path.relative_to(root)): digest(path) for path in helpers},
    }
    with (approved.case / "commands.jsonl").open("a") as stream:
        _ = stream.write(json.dumps(record) + "\n")
    _ = (run / "invocation.json").write_text(json.dumps(record, indent=2) + "\n")
    return argv


def capture_launch(approved: ApprovedRun, run: Path, name: str, token: str, server: str) -> list[str]:
    """The sidecar joins only the server's network namespace; tcpdump opens the
    interface as root with NET_RAW, then drops to the image's uid 1000 user
    before writing the capture into the run's own directory."""
    if approved.capture_image is None or re.fullmatch(r"[0-9a-f]{64}", server) is None:
        raise ValueError("capture needs a bound image and the verified server ID")
    capture = run / "capture"
    capture.mkdir()
    # tcpdump writes as the image's uid 1000, whichever host user launches.
    capture.chmod(0o777)
    return ["docker", "run", "--detach", "--pull", "never", "--name", name,
            "--label", "icarus.research.run=" + name, "--label", "icarus.research.owner=" + token,
            "--cidfile", str(run / "capture-container.cid"), "--network", "container:" + server,
            "--read-only", "--cap-drop", "ALL", "--cap-add", "NET_RAW", "--cap-add", "SETUID",
            "--cap-add", "SETGID", "--security-opt", "no-new-privileges", "--memory", "256m",
            "--cpus", "0.5", "--pids-limit", "32",
            "--mount", f"type=bind,src={capture},dst=/capture",
            approved.capture_image, "tcpdump", "-i", "eth0", "-p", "-n", "-U", "-s", "0",
            "-Z", "capture", "-w", "/capture/" + CAPTURE_FILE, CAPTURE_FILTER]


def capture_running(budget: Budget, name: str, token: str, container: str) -> bool:
    inspected = budget.run(["docker", "inspect", name])
    _ = owned_container(inspected.stdout, name, token, container)
    return parse_container_state(inspected.stdout).running


def returned_to_lobby(log: str) -> bool:
    """The prospect loaded, then the server saved and loaded its lobby map."""
    loaded = log.find(PROSPECT_LOADED)
    saving = log.find(SAVING, loaded) if loaded >= 0 else -1
    return saving >= 0 and log.find(LOBBY_LOADED, saving) >= 0


def observe(approved: ApprovedRun, run: Path, name: str, token: str, argv: list[str]) -> JsonObject:
    start = time.monotonic()
    now = datetime.datetime.now(datetime.UTC)
    end = datetime.datetime.fromisoformat(string_field(approved.session, "end_utc"))
    remaining = (end - now).total_seconds()
    if remaining <= approved.observation_seconds + CLEANUP_SECONDS:
        raise ValueError("session has insufficient approved time at launch")
    budget = Budget(start + min(approved.lifetime_seconds, remaining))
    observation = Budget(start + approved.observation_seconds)

    def interrupted(_signum: int, _frame: FrameType | None) -> None:
        raise KeyboardInterrupt("connected observation interrupted")

    previous = signal.signal(signal.SIGTERM, interrupted)
    previous_int = signal.signal(signal.SIGINT, interrupted)
    container: str | None = None
    capture_name = name + "-capture"
    capture_id: str | None = None
    capture_started = False
    capture: JsonObject = {}
    ended = "deadline"
    try:
        result = observation.run(argv, cap=45)
        candidate = result.stdout.strip()
        if re.fullmatch(r"[0-9a-f]{64}", candidate) is None:
            raise RuntimeError("unexpected container ID")
        container = candidate
        if approved.capture_image is not None:
            capture_argv = capture_launch(approved, run, capture_name, token, container)
            with (approved.case / "commands.jsonl").open("a") as stream:
                _ = stream.write(json.dumps({"time": datetime.datetime.now(datetime.UTC).isoformat(),
                                             "capture_argv": capture_argv}) + "\n")
            capture_started = True
            started = observation.run(capture_argv, cap=30).stdout.strip()
            if re.fullmatch(r"[0-9a-f]{64}", started) is None:
                raise RuntimeError("unexpected capture container ID")
            capture_id = started
            pcap = run / "capture" / CAPTURE_FILE
            started_by = time.monotonic() + CAPTURE_START_SECONDS
            while not pcap.is_file():
                if not capture_running(observation, capture_name, token, capture_id) or time.monotonic() > started_by:
                    raise RuntimeError("capture sidecar did not start writing its capture")
                time.sleep(1)
        log = run / "home" / SERVER_LOG
        with (run / "resource-samples.jsonl").open("x") as samples:
            while time.monotonic() < observation.deadline:
                if available_memory() < 3 * 1024**3 or shutil.disk_usage(approved.case).free < 30 * 1024**3:
                    raise RuntimeError("resource floor reached during observation")
                inspected = observation.run(["docker", "inspect", container])
                _ = owned_container(inspected.stdout, name, token, container)
                pcap = run / "capture" / CAPTURE_FILE
                if capture_id is not None and not capture_running(observation, capture_name, token, capture_id):
                    raise RuntimeError("capture sidecar stopped during the run")
                if (capture_id is not None and "truncated" not in capture and pcap.is_file()
                        and pcap.stat().st_size >= approved.capture_max_bytes):
                    capture.update(cleanup_container(run, capture_name, token, capture_id, observation,
                                                     label="capture-"))
                    capture["truncated"] = True
                    capture_id = None
                if not parse_container_state(inspected.stdout).running:
                    ended = "exited"
                    break
                if log.is_file() and returned_to_lobby(log.read_text(errors="replace")):
                    ended = "lobby"
                    break
                _ = samples.write(observation.run(["docker", "stats", "--no-stream", "--format",
                                                   "{{json .}}", container]).stdout)
                samples.flush()
                time.sleep(min(5, max(0, observation.deadline - time.monotonic())))
        _ = (run / "observed.json").write_text(json.dumps({"seconds": time.monotonic() - start, "ended": ended}) + "\n")
    finally:
        try:
            try:
                # The sidecar shares the server's namespace, so it stops first.
                if capture_started and "truncated" not in capture:
                    capture.update(cleanup_container(run, capture_name, token, capture_id, budget,
                                                     label="capture-"))
            finally:
                cleanup = cleanup_container(run, name, token, container, budget)
        finally:
            _ = signal.signal(signal.SIGTERM, previous)
            _ = signal.signal(signal.SIGINT, previous_int)
    cleanup["ended"] = ended
    if capture_started:
        pcap = run / "capture" / CAPTURE_FILE
        capture["file"] = str(pcap.relative_to(run)) if pcap.is_file() else None
        capture["bytes"] = pcap.stat().st_size if pcap.is_file() else 0
        capture["sha256"] = digest(pcap) if pcap.is_file() else None
        cleanup["capture"] = capture
    return cleanup


def record_outputs(run: Path, approved: ApprovedRun) -> JsonObject:
    player_data = run / "home" / PLAYER_DATA
    files: list[JsonObject] = []
    if player_data.is_dir():
        for path in sorted(player_data.rglob("*")):
            if path.is_file() and not path.is_symlink():
                files.append({"relative_path": str(path.relative_to(run / "home")),
                              "bytes": path.stat().st_size, "sha256": digest(path)})
    outputs: JsonObject = {"input_sha256": string_field(approved.session, "save_input_sha256"), "files": list(files)}
    _ = (run / "save-outputs.json").write_text(json.dumps(outputs, indent=2) + "\n")
    return outputs


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--case-root", required=True)
    _ = parser.add_argument("--session", required=True)
    _ = parser.add_argument("--run-name", required=True)
    args = parser.parse_args(namespace=Arguments())
    root = Path(__file__).resolve().parents[1]
    approved = validate_session(root, args, datetime.datetime.now(datetime.UTC))
    if available_memory() < 3 * 1024**3 or shutil.disk_usage(approved.case).free < 30 * 1024**3:
        raise RuntimeError("resource floor reached; defer the run")
    with reserve_run(approved, args.run_name) as run:
        token = uuid.uuid4().hex
        name = "icarus-connected-" + token
        password = secrets.token_hex(4)
        argv = prepare_launch(root, approved, run, name, token, password)
        if validate_session(root, args, datetime.datetime.now(datetime.UTC)) != approved:
            raise RuntimeError("session changed during preparation")
        print(json.dumps({"run": str(run), "server_name": "icarus-research-" + approved.scenario,
                          "join_password": password}), flush=True)
        try:
            cleanup = observe(approved, run, name, token, argv)
        finally:
            _ = record_outputs(run, approved)
        print(json.dumps({"run": str(run), **cleanup}))


if __name__ == "__main__":
    main()
