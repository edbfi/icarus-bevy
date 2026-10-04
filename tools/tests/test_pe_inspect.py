"""Authored PE fixtures; no original executable needed."""

from pathlib import Path
import struct
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from pe_inspect import PE  # noqa: E402


BASE = 0x140000000


def fixture():
    data = bytearray(1024)
    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, 0x80)
    data[0x80:0x84] = b"PE\0\0"
    struct.pack_into("<HH", data, 0x84, 0x8664, 1)
    struct.pack_into("<H", data, 0x94, 240)
    optional = 0x98
    struct.pack_into("<H", data, optional, 0x20B)
    struct.pack_into("<Q", data, optional + 24, BASE)
    struct.pack_into("<II", data, optional + 136, 0x1080, 12)
    table = optional + 240
    data[table:table + 8] = b".text\0\0\0"
    struct.pack_into("<IIII", data, table + 8, 0x200, 0x1000, 0x100, 0x200)
    struct.pack_into("<I", data, table + 36, 0x60000020)
    struct.pack_into("<III", data, 0x280, 0x1000, 0x1020, 0x10A0)
    data[0x200:0x203] = b"\x48\x8d\x05"
    struct.pack_into("<i", data, 0x203, 0x50 - 7)
    data[0x210] = 0xE8
    struct.pack_into("<i", data, 0x211, -0x15)
    return data


class PETest(unittest.TestCase):
    def test_file_mapping_rejects_virtual_padding_and_cross_section_reads(self):
        pe = PE(fixture())
        self.assertEqual(pe.offset(BASE + 0x1050, 4), 0x250)
        for address, length in [(BASE + 0x1100, 1), (BASE + 0x10FF, 2), (BASE, 1)]:
            with self.assertRaises(ValueError):
                pe.offset(address, length)

    def test_signed_reference_and_runtime_range_end_exclusive(self):
        pe = PE(fixture())
        ref = pe.references({BASE + 0x1050}, False)
        self.assertEqual(len(ref), 1)
        self.assertEqual(ref[0]["address"], hex(BASE + 0x1000))
        call = pe.references({BASE + 0x1000}, True)
        self.assertEqual(call[0]["address"], hex(BASE + 0x1010))
        self.assertIsNone(pe.runtime_range(BASE + 0x1020))
        self.assertIsNone(pe.runtime_range(BASE))

    def test_truncated_and_invalid_pe_fail_closed(self):
        for length in [0, 63, 0x90, 0x180, 0x280]:
            with self.assertRaises(ValueError):
                PE(fixture()[:length])
        data = fixture()
        struct.pack_into("<I", data, 0x98 + 140, 13)
        with self.assertRaises(ValueError):
            PE(data)
        data = fixture()
        struct.pack_into("<I", data, 0x284, 0xFFF)
        with self.assertRaises(ValueError):
            PE(data)


if __name__ == "__main__":
    unittest.main()
