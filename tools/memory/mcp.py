# SPDX-License-Identifier: AGPL-3.0-or-later
"""``python3 -m tools.memory mcp``: the memory tool as a stdio MCP server (JSON-RPC 2.0), stdlib only.

Transport: one JSON-RPC message per line on standard input and output (UTF-8, no embedded newline).
Standard output carries protocol messages only; anything a command might print goes to standard error.
Methods: ``initialize`` (protocol version negotiation), ``notifications/initialized``, ``ping``,
``tools/list`` and ``tools/call``. Errors are JSON-RPC error objects: -32700 parse error, -32600 invalid
request, -32601 unknown method, -32602 invalid params (unknown tool, arguments outside its schema). A
command that refuses or fails returns a tool result with ``isError: true`` and the message.

Each tool calls the same plain function as the CLI command of the same name (``online.get``,
``online.recall``, ... and ``check.run_check``) and returns its rendered text as ``content`` and
``{"exit_status": N, "data": {...}}`` as ``structuredContent``. Lead-only and service operations
(``sync``, ``rebuild``, ``backup``, ``restore``, ``service``, ``salvage``) are not exposed.

The working directory of the server process decides the checkout: reads replay its journal first and
writes land in its ``memory/journal/``. Clients start the server in the checkout they work in.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Callable, Mapping
from contextlib import redirect_stdout
from dataclasses import dataclass
from pathlib import Path
from typing import BinaryIO, cast

from tools.memory import online
from tools.memory.bank import Bank, BankError
from tools.memory.check import run_check
from tools.memory.gitio import GitError, git
from tools.memory.hindsight import Client, HindsightError
from tools.memory.journal import JournalError
from tools.memory.model import JSON, TYPES, RecordError, is_array, is_object
from tools.memory.paths import ProtectedPathsError
from tools.memory.service import ServiceError, Subcommands, connection

PROTOCOL_VERSIONS = ("2025-06-18", "2025-03-26", "2024-11-05")
SERVER = {"name": "memory", "title": "icarus memory", "version": "1"}
PARSE_ERROR, INVALID_REQUEST, METHOD_NOT_FOUND, INVALID_PARAMS, INTERNAL_ERROR = -32700, -32600, -32601, -32602, -32603
FAILURES = (online.Refused, RecordError, JournalError, GitError, LookupError, HindsightError, BankError, ProtectedPathsError, ServiceError, OSError)

type Arguments = Mapping[str, JSON]


class ProtocolError(Exception):
    """A JSON-RPC error: ``code`` and message."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code: int = code


@dataclass(frozen=True)
class Context:
    """What a tool needs: the checkout and a way to reach the bank (the service, or a test's fake)."""

    root: Path
    bank: Callable[[], Bank]


@dataclass(frozen=True)
class Tool:
    name: str
    description: str
    properties: Mapping[str, JSON]
    required: tuple[str, ...]
    call: Callable[[Context, Arguments], online.Output]

    def json(self, root: Path) -> JSON:
        schema: dict[str, JSON] = {"type": "object", "properties": dict(self.properties), "additionalProperties": False}
        if self.required:
            schema["required"] = list(self.required)
        where = f" Reads and writes the checkout this server was started in ({root}): writes land in its memory/journal/."
        return {"name": self.name, "description": self.description + where, "inputSchema": schema}


def text(argument: Arguments, name: str) -> str:
    value = argument.get(name)
    return value if isinstance(value, str) else ""


def optional(argument: Arguments, name: str) -> str | None:
    value = argument.get(name)
    return value if isinstance(value, str) else None


def flag(argument: Arguments, name: str) -> bool:
    return argument.get(name) is True


def changes(argument: Arguments) -> online.Changes:
    fields = argument.get("fields")
    return online.Changes(dict(fields) if isinstance(fields, dict) else {}, optional(argument, "note"), optional(argument, "body"))


def run_object(argument: Arguments) -> Mapping[str, JSON] | None:
    run = argument.get("run")
    return run if isinstance(run, dict) else None


def strings(argument: Arguments, name: str) -> list[str]:
    value = argument.get(name)
    return [item for item in value if isinstance(item, str)] if isinstance(value, list) else []


def check(context: Context, argument: Arguments) -> online.Output:
    """``check``: ``ci`` (no service), ``bank``, ``pin``; none of them means ``ci`` and ``bank``, as on the CLI."""
    ci, bank, pin = flag(argument, "ci"), flag(argument, "bank"), flag(argument, "pin")
    plain = not (ci or bank or pin)
    merged = online.Output({}, [], 0)
    if ci or plain:
        report = run_check(context.root, optional(argument, "base") or "origin/main")
        if report.no_journal:
            merged.lines.append("memory check: no journal (memory/journal/ holds no segment); nothing checked")
            merged.status = 3
        else:
            merged.lines.extend([*report.problems, *(f"notice: {notice}" for notice in report.notices),
                                 f"memory check: {len(report.problems)} problem(s), {len(report.notices)} notice(s)"])
            merged.status = 1 if report.problems else 0
        merged.data["ci"] = {"problems": list[JSON](report.problems), "notices": list[JSON](report.notices), "no_journal": report.no_journal}
    for wanted, produce in ((pin, lambda: online.check_pin(context.root)), (bank or plain, lambda: online.check_bank(context.bank(), context.root))):
        if wanted:
            output = produce()
            merged.data.update(output.data)
            merged.lines.extend(output.lines)
            merged.status = max(merged.status, output.status)
    return merged


STRING: JSON = {"type": "string"}
BOOLEAN: JSON = {"type": "boolean"}
FIELDS: JSON = {"type": "object", "description": "record fields to set, as the record's JSON names them (null removes one)"}
NOTE: JSON = {"type": "string", "description": "a note to append"}
BODY: JSON = {"type": "string", "description": "a reference's Markdown body"}
TOOLS: tuple[Tool, ...] = (
    Tool("get", "Exact GET of one record (ID or evidence selector M-N/ev_<12 hex>), verified against the journal: state, status from " +
         "accepted links, partition, pending revisions, conflicts, evidence status and this worktree's proposed superseders and resolvers.",
         {"id": STRING, "history": BOOLEAN, "all_proposals": BOOLEAN}, ("id",),
         lambda c, a: online.get(c.bank(), c.root, text(a, "id"), history=flag(a, "history"), all_proposals=flag(a, "all_proposals"))),
    Tool("recall", "Ranked search with strict tags, partitioned against a release (default the pin); IDs named in the query are fetched by exact " +
         "GET first. Accepted records plus this worktree's proposals; other worktrees' with all_proposals; evidence with include_evidence; " +
         "text adds a substring search.",
         {"query": STRING, "release": STRING, "text": STRING, "all_proposals": BOOLEAN, "include_evidence": BOOLEAN}, ("query",),
         lambda c, a: online.recall(c.bank(), c.root, text(a, "query"), release=optional(a, "release"), text=optional(a, "text"),
                                    all_proposals=flag(a, "all_proposals"), include_evidence=flag(a, "include_evidence"))),
    Tool("area", "One area's records by status and partition, its open unknowns and the closure of their links.", {"area": STRING}, ("area",),
         lambda c, a: online.area(c.bank(), c.root, text(a, "area"))),
    Tool("status", "The pin; counts by type, status and partition; cited records not current; open unknowns; evidence not known good on disk.",
         {}, (), lambda c, _a: online.status(c.bank(), c.root)),
    Tool("propose", "Propose a new record of a type; the tool allocates the ID (a release names its REL-x.y.z in fields.id). Schema, scope, " +
         "link targets, selectors and CP proofs are checked before the segment is written.",
         {"type": {"type": "string", "enum": list[JSON](TYPES)}, "fields": FIELDS, "note": NOTE, "body": BODY}, ("type",),
         lambda c, a: online.propose(c.bank(), c.root, text(a, "type"), changes(a))),
    Tool("revise", "Revise a record in place: only what the rules make revisable (REF body, unknown status and next probe, evidence status " +
         "with a note, appended notes, verified_on, supersedes and resolves, stored status). A record with two heads is reconciled.",
         {"id": STRING, "fields": FIELDS, "note": NOTE, "body": BODY}, ("id",),
         lambda c, a: online.revise(c.bank(), c.root, text(a, "id"), changes(a))),
    Tool("supersede", "Record on the newer record `by` that it supersedes `old`; `old` gets no revision and its status changes only once the " +
         "link is accepted.", {"old": STRING, "by": STRING}, ("old", "by"),
         lambda c, a: online.supersede(c.bank(), c.root, text(a, "old"), text(a, "by"))),
    Tool("resolve", "Record on finding `by` (F-N) that it resolves unknown, lead or report `id`.", {"id": STRING, "by": STRING}, ("id", "by"),
         lambda c, a: online.resolve(c.bank(), c.root, text(a, "id"), text(a, "by"))),
    Tool("evidence_add", "Record an evidence file (relative paths resolve against the primary checkout) with sha256, size, date and REA " +
         "bundle members taken from its bytes; kind run needs the run object.",
         {"path": STRING, "kind": {"type": "string", "enum": list[JSON](TYPES["evidence"].fields["kind"].vocabulary)}, "label": STRING,
          "run": {"type": "object", "description": "workload, targets, matched_reference, identities, start, end, environment, thresholds, " +
                  "outcome, deviations"}, "without_members": {"type": "boolean", "description": "record an REA bundle above the 64 MiB " +
                  "parse cap without listing its members"}}, ("path", "kind", "label"),
         lambda c, a: online.evidence_add(c.bank(), c.root, text(a, "path"), text(a, "kind"), text(a, "label"), run=run_object(a),
                                          without_members=flag(a, "without_members"))),
    Tool("verify", "Verify evidence bytes: verified, unavailable, mismatched or legacy-unverifiable per record (journal and disk only).",
         {"ids": {"type": "array", "items": STRING}, "all": BOOLEAN}, (),
         lambda c, a: online.verify(c.root, strings(a, "ids"), every=flag(a, "all"))),
    Tool("check", "Check the journal and citations (ci, no service), the bank against the journal (bank) and cited records not current " +
         "(pin); with none of them, ci and bank.", {"ci": BOOLEAN, "bank": BOOLEAN, "pin": BOOLEAN, "base": STRING}, (), check),
)


def matches(schema: JSON, value: JSON) -> bool:
    """``value`` against the small JSON Schema subset the tools use: type, enum and array items."""
    if not isinstance(schema, dict):
        return True
    match schema.get("type"):
        case "string":
            enum = schema.get("enum")
            return isinstance(value, str) and (not isinstance(enum, list) or value in enum)
        case "boolean":
            return isinstance(value, bool)
        case "object":
            return isinstance(value, dict)
        case "array":
            return isinstance(value, list) and all(matches(schema.get("items"), item) for item in value)
        case _:
            return True


def arguments_problem(tool: Tool, argument: Arguments) -> str | None:
    if missing := [name for name in tool.required if name not in argument]:
        return f"{tool.name}: missing {', '.join(missing)}"
    if extra := sorted(set(argument) - set(tool.properties)):
        return f"{tool.name}: unknown argument {', '.join(extra)}"
    if wrong := [name for name, value in argument.items() if not matches(tool.properties[name], value)]:
        return f"{tool.name}: {', '.join(wrong)} does not match the schema"
    return None


class Server:
    """The protocol state machine; ``handle`` maps one decoded message to its response (``None`` for a notification)."""

    def __init__(self, root: Path | None = None, bank: Callable[[], Bank] | None = None) -> None:
        self._root: Path | None = root
        self._bank: Callable[[], Bank] = bank or (lambda: Client(connection()))
        self.tools: dict[str, Tool] = {tool.name: tool for tool in TOOLS}

    def root(self) -> Path:
        if self._root is None:
            self._root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())
        return self._root

    def handle(self, message: object) -> dict[str, JSON] | None:
        if not is_object(message) or message.get("jsonrpc") != "2.0" or not isinstance(message.get("method"), str):
            identifier = message.get("id") if is_object(message) else None
            return error(identifier if isinstance(identifier, (str, int)) else None, INVALID_REQUEST, "not a JSON-RPC 2.0 request")
        method = cast(str, message["method"])
        identifier = message.get("id")
        if "id" not in message:
            return None
        if not isinstance(identifier, (str, int)) or isinstance(identifier, bool):
            return error(None, INVALID_REQUEST, "id must be a string or an integer")
        params = message.get("params", {})
        try:
            if not is_object(params):
                raise ProtocolError(INVALID_PARAMS, "params must be an object")
            return {"jsonrpc": "2.0", "id": identifier, "result": self.dispatch(method, {str(key): as_json(value) for key, value in params.items()})}
        except ProtocolError as problem:
            return error(identifier, problem.code, str(problem))
        except Exception as problem:
            # The server answers every request; an unexpected failure is an internal error, never a dead session.
            return error(identifier, INTERNAL_ERROR, f"internal error: {type(problem).__name__}: {problem}")

    def dispatch(self, method: str, params: dict[str, JSON]) -> JSON:
        match method:
            case "initialize":
                asked = params.get("protocolVersion")
                version = asked if isinstance(asked, str) and asked in PROTOCOL_VERSIONS else PROTOCOL_VERSIONS[0]
                return {"protocolVersion": version, "capabilities": {"tools": {"listChanged": False}}, "serverInfo": dict[str, JSON](SERVER),
                        "instructions": "The project's memory (.agents/rules/memory.md). Look up a known ID with get, never recall. " +
                                        "Writes land in the journal of the checkout this server runs in; the lead commits them."}
            case "ping":
                return {}
            case "tools/list":
                root = self.root_or_unknown()
                return {"tools": [tool.json(root) for tool in TOOLS]}
            case "tools/call":
                return self.call(params)
            case _:
                raise ProtocolError(METHOD_NOT_FOUND, f"unknown method {method}")

    def root_or_unknown(self) -> Path:
        try:
            return self.root()
        except GitError:
            return Path.cwd()

    def call(self, params: dict[str, JSON]) -> JSON:
        name, argument = params.get("name"), params.get("arguments", {})
        tool = self.tools.get(name) if isinstance(name, str) else None
        if tool is None:
            raise ProtocolError(INVALID_PARAMS, f"unknown tool {name!r}")
        if not isinstance(argument, dict):
            raise ProtocolError(INVALID_PARAMS, f"{tool.name}: arguments must be an object")
        if problem := arguments_problem(tool, argument):
            raise ProtocolError(INVALID_PARAMS, problem)
        try:
            with redirect_stdout(sys.stderr):
                output = tool.call(Context(self.root(), self._bank), argument)
        except FAILURES as failure:
            return {"content": [{"type": "text", "text": f"memory {tool.name}: {failure}"}], "isError": True}
        rendered = json.dumps(output.data, ensure_ascii=False, sort_keys=True) if not output.lines else "\n".join(output.lines)
        return {"content": [{"type": "text", "text": rendered}], "structuredContent": {"exit_status": output.status, "data": output.data},
                "isError": False}


def as_json(value: object) -> JSON:
    """A decoded JSON value as ``JSON``."""
    if value is None or isinstance(value, (bool, int, str)):
        return value
    if isinstance(value, float):
        raise ProtocolError(INVALID_PARAMS, "non-integer numbers are not accepted")
    if is_array(value):
        return [as_json(item) for item in value]
    if is_object(value):
        return {str(key): as_json(item) for key, item in value.items()}
    raise ProtocolError(INVALID_PARAMS, f"unsupported JSON value {value!r}")


def error(identifier: str | int | None, code: int, message: str) -> dict[str, JSON]:
    return {"jsonrpc": "2.0", "id": identifier, "error": {"code": code, "message": message}}


def serve(server: Server, stdin: BinaryIO, stdout: BinaryIO) -> None:
    """Read messages line by line until end of input; write each response as one line and flush it."""
    for raw in stdin:
        line = raw.strip()
        if not line:
            continue
        try:
            # json.loads is untyped; ``handle`` narrows the object.
            message = cast(object, json.loads(line))
        except (ValueError, UnicodeDecodeError) as problem:
            response: dict[str, JSON] | None = error(None, PARSE_ERROR, f"parse error: {problem}")
        else:
            response = server.handle(message)
        if response is not None:
            _ = stdout.write(json.dumps(response, ensure_ascii=False, separators=(",", ":")).encode() + b"\n")
            stdout.flush()


def run_mcp(_options: argparse.Namespace) -> int:
    serve(Server(), sys.stdin.buffer, sys.stdout.buffer)
    return 0


def register(commands: Subcommands) -> None:
    """``memory mcp``: serve the tools over stdio."""
    parser = commands.add_parser("mcp", help="serve the read and write commands as a stdio MCP server")
    parser.set_defaults(handler=run_mcp)
