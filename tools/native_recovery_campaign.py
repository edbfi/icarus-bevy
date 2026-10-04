#!/usr/bin/env python3
"""Bounded authored start/save/stop/kill/recovery campaign; no original files."""

import argparse
import hashlib
import json
from pathlib import Path
import queue
import signal
import subprocess
import threading
import time


def read_state(directory):
    manifest = json.loads((directory / "manifest.json").read_text())
    current = manifest["current"]
    data = (directory / f"snapshot-{current['number']}.json").read_bytes()
    if hashlib.sha256(data).hexdigest() != current["sha256"]:
        raise RuntimeError("published generation checksum mismatch")
    return current["number"], json.loads(data)


def main():
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt("campaign interrupted")
    signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cycles", type=int, default=24)
    args = parser.parse_args()
    if not 1 <= args.cycles <= 120:
        parser.error("cycles must be 1..120")
    binary = args.binary.resolve(strict=True)
    args.output.mkdir()  # Never reuse or overwrite an existing campaign.
    state = args.output / "state"
    state.mkdir()
    config = args.output / "campaign.toml"
    config.write_text("shutdown_timeout_seconds = 3\nstatus_interval_seconds = 1\n")
    entities = [{"id": i, "position": [1.25, float(i % 100), -2.5]} for i in range(1, 10001)]
    snapshot = {"schema_version": 1, "tick": 0, "entities": entities}
    raw = json.dumps(snapshot).encode()
    (state / "snapshot-1.json").write_bytes(raw)
    (state / "manifest.json").write_text(json.dumps({"schema_version": 1,
        "current": {"number": 1, "sha256": hashlib.sha256(raw).hexdigest()}, "previous": None}))
    provenance = {"binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "helper_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  "entities": len(entities), "cycles": args.cycles,
                  "claim": "process termination only, not host power failure"}
    (args.output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
    with (args.output / "results.jsonl").open("x") as report:
        for cycle in range(args.cycles):
            before_generation, before = read_state(state)
            argv = [str(binary), "run", "--config", str(config), "--data-dir", str(state),
                    "--tick-hz", "120", "--autosave-seconds", "1"]
            with (args.output / "commands.jsonl").open("a") as journal:
                journal.write(json.dumps({"cycle": cycle, "argv": argv, "timeout_seconds": 20}) + "\n")
            events = queue.Queue()
            records = []
            with (args.output / f"cycle-{cycle:03}.log").open("x") as log:
                child = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)

                def collect():
                    for line in child.stderr:
                        log.write(line)
                        log.flush()
                        try:
                            record = json.loads(line)
                        except json.JSONDecodeError:
                            continue
                        records.append(record)
                        events.put(record)
                    events.put({"event": "process_eof"})

                reader = threading.Thread(target=collect, daemon=True)
                reader.start()
                try:
                    deadline = time.monotonic() + 10
                    started = None
                    while True:
                        try:
                            event = events.get(timeout=max(0.01, deadline - time.monotonic()))
                        except queue.Empty:
                            raise RuntimeError("first save acknowledgement timed out") from None
                        if event.get("event") == "started":
                            started = event
                        if event.get("event") == "fatal":
                            raise RuntimeError("native process reported fatal failure")
                        if event.get("event") == "process_eof":
                            raise RuntimeError("native process exited before first save; inspect cycle log")
                        if event.get("event") == "save_committed":
                            break
                        if time.monotonic() >= deadline:
                            raise RuntimeError("first save acknowledgement timed out")
                    if started is None or started["tick"] != before["tick"]:
                        raise RuntimeError("restart did not restore the preceding published clock")
                    # Vary termination around the following autosave window. This
                    # does not claim deterministic interception of each I/O stage.
                    time.sleep([0.96, 0.99, 1.02, 1.05][cycle % 4])
                    abrupt = cycle % 2 == 0
                    child.send_signal(signal.SIGKILL if abrupt else signal.SIGTERM)
                    code = child.wait(timeout=5)
                    reader.join(timeout=2)
                    if reader.is_alive():
                        raise RuntimeError("log collector failed to finish")
                    if code != (-signal.SIGKILL if abrupt else 0):
                        raise RuntimeError(f"unexpected child exit: {code}")
                finally:
                    if child.poll() is None:
                        child.kill()
                        child.wait(timeout=5)
                    reader.join(timeout=2)
                    child.stderr.close()
            generation, recovered = read_state(state)
            acknowledged = [r for r in records if r.get("event") == "save_committed"]
            last_ack = acknowledged[-1]
            if (generation < last_ack["generation"] or recovered["tick"] < last_ack["tick"]
                    or recovered["entities"] != entities or generation <= before_generation):
                raise RuntimeError("acknowledged state or authored entities lost")
            result = {"cycle": cycle, "termination": "SIGKILL" if abrupt else "SIGTERM",
                      "generation": generation, "recovered_tick": recovered["tick"],
                      "last_acknowledged_tick": last_ack["tick"], "state_verified": True}
            report.write(json.dumps(result) + "\n")
            report.flush()
            print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
