"""Bounded source-authored controller fixtures; never launch the recovery workload."""
from contextlib import redirect_stdout
from dataclasses import dataclass
from io import StringIO
import json
from pathlib import Path
import queue
import signal
import subprocess
import sys
import tempfile
import unittest
from typing import TextIO, override
from unittest.mock import patch
from uuid import UUID

from tools.native_recovery_campaign import (
    Arguments, INVENTORY_FIXTURE, INVENTORY_PROTOCOL, PLAYER_FIXTURE, PLAYER_PROTOCOL, PlayerFixture,
    PlayerState, authored_campaign, main, player_child, player_guard, player_identity, player_preflight,
    strict_player_events, validate_player_cycle, validate_player_exits,
)
from tools.runtime_types import JsonObject, json_object, parse_json


@dataclass
class MockChild:
    """Only the controller boundary is simulated; no executable is invoked."""

    stdout: TextIO | None
    pid: int = 123
    code: int | None = 0
    kill_error: bool = False
    wait_error: bool = False
    killed: bool = False

    def poll(self) -> int | None:
        return self.code

    def kill(self) -> None:
        if self.kill_error:
            raise OSError("fixture kill failure")
        self.killed = True
        self.code = -signal.SIGKILL

    def wait(self, timeout: float) -> int:
        if self.wait_error or self.code is None:
            raise subprocess.TimeoutExpired("mock", timeout)
        return self.code

    def send_signal(self, signum: int) -> None:
        self.code = -signum if signum == signal.SIGKILL else 0


def journal_exit(output: Path, label: str, code: int, pid: int) -> None:
    with (output / "exits.jsonl").open("a") as journal:
        _ = journal.write(json.dumps({"label": label, "pid": pid, "child_exit": code,
            "run_id": "fixture", "error": None, "cleanup_errors": [], "collection_errors": [],
            "signal_sent": label.startswith("cycle-"),
            "collection_complete": True, "complete": True}) + "\n")


class PlayerCampaignTest(unittest.TestCase):
    before: PlayerState = PlayerState(1, 0, "a" * 64)

    def record(self, event: str, generation: int = 2, tick: int = 3) -> JsonObject:
        return {"protocol": PLAYER_PROTOCOL, "run_id": "fixture", "pid": 123,
                "event": event, "generation": generation, "tick": tick, "digest": "a" * 64}

    def started(self) -> JsonObject:
        return self.record("started", 1, 0)

    def test_every_acknowledgement_and_shutdown_drain_are_retained(self) -> None:
        records = [self.started(), self.record("save_committed"), self.record("stopping"),
                   self.record("save_committed", 3, 4), self.record("completed", 3, 4)]
        acks = validate_player_cycle(records, "fixture", 123, 0, False, self.before)
        self.assertEqual([(a.generation, a.tick) for a in acks], [(2, 3), (3, 4)])
        killed = validate_player_cycle(records[:2], "fixture", 123, -signal.SIGKILL, True, self.before)
        self.assertEqual(len(killed), 1)

    def test_missing_malformed_early_final_wrong_identity_and_receipts_fail(self) -> None:
        clean = [self.started(), self.record("save_committed"), self.record("stopping"),
                 self.record("save_committed", 3, 4), self.record("completed", 3, 4)]
        invalid = [[], clean[:-1], [self.started(), self.record("completed")],
                   [self.started(), self.record("save_committed", 3)],
                   [self.started(), self.record("save_committed", 2, -1)],
                   [self.started(), {**self.record("save_committed"), "pid": 124}],
                   [self.started(), {**self.record("save_committed"), "run_id": "wrong"}],
                   [self.started(), {**self.record("save_committed"), "protocol": "wrong"}],
                   [self.started(), self.record("collector_error")],
                   [*clean, self.record("save_committed", 3)],
                   [*clean[:-1], self.record("completed", 4, 4)],
                   [self.started(), self.record("save_committed"), self.record("stopping"), self.record("completed")],
                   [self.started(), self.record("save_committed"), self.record("save_committed")]]
        for records in invalid:
            with self.subTest(records=records), self.assertRaises((RuntimeError, ValueError, KeyError)):
                _ = validate_player_cycle(records, "fixture", 123, 0, False, self.before)
        with self.assertRaises(RuntimeError):
            _ = validate_player_cycle(clean, "fixture", 123, 1, False, self.before)
        with self.assertRaises(RuntimeError):
            _ = validate_player_cycle(clean, "fixture", 123, -signal.SIGKILL, True, self.before)

    def test_strict_collection_rejects_all_non_json_truncated_and_oversize_output(self) -> None:
        for source in ("ordinary line\n", "[]\n", "{oops}\n", '{"event":"x"}', "x" * 16_385 + "\n"):
            records: list[JsonObject] = []
            events: queue.Queue[JsonObject] = queue.Queue()
            strict_player_events(StringIO(source), StringIO(), records, events)
            self.assertEqual(events.get_nowait()["event"], "collector_error")
            self.assertEqual(events.get_nowait()["event"], "process_eof")
        records = []
        events = queue.Queue()
        strict_player_events(StringIO(json.dumps(self.started()) + "\n"), StringIO(), records, events)
        self.assertEqual(records, [self.started()])

    def test_state_parser_rejects_boolean_tick_and_non_hash_digest(self) -> None:
        values: list[JsonObject] = [{"generation": 1, "tick": True, "digest": "a" * 64},
                                    {"generation": 1, "tick": 0, "digest": "z" * 64}]
        for value in values:
            with self.assertRaises((RuntimeError, ValueError)):
                _ = PlayerState.parse(value)

    def test_deadline_and_disk_guard_deny_success(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            with self.assertRaisesRegex(RuntimeError, "deadline"):
                player_guard(path, 0)
            with patch("tools.native_recovery_campaign.DISK_FLOOR", 2**100):
                with self.assertRaisesRegex(RuntimeError, "disk floor"):
                    player_guard(path, float("inf"))

    def test_bounded_mock_child_failures_retain_actual_exit(self) -> None:
        # Python is the temporary authored mock, not the Rust workload. It accepts
        # the controller's flags and emits deliberately bad stdout.
        for source, expected_code in (("print('malformed')", 0), ("raise SystemExit(7)", 7)):
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory)
                # shell-free executable wrapper is an authored temporary fixture.
                binary = path / "mock"
                _ = binary.write_text(f"#!{sys.executable}\n{source}\n")
                binary.chmod(0o700)
                with patch("tools.native_recovery_campaign.DISK_FLOOR", 0):
                    with self.assertRaises(RuntimeError):
                        _ = player_child(binary, path, path / "state", "mock", ["init"])
                exit_record = json_object(parse_json((path / "exits.jsonl").read_text()))
                self.assertIn(exit_record["child_exit"], (0, -signal.SIGKILL) if expected_code == 0 else (expected_code,))
                self.assertIsNotNone(exit_record["error"])

    def test_mock_child_success_and_early_final_output(self) -> None:
        prefix = f"#!{sys.executable}\n" + """import json, os, sys, signal, time
run_id = sys.argv[sys.argv.index('--run-id') + 1]
def emit(event, generation=2, tick=3):
    print(json.dumps(dict(protocol='authored_player_recovery_v1', run_id=run_id,
        pid=os.getpid(), event=event, generation=generation, tick=tick, digest='a'*64)), flush=True)
def stop(*args):
    emit('stopping')
    emit('save_committed', 3, 4)
    emit('completed', 3, 4)
    raise SystemExit(0)
signal.signal(signal.SIGTERM, stop)
"""
        for body, before, succeeds in (
            ("emit('initialized', 1, 0)\n", None, True),
            ("emit('started', 1, 0)\nemit('save_committed')\ntime.sleep(10)\n", self.before, True),
            ("emit('started', 1, 0)\nemit('save_committed')\nemit('completed')\n", self.before, False),
        ):
            with tempfile.TemporaryDirectory() as directory:
                path = Path(directory)
                binary = path / "mock"
                _ = binary.write_text(prefix + body)
                binary.chmod(0o700)
                with patch("tools.native_recovery_campaign.DISK_FLOOR", 0):
                    if succeeds:
                        records, code, _ = player_child(binary, path, path / "state", "mock", ["init"], before, 1)
                        self.assertEqual(code, 0)
                        self.assertTrue(records)
                    else:
                        with self.assertRaisesRegex(RuntimeError, "early final"):
                            _ = player_child(binary, path, path / "state", "mock", ["run"], before, 1)
                exit_record = json_object(parse_json((path / "exits.jsonl").read_text()))
                self.assertEqual(exit_record["signal_sent"], before is not None and succeeds)

    def test_mock_campaign_verifier_failure_denies_controller_success(self) -> None:
        for resumed_tick in (11, 12):
            with tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "fresh-output"
                args = Arguments()
                args.binary = Path(sys.executable)
                args.output = output
                args.cycles = 1
                args.mode = "authored-player"
                verified: JsonObject = {**self.record("verified"), "pid": 124,
                    "resumed_tick": resumed_tick, "acknowledgements": 2, "schema": 14}
                results = [([self.record("initialized", 1, 0)], 0, 123),
                    ([self.started(), self.record("save_committed")], -signal.SIGKILL, 123),
                    ([verified], 0, 124)]
                pending = iter(results)

                def mock_child(_binary: Path, output: Path, _state: Path, label: str, _command: list[str],
                               _before: PlayerState | None = None, _cycle: int = 0, *,
                               fixture: PlayerFixture, workload_deadline: float) -> tuple[list[JsonObject], int, int]:
                    self.assertEqual(fixture, PLAYER_FIXTURE)
                    self.assertGreater(workload_deadline, 0)
                    records, code, pid = next(pending)
                    journal_exit(output, label, code, pid)
                    return records, code, pid

                with patch("tools.native_recovery_campaign.player_child", side_effect=mock_child), patch(
                    "tools.native_recovery_campaign.DISK_FLOOR", 0
                ), redirect_stdout(StringIO()):
                    if resumed_tick == 11:
                        authored_campaign(args)
                    else:
                        with self.assertRaisesRegex(RuntimeError, "verifier final receipt"):
                            authored_campaign(args)
                controller = json_object(parse_json((output / "controller.json").read_text()))
                self.assertEqual(controller["controller_exit"], 0 if resumed_tick == 11 else 1)
                self.assertEqual(controller["complete"], resumed_tick == 11)
                receipts = parse_json((output / "receipts-000.json").read_text())
                self.assertEqual(receipts, [self.before.receipt(), PlayerState(2, 3, "a" * 64).receipt()])

    def test_mock_child_deadline_kills_owned_process_and_records_exit(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            binary = path / "mock"
            _ = binary.write_text(f"#!{sys.executable}\nimport time\ntime.sleep(30)\n")
            binary.chmod(0o700)
            with patch("tools.native_recovery_campaign.DISK_FLOOR", 0), patch(
                "tools.native_recovery_campaign.time.monotonic", side_effect=[0., 0., *([21.] * 20)]
            ):
                with self.assertRaisesRegex(RuntimeError, "deadline"):
                    _ = player_child(binary, path, path / "state", "mock", ["init"])
            exit_record = json_object(parse_json((path / "exits.jsonl").read_text()))
            self.assertEqual(exit_record["child_exit"], -signal.SIGKILL)
            self.assertFalse(exit_record["signal_sent"])


class InventoryCampaignTest(unittest.TestCase):
    before: PlayerState = PlayerState(1, 0, "a" * 64)

    def record(self, event: str, generation: int = 2, tick: int = 3, pid: int = 123) -> JsonObject:
        return {"protocol": INVENTORY_PROTOCOL, "run_id": "fixture", "pid": pid,
                "event": event, "generation": generation, "tick": tick, "digest": "a" * 64}

    def test_inventory_protocol_isolation_and_old_drain_then_exact_final_receipt(self) -> None:
        records = [self.record("started", 1, 0), self.record("save_committed"), self.record("stopping"),
                   self.record("save_committed", 3, 4), self.record("save_committed", 4, 4),
                   self.record("completed", 4, 4)]
        acks = validate_player_cycle(records, "fixture", 123, 0, False, self.before, INVENTORY_FIXTURE)
        self.assertEqual([(ack.generation, ack.tick) for ack in acks], [(2, 3), (3, 4), (4, 4)])
        for fixture in (PLAYER_FIXTURE, INVENTORY_FIXTURE):
            other = INVENTORY_FIXTURE if fixture == PLAYER_FIXTURE else PLAYER_FIXTURE
            for event in ("initialized", "started", "save_committed", "stopping", "completed", "verified"):
                with self.subTest(mode=fixture.mode, event=event), self.assertRaises(RuntimeError):
                    _ = player_identity({**self.record(event), "protocol": other.protocol}, "fixture", 123, fixture)
        for ending in ({**records[-1], "digest": "b" * 64}, self.record("completed", 3, 4)):
            with self.subTest(ending=ending), self.assertRaises(RuntimeError):
                _ = validate_player_cycle([*records[:-1], ending], "fixture", 123, 0, False, self.before, INVENTORY_FIXTURE)
        for invalid in (records[:-1], records[1:], [*records[:3], records[-1]],
                        [*records[:-1], self.record("save_committed", 5, 4), self.record("completed", 5, 4)]):
            with self.subTest(records=invalid), self.assertRaises(RuntimeError):
                _ = validate_player_cycle(invalid, "fixture", 123, 0, False, self.before, INVENTORY_FIXTURE)

    def simulate_campaign(self, output: Path, *, cycles: int = 4, overrides: JsonObject | None = None,
                          verify_exit: int = 0, journal_complete: bool = True,
                          mode: str = "authored-player-inventory", override_cycle: int | None = None) -> None:
        args = Arguments()
        args.binary = Path(sys.executable)
        args.output = output
        args.cycles = cycles
        args.mode = mode
        expected_receipts = [self.before.receipt()]
        calls: list[str] = []
        fixture_expected = INVENTORY_FIXTURE if mode == INVENTORY_FIXTURE.mode else PLAYER_FIXTURE

        def mock_child(_binary: Path, output: Path, _state: Path, label: str, command: list[str],
                       before: PlayerState | None = None, cycle: int = 0, *, fixture: PlayerFixture,
                       workload_deadline: float) -> tuple[list[JsonObject], int, int]:
            self.assertEqual(fixture, fixture_expected)
            self.assertGreater(workload_deadline, 0)
            calls.append(label)
            pid = 100 + len(calls)

            def emit(event: str, state: PlayerState) -> JsonObject:
                return {**self.record(event, state.generation, state.tick, pid), "protocol": fixture.protocol}

            if command[0] == "init":
                records = [emit("initialized", self.before)]
                code = 0
            elif command[0] == "run":
                assert before is not None
                ack = PlayerState(before.generation + 1, before.tick + 3, "a" * 64)
                records = [emit("started", before), emit("save_committed", ack)]
                expected_receipts.append(ack.receipt())
                if cycle % 2:
                    drained = PlayerState(ack.generation + 1, ack.tick + 1, ack.digest)
                    final = PlayerState(ack.generation + 2, ack.tick + 1, ack.digest)
                    records.extend([emit("stopping", ack), emit("save_committed", drained),
                                    emit("save_committed", final), emit("completed", final)])
                    expected_receipts.extend([drained.receipt(), final.receipt()])
                code = -signal.SIGKILL if cycle % 2 == 0 else 0
            else:
                self.assertEqual(command[0], "verify")
                receipts = parse_json(Path(command[2]).read_text())
                self.assertEqual(receipts, expected_receipts)
                selected = PlayerState.parse(expected_receipts[-1])
                fields = overrides if override_cycle is None or label == f"verify-{override_cycle:03}" else None
                records = [{**emit("verified", selected), "resumed_tick": selected.tick + 8,
                    "acknowledgements": len(expected_receipts), "schema": fixture.schema, **(fields or {})}]
                code = verify_exit
            if journal_complete:
                journal_exit(output, label, code, pid)
            return records, code, pid

        with patch("tools.native_recovery_campaign.player_child", side_effect=mock_child), patch(
            "tools.native_recovery_campaign.DISK_FLOOR", 0
        ), patch("tools.native_recovery_campaign.player_preflight"), redirect_stdout(StringIO()):
            authored_campaign(args)
        self.assertEqual(calls, ["init", *[label for n in range(cycles) for label in (f"cycle-{n:03}", f"verify-{n:03}")]])

    def test_four_cycle_inventory_campaign_retains_all_receipts_and_nine_exits(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "campaign"
            self.simulate_campaign(output)
            controller = json_object(parse_json((output / "controller.json").read_text()))
            self.assertTrue(controller["complete"])
            self.assertEqual(controller["controller_exit"], 0)
            self.assertEqual(controller["children"], 9)
            self.assertEqual(controller["completed_cycles"], 4)
            receipts = parse_json((output / "receipts-003.json").read_text())
            self.assertIsInstance(receipts, list)
            self.assertEqual(receipts, [PlayerState(generation, tick, "a" * 64).receipt() for generation, tick in
                ((1, 0), (2, 3), (3, 6), (4, 7), (5, 7), (6, 10), (7, 13), (8, 14), (9, 14))])
            provenance = json_object(parse_json((output / "provenance.json").read_text()))
            self.assertEqual((provenance["mode"], provenance["schema"], provenance["protocol"]),
                             (INVENTORY_FIXTURE.mode, 15, INVENTORY_PROTOCOL))

    def test_inventory_verifier_adversaries_and_missing_exit_journal_fail_controller(self) -> None:
        bad_fields: list[JsonObject] = [{"resumed_tick": 12}, {"acknowledgements": 1}, {"schema": 14},
            {"schema": True}, {"protocol": PLAYER_PROTOCOL}, {"digest": "b" * 64},
            {"event": "completed"}, {"pid": 0}, {"tick": 85}, {"generation": 4}]
        scenarios = [(fields, 0, True) for fields in bad_fields] + [({}, 7, True), ({}, -9, True), ({}, 0, False)]
        for overrides, exit_code, journal_complete in scenarios:
            with self.subTest(overrides=overrides, exit_code=exit_code, journal_complete=journal_complete):
                with tempfile.TemporaryDirectory() as directory:
                    output = Path(directory) / "campaign"
                    with self.assertRaises((RuntimeError, ValueError, FileNotFoundError)):
                        self.simulate_campaign(output, cycles=1, overrides=overrides, verify_exit=exit_code,
                                               journal_complete=journal_complete)
                    controller = json_object(parse_json((output / "controller.json").read_text()))
                    self.assertFalse(controller["complete"])
                    self.assertEqual(controller["controller_exit"], 1)
                    self.assertIsNotNone(controller["error"])
                    if journal_complete:
                        exits = [json_object(parse_json(line)) for line in (output / "exits.jsonl").read_text().splitlines()]
                        self.assertEqual(exits[-1]["child_exit"], exit_code)

    def test_later_unacknowledged_generation_only_allowed_after_kill(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.simulate_campaign(Path(directory) / "campaign", cycles=1,
                overrides={"generation": 3, "tick": 4, "digest": "b" * 64, "resumed_tick": 12})
        invalid: list[JsonObject] = [{"generation": 6}, {"digest": "b" * 64}]
        for overrides in invalid:
            with self.subTest(overrides=overrides), tempfile.TemporaryDirectory() as directory:
                with self.assertRaisesRegex(RuntimeError, "verifier final receipt"):
                    self.simulate_campaign(Path(directory) / "campaign", cycles=2,
                                           overrides=overrides, override_cycle=1)
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(RuntimeError, "verifier final receipt"):
                self.simulate_campaign(Path(directory) / "campaign", cycles=1,
                                       mode=PLAYER_FIXTURE.mode, overrides={"schema": 15})

    def test_cli_keeps_old_cycle_limits_and_inventory_requires_one_to_four(self) -> None:
        for mode, cycles in (("entities", 120), ("authored-player", 120), ("authored-player-inventory", 4)):
            with patch.object(sys, "argv", ["campaign", "--binary", "missing", "--output", "unused",
                                          "--mode", mode, "--cycles", str(cycles)]), patch(
                "tools.native_recovery_campaign.authored_campaign"
            ) as campaign, patch("tools.native_recovery_campaign.read_state"), patch.object(
                Path, "resolve", side_effect=RuntimeError("entity dispatch reached")
            ):
                if mode == "entities":
                    with self.assertRaisesRegex(RuntimeError, "entity dispatch"):
                        main()
                    campaign.assert_not_called()
                else:
                    main()
                    campaign.assert_called_once()
        for mode, cycles in (("authored-player-inventory", 0), ("authored-player-inventory", 5),
                             ("authored-player", 121), ("entities", 0)):
            with patch.object(sys, "argv", ["campaign", "--binary", "missing", "--output", "unused",
                                          "--mode", mode, "--cycles", str(cycles)]), patch(
                "tools.native_recovery_campaign.authored_campaign"
            ) as campaign, self.assertRaises(SystemExit), patch("sys.stderr", new_callable=StringIO):
                main()
            campaign.assert_not_called()
        defaults: list[tuple[str, int]] = []

        def dispatched(args: Arguments) -> None:
            defaults.append((args.mode, args.cycles))

        for mode in ("authored-player", "authored-player-inventory"):
            with patch.object(sys, "argv", ["campaign", "--binary", "missing", "--output", "unused", "--mode", mode]), patch(
                "tools.native_recovery_campaign.authored_campaign", side_effect=dispatched
            ):
                main()
        self.assertEqual(defaults, [("authored-player", 24), ("authored-player-inventory", 4)])

    def test_inventory_flags_on_every_child_and_nonzero_verifier_exit_is_recorded(self) -> None:
        nonce = UUID(int=1)
        for command, event, code in ((["init"], "initialized", 0),
                                     (["verify", "--receipts", "RECEIPTS"], "verified", 0),
                                     (["verify", "--receipts", "RECEIPTS"], "verified", 7)):
            with self.subTest(command=command, code=code), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                receipts = output / "receipts.json"
                _ = receipts.write_text("[]\n")
                argv = [str(receipts) if arg == "RECEIPTS" else arg for arg in command]
                record = {**self.record(event, 1, 0), "run_id": nonce.hex}
                child = MockChild(StringIO(json.dumps(record) + "\n"), code=code)
                with patch("tools.native_recovery_campaign.uuid.uuid4", return_value=nonce), patch(
                    "tools.native_recovery_campaign.subprocess.Popen", return_value=child
                ) as spawn, patch("tools.native_recovery_campaign.DISK_FLOOR", 0):
                    if code:
                        with self.assertRaisesRegex(RuntimeError, "final output"):
                            _ = player_child(Path("unused"), output, output / "state", "mock", argv, fixture=INVENTORY_FIXTURE)
                    else:
                        records, actual, pid = player_child(Path("unused"), output, output / "state", "mock", argv,
                                                            fixture=INVENTORY_FIXTURE)
                        self.assertEqual((records, actual, pid), ([record], code, 123))
                expected_argv = ["unused", "--data-dir", str(output / "state"), "--run-id", nonce.hex,
                                 "--inventory", *argv]
                spawn.assert_called_once()
                journal = json_object(parse_json((output / "commands.jsonl").read_text()))
                self.assertEqual(journal["argv"], expected_argv)
                exit_record = json_object(parse_json((output / "exits.jsonl").read_text()))
                self.assertEqual(exit_record["child_exit"], code)
                self.assertEqual(exit_record["complete"], code == 0)

    def test_inventory_run_uses_fixture_flag_and_signal_before_success(self) -> None:
        nonce = UUID(int=1)
        records = [{**self.record(event, generation, tick), "run_id": nonce.hex} for event, generation, tick in
                   (("started", 1, 0), ("save_committed", 2, 3))]
        events = [*records, {"event": "process_eof"}]
        child = MockChild(StringIO(), code=None)

        def collect(_source: TextIO, _log: TextIO, collected: list[JsonObject], _events: queue.Queue[JsonObject]) -> None:
            collected.extend(records)

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with patch("tools.native_recovery_campaign.uuid.uuid4", return_value=nonce), patch(
                "tools.native_recovery_campaign.subprocess.Popen", return_value=child
            ), patch("tools.native_recovery_campaign.strict_player_events", side_effect=collect), patch(
                "tools.native_recovery_campaign.queue.Queue.get", side_effect=events
            ), patch("tools.native_recovery_campaign.time.monotonic", side_effect=[0., 0., 0., 0., 0., 1., *([2.] * 30)]), patch(
                "tools.native_recovery_campaign.DISK_FLOOR", 0
            ):
                _, code, _ = player_child(Path("unused"), output, output / "state", "run", ["run"],
                                           self.before, fixture=INVENTORY_FIXTURE)
            self.assertEqual(code, -signal.SIGKILL)
            journal = json_object(parse_json((output / "commands.jsonl").read_text()))
            self.assertEqual(journal["argv"], ["unused", "--data-dir", str(output / "state"), "--run-id", nonce.hex,
                                               "--inventory", "run"])
            exit_record = json_object(parse_json((output / "exits.jsonl").read_text()))
            self.assertTrue(exit_record["signal_sent"])
            self.assertTrue(exit_record["complete"])

    def test_cleanup_failures_preserve_original_error_and_never_invent_exit(self) -> None:
        for kill_error, wait_error in ((True, False), (False, True)):
            with self.subTest(kill_error=kill_error), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                child = MockChild(StringIO("malformed\n"), code=None, kill_error=kill_error, wait_error=wait_error)
                # A failed kill is followed by an actual observed exit; a failed
                # reap has no observed code, even if kill was requested.
                if kill_error:
                    with patch.object(MockChild, "poll", return_value=None), patch.object(MockChild, "wait", return_value=7), patch(
                        "tools.native_recovery_campaign.subprocess.Popen", return_value=child
                    ), patch("tools.native_recovery_campaign.DISK_FLOOR", 0):
                        with self.assertRaisesRegex(RuntimeError, "identity failure"):
                            _ = player_child(Path("unused"), output, output / "state", "mock", ["init"], fixture=INVENTORY_FIXTURE)
                else:
                    with patch("tools.native_recovery_campaign.subprocess.Popen", return_value=child), patch(
                        "tools.native_recovery_campaign.DISK_FLOOR", 0
                    ):
                        with self.assertRaisesRegex(RuntimeError, "identity failure"):
                            _ = player_child(Path("unused"), output, output / "state", "mock", ["init"], fixture=INVENTORY_FIXTURE)
                receipt = json_object(parse_json((output / "exits.jsonl").read_text()))
                self.assertEqual(receipt["child_exit"], 7 if kill_error else None)
                self.assertIn("identity failure", str(receipt["error"]))
                self.assertTrue(receipt["cleanup_errors"])
                self.assertTrue(receipt["collection_errors"])
                self.assertFalse(receipt["complete"])

    def test_inventory_resource_guard_and_whole_workload_deadline_fail_with_receipts(self) -> None:
        for resource in ("deadline", "disk", "case"):
            with self.subTest(resource=resource), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "campaign"
                args = Arguments()
                args.binary, args.output, args.cycles, args.mode = Path(sys.executable), output, 1, INVENTORY_FIXTURE.mode
                with patch("tools.native_recovery_campaign.time.monotonic",
                           side_effect=[0., 241.] if resource == "deadline" else [0., 0.]), patch(
                    "tools.native_recovery_campaign.DISK_FLOOR", 2**100 if resource == "disk" else 0
                ), patch("tools.native_recovery_campaign.MAX_CASE_BYTES", -1 if resource == "case" else 64 * 1024**2), patch(
                    "tools.native_recovery_campaign.player_child"
                ) as child:
                    with self.assertRaisesRegex(RuntimeError, "deadline" if resource == "deadline" else "floor|byte bound"):
                        authored_campaign(args)
                    child.assert_not_called()
                controller = json_object(parse_json((output / "controller.json").read_text()))
                self.assertFalse(controller["complete"])
                self.assertEqual(controller["children"], 0)
        with patch.object(Path, "read_text", return_value="MemAvailable: 1 kB\n"):
            with self.assertRaisesRegex(RuntimeError, "memory floor"):
                player_preflight()
        with patch.object(Path, "read_text", return_value="MemTotal: 1 kB\n"):
            with self.assertRaisesRegex(RuntimeError, "missing"):
                player_preflight()

    def test_mocked_child_deadline_and_resource_failures_always_journal(self) -> None:
        for deadline, case_limit in ((0., 64 * 1024**2), (float("inf"), -1)):
            with self.subTest(deadline=deadline), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                with patch("tools.native_recovery_campaign.DISK_FLOOR", 0), patch(
                    "tools.native_recovery_campaign.MAX_CASE_BYTES", case_limit
                ), patch("tools.native_recovery_campaign.subprocess.Popen") as spawn:
                    with self.assertRaisesRegex(RuntimeError, "deadline|byte bound"):
                        _ = player_child(Path("unused"), output, output / "state", "mock", ["init"],
                                           fixture=INVENTORY_FIXTURE, workload_deadline=deadline)
                    spawn.assert_not_called()
                receipt = json_object(parse_json((output / "exits.jsonl").read_text()))
                self.assertIsNone(receipt["child_exit"])
                self.assertIsNone(receipt["pid"])
                self.assertFalse(receipt["collection_complete"])
                self.assertFalse(receipt["complete"])

    def test_partial_or_failed_exit_receipts_cannot_be_admitted(self) -> None:
        for change in ({"complete": False}, {"child_exit": 7}, {"child_exit": None}, {"collection_complete": False},
                       {"cleanup_errors": ["close failed"]}, {"collection_errors": [{"event": "collector_error"}]},
                       {"label": "wrong"}, {"pid": 0}):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                journal_exit(output, "init", 0, 123)
                path = output / "exits.jsonl"
                record = json_object(parse_json(path.read_text()))
                _ = path.write_text(json.dumps({**record, **change}) + "\n")
                with self.assertRaises((RuntimeError, ValueError)):
                    validate_player_exits(output, [("init", 0, 123)])
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            journal_exit(output, "init", 0, 123)
            path = output / "exits.jsonl"
            _ = path.write_text(path.read_text().rstrip("\n"))
            with self.assertRaisesRegex(RuntimeError, "incomplete"):
                validate_player_exits(output, [("init", 0, 123)])

    def test_live_mock_deadline_and_malformed_collection_deny_both_success_receipts(self) -> None:
        for malformed in (False, True):
            with self.subTest(malformed=malformed), tempfile.TemporaryDirectory() as directory:
                output = Path(directory) / "campaign"
                args = Arguments()
                args.binary, args.output, args.cycles, args.mode = Path(sys.executable), output, 1, INVENTORY_FIXTURE.mode
                child = MockChild(StringIO("malformed\n" if malformed else ""), code=None)
                calls = 0

                def guarded(_output: Path, _deadline: float, _fixture: PlayerFixture) -> None:
                    nonlocal calls
                    calls += 1
                    if not malformed and calls >= 3:
                        raise RuntimeError("fixture workload deadline")

                with patch("tools.native_recovery_campaign.player_guard", side_effect=guarded), patch(
                    "tools.native_recovery_campaign.subprocess.Popen", return_value=child
                ), patch("tools.native_recovery_campaign.player_preflight"):
                    with self.assertRaisesRegex(RuntimeError, "identity failure|deadline"):
                        authored_campaign(args)
                self.assertTrue(child.killed)
                receipt = json_object(parse_json((output / "exits.jsonl").read_text()))
                self.assertEqual(receipt["child_exit"], -signal.SIGKILL)
                self.assertFalse(receipt["complete"])
                controller = json_object(parse_json((output / "controller.json").read_text()))
                self.assertFalse(controller["complete"])
                self.assertEqual(controller["controller_exit"], 1)

    def test_stdout_close_failure_is_journaled_and_turns_success_into_failure(self) -> None:
        class BadClose(StringIO):
            @override
            def close(self) -> None:
                if not self.closed:
                    super().close()
                    raise OSError("fixture close failure")

        nonce = UUID(int=1)
        record = {**self.record("initialized", 1, 0), "run_id": nonce.hex}
        child = MockChild(BadClose(json.dumps(record) + "\n"))
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with patch("tools.native_recovery_campaign.uuid.uuid4", return_value=nonce), patch(
                "tools.native_recovery_campaign.subprocess.Popen", return_value=child
            ), patch("tools.native_recovery_campaign.DISK_FLOOR", 0):
                with self.assertRaisesRegex(RuntimeError, "cleanup incomplete"):
                    _ = player_child(Path("unused"), output, output / "state", "mock", ["init"], fixture=INVENTORY_FIXTURE)
            receipt = json_object(parse_json((output / "exits.jsonl").read_text()))
            self.assertEqual(receipt["child_exit"], 0)
            self.assertEqual(receipt["cleanup_errors"], ["close: fixture close failure"])
            self.assertFalse(receipt["complete"])

    def test_byte_bounds_cover_utf8_record_aggregate_logs_receipts_and_state(self) -> None:
        for source, limit in ((json.dumps({"message": "é" * 8200}, ensure_ascii=False) + "\n", 1024**2),
                              ('{"event":"one"}\n{"event":"two"}\n', 20)):
            records: list[JsonObject] = []
            events: queue.Queue[JsonObject] = queue.Queue()
            with patch("tools.native_recovery_campaign.MAX_OUTPUT", limit):
                strict_player_events(StringIO(source), StringIO(), records, events)
            self.assertEqual(records[-1]["event"], "collector_error")
        for filename, limit in (("child.stderr", "MAX_OUTPUT"), ("state/snapshot.json", "MAX_CASE_BYTES"),
                                ("receipts.json", "MAX_OUTPUT")):
            with self.subTest(filename=filename), tempfile.TemporaryDirectory() as directory:
                output = Path(directory)
                path = output / filename
                path.parent.mkdir(exist_ok=True)
                _ = path.write_text("{}\n")
                with patch(f"tools.native_recovery_campaign.{limit}", 1), patch(
                    "tools.native_recovery_campaign.DISK_FLOOR", 0
                ), patch("tools.native_recovery_campaign.subprocess.Popen") as child:
                    with self.assertRaisesRegex(RuntimeError, "bound"):
                        if filename == "receipts.json":
                            _ = player_child(Path("unused"), output, output / "state", "mock",
                                               ["verify", "--receipts", str(path)], fixture=INVENTORY_FIXTURE)
                        else:
                            player_guard(output, float("inf"), INVENTORY_FIXTURE)
                    child.assert_not_called()

    def test_case_guard_tolerates_a_retired_generation_during_enumeration(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            # The path is deliberately absent, as after an atomic publication
            # retires a generation already returned by directory enumeration.
            with patch.object(Path, "rglob", return_value=iter([output / "retired.json"])), patch(
                "tools.native_recovery_campaign.DISK_FLOOR", 0
            ):
                player_guard(output, float("inf"), INVENTORY_FIXTURE)


if __name__ == "__main__":
    _ = unittest.main()
