"""Convert local caller-exported field JSONL; never dissect or capture traffic.

Each row is an object mapping field names to arrays of strings (including
ip.flags.mf and ip.frag_offset). Required fields must have exactly one value:
frame.number, frame.time_epoch, frame.len, frame.cap_len, ip.src, ip.dst,
udp.srcport, udp.dstport, udp.length, udp.payload, ip.flags.mf, ip.frag_offset.
Timestamps are unsigned decimal seconds with at most 18 fractional digits.
Outputs contain private raw datagrams and must remain beneath the work root.
"""

import argparse
from dataclasses import dataclass
from ipaddress import IPv4Address
import json
import os
from pathlib import Path
import re
import stat
import sys
from typing import cast


@dataclass(frozen=True)
class Limits:
    input_bytes: int = 64 * 1024 * 1024
    line_bytes: int = 132 * 1024
    rows: int = 100_000
    output_bytes: int = 64 * 1024 * 1024


DEFAULT_LIMITS = Limits()


@dataclass(frozen=True)
class Endpoint:
    address: str
    port: int

    @classmethod
    def parse(cls, text: str) -> "Endpoint":
        address, separator, port = text.rpartition(":")
        if not separator or not re.fullmatch(r"[0-9]{1,5}", port):
            raise ValueError("endpoint must be IPv4:port")
        result = cls(str(IPv4Address(address)), int(port))
        if not 1 <= result.port <= 65535:
            raise ValueError("endpoint port must be 1..65535")
        return result


FIELDS = (
    "frame.number", "frame.time_epoch", "frame.len", "frame.cap_len",
    "ip.src", "ip.dst", "udp.srcport", "udp.dstport", "udp.length",
    "udp.payload", "ip.flags.mf", "ip.frag_offset",
)


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("duplicate JSON key")
        result[key] = value
    return result


def parse_row(line: bytes) -> dict[str, str]:
    value = cast(object, json.loads(line, object_pairs_hook=unique_object))
    if not isinstance(value, dict):
        raise ValueError("row must be an object")
    fields: dict[str, str] = {}
    for key, values in cast(dict[object, object], value).items():
        if not isinstance(key, str) or not isinstance(values, list):
            raise ValueError("fields must map to string arrays")
        items = cast(list[object], values)
        if any(not isinstance(item, str) for item in items):
            raise ValueError("fields must map to string arrays")
        if key in FIELDS:
            if len(items) != 1 or not isinstance(items[0], str):
                raise ValueError("missing or ambiguous required field")
            fields[key] = items[0]
    if any(key not in fields for key in FIELDS):
        raise ValueError("missing required field")
    return fields


def unsigned(text: str, maximum: int) -> int:
    if not re.fullmatch(r"[0-9]{1,20}", text):
        raise ValueError("invalid unsigned integer")
    value = int(text)
    if value > maximum:
        raise ValueError("unsigned integer exceeds limit")
    return value


def timestamp(text: str) -> int:
    if not re.fullmatch(r"[0-9]{1,20}(?:\.[0-9]{1,18})?", text):
        raise ValueError("invalid decimal timestamp")
    seconds, _, fraction = text.partition(".")
    return int(seconds) * 10**18 + int(fraction.ljust(18, "0"))


def convert(
    source: Path, output: Path, work: Path, client: Endpoint, server: Endpoint,
    limits: Limits = DEFAULT_LIMITS,
) -> int:
    """Return emitted count. Any failure after creation leaves incomplete output."""
    # Preflight all configuration before creating a sensitive output file.
    for limit, ceiling in zip(
        (limits.input_bytes, limits.line_bytes, limits.rows, limits.output_bytes),
        (Limits().input_bytes, Limits().line_bytes, Limits().rows, Limits().output_bytes),
        strict=True,
    ):
        if not 0 < limit <= ceiling:
            raise ValueError("resource limits must be positive and within hard ceilings")
    for endpoint in (client, server):
        if Endpoint.parse(f"{endpoint.address}:{endpoint.port}") != endpoint:
            raise ValueError("endpoint must be canonical IPv4:port")
    if client == server:
        raise ValueError("client and server endpoints must differ")
    root = work.resolve(strict=True)
    parent = output.parent.resolve(strict=True)
    if not root.is_dir() or not parent.is_relative_to(root):
        raise ValueError("output must be beneath work root")
    target = parent / output.name
    # Reject FIFOs without waiting for a writer, then verify the opened object.
    source_descriptor = os.open(source, os.O_RDONLY | os.O_NONBLOCK)
    with os.fdopen(source_descriptor, "rb") as incoming:
        if not stat.S_ISREG(os.fstat(incoming.fileno()).st_mode):
            raise ValueError("input must be a regular file")
        if os.fstat(incoming.fileno()).st_size > limits.input_bytes:
            raise ValueError("input byte ceiling exceeded")
        descriptor = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        count = total = written = rows = 0
        first: int | None = None
        previous_time: int | None = None
        previous_frame = 0
        try:
            with os.fdopen(descriptor, "wb") as outgoing:
                while line := incoming.readline(limits.line_bytes + 1):
                    total += len(line)
                    rows += 1
                    if len(line) > limits.line_bytes or total > limits.input_bytes or rows > limits.rows:
                        raise ValueError("input resource ceiling exceeded")
                    fields = parse_row(line)
                    src = Endpoint.parse(fields["ip.src"] + ":" + fields["udp.srcport"])
                    dst = Endpoint.parse(fields["ip.dst"] + ":" + fields["udp.dstport"])
                    if (src, dst) == (client, server):
                        direction = "client_to_server"
                    elif (src, dst) == (server, client):
                        direction = "server_to_client"
                    else:
                        continue
                    frame = unsigned(fields["frame.number"], (1 << 64) - 1)
                    now = timestamp(fields["frame.time_epoch"])
                    if frame <= previous_frame or (previous_time is not None and now < previous_time):
                        raise ValueError("selected frame or timestamp order regressed")
                    length = unsigned(fields["frame.len"], (1 << 32) - 1)
                    if length == 0 or unsigned(fields["frame.cap_len"], (1 << 32) - 1) != length:
                        raise ValueError("truncated or invalid frame")
                    # TShark field output uses False/True; numeric exports use 0/1.
                    more_fragments = fields["ip.flags.mf"]
                    if more_fragments not in ("False", "True", "0", "1"):
                        raise ValueError("invalid fragmentation flag")
                    if more_fragments in ("True", "1") or unsigned(fields["ip.frag_offset"], 8191):
                        raise ValueError("fragmented packet")
                    payload = fields["udp.payload"]
                    if not re.fullmatch(r"(?:[0-9a-fA-F]{2})*|(?:[0-9a-fA-F]{2}:)*[0-9a-fA-F]{2}", payload):
                        raise ValueError("invalid payload hex")
                    raw = bytes.fromhex(payload.replace(":", ""))
                    if not raw or len(raw) > 65507:
                        raise ValueError("payload is outside inbound decoder bounds")
                    udp_length = unsigned(fields["udp.length"], 65535)
                    if udp_length != len(raw) + 8 or length < udp_length + 20:
                        raise ValueError("invalid UDP length")
                    if first is None:
                        first = now
                    ms = (now - first) // 10**15
                    if ms > 2**64 - 1:
                        raise ValueError("relative timestamp exceeds u64")
                    encoded = (json.dumps({"event": "datagram", "direction": direction,
                                           "ms": ms, "hex": raw.hex()}, separators=(",", ":")) + "\n").encode()
                    if written + len(encoded) > limits.output_bytes:
                        raise ValueError("output byte ceiling exceeded")
                    _ = outgoing.write(encoded)
                    written += len(encoded)
                    count += 1
                    previous_frame, previous_time = frame, now
        except (ValueError, OSError, UnicodeError, RecursionError) as error:
            raise ValueError(
                f"incomplete output: conversion failed after {count} datagrams "
                + f"at input row {rows} ({type(error).__name__})"
            ) from error
    return count


class Arguments(argparse.Namespace):
    input: Path = Path()
    output: Path = Path()
    work_root: Path = Path("work")
    client: str = ""
    server: str = ""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("input", type=Path)
    _ = parser.add_argument("output", type=Path)
    _ = parser.add_argument("--work-root", type=Path, default=Path("work"))
    _ = parser.add_argument("--client", required=True)
    _ = parser.add_argument("--server", required=True)
    args = parser.parse_args(namespace=Arguments())
    try:
        count = convert(args.input, args.output, args.work_root,
                        Endpoint.parse(args.client), Endpoint.parse(args.server))
    except (ValueError, OSError) as error:
        print(str(error), file=sys.stderr)
        return 1
    print(f"converted {count} datagrams; keep output local")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
