# SPDX-License-Identifier: AGPL-3.0-or-later
"""The memory MCP server: JSON-RPC over an in-process pipe against the fake bank, then a real stdio
session with ``python -m tools.memory mcp``. Invented records only; no service, no network."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path
from typing import BinaryIO, cast, override
from unittest.mock import patch

from tools.memory.bank import Bank
from tools.memory.journal import read_tree
from tools.memory.mcp import PROTOCOL_VERSIONS, Server, serve
from tools.memory.model import JSON
from tools.tests.memory_fixtures import NEW_SERVER, chain, clean_git_environment, commit_all, new_repository, write
from tools.tests.test_memory_online import FakeBank

TOOL_NAMES = ["get", "recall", "area", "status", "propose", "revise", "supersede", "resolve", "evidence_add", "verify", "check"]
WORKTREE = Path(__file__).resolve().parents[2]


class Pipe:
    """A server thread reading one OS pipe and writing another, as a client sees it over stdio."""

    def __init__(self, server: Server) -> None:
        read_in, write_in = os.pipe()
        read_out, write_out = os.pipe()
        self.requests: BinaryIO = os.fdopen(write_in, "wb", buffering=0)
        self.responses: BinaryIO = os.fdopen(read_out, "rb", buffering=0)
        self.stdin: BinaryIO = os.fdopen(read_in, "rb")
        self.stdout: BinaryIO = os.fdopen(write_out, "wb")
        self.thread: threading.Thread = threading.Thread(target=self.run, args=(server,), daemon=True)
        self.thread.start()

    def run(self, server: Server) -> None:
        try:
            serve(server, self.stdin, self.stdout)
        finally:
            self.stdout.close()

    def send(self, message: JSON | str) -> None:
        _ = self.requests.write((message if isinstance(message, str) else json.dumps(message)).encode() + b"\n")

    def receive(self) -> dict[str, JSON]:
        line = b""
        while not line.endswith(b"\n"):
            chunk = self.responses.read(1)
            if not chunk:
                raise AssertionError(f"server closed the pipe after {line!r}")
            line += chunk
        return cast(dict[str, JSON], json.loads(line))

    def request(self, identifier: int, method: str, params: JSON = None) -> dict[str, JSON]:
        message: dict[str, JSON] = {"jsonrpc": "2.0", "id": identifier, "method": method}
        if params is not None:
            message["params"] = params
        self.send(message)
        return self.receive()

    def close(self) -> bytes:
        self.requests.close()
        self.thread.join(10)
        rest = self.responses.read()
        self.responses.close()
        self.stdin.close()
        return rest


def result(response: dict[str, JSON]) -> dict[str, JSON]:
    found = response.get("result")
    assert isinstance(found, dict), response
    return found


def error_code(response: dict[str, JSON]) -> JSON:
    found = response.get("error")
    assert isinstance(found, dict), response
    return found.get("code")


class Protocol(unittest.TestCase):
    root: Path = Path()
    bank: FakeBank = FakeBank()
    pipe: Pipe | None = None

    @override
    def setUp(self) -> None:
        top = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.root = top / "primary"
        self.root.mkdir()
        self.enterContext(clean_git_environment())
        self.enterContext(patch.dict(os.environ, {"ICARUS_MEMORY_HOME": str(top / "home")}))
        new_repository(self.root)
        (self.root / "config").mkdir()
        _ = (self.root / "config/release.toml").write_text(f"[release]\nbuild_id = {NEW_SERVER}\n", encoding="utf-8")
        _ = write(self.root, chain())
        commit_all(self.root)
        self.bank = FakeBank()
        bank = self.bank

        def factory() -> Bank:
            return bank

        self.pipe = Pipe(Server(self.root, factory))

    @override
    def tearDown(self) -> None:
        if self.pipe is not None:
            self.assertEqual(b"", self.pipe.close(), "nothing but responses on standard output")

    def call(self, identifier: int, name: str, arguments: JSON) -> dict[str, JSON]:
        assert self.pipe is not None
        return result(self.pipe.request(identifier, "tools/call", {"name": name, "arguments": arguments}))

    def test_handshake_and_tool_list(self) -> None:
        assert self.pipe is not None
        hello = result(self.pipe.request(1, "initialize", {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}))
        self.assertEqual("2025-03-26", hello["protocolVersion"])
        self.assertEqual({"tools": {"listChanged": False}}, hello["capabilities"])
        self.pipe.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        self.assertEqual({"jsonrpc": "2.0", "id": 2, "result": {}}, self.pipe.request(2, "ping"))
        newer = result(self.pipe.request(3, "initialize", {"protocolVersion": "2099-01-01"}))
        self.assertEqual(PROTOCOL_VERSIONS[0], newer["protocolVersion"])
        tools = result(self.pipe.request(4, "tools/list"))["tools"]
        assert isinstance(tools, list)
        names = [tool["name"] for tool in tools if isinstance(tool, dict)]
        self.assertEqual(TOOL_NAMES, names)
        for tool in tools:
            assert isinstance(tool, dict)
            schema, description = tool["inputSchema"], tool["description"]
            assert isinstance(schema, dict) and isinstance(description, str)
            self.assertEqual(("object", False), (schema["type"], schema["additionalProperties"]))
            self.assertIn(f"checkout this server was started in ({self.root})", description)
        self.assertFalse({"sync", "rebuild", "backup", "restore", "service", "salvage"} & set(map(str, names)))

    def test_reads_and_writes_call_the_cli_functions(self) -> None:
        got = self.call(1, "get", {"id": "F-102"})
        structured = got["structuredContent"]
        assert isinstance(structured, dict) and isinstance(structured["data"], dict)
        self.assertEqual((False, 0, "F-102", "superseded"), (got["isError"], structured["exit_status"], structured["data"]["id"], structured["data"]["status"]))
        content = got["content"]
        assert isinstance(content, list) and isinstance(content[0], dict)
        self.assertTrue(str(content[0]["text"]).startswith("F-102 finding superseded\n"))
        before = len(read_tree(self.root))
        proposed = self.call(2, "propose", {"type": "finding", "fields": {
            "claim": "Invented: proposed over MCP", "kind": "observed", "confidence": "low", "standing": "candidate", "area": "listing",
            "scope": {"basis": "explicit", "subjects": ["original-server"], "builds": [f"server:{NEW_SERVER}"]}}})
        self.assertFalse(proposed["isError"])
        self.assertEqual(before + 1, len(read_tree(self.root)))
        self.assertIn("F-107", self.bank.held)
        superseded = self.call(3, "supersede", {"old": "F-106", "by": "F-107"})
        self.assertFalse(superseded["isError"])
        self.assertIn("status:active", self.bank.held["F-106"].tags)
        recalled = self.call(4, "recall", {"query": "lock follows the key"})
        recall_text = recalled["content"]
        assert isinstance(recall_text, list) and isinstance(recall_text[0], dict)
        self.assertIn("proposed superseder: F-107 (proposed, this worktree)", str(recall_text[0]["text"]))
        checked = self.call(5, "check", {"ci": True})
        structured = checked["structuredContent"]
        assert isinstance(structured, dict)
        self.assertEqual(0, structured["exit_status"])
        others: list[tuple[str, JSON]] = [("area", {"area": "listing"}), ("status", {}), ("verify", {"all": True}), ("check", {"bank": True, "pin": True})]
        for name, arguments in others:
            self.assertFalse(self.call(6, name, arguments)["isError"], name)

    def test_errors_are_json_rpc_error_objects_and_refusals_are_tool_errors(self) -> None:
        assert self.pipe is not None
        self.pipe.send("{not json")
        parsed = self.pipe.receive()
        self.assertEqual((None, -32700), (parsed["id"], error_code(parsed)))
        self.assertEqual(-32601, error_code(self.pipe.request(1, "resources/list")))
        self.assertEqual(-32602, error_code(self.pipe.request(2, "tools/call", {"name": "sync", "arguments": {}})))
        self.assertEqual(-32602, error_code(self.pipe.request(3, "tools/call", {"name": "get", "arguments": {}})))
        self.assertEqual(-32602, error_code(self.pipe.request(4, "tools/call", {"name": "get", "arguments": {"id": "F-101", "extra": 1}})))
        self.assertEqual(-32602, error_code(self.pipe.request(5, "tools/call", {"name": "get", "arguments": {"id": 7}})))
        self.pipe.send({"jsonrpc": "1.0", "id": 6, "method": "ping"})
        self.assertEqual(-32600, error_code(self.pipe.receive()))
        self.pipe.send([{"jsonrpc": "2.0", "id": 7, "method": "ping"}])
        self.assertEqual(-32600, error_code(self.pipe.receive()))
        refused = self.call(8, "propose", {"type": "finding", "fields": {"id": "F-900", "claim": "x"}})
        content = refused["content"]
        assert isinstance(content, list) and isinstance(content[0], dict)
        self.assertTrue(refused["isError"])
        self.assertIn("allocates IDs", str(content[0]["text"]))
        missing = self.call(9, "get", {"id": "F-999"})
        self.assertTrue(missing["isError"])


class StdioSession(unittest.TestCase):
    """``python -m tools.memory mcp`` as a client starts it: a raw JSON-RPC session over its stdin and stdout."""

    def test_a_raw_session(self) -> None:
        with tempfile.TemporaryDirectory() as directory, clean_git_environment():
            top = Path(directory)
            root = top / "checkout"
            root.mkdir()
            new_repository(root)
            _ = write(root, chain())
            commit_all(root)
            requests: list[JSON] = [
                {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                                                                              "clientInfo": {"name": "raw", "version": "0"}}},
                {"jsonrpc": "2.0", "method": "notifications/initialized"},
                {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "check", "arguments": {"ci": True}}},
                {"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "status", "arguments": {}}},
            ]
            environment = {**os.environ, "ICARUS_MEMORY_HOME": str(top / "no-service"), "PYTHONPATH": str(WORKTREE), "PYTHONDONTWRITEBYTECODE": "1"}
            done = subprocess.run([sys.executable, "-m", "tools.memory", "mcp"], cwd=root, env=environment, check=False, timeout=120,
                                  input="".join(json.dumps(item) + "\n" for item in requests).encode(), capture_output=True)
        self.assertEqual(0, done.returncode, done.stderr)
        responses = [cast(dict[str, JSON], json.loads(line)) for line in done.stdout.decode().splitlines()]
        self.assertEqual([1, 2, 3, 4], [item["id"] for item in responses])
        self.assertEqual("2025-06-18", result(responses[0])["protocolVersion"])
        tools = result(responses[1])["tools"]
        assert isinstance(tools, list)
        self.assertEqual(TOOL_NAMES, [tool["name"] for tool in tools if isinstance(tool, dict)])
        checked = result(responses[2])
        structured = checked["structuredContent"]
        assert isinstance(structured, dict)
        self.assertEqual((False, 0), (checked["isError"], structured["exit_status"]))
        status = result(responses[3])
        content = status["content"]
        assert isinstance(content, list) and isinstance(content[0], dict)
        self.assertTrue(status["isError"])
        self.assertIn("connection.env", str(content[0]["text"]))
