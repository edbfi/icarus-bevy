#!/usr/bin/env python3
"""Read-only PE32+ code metadata and candidate reference inspection.

No target execution, patching or asset extraction. Run only in an approved case.
Runtime-function ranges and byte-pattern references are candidates, not proof of
complete function boundaries or instruction alignment. Outputs belong in work/.
"""

import argparse
import bisect
import hashlib
import json
from pathlib import Path
import re
import struct


class PE:
    def __init__(self, data: bytes):
        self.data = data
        if len(data) < 64 or data[:2] != b"MZ":
            raise ValueError("not a PE executable")
        pe = self.unpack("<I", 0x3C)[0]
        if data[pe:pe + 4] != b"PE\0\0":
            raise ValueError("invalid PE signature")
        machine, count = self.unpack("<HH", pe + 4)
        optional_size = self.unpack("<H", pe + 20)[0]
        optional = pe + 24
        if machine != 0x8664 or optional_size < 144 or self.unpack("<H", optional)[0] != 0x20B:
            raise ValueError("requires x64 PE32+ with an exception directory")
        self.base = self.unpack("<Q", optional + 24)[0]
        self.sections = []
        table = optional + optional_size
        for index in range(count):
            offset = table + 40 * index
            self.unpack("<40s", offset)
            name = data[offset:offset + 8].rstrip(b"\0").decode("ascii", errors="replace")
            virtual_size, rva, size, raw = self.unpack("<IIII", offset + 8)
            flags = self.unpack("<I", offset + 36)[0]
            if raw + size > len(data):
                raise ValueError("section extends beyond input")
            self.sections.append(dict(name=name, rva=rva, size=size, raw=raw,
                                      virtual_size=virtual_size, executable=bool(flags & 0x20000000)))
        self.ranges = []
        rva, size = self.unpack("<II", optional + 136)
        if size:
            if size % 12:
                raise ValueError("unaligned exception directory size")
            raw = self.offset(self.base + rva, size)
            for position in range(raw, raw + size, 12):
                start, end, unwind = self.unpack("<III", position)
                if start >= end:
                    raise ValueError("invalid runtime range")
                self.ranges.append((self.base + start, self.base + end, self.base + unwind))
        self.ranges.sort()
        self.starts = [entry[0] for entry in self.ranges]
        self._roots = {}

    def unpack(self, fmt: str, offset: int):
        if offset < 0 or offset + struct.calcsize(fmt) > len(self.data):
            raise ValueError("truncated PE structure")
        return struct.unpack_from(fmt, self.data, offset)

    def offset(self, address: int, length: int = 1) -> int:
        if length <= 0:
            raise ValueError("length must be positive")
        for section in self.sections:
            delta = address - self.base - section["rva"]
            if 0 <= delta and delta + length <= section["size"]:
                return section["raw"] + delta
        raise ValueError(f"address range is not file-backed: {address:#x}+{length}")

    def runtime_range(self, address: int):
        index = bisect.bisect_right(self.starts, address) - 1
        if index >= 0:
            start, end, unwind = self.ranges[index]
            if start <= address < end:
                return dict(start=hex(start), end=hex(end), unwind=hex(unwind))
        return None

    def unwind_root(self, entry, visiting=None):
        """Resolve chained UNWIND_INFO, including chains through intermediate ranges."""
        if entry in self._roots:
            return self._roots[entry]
        visiting = set() if visiting is None else visiting
        if entry in visiting or len(visiting) >= 32:
            raise ValueError("cyclic or excessively deep unwind chain")
        visiting.add(entry)
        offset = self.offset(entry[2], 4)
        version_flags, _, slots, _ = self.unpack("<4B", offset)
        if version_flags & 7 not in (1, 2):
            raise ValueError("unsupported unwind version")
        flags = version_flags >> 3
        root = entry
        if flags & 4:
            if flags & 3:
                raise ValueError("unwind chain cannot also have an exception handler")
            length = 4 + ((slots + 1) // 2) * 4
            chained = self.offset(entry[2] + length, 12)
            start, end, unwind = self.unpack("<III", chained)
            if start >= end:
                raise ValueError("invalid chained runtime range")
            root = self.unwind_root((self.base + start, self.base + end, self.base + unwind), visiting)
        visiting.remove(entry)
        self._roots[entry] = root
        return root

    def related_ranges(self, address: int):
        index = bisect.bisect_right(self.starts, address) - 1
        if index < 0 or address >= self.ranges[index][1]:
            return []
        root = self.unwind_root(self.ranges[index])
        return [dict(start=hex(start), end=hex(end), unwind=hex(unwind))
                for start, end, unwind in self.ranges
                if self.unwind_root((start, end, unwind)) == root]

    def references(self, targets: set[int], calls: bool):
        expression = rb"\xe8" if calls else rb"[\x48-\x4f][\x8d\x8b\x89][\x05\x0d\x15\x1d\x25\x2d\x35\x3d]"
        length, displacement = (5, 1) if calls else (7, 3)
        rows = []
        for section in self.sections:
            if not section["executable"]:
                continue
            code = self.data[section["raw"]:section["raw"] + section["size"]]
            for match in re.finditer(expression, code):
                offset = match.start()
                if offset + length > len(code):
                    continue
                address = self.base + section["rva"] + offset
                target = address + length + struct.unpack_from("<i", code, offset + displacement)[0]
                if target in targets:
                    rows.append(dict(address=hex(address), target=hex(target),
                                     bytes=code[offset:offset + length].hex(),
                                     runtime_range=self.runtime_range(address)))
        return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sample", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--address", type=lambda value: int(value, 16), action="append", required=True)
    parser.add_argument("--references", choices=["rip", "calls"])
    parser.add_argument("--chained-ranges", action="store_true")
    args = parser.parse_args()
    sample = args.sample.resolve(strict=True)
    if sample.stat().st_size > 256 * 1024 * 1024:
        parser.error("sample exceeds 256 MiB analysis bound")
    if args.output.resolve() == sample:
        parser.error("output must not replace sample")
    data = sample.read_bytes()
    pe = PE(data)
    result = dict(sample_sha256=hashlib.sha256(data).hexdigest(), image_base=hex(pe.base),
                  caveat="Static metadata and byte-pattern candidates; validate instruction alignment and complete control flow.",
                  addresses={hex(address): pe.runtime_range(address) for address in args.address})
    if args.references:
        result["references"] = pe.references(set(args.address), args.references == "calls")
    if args.chained_ranges:
        result["chained_ranges"] = {hex(address): pe.related_ranges(address) for address in args.address}
    with args.output.open("x", encoding="utf-8") as stream:
        json.dump(result, stream, indent=2)
        stream.write("\n")
    print(f"Recorded {len(args.address)} addresses; {len(result.get('references', []))} candidates")


if __name__ == "__main__":
    main()
