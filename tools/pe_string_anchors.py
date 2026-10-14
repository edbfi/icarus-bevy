#!/usr/bin/env python3
"""Locate selected ASCII/UTF-16 diagnostic anchors in an authorized PE32+ sample.

Does not execute the sample or recover assets. Output contains private analysis
strings and must stay under the ignored case directory.
"""

import argparse
import json
import mmap
from pathlib import Path
import re
from typing import TypedDict, final


class Anchor(TypedDict):
    file_offset: int
    virtual_address: str | None
    encoding: str
    text: str


@final
class Arguments(argparse.Namespace):
    def __init__(self) -> None:
        super().__init__()
        self.sample: Path = Path()
        self.output: Path = Path()


def read_integer(data: mmap.mmap, offset: int, length: int) -> int:
    if offset < 0 or offset + length > len(data):
        raise ValueError("truncated PE structure")
    return int.from_bytes(data[offset:offset + length], "little")


PATTERN = re.compile(
    r"4\.27\.|EngineVersion|NetworkVersion|NetVersion|"
    + r"Prospect.*(?:Save|Load)|(?:Save|Load).*Prospect|"
    + r"ServerSettings|ResumeProspect|AutoSave|Autosave|"
    + r"NetDriver|PacketHandler|StatelessConnectHandler|Tamed|Tame.*Save",
    re.IGNORECASE,
)


def scan(sample: Path) -> list[Anchor]:
    with sample.open("rb") as stream, mmap.mmap(
        stream.fileno(), 0, access=mmap.ACCESS_READ
    ) as data:
        if len(data) < 64 or data[:2] != b"MZ":
            raise ValueError("not a DOS/PE executable")
        pe = read_integer(data, 0x3C, 4)
        if pe + 48 > len(data) or data[pe:pe + 4] != b"PE\0\0":
            raise ValueError("invalid PE header")
        count = read_integer(data, pe + 6, 2)
        optional_size = read_integer(data, pe + 20, 2)
        table = pe + 24 + optional_size
        if optional_size < 32 or table + count * 40 > len(data):
            raise ValueError("truncated PE headers")
        if read_integer(data, pe + 24, 2) != 0x20B:
            raise ValueError("only PE32+ is supported")
        base = read_integer(data, pe + 48, 8)
        sections: list[tuple[int, int, int]] = []
        for index in range(count):
            offset = table + index * 40 + 8
            rva = read_integer(data, offset + 4, 4)
            size = read_integer(data, offset + 8, 4)
            pointer = read_integer(data, offset + 12, 4)
            if pointer + size > len(data):
                raise ValueError("section extends beyond file")
            sections.append((pointer, size, rva))
        rows: list[Anchor] = []
        for regex, encoding in [
            (rb"[\x20-\x7e]{6,}", "ascii"),
            (rb"(?:[\x20-\x7e]\x00){6,}", "utf-16-le"),
        ]:
            for match in re.finditer(regex, data):
                text = match.group().decode(encoding)
                if not PATTERN.search(text):
                    continue
                offset = match.start()
                address = next((
                    base + rva + offset - pointer
                    for pointer, size, rva in sections
                    if pointer <= offset < pointer + size
                ), None)
                rows.append({
                    "file_offset": offset,
                    "virtual_address": hex(address) if address else None,
                    "encoding": encoding,
                    "text": text,
                })
        return rows


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--sample", type=Path, required=True)
    _ = parser.add_argument("--output", type=Path, required=True)
    args = Arguments()
    _ = parser.parse_args(namespace=args)
    sample = args.sample.resolve(strict=True)
    output = args.output.resolve()
    if output == sample:
        parser.error("output must not replace the original sample")
    rows = scan(sample)
    # Require an existing case directory rather than creating implicit scope.
    with output.open("x", encoding="utf-8") as stream:
        json.dump(rows, stream, indent=2)
        _ = stream.write("\n")
    print(f"Recorded {len(rows)} diagnostic anchors")


if __name__ == "__main__":
    main()
