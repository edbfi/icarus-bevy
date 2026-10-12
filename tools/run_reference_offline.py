#!/usr/bin/env python3
"""Bounded original-server execution under the approved overnight contract.

No image pull, external network, identity override, asset extraction or original
modification. Run only after explicit batch approval and case-scope setup.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import signal
import subprocess
import time
import sys
from collections.abc import Sequence
from types import FrameType
from typing import Literal, NotRequired, TypedDict

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.runtime_types import JsonObject, json_object, parse_container_identity, parse_json, string_field

IMAGE = "ghcr.io/pelican-eggs/steamcmd@sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237"


class Arguments(argparse.Namespace):
    run_name: str = ""
    seconds: int = 120
    startup_probe: Literal["none", "resume"] = "none"
    load_fixture: Literal["none", "missing", "truncated-json"] = "none"
    handshake_probe: bool = False
    complete_stateless: bool = False


class FixtureManifest(TypedDict):
    kind: str
    relative_path: str
    exists_at_start: bool
    bytes: NotRequired[int]
    sha256: NotRequired[str]


class RunJournal(TypedDict):
    time: str
    argv: list[str]
    timeout_seconds: int
    approval: JsonObject
    helpers: dict[str, str]
    fixture: NotRequired[FixtureManifest]
    probe_argv: NotRequired[list[str]]


class Cleanup(TypedDict):
    removed: bool
    container_id: NotRequired[str]
    no_container_found: NotRequired[bool]


def command(args: Sequence[str], timeout: float = 30, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout, check=check)


def available_memory() -> int:
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    raise RuntimeError("cannot determine available memory")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--run-name", required=True)
    _ = parser.add_argument("--seconds", type=int, default=120)
    _ = parser.add_argument("--startup-probe", choices=["none", "resume"], default="none")
    _ = parser.add_argument("--load-fixture", choices=["none", "missing", "truncated-json"], default="none")
    _ = parser.add_argument("--handshake-probe", action="store_true")
    _ = parser.add_argument("--complete-stateless", action="store_true")
    args = parser.parse_args(namespace=Arguments())
    if args.complete_stateless and not args.handshake_probe:
        parser.error("--complete-stateless requires --handshake-probe")
    if args.load_fixture != "none" and args.startup_probe != "none":
        parser.error("select only one startup experiment")
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,39}", args.run_name) or not 15 <= args.seconds <= 780:
        parser.error("run-name must be short lowercase alphanumeric/hyphens; seconds must be 15..780 (cleanup reserve)")
    root = Path(__file__).resolve().parents[1]
    case = root / "work/icarus-overnight"
    session = json_object(parse_json((case / "session.json").read_text()))
    end = datetime.datetime.fromisoformat(string_field(session, "end_utc"))
    if datetime.datetime.now(datetime.UTC) + datetime.timedelta(seconds=args.seconds + 120) >= end:
        raise RuntimeError("insufficient time remaining in approved window")
    # Scope guard checks authorization; the helper itself does not grant it.
    _ = command(["bash", str(root / ".agents/vendor/reverse-skill/skills/scripts/case-guard.sh"),
             "--case-root", str(case)])
    if available_memory() < 3 * 1024**3 or shutil.disk_usage(case).free < 30 * 1024**3:
        raise RuntimeError("resource floor reached; defer original execution")
    runs = case / "runtime"
    runs.mkdir(exist_ok=True)
    if len(list(runs.glob("run-*"))) >= 12:
        raise RuntimeError("approved run count exhausted")
    run = runs / ("run-" + args.run_name)
    run.mkdir()  # Never overwrite a prior run.
    home = run / "home"
    home.mkdir()
    fixture_manifest: FixtureManifest | None = None
    if args.load_fixture != "none":
        # Fixed authored name/path, never an arbitrary host save or game data.
        relative = Path("Icarus/Saved/PlayerData/DedicatedServer/Prospects/overnight-authored.json")
        fixture_manifest = {"kind": args.load_fixture, "relative_path": str(relative), "exists_at_start": False}
        if args.load_fixture == "truncated-json":
            payload = b'{"ProspectInfo":{},"ProspectBlob":'
            inputs = run / "inputs"
            inputs.mkdir()
            _ = (inputs / "overnight-authored.json").write_bytes(payload)
            (home / relative).parent.mkdir(parents=True)
            _ = (home / relative).write_bytes(payload)
            fixture_manifest.update(exists_at_start=True, bytes=len(payload), sha256=hashlib.sha256(payload).hexdigest())
        _ = (run / "fixture.json").write_text(json.dumps(fixture_manifest, indent=2) + "\n")
    name = "icarus-offline-" + args.run_name + "-" + str(os.getpid())
    label = "icarus.research.run=" + name
    launch = ('mkdir -p "$STEAM_COMPAT_DATA_PATH"; exec xvfb-run -a proton run '
              '/reference/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe '
              '-UserDir="Z:/home/container/Icarus" -Log '
              '-SteamServerName="icarus-research-offline" -PORT=17777 -QueryPort=27015')
    if args.startup_probe == "resume":
        launch += ' -ExecCmds="ResumeProspect"'
    if args.load_fixture != "none":
        launch += ' -LoadProspect=overnight-authored'
    argv = ["docker", "run", "--detach", "--pull", "never", "--name", name,
            "--label", label, "--cidfile", str(run / "container.cid"),
            "--network", "none", "--read-only", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--memory", "6g", "--cpus", "2",
            "--pids-limit", "512", "--shm-size", "256m",
            "--tmpfs", "/tmp:rw,exec,nosuid,size=1g", "--tmpfs", "/run:rw,nosuid,size=16m",
            "--mount", f"type=bind,src={root / 'icarus-orig'},dst=/reference,readonly",
            "--mount", f"type=bind,src={home},dst=/home/container", "--workdir", "/reference",
            "--env", "SteamAppId=1149460", "--env", "SteamGameId=1149460",
            "--env", "STEAM_COMPAT_DATA_PATH=/home/container/proton-prefix",
            "--env", "STEAM_COMPAT_CLIENT_INSTALL_PATH=/home/container",
            IMAGE, "/bin/bash", "-lc", launch]
    journal: RunJournal = {"time": datetime.datetime.now(datetime.UTC).isoformat(), "argv": argv,
               "timeout_seconds": args.seconds + 120, "approval": session,
               "helpers": {"tools/run_reference_offline.py": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                           "tools/runtime_types.py": hashlib.sha256((root / "tools/runtime_types.py").read_bytes()).hexdigest()}}
    if fixture_manifest:
        journal["fixture"] = fixture_manifest
    probe_argv: list[str] | None = None
    if args.handshake_probe:
        if args.seconds < 60:
            raise RuntimeError("handshake probe requires at least 60 observation seconds")
        probe = case / "probes/handshake_probe"
        if not probe.is_file() or probe.resolve() != probe:
            raise RuntimeError("missing or redirected case-local probe binary")
        argv[2:2] = ["--mount", f"type=bind,src={probe.parent},dst=/probes,readonly"]
        journal["helpers"][str(probe.relative_to(root))] = hashlib.sha256(probe.read_bytes()).hexdigest()
        probe_argv = ["docker", "exec", "--env", "ICARUS_OFFLINE_PROBE=1", name, "/probes/handshake_probe"]
        if args.complete_stateless:
            probe_argv.append("--complete-stateless")
        journal["probe_argv"] = probe_argv
    with (case / "commands.jsonl").open("a") as stream:
        _ = stream.write(json.dumps(journal) + "\n")
    _ = (run / "invocation.json").write_text(json.dumps(journal, indent=2) + "\n")
    container = None
    cleanup: Cleanup = {"removed": False}
    # SIGTERM must unwind through finally rather than abandon a detached container.
    def interrupted(_signum: int, _frame: FrameType | None) -> None:
        raise KeyboardInterrupt("reference observation interrupted")
    _ = signal.signal(signal.SIGTERM, interrupted)
    start = time.monotonic()
    try:
        result = command(argv, timeout=45)
        container = result.stdout.strip()
        if not re.fullmatch(r"[0-9a-f]{64}", container):
            raise RuntimeError("unexpected container ID")
        _ = (run / "container-start.json").write_text(command(["docker", "inspect", container]).stdout)
        with (run / "resource-samples.jsonl").open("x") as samples:
            probed = False
            while time.monotonic() - start < args.seconds:
                if available_memory() < 3 * 1024**3 or shutil.disk_usage(case).free < 30 * 1024**3:
                    raise RuntimeError("resource floor reached during observation")
                state = command(["docker", "inspect", "--format", "{{.State.Running}}", container]).stdout.strip()
                if state != "true":
                    break
                if probe_argv is not None and not probed and time.monotonic() - start >= 40:
                    probe_result = command(probe_argv, timeout=15, check=False)
                    _ = (run / "probe.jsonl").write_text(probe_result.stdout)
                    _ = (run / "probe.stderr").write_text(probe_result.stderr)
                    _ = (run / "probe-exit.txt").write_text(str(probe_result.returncode) + "\n")
                    probed = True
                _ = samples.write(command(["docker", "stats", "--no-stream", "--format", "{{json .}}", container]).stdout)
                samples.flush()
                time.sleep(min(5, max(0, args.seconds - (time.monotonic() - start))))
        _ = (run / "observed-seconds.txt").write_text(str(time.monotonic() - start) + "\n")
    finally:
        # Resolve only our exact unique name if docker run failed after creation.
        inspected = command(["docker", "inspect", name], check=False)
        if inspected.returncode == 0:
            info = parse_container_identity(inspected.stdout)
            if info.labels.get("icarus.research.run") != name:
                raise RuntimeError("cleanup refused: container ownership label mismatch")
            container = info.container_id
            _ = (run / "container-before-stop.json").write_text(inspected.stdout)
            try:
                for phase in ["before", "after"]:
                    try:
                        logs = command(["docker", "logs", container], check=False)
                        _ = (run / f"console-{phase}.log").write_text(logs.stdout + logs.stderr)
                    finally:
                        if phase == "before":
                            _ = command(["docker", "stop", "--time", "10", container], timeout=25, check=False)
                _ = (run / "container-after-stop.json").write_text(command(["docker", "inspect", container]).stdout)
            finally:
                # rm -f targets only this exact labeled container if capture/stop failed.
                _ = command(["docker", "rm", "--force", container])
                remaining = command(["docker", "ps", "--all", "--filter", f"id={container}", "--format", "{{.ID}}"])
                cleanup["removed"] = not remaining.stdout.strip()
            cleanup["container_id"] = container
        else:
            cleanup["no_container_found"] = True
        _ = (run / "cleanup.json").write_text(json.dumps(cleanup, indent=2) + "\n")
    print(json.dumps({"run": str(run), **cleanup}))


if __name__ == "__main__":
    main()
