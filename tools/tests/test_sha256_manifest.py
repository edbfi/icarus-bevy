"""Authored fixtures for the shared sha256 manifest reader; no repository files are read."""

from pathlib import Path
import sys
import tempfile
from typing import override
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from tools import sha256_manifest as manifests  # noqa: E402

DIGEST = "Ab" * 32


class Sha256ManifestTest(unittest.TestCase):
    root: Path = Path()

    @override
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)

    def write(self, rel: str, body: bytes) -> Path:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        _ = path.write_bytes(body)
        return path

    def test_entries_resolve_beside_the_manifest_first_then_at_the_root(self) -> None:
        manifest = self.root / "work/c/out.sha256"
        self.assertEqual(manifests.entry_candidates(self.root, manifest, "a.log"), (self.root / "work/c/a.log", self.root / "a.log"))
        self.assertEqual(manifests.entry_candidates(self.root, manifest, "./a.log"), (self.root / "work/c/a.log", self.root / "a.log"))
        self.assertEqual(manifests.entry_candidates(self.root, manifest, "../b/x"), (self.root / "work/b/x",))
        self.assertEqual(manifests.entry_candidates(self.root, manifest, "../../../x"), ())
        self.assertEqual(manifests.entry_candidates(self.root, self.root / "m.sha256", "x"), (self.root / "x",))

    def test_lines_digest_and_size(self) -> None:
        path = self.write("work/c/out.sha256", f"{DIGEST} *bin/a.exe\n\n{DIGEST}  b c.txt\n12 bin/a.exe\n".encode())
        entries = manifests.read_manifest(self.root, path) or []
        self.assertEqual([(entry.line, entry.path, entry.sha256, entry.size) for entry in entries], [
            (1, "bin/a.exe", DIGEST.lower(), None),
            (3, "b c.txt", DIGEST.lower(), None),
            (4, "bin/a.exe", None, 12),
        ])

    def test_recognition_by_name_or_first_line_only(self) -> None:
        self.assertIsNone(manifests.read_manifest(self.root, self.write("work/c/notes.md", b"notes\n")))
        self.assertIsNone(manifests.read_manifest(self.root, self.root / "work/c/absent.log"))
        (self.root / "work/d").mkdir(parents=True)
        self.assertIsNone(manifests.read_manifest(self.root, self.root / "work/d"))
        late = self.write("work/c/late.txt", b"x" * manifests.SNIFF_BYTES + f"\n{DIGEST}  a\n".encode())
        self.assertIsNone(manifests.read_manifest(self.root, late))
        sniffed = self.write("work/c/hashes.txt", f"\n{DIGEST}  a\n".encode())
        self.assertEqual([entry.path for entry in manifests.read_manifest(self.root, sniffed) or []], ["a"])

    def test_unknown_entries_raise(self) -> None:
        cases = {
            "work/c/missing.sha256": None,
            "work/c/fixity-dir": b"",
            "work/c/bad.sha256": b"junk\n",
            "work/c/empty-fixity.txt": b"\n",
            "work/c/abs.sha256": f"{DIGEST}  /etc/x\n".encode(),
            "work/c/escape.sha256": f"{DIGEST}  ../../../x\n".encode(),
            "work/c/binary.sha256": b"\xff\xfe",
        }
        for rel, body in cases.items():
            with self.subTest(rel):
                if rel.endswith("-dir"):
                    (self.root / rel).mkdir(parents=True)
                elif body is not None:
                    _ = self.write(rel, body)
                with self.assertRaises(manifests.ManifestError):
                    _ = manifests.read_manifest(self.root, self.root / rel)
        big = self.write("work/c/big.sha256", f"{DIGEST}  a\n".encode())
        with self.assertRaises(manifests.ManifestTooLarge):
            _ = manifests.read_manifest(self.root, big, max_bytes=8)


if __name__ == "__main__":
    _ = unittest.main()
