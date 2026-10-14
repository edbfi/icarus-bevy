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
import sys
from types import FrameType
from typing import TextIO

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.runtime_types import JsonObject, integer_field, json_object, parse_json, string_field


class Arguments(argparse.Namespace):
    binary: Path = Path()
    output: Path = Path()
    cycles: int = 24


def read_state(directory: Path) -> tuple[int, JsonObject]:
    manifest = json_object(parse_json((directory / "manifest.json").read_text()))
    current = json_object(manifest["current"])
    number = integer_field(current, "number")
    data = (directory / f"snapshot-{number}.json").read_bytes()
    if hashlib.sha256(data).hexdigest() != string_field(current, "sha256"):
        raise RuntimeError("published generation checksum mismatch")
    return number, json_object(parse_json(data))



def collect_events(
    stderr: TextIO,
    log: TextIO,
    records: list[JsonObject],
    events: queue.Queue[JsonObject],
) -> None:
    """Drain logs and report malformed records without abandoning the reader."""
    try:
        for line in stderr:
            _ = log.write(line)
            log.flush()
            try:
                record = json_object(parse_json(line))
            except json.JSONDecodeError:
                continue
            records.append(record)
            events.put(record)
    except (OSError, ValueError) as error:
        failure: JsonObject = {"event": "collector_error", "message": str(error)}
        records.append(failure)
        events.put(failure)
    finally:
        events.put({"event": "process_eof"})


def main() -> None:
    def interrupted(_signum: int, _frame: FrameType | None) -> None:
        raise KeyboardInterrupt("campaign interrupted")
    _ = signal.signal(signal.SIGTERM, interrupted)
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--binary", type=Path, required=True)
    _ = parser.add_argument("--output", type=Path, required=True)
    _ = parser.add_argument("--cycles", type=int, default=24)
    args = parser.parse_args(namespace=Arguments())
    if not 1 <= args.cycles <= 120:
        parser.error("cycles must be 1..120")
    binary = args.binary.resolve(strict=True)
    args.output.mkdir()  # Never reuse or overwrite an existing campaign.
    state = args.output / "state"
    state.mkdir()
    config = args.output / "campaign.toml"
    _ = config.write_text("shutdown_timeout_seconds = 3\nstatus_interval_seconds = 1\n")
    entities = [{"id": i, "position": [1.25, float(i % 100), -2.5]} for i in range(1, 10001)]
    snapshot = {"schema_version": 1, "tick": 0, "entities": entities}
    raw = json.dumps(snapshot).encode()
    _ = (state / "snapshot-1.json").write_bytes(raw)
    _ = (state / "manifest.json").write_text(json.dumps({"schema_version": 1,
        "current": {"number": 1, "sha256": hashlib.sha256(raw).hexdigest()}, "previous": None}))
    provenance = {"binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "helper_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  "helpers": {"tools/runtime_types.py": hashlib.sha256(
                      (Path(__file__).resolve().parent / "runtime_types.py").read_bytes()).hexdigest()},
                  "entities": len(entities), "cycles": args.cycles,
                  "claim": "process termination only, not host power failure"}
    _ = (args.output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
    with (args.output / "results.jsonl").open("x") as report:
        for cycle in range(args.cycles):
            before_generation, before = read_state(state)
            argv = [str(binary), "run", "--config", str(config), "--data-dir", str(state),
                    "--tick-hz", "120", "--autosave-seconds", "1"]
            with (args.output / "commands.jsonl").open("a") as journal:
                _ = journal.write(json.dumps({"cycle": cycle, "argv": argv, "timeout_seconds": 20}) + "\n")
            events: queue.Queue[JsonObject] = queue.Queue()
            records: list[JsonObject] = []
            with (args.output / f"cycle-{cycle:03}.log").open("x") as log:
                child = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
                stderr = child.stderr
                if stderr is None:
                    raise RuntimeError("missing child stderr pipe")

                reader = threading.Thread(
                    target=collect_events, args=(stderr, log, records, events), daemon=True
                )
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
                        if event.get("event") == "collector_error":
                            raise RuntimeError(f"log collector rejected record: {string_field(event, 'message')}")
                        if event.get("event") == "fatal":
                            raise RuntimeError("native process reported fatal failure")
                        if event.get("event") == "process_eof":
                            raise RuntimeError("native process exited before first save; inspect cycle log")
                        if event.get("event") == "save_committed":
                            break
                        if time.monotonic() >= deadline:
                            raise RuntimeError("first save acknowledgement timed out")
                    if started is None or integer_field(started, "tick") != integer_field(before, "tick"):
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
                    failures = [r for r in records if r.get("event") == "collector_error"]
                    if failures:
                        raise RuntimeError(f"log collector rejected record: {string_field(failures[0], 'message')}")
                    if code != (-signal.SIGKILL if abrupt else 0):
                        raise RuntimeError(f"unexpected child exit: {code}")
                finally:
                    if child.poll() is None:
                        child.kill()
                        _ = child.wait(timeout=5)
                    reader.join(timeout=2)
                    stderr.close()
            generation, recovered = read_state(state)
            acknowledged = [r for r in records if r.get("event") == "save_committed"]
            last_ack = acknowledged[-1]
            if (generation < integer_field(last_ack, "generation") or integer_field(recovered, "tick") < integer_field(last_ack, "tick")
                    or recovered["entities"] != entities or generation <= before_generation):
                raise RuntimeError("acknowledged state or authored entities lost")
            result = {"cycle": cycle, "termination": "SIGKILL" if abrupt else "SIGTERM",
                      "generation": generation, "recovered_tick": recovered["tick"],
                      "last_acknowledged_tick": last_ack["tick"], "state_verified": True}
            _ = report.write(json.dumps(result) + "\n")
            report.flush()
            print(json.dumps(result), flush=True)


if __name__ == "__main__":
    main()
