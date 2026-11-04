"""Authored field rows only; no capture or original input is read."""

from dataclasses import replace
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools.capture_rows_to_jsonl import Endpoint, Limits, convert  # noqa: E402


CLIENT = Endpoint("192.0.2.1", 40000)
SERVER = Endpoint("192.0.2.2", 17777)


def row(frame: int = 1, time: str = "100.999999999999999999", reverse: bool = False) -> dict[str, list[str]]:
    src, dst = (SERVER, CLIENT) if reverse else (CLIENT, SERVER)
    return {key: [value] for key, value in {
        "frame.number": str(frame), "frame.time_epoch": time,
        "frame.len": "44", "frame.cap_len": "44", "ip.src": src.address,
        "ip.dst": dst.address, "udp.srcport": str(src.port), "udp.dstport": str(dst.port),
        "udp.length": "10", "udp.payload": "AA:00", "ip.flags.mf": "0", "ip.frag_offset": "0",
    }.items()}


class ConverterTest(unittest.TestCase):
    def fixture(self, root: Path, text: str) -> tuple[Path, Path, Path]:
        work = root / "work"
        work.mkdir()
        source = work / "rows.jsonl"
        _ = source.write_text(text)
        return source, work / "datagrams.jsonl", work

    def test_exact_endpoint_selection_order_duplicates_and_decimal_floor(self) -> None:
        rows = [row(), row(2, "101.000999999999999998", True), row(3, "101.001", True)]
        unrelated = row(4)
        unrelated["udp.srcport"] = ["40001"]
        rows.insert(1, unrelated)
        with tempfile.TemporaryDirectory() as directory:
            source, output, work = self.fixture(Path(directory), "".join(json.dumps(item) + "\n" for item in rows))
            self.assertEqual(convert(source, output, work, CLIENT, SERVER), 3)
            self.assertEqual(output.read_text(),
                             '{"event":"datagram","direction":"client_to_server","ms":0,"hex":"aa00"}\n'
                             + '{"event":"datagram","direction":"server_to_client","ms":0,"hex":"aa00"}\n'
                             + '{"event":"datagram","direction":"server_to_client","ms":1,"hex":"aa00"}\n')
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)

    def test_invalid_selected_packets_and_order_leave_incomplete_output(self) -> None:
        changes: list[tuple[str, list[str]]] = [("frame.cap_len", ["43"]), ("ip.flags.mf", ["1"]),
                   ("ip.frag_offset", ["1"]), ("udp.length", ["11"]),
                   ("udp.payload", ["a:00"]), ("frame.number", ["1"]),
                   ("frame.time_epoch", ["99"]), ("ip.flags.mf", []),
                   ("udp.payload", ["00", "01"])]
        for key, values in changes:
            with self.subTest(key=key, values=values), tempfile.TemporaryDirectory() as directory:
                bad = row(2, "102")
                bad[key] = values
                source, output, work = self.fixture(Path(directory), json.dumps(row()) + "\n" + json.dumps(bad) + "\n")
                with self.assertRaisesRegex(ValueError, "incomplete output.*1 datagrams"):
                    _ = convert(source, output, work, CLIENT, SERVER)
                self.assertEqual(len(output.read_text().splitlines()), 1)

    def test_fifo_is_rejected_before_output_creation(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fifo = root / "input.fifo"
            os.mkfifo(fifo)
            output = root / "output.jsonl"
            with self.assertRaisesRegex(ValueError, "regular file"):
                _ = convert(fifo, output, root, CLIENT, SERVER)
            self.assertFalse(output.exists())

    def test_empty_udp_payload_cannot_produce_decoder_input(self) -> None:
        empty = row()
        empty["udp.payload"] = [""]
        empty["udp.length"] = ["8"]
        with tempfile.TemporaryDirectory() as directory:
            source, output, work = self.fixture(Path(directory), json.dumps(empty) + "\n")
            with self.assertRaisesRegex(ValueError, "incomplete output"):
                _ = convert(source, output, work, CLIENT, SERVER)
            self.assertEqual(output.read_bytes(), b"")

    def test_malformed_json_and_duplicate_keys(self) -> None:
        for text in ['[]\n', '{"x":[],"x":[]}\n', '{\n', '{"x":[1]}\n']:
            with self.subTest(text=text), tempfile.TemporaryDirectory() as directory:
                source, output, work = self.fixture(Path(directory), text)
                with self.assertRaisesRegex(ValueError, "incomplete output"):
                    _ = convert(source, output, work, CLIENT, SERVER)
                self.assertEqual(output.read_bytes(), b"")

    def test_resource_ceilings_and_configuration_preflight(self) -> None:
        text = json.dumps(row()) + "\n"
        limits = [replace(Limits(), input_bytes=1), replace(Limits(), line_bytes=1),
                  replace(Limits(), rows=1), replace(Limits(), output_bytes=1),
                  replace(Limits(), rows=0), replace(Limits(), rows=100001)]
        for limit in limits:
            with self.subTest(limits=limit), tempfile.TemporaryDirectory() as directory:
                source, output, work = self.fixture(Path(directory), text * 2)
                with self.assertRaises(ValueError):
                    _ = convert(source, output, work, CLIENT, SERVER, limit)
                if limit.input_bytes == 1 or limit.rows in (0, 100001):
                    self.assertFalse(output.exists())

    def test_exclusive_output_and_path_preflight(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source, output, work = self.fixture(root, json.dumps(row()) + "\n")
            with self.assertRaises(ValueError):
                _ = convert(source, root / "outside", work, CLIENT, SERVER)
            self.assertFalse((root / "outside").exists())
            with self.assertRaises(ValueError):
                _ = convert(source, output, work, CLIENT, CLIENT)
            self.assertFalse(output.exists())
            _ = output.write_text("sentinel")
            with self.assertRaises(FileExistsError):
                _ = convert(source, output, work, CLIENT, SERVER)
            self.assertEqual(output.read_text(), "sentinel")
            link = work / "link"
            link.symlink_to(source)
            with self.assertRaises(FileExistsError):
                _ = convert(source, link, work, CLIENT, SERVER)
            self.assertTrue(link.is_symlink())
            with self.assertRaises(FileExistsError):
                _ = convert(source, source, work, CLIENT, SERVER)


if __name__ == "__main__":
    _ = unittest.main()
