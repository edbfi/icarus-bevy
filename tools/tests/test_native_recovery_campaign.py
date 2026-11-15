"""Bounded source-authored controller fixtures; never launch the recovery workload."""
from contextlib import redirect_stdout
from io import StringIO
import json
from pathlib import Path
import queue
import signal
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools.native_recovery_campaign import (
    Arguments, PLAYER_PROTOCOL, PlayerState, authored_campaign, player_child, player_guard, strict_player_events,
    validate_player_cycle,
)
from tools.runtime_types import JsonObject, json_object, parse_json


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
                verified: JsonObject = {"generation": 2, "tick": 3, "digest": "a" * 64,
                    "resumed_tick": resumed_tick, "acknowledgements": 2, "schema": 14}
                results = [([self.record("initialized", 1, 0)], 0, 123),
                    ([self.started(), self.record("save_committed")], -signal.SIGKILL, 123),
                    ([verified], 0, 124)]
                with patch("tools.native_recovery_campaign.player_child", side_effect=results), patch(
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
                "tools.native_recovery_campaign.time.monotonic", side_effect=[0., 0., 21.]
            ):
                with self.assertRaisesRegex(RuntimeError, "deadline"):
                    _ = player_child(binary, path, path / "state", "mock", ["init"])
            exit_record = json_object(parse_json((path / "exits.jsonl").read_text()))
            self.assertEqual(exit_record["child_exit"], -signal.SIGKILL)
            self.assertFalse(exit_record["signal_sent"])


if __name__ == "__main__":
    _ = unittest.main()
