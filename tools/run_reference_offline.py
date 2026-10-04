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

IMAGE = "ghcr.io/pelican-eggs/steamcmd@sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237"


def command(args, timeout=30, check=True):
    return subprocess.run(args, capture_output=True, text=True, timeout=timeout, check=check)


def available_memory():
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    raise RuntimeError("cannot determine available memory")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-name", required=True)
    parser.add_argument("--seconds", type=int, default=120)
    parser.add_argument("--startup-probe", choices=["none", "resume"], default="none")
    parser.add_argument("--handshake-probe", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,39}", args.run_name) or not 15 <= args.seconds <= 780:
        parser.error("run-name must be short lowercase alphanumeric/hyphens; seconds must be 15..780 (cleanup reserve)")
    root = Path(__file__).resolve().parents[1]
    case = root / "work/icarus-overnight"
    session = json.loads((case / "session.json").read_text())
    end = datetime.datetime.fromisoformat(session["end_utc"])
    if datetime.datetime.now(datetime.UTC) + datetime.timedelta(seconds=args.seconds + 120) >= end:
        raise RuntimeError("insufficient time remaining in approved window")
    # Scope guard checks authorization; the helper itself does not grant it.
    command(["bash", str(root / ".agents/vendor/reverse-skill/skills/scripts/case-guard.sh"),
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
    name = "icarus-offline-" + args.run_name + "-" + str(os.getpid())
    label = "icarus.research.run=" + name
    launch = ('mkdir -p "$STEAM_COMPAT_DATA_PATH"; exec xvfb-run -a proton run '
              '/reference/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe '
              '-UserDir="Z:/home/container/Icarus" -Log '
              '-SteamServerName="icarus-research-offline" -PORT=17777 -QueryPort=27015')
    if args.startup_probe == "resume":
        launch += ' -ExecCmds="ResumeProspect"'
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
    journal = {"time": datetime.datetime.now(datetime.UTC).isoformat(), "argv": argv,
               "timeout_seconds": args.seconds + 120, "approval": session,
               "helpers": {"tools/run_reference_offline.py": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}}
    if args.handshake_probe:
        if args.seconds < 60:
            raise RuntimeError("handshake probe requires at least 60 observation seconds")
        probe = case / "probes/handshake_probe"
        if not probe.is_file() or probe.resolve() != probe:
            raise RuntimeError("missing or redirected case-local probe binary")
        argv[2:2] = ["--mount", f"type=bind,src={probe.parent},dst=/probes,readonly"]
        journal["helpers"][str(probe.relative_to(root))] = hashlib.sha256(probe.read_bytes()).hexdigest()
        journal["probe_argv"] = ["docker", "exec", "--env", "ICARUS_OFFLINE_PROBE=1", name, "/probes/handshake_probe"]
    with (case / "commands.jsonl").open("a") as stream:
        stream.write(json.dumps(journal) + "\n")
    (run / "invocation.json").write_text(json.dumps(journal, indent=2) + "\n")
    container = None
    cleanup = {"removed": False}
    # SIGTERM must unwind through finally rather than abandon a detached container.
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt("reference observation interrupted")
    signal.signal(signal.SIGTERM, interrupted)
    start = time.monotonic()
    try:
        result = command(argv, timeout=45)
        container = result.stdout.strip()
        if not re.fullmatch(r"[0-9a-f]{64}", container):
            raise RuntimeError("unexpected container ID")
        (run / "container-start.json").write_text(command(["docker", "inspect", container]).stdout)
        with (run / "resource-samples.jsonl").open("x") as samples:
            probed = False
            while time.monotonic() - start < args.seconds:
                if available_memory() < 3 * 1024**3 or shutil.disk_usage(case).free < 30 * 1024**3:
                    raise RuntimeError("resource floor reached during observation")
                state = command(["docker", "inspect", "--format", "{{.State.Running}}", container]).stdout.strip()
                if state != "true":
                    break
                if args.handshake_probe and not probed and time.monotonic() - start >= 40:
                    probe_result = command(journal["probe_argv"], timeout=15, check=False)
                    (run / "probe.jsonl").write_text(probe_result.stdout)
                    (run / "probe.stderr").write_text(probe_result.stderr)
                    (run / "probe-exit.txt").write_text(str(probe_result.returncode) + "\n")
                    probed = True
                samples.write(command(["docker", "stats", "--no-stream", "--format", "{{json .}}", container]).stdout)
                samples.flush()
                time.sleep(min(5, max(0, args.seconds - (time.monotonic() - start))))
        (run / "observed-seconds.txt").write_text(str(time.monotonic() - start) + "\n")
    finally:
        # Resolve only our exact unique name if docker run failed after creation.
        inspected = command(["docker", "inspect", name], check=False)
        if inspected.returncode == 0:
            info = json.loads(inspected.stdout)[0]
            if info["Config"]["Labels"].get("icarus.research.run") != name:
                raise RuntimeError("cleanup refused: container ownership label mismatch")
            container = info["Id"]
            (run / "container-before-stop.json").write_text(inspected.stdout)
            try:
                for phase in ["before", "after"]:
                    try:
                        logs = command(["docker", "logs", container], check=False)
                        (run / f"console-{phase}.log").write_text(logs.stdout + logs.stderr)
                    finally:
                        if phase == "before":
                            command(["docker", "stop", "--time", "10", container], timeout=25, check=False)
                (run / "container-after-stop.json").write_text(command(["docker", "inspect", container]).stdout)
            finally:
                # rm -f targets only this exact labeled container if capture/stop failed.
                command(["docker", "rm", "--force", container])
                remaining = command(["docker", "ps", "--all", "--filter", f"id={container}", "--format", "{{.ID}}"])
                cleanup["removed"] = not remaining.stdout.strip()
            cleanup["container_id"] = container
        else:
            cleanup["no_container_found"] = True
        (run / "cleanup.json").write_text(json.dumps(cleanup, indent=2) + "\n")
    print(json.dumps({"run": str(run), **cleanup}))


if __name__ == "__main__":
    main()
