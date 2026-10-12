"""Authored runtime JSON fixtures; no container or original-server execution."""

import hashlib
from io import StringIO
import json
from pathlib import Path
import queue
import tempfile
import unittest

from tools.native_recovery_campaign import collect_events, read_state
from tools.runtime_types import (
    JsonObject,
    integer_field,
    json_object,
    parse_container_identity,
    parse_container_state,
    parse_json,
)


class RuntimeTypesTest(unittest.TestCase):
    def test_json_retains_nested_values_and_integer_fields_reject_boolean(self) -> None:
        value = json_object(parse_json('{"tick": 15, "nested": [null, true, {"x": 1.5}]}'))
        self.assertEqual(integer_field(value, "tick"), 15)
        self.assertEqual(value["nested"], [None, True, {"x": 1.5}])
        with self.assertRaisesRegex(ValueError, "must be an integer"):
            _ = integer_field({"tick": True}, "tick")
        with self.assertRaisesRegex(ValueError, "expected a JSON object"):
            _ = json_object(parse_json("[]"))

    def test_container_identity_and_state_validate_independently(self) -> None:
        identity = parse_container_identity('[{"Id":"owned", "Config":{"Labels":{"icarus.research.run":"run-1"}}}]')
        self.assertEqual(identity.container_id, "owned")
        self.assertEqual(identity.labels.get("icarus.research.run"), "run-1")
        state = parse_container_state('[{"State":{"Running":false,"ExitCode":0}}]')
        self.assertFalse(state.running)
        self.assertEqual(state.exit_code, 0)
        for text in ('[]', '[{"State":{"Running":"false","ExitCode":0}}]', '[{"State":{"Running":false,"ExitCode":true}}]'):
            with self.assertRaises(ValueError):
                _ = parse_container_state(text)
        with self.assertRaises(ValueError):
            _ = parse_container_identity('[{"Id":"owned", "Config":{"Labels":{"icarus.research.run":true}}}]')

    def test_snapshot_checksum_is_verified_before_state_is_returned(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            payload = b'{"schema_version":1,"tick":15,"entities":[]}'
            _ = (state / "snapshot-2.json").write_bytes(payload)
            _ = (state / "manifest.json").write_text(json.dumps({
                "current": {"number": 2, "sha256": hashlib.sha256(payload).hexdigest()}
            }))
            generation, snapshot = read_state(state)
            self.assertEqual(generation, 2)
            self.assertEqual(snapshot["tick"], 15)
            _ = (state / "snapshot-2.json").write_bytes(payload + b" ")
            with self.assertRaisesRegex(RuntimeError, "checksum mismatch"):
                _ = read_state(state)

    def test_collector_reports_malformed_records_and_always_signals_eof(self) -> None:
        events: queue.Queue[JsonObject] = queue.Queue()
        records: list[JsonObject] = []
        log = StringIO()
        source = 'ordinary log line\n{"event":"started","tick":0}\n[]\n'
        collect_events(StringIO(source), log, records, events)
        self.assertEqual(log.getvalue(), source)
        self.assertEqual(events.get_nowait()["event"], "started")
        failure = events.get_nowait()
        self.assertEqual(failure["event"], "collector_error")
        self.assertEqual(failure["message"], "expected a JSON object")
        self.assertEqual(events.get_nowait()["event"], "process_eof")
        self.assertEqual(len(records), 2)


if __name__ == "__main__":
    _ = unittest.main()
