#!/usr/bin/env python3
"""Run the authored ENOSPC/EACCES probe in a disposable 256 KiB container tmpfs.

Requires the approved overnight case. No original execution, downloads, network
traffic or host filesystem exhaustion. The existing image only supplies Linux.
"""
import datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import time

from run_reference_offline import IMAGE, available_memory, command


def main():
    root = Path(__file__).resolve().parents[1]
    case = root / "work/icarus-overnight"
    session = json.loads((case / "session.json").read_text())
    if datetime.datetime.now(datetime.UTC) + datetime.timedelta(seconds=180) >= datetime.datetime.fromisoformat(session["end_utc"]):
        raise RuntimeError("insufficient approved time remaining")
    command(["bash", str(root / ".agents/vendor/reverse-skill/skills/scripts/case-guard.sh"), "--case-root", str(case)])
    if available_memory() < 3 * 1024**3 or shutil.disk_usage(case).free < 30 * 1024**3:
        raise RuntimeError("resource floor reached")
    run = case / "native-io-faults"
    run.mkdir()  # Preserve prior evidence; do not silently rerun.
    home = run / "home"
    home.mkdir()
    probe = case / "probes/io_fault_probe"
    if not probe.is_file() or probe.resolve() != probe:
        raise RuntimeError("missing or redirected reviewed native probe")
    name = "icarus-native-io-faults-" + str(os.getpid())
    label = "icarus.research.run=" + name
    argv = ["docker", "run", "--detach", "--pull", "never", "--name", name,
            "--label", label, "--network", "none", "--read-only", "--cap-drop", "ALL",
            "--security-opt", "no-new-privileges", "--memory", "6g", "--cpus", "2",
            "--pids-limit", "512", "--shm-size", "256m",
            "--tmpfs", "/tmp:rw,noexec,nosuid,size=256k", "--tmpfs", "/run:rw,nosuid,size=16m",
            "--mount", f"type=bind,src={probe.parent},dst=/probes,readonly",
            "--mount", f"type=bind,src={home},dst=/home/container",
            "--workdir", "/tmp", "--env", "ICARUS_BOUNDED_TMPFS_BYTES=262144",
            IMAGE, "/probes/io_fault_probe"]
    helpers = [Path(__file__), root / "tools/run_reference_offline.py", probe,
               root / "examples/io_fault_probe.rs", root / "src/persistence.rs", root / "Cargo.lock"]
    record = {"time": datetime.datetime.now(datetime.UTC).isoformat(), "argv": argv,
              "timeout_seconds": 120, "approval": session,
              "helpers": {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in helpers}}
    with (case / "commands.jsonl").open("a") as stream:
        stream.write(json.dumps(record) + "\n")
    (run / "invocation.json").write_text(json.dumps(record, indent=2) + "\n")

    def interrupted(_signum, _frame):
        raise KeyboardInterrupt("native I/O probe interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    cleanup = {"removed": False}
    outcome = None
    try:
        command(argv, timeout=45)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            info = json.loads(command(["docker", "inspect", name]).stdout)[0]
            if not info["State"]["Running"]:
                outcome = info["State"]["ExitCode"]
                break
            time.sleep(1)
        if outcome is None:
            raise RuntimeError("native probe exceeded 30-second observation deadline")
    finally:
        inspected = command(["docker", "inspect", name], check=False)
        if inspected.returncode == 0:
            info = json.loads(inspected.stdout)[0]
            if info["Config"]["Labels"].get("icarus.research.run") != name:
                raise RuntimeError("container ownership mismatch")
            container = info["Id"]
            try:
                (run / "container.json").write_text(inspected.stdout)
                logs = command(["docker", "logs", container], check=False)
                (run / "probe.jsonl").write_text(logs.stdout)
                (run / "probe.stderr").write_text(logs.stderr)
            finally:
                command(["docker", "rm", "--force", container])
                remaining = command(["docker", "ps", "--all", "--filter", f"id={container}", "--format", "{{.ID}}"])
                cleanup.update(container_id=container, removed=not remaining.stdout.strip())
        else:
            cleanup["no_container_found"] = True
        (run / "cleanup.json").write_text(json.dumps(cleanup, indent=2) + "\n")
    if outcome != 0 or not cleanup["removed"]:
        raise RuntimeError(f"native probe or cleanup failed: exit={outcome}, cleanup={cleanup}")
    print((run / "probe.jsonl").read_text())
    print(json.dumps(cleanup))


if __name__ == "__main__":
    main()
