#!/usr/bin/env python3
"""Bounded authored start/save/stop/kill/recovery campaign; no original files."""

import argparse
import hashlib
import json
from pathlib import Path
import queue
import shutil
import uuid
from dataclasses import dataclass
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
    mode: str = "entities"


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
    _ = parser.add_argument("--mode", choices=("entities", "authored-player"), default="entities")
    args = parser.parse_args(namespace=Arguments())
    if not 1 <= args.cycles <= 120:
        parser.error("cycles must be 1..120")
    if args.mode == "authored-player":
        authored_campaign(args)
        return
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


PLAYER_PROTOCOL = "authored_player_recovery_v1"
DISK_FLOOR = 30 * 1024**3
MAX_OUTPUT = 1024 * 1024


@dataclass(frozen=True)
class PlayerState:
    generation: int
    tick: int
    digest: str

    @classmethod
    def parse(cls, value: JsonObject) -> "PlayerState":
        generation = integer_field(value, "generation")
        tick = integer_field(value, "tick")
        digest = string_field(value, "digest")
        if generation <= 0 or not 0 <= tick <= 100_000 or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
            raise RuntimeError("invalid authored state receipt")
        return cls(generation, tick, digest)

    def receipt(self) -> JsonObject:
        return {"generation": self.generation, "tick": self.tick, "digest": self.digest}


def strict_player_events(source: TextIO, log: TextIO, records: list[JsonObject], events: queue.Queue[JsonObject]) -> None:
    """The standalone workload has an exclusively JSON stdout contract."""
    size = 0
    try:
        while line := source.readline(16_385):
            size += len(line.encode())
            if size > MAX_OUTPUT or len(line) > 16_384 or not line.endswith("\n"):
                raise RuntimeError("oversize or truncated authored record")
            _ = log.write(line)
            log.flush()
            record = json_object(parse_json(line))
            records.append(record)
            events.put(record)
    except (OSError, ValueError, RuntimeError) as error:
        failure: JsonObject = {"event": "collector_error", "message": str(error)}
        records.append(failure)
        events.put(failure)
    finally:
        events.put({"event": "process_eof"})


def player_identity(record: JsonObject, run_id: str, pid: int) -> str:
    if (record.get("event") == "collector_error" or record.get("protocol") != PLAYER_PROTOCOL
            or record.get("run_id") != run_id or integer_field(record, "pid") != pid):
        raise RuntimeError("authored event collection or child identity failure")
    return string_field(record, "event")


def validate_player_cycle(records: list[JsonObject], run_id: str, pid: int, code: int,
                          abrupt: bool, before: PlayerState) -> list[PlayerState]:
    """Account for *every* acknowledgement, including shutdown's drained save."""
    if not records or player_identity(records[0], run_id, pid) != "started" or PlayerState.parse(records[0]) != before:
        raise RuntimeError("missing or incorrect restored state")
    phase = "running"
    shutdown_acknowledged = False
    acknowledgements: list[PlayerState] = []
    previous = before
    for record in records[1:]:
        event = player_identity(record, run_id, pid)
        if event == "save_committed" and phase in ("running", "stopping"):
            state = PlayerState.parse(record)
            if state.generation != previous.generation + 1 or state.tick < previous.tick:
                raise RuntimeError("acknowledged generation/tick sequence differs")
            acknowledgements.append(state)
            if phase == "stopping":
                shutdown_acknowledged = True
            previous = state
        elif event == "stopping" and phase == "running" and not abrupt:
            phase = "stopping"
        elif event == "completed" and phase == "stopping" and shutdown_acknowledged:
            if (integer_field(record, "generation"), integer_field(record, "tick")) != (previous.generation, previous.tick):
                raise RuntimeError("final output disagrees with writer receipt")
            phase = "completed"
        else:
            raise RuntimeError(f"unexpected or early final authored output: {event}")
    if not acknowledgements or code != (-signal.SIGKILL if abrupt else 0) or (not abrupt and phase != "completed"):
        raise RuntimeError(f"incomplete authored cycle or unexpected child exit: {code}")
    return acknowledgements


def player_guard(directory: Path, deadline: float) -> None:
    if time.monotonic() >= deadline:
        raise RuntimeError("authored controller deadline exceeded")
    if shutil.disk_usage(directory).free < DISK_FLOOR:
        raise RuntimeError("authored controller disk floor reached")
    if sum(p.stat().st_size for p in directory.glob("*.stderr")) > MAX_OUTPUT:
        raise RuntimeError("authored child stderr exceeds bound")


def player_child(binary: Path, output: Path, state: Path, label: str, command: list[str],
                 before: PlayerState | None = None, cycle: int = 0) -> tuple[list[JsonObject], int, int]:
    """Retain actual exits even on timeout/collection failure; kill only this child."""
    run_id = uuid.uuid4().hex
    argv = [str(binary), "--data-dir", str(state), "--run-id", run_id, *command]
    deadline = time.monotonic() + 20
    records: list[JsonObject] = []
    events: queue.Queue[JsonObject] = queue.Queue()
    with (output / "commands.jsonl").open("a") as journal:
        _ = journal.write(json.dumps({"label": label, "argv": argv, "timeout_seconds": 20}) + "\n")
    child: subprocess.Popen[str] | None = None
    reader: threading.Thread | None = None
    signalled = False
    error_message: str | None = None
    with (output / f"{label}.jsonl").open("x") as log, (output / f"{label}.stderr").open("x") as stderr:
        try:
            player_guard(output, deadline)
            child = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=stderr, text=True)
            stdout = child.stdout
            if stdout is None:
                raise RuntimeError("missing authored stdout pipe")
            reader = threading.Thread(target=strict_player_events, args=(stdout, log, records, events), daemon=True)
            reader.start()
            abrupt = cycle % 2 == 0
            stop_at: float | None = None
            eof = False
            while not eof:
                player_guard(output, deadline)
                try:
                    record = events.get(timeout=0.02)
                except queue.Empty:
                    record = None
                if record is not None:
                    event = string_field(record, "event")
                    if event == "process_eof":
                        eof = True
                    else:
                        event = player_identity(record, run_id, child.pid)
                        if before is not None:
                            if event in ("completed", "stopping") and not signalled:
                                raise RuntimeError("authored process emitted early final output")
                            if event == "save_committed" and stop_at is None:
                                # Relative to observing one commit, cross the next
                                # autosave window. No deterministic I/O interception.
                                stop_at = time.monotonic() + (0.96, 0.99, 1.02, 1.05)[cycle % 4]
                if before is not None and not signalled and stop_at is not None and time.monotonic() >= stop_at:
                    if child.poll() is not None:
                        raise RuntimeError("authored process exited before controller signal")
                    # Drain already-collected lifecycle events before signalling.
                    if any(r.get("event") in ("completed", "stopping") for r in records):
                        raise RuntimeError("authored process completed before controller signal")
                    child.send_signal(signal.SIGKILL if abrupt else signal.SIGTERM)
                    signalled = True
            code = child.wait(timeout=max(0.01, deadline - time.monotonic()))
            reader.join(timeout=1)
            if reader.is_alive():
                raise RuntimeError("authored collector failed to finish")
            player_guard(output, deadline)
            if before is None:
                wanted = "initialized" if command[0] == "init" else "verified"
                if code != 0 or len(records) != 1 or player_identity(records[0], run_id, child.pid) != wanted:
                    raise RuntimeError("missing, malformed or incomplete authored final output")
            else:
                if not signalled:
                    raise RuntimeError("authored child ended before controller signal")
                _ = validate_player_cycle(records, run_id, child.pid, code, abrupt, before)
            return records, code, child.pid
        except BaseException as error:
            error_message = str(error)
            raise
        finally:
            if child is not None:
                if child.poll() is None:
                    child.kill()
                code = child.wait(timeout=5)
                if reader is not None:
                    reader.join(timeout=2)
                if child.stdout is not None:
                    child.stdout.close()
                with (output / "exits.jsonl").open("a") as exits:
                    _ = exits.write(json.dumps({"label": label, "pid": child.pid, "run_id": run_id,
                        "child_exit": code, "signal_sent": signalled, "error": error_message,
                        "collection_complete": reader is not None and not reader.is_alive()}) + "\n")


def authored_campaign(args: Arguments) -> None:
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir()  # Fresh campaign only, never replace receipts.
    controller_exit = 1
    failure: str | None = None
    try:
        state = output / "state"
        player_guard(output, time.monotonic() + 20)
        provenance: JsonObject = {"mode": "authored-player", "binary": str(binary),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "helper_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "runtime_types_sha256": hashlib.sha256(Path(__file__).with_name("runtime_types.py").read_bytes()).hexdigest(),
            "cycles": args.cycles, "protocol": PLAYER_PROTOCOL,
            "claim": "authored process termination only; no physical power loss or filesystem fault-point coverage"}
        _ = (output / "provenance.json").write_text(json.dumps(provenance, indent=2) + "\n")
        initialized, _, _ = player_child(binary, output, state, "init", ["init"])
        before = PlayerState.parse(initialized[0])
        if (before.generation, before.tick) != (1, 0):
            raise RuntimeError("initial authored receipt differs")
        acknowledgements = [before.receipt()]
        with (output / "results.jsonl").open("x") as report:
            for cycle in range(args.cycles):
                records, code, pid = player_child(binary, output, state, f"cycle-{cycle:03}",
                    ["run", "--duration-seconds", "12", "--save-millis", "1000"], before, cycle)
                run_id = string_field(records[0], "run_id")
                acks = validate_player_cycle(records, run_id, pid, code, cycle % 2 == 0, before)
                acknowledgements.extend(ack.receipt() for ack in acks)
                receipt_file = output / f"receipts-{cycle:03}.json"
                _ = receipt_file.write_text(json.dumps(acknowledgements) + "\n")
                verified, verify_exit, verifier_pid = player_child(binary, output, state, f"verify-{cycle:03}",
                    ["verify", "--receipts", str(receipt_file)])
                selected = PlayerState.parse(verified[0])
                last = acks[-1]
                if (not last.generation <= selected.generation <= last.generation + 1 or selected.tick < last.tick
                        or selected.tick > before.tick + 84 or (cycle % 2 == 1 and selected != last)
                        or integer_field(verified[0], "resumed_tick") != selected.tick + 8
                        or integer_field(verified[0], "acknowledgements") != len(acknowledgements)
                        or integer_field(verified[0], "schema") != 14):
                    raise RuntimeError("typed verifier final receipt differs")
                result: JsonObject = {"cycle": cycle, "termination": "SIGKILL" if cycle % 2 == 0 else "SIGTERM",
                    "child_pid": pid, "child_exit": code, "verifier_pid": verifier_pid, "verifier_exit": verify_exit,
                    "selected": selected.receipt(), "acknowledged": len(acks), "state_verified": True}
                _ = report.write(json.dumps(result) + "\n")
                report.flush()
                print(json.dumps(result), flush=True)
                before = selected
        controller_exit = 0
    except BaseException as error:
        failure = str(error)
        raise
    finally:
        _ = (output / "controller.json").write_text(json.dumps({"controller_exit": controller_exit,
            "complete": controller_exit == 0, "error": failure}) + "\n")


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        raise SystemExit(1) from None
