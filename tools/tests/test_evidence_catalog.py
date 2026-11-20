# SPDX-License-Identifier: AGPL-3.0-or-later
from __future__ import annotations

import io
import os
import subprocess
import tempfile
import unittest
from collections.abc import Generator
from contextlib import contextmanager, redirect_stderr, redirect_stdout
from pathlib import Path
from typing import ClassVar, override
from unittest.mock import patch as patch_environment

from tools.evidence_catalog import main
from tools.evidence_catalog_model import CatalogError, Index, Record, json_line
from tools.evidence_catalog_publish import Request, check_checkout, patch, prepare, resolved_catalog
from tools.evidence_catalog_store import Catalog, commit_id, git, legacy_files, load_directory

ROOT = Path(__file__).resolve().parents[2]
PIN = "9fb2b6ef9e0a7c1788fada5a054dd13ce2ac0e1d"
FINDING = "docs/ledgers/findings/foundation.md"
SHARD = "evidence/foundation/001.jsonl"


@contextmanager
def fixture_git_environment() -> Generator[None]:
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    with patch_environment.dict(os.environ, environment, clear=True):
        yield


class PinnedCatalogTests(unittest.TestCase):
    catalog: ClassVar[Catalog]
    index: ClassVar[Index]

    @classmethod
    @override
    def setUpClass(cls) -> None:
        try:
            revision = commit_id(ROOT, PIN)
        except CatalogError:
            # CI uses a shallow checkout. Exercise the full available tree there.
            revision = commit_id(ROOT, "HEAD")
        cls.catalog = resolved_catalog(ROOT, revision)
        cls.index = Index(cls.catalog.documents)

    def test_exact_full_corpus_round_trip_and_stable_manifest(self) -> None:
        source = legacy_files(ROOT, self.catalog.revision)
        expected = self.catalog.files()
        self.assertEqual(expected, resolved_catalog(ROOT, self.catalog.revision).files())
        self.assertEqual(expected, Catalog.from_files(expected).files())
        self.assertEqual(source, {path: doc.render() for path, doc in self.catalog.documents.items()})
        for target in self.index.reverse:
            if "/" in target:
                _ = self.index.selector(target)
        if self.catalog.revision == PIN:
            self.assertEqual(36, len(source))
            self.assertEqual(3002, len(self.index.entries))

    def test_real_escaped_pipe_legacy_metadata_and_independent_supersession(self) -> None:
        entry = self.index.get("F-750")
        self.assertIn("||ShouldDrop", str(entry.record.fields["claim"]))
        self.assertIn(r"\|\|ShouldDrop".encode(), self.catalog.documents[entry.path].render())
        evidence = self.index.get("M-146")
        self.assertIsNone(evidence.record.fields["date"])
        self.assertIsNone(evidence.record.fields["sha256"])
        self.assertTrue(str(evidence.record.fields["path"]).endswith("/"))
        retired = self.index.get("F-002")
        self.assertEqual("superseded-by F-487", retired.record.fields["status"])
        self.assertIn({"id": "F-002", "kind": "supersedes_to", "field": "status"}, self.index.incoming("F-487"))
        self.assertEqual("active", evidence.record.fields["status"])

    def test_partial_bundle_members_and_reverse_support(self) -> None:
        selector = "M-761/unk_2f527d4ef39f"
        member = self.index.selector(selector)
        self.assertEqual("listed", member["membership"])
        self.assertTrue(str(member["member"]).startswith("unk_2f527d4ef39f"))
        self.assertIn({"id": "U-003", "kind": "reference", "field": "rea_id"}, self.index.incoming(selector))
        evidence = self.index.get("M-761").record
        bundle = evidence.evidence("", 0)
        self.assertGreater(bundle.records or 0, len(bundle.ev) + len(bundle.unk))
        with self.assertRaisesRegex(CatalogError, "0 listed members"):
            _ = self.index.selector("M-761/unk_000000000000")
        self.assertIn({"id": "F-750", "kind": "support", "field": "evidence"}, self.index.incoming("M-801"))


class PublicationTests(unittest.TestCase):
    @override
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.parent: Path = Path()
        self.root: Path = Path()
        self.fields: dict[str, str] = {}
        self.evidence: dict[str, object] = {}

    @override
    def setUp(self) -> None:
        self.enterContext(fixture_git_environment())
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.parent = Path(temp.name)
        self.root = self.parent / "repo"
        self.root.mkdir()
        _ = git(self.root, "init", "--quiet", "--template=")
        (self.root / FINDING).parent.mkdir(parents=True)
        (self.root / SHARD).parent.mkdir(parents=True)
        self.fields = {
            "claim": "Historical claim",
            "kind": "observed",
            "evidence": "authored-test; M-001",
            "scope_and_limits": "single case",
            "unknowns": "",
            "status": "active",
        }
        _ = (self.root / FINDING).write_text(
            "# Foundation\n\nLast reconciled: 2026-11-20\n\n"
            + "| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |\n"
            + "| --- | --- | --- | --- | --- | --- | --- |\n"
            + Record("F-001", dict(self.fields)).render() + "\n"
        )
        self.evidence = {
            "path": "work/fake/old-run.txt", "class": "local-private", "kind": "receipt", "date": "2026-11-20",
            "label": "Fixture result", "status": "active", "sha256": "a" * 64,
        }
        _ = (self.root / SHARD).write_text(json_line({"id": "M-001", **self.evidence}) + "\n")
        self.commit("fixture")

    def commit(self, message: str) -> None:
        _ = git(self.root, "add", "-A")
        _ = git(
            self.root, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test",
            "-c", "commit.gpgsign=false", "commit", "--quiet", "--allow-empty", "-m", message,
        )

    def request(self, **changes: object) -> Request:
        return Request.parse({"base": commit_id(self.root, "HEAD"), **changes})

    def apply(self, request: Request) -> None:
        path = self.parent / "proposal.patch"
        _ = path.write_bytes(patch(prepare(self.root, request)))
        _ = git(self.root, "apply", "--index", str(path))

    def cli(self, *args: str) -> tuple[int, str]:
        output = io.StringIO()
        with redirect_stdout(output), redirect_stderr(output):
            code = main(["--repo", str(self.root), *args])
        return code, output.getvalue()

    def test_activation_edit_and_release_keep_legacy_views(self) -> None:
        legacy = (self.root / FINDING).read_bytes()
        self.apply(self.request(activate=["foundation"]))
        check_checkout(self.root)
        check_checkout(self.root, staged=True)
        self.assertEqual(legacy, (self.root / FINDING).read_bytes())
        self.commit("activate")

        self.apply(self.request(
            reconciled="2026-11-21",
            upsert=[
                {"path": FINDING, "id": "F-001", "fields": {**self.fields, "status": "superseded-by F-002"}},
                {"path": FINDING, "id": "F-002", "fields": {**self.fields, "claim": "New | claim"}},
            ],
        ))
        check_checkout(self.root)
        self.commit("typed update")
        updated = (self.root / FINDING).read_bytes()
        self.assertIn(rb"New \| claim", updated)
        self.assertIn(b"Last reconciled: 2026-11-21", updated)
        idx = Index(resolved_catalog(self.root, commit_id(self.root, "HEAD")).documents)
        self.assertIn({"id": "F-001", "kind": "supersedes_to", "field": "status"}, idx.incoming("F-002"))
        self.assertEqual("active", idx.get("M-001").record.fields["status"])

        self.apply(self.request(release=["foundation"]))
        check_checkout(self.root)
        self.commit("release")
        self.assertEqual(updated, (self.root / FINDING).read_bytes())
        _ = (self.root / FINDING).write_bytes(updated + b"\n")
        check_checkout(self.root)

    def test_approval_stale_sources_and_clean_output(self) -> None:
        request = self.request(activate=["foundation"])
        request_path = self.parent / "request.json"
        _ = request_path.write_text(json_line(request.payload))
        out = self.parent / "out"
        args = ("publish", str(request_path), "--out", str(out), "--approved-sha256")
        self.assertEqual(1, self.cli(*args, "wrong")[0])
        self.assertFalse(out.exists())
        source = self.root / FINDING
        original = source.read_bytes()
        _ = source.write_bytes(original + b"\n")
        result = self.cli(*args, request.approval())
        self.assertIn("stale", result[1])
        self.assertFalse(out.exists())
        _ = source.write_bytes(original)
        self.assertEqual(0, self.cli(*args, request.approval())[0])
        self.assertEqual(original, source.read_bytes())
        self.assertEqual(1, self.cli(*args, request.approval())[0])
        self.commit("move HEAD")
        stale = self.cli(*args, request.approval())
        self.assertIn("stale", stale[1])

    def test_competing_patch_and_view_drift_fail_closed(self) -> None:
        self.apply(self.request(activate=["foundation"]))
        self.commit("activate")
        first = self.request(upsert=[{
            "path": FINDING, "id": "F-001", "fields": {**self.fields, "claim": "First writer"},
        }])
        second = self.request(upsert=[{
            "path": SHARD, "id": "M-001", "fields": {**self.evidence, "status": "retired", "note": "Second writer"},
        }])
        proposal = self.parent / "competing.patch"
        _ = proposal.write_bytes(patch(prepare(self.root, second)))
        self.apply(first)
        result = subprocess.run(
            ["git", "-C", str(self.root), "apply", "--index", str(proposal)],
            capture_output=True, check=False,
        )
        self.assertNotEqual(0, result.returncode)
        self.assertNotIn(b"Second writer", (self.root / SHARD).read_bytes())
        check_checkout(self.root)
        self.commit("first writer")
        _ = (self.root / FINDING).write_bytes((self.root / FINDING).read_bytes() + b"\n")
        with self.assertRaisesRegex(CatalogError, "generated source drift"):
            check_checkout(self.root)

    def test_new_shard_immutable_evidence_and_unmanaged_write(self) -> None:
        update = {"path": "evidence/foundation/002.jsonl", "id": "M-002", "fields": self.evidence}
        with self.assertRaisesRegex(CatalogError, "outside requested"):
            _ = prepare(self.root, self.request(upsert=[update]))
        self.apply(self.request(activate=["foundation"], upsert=[update]))
        check_checkout(self.root)
        self.commit("new shard")
        self.assertTrue((self.root / str(update["path"])).is_file())
        changed = {**self.evidence, "date": "2026-11-21"}
        with self.assertRaisesRegex(CatalogError, "date is immutable"):
            _ = prepare(self.root, self.request(upsert=[{**update, "fields": changed}]))
        catalog_path = self.root / "evidence-catalog"
        self.assertIsNotNone(load_directory(catalog_path).authority)
        rogue = self.root / "evidence/foundation/003.jsonl"
        _ = rogue.write_bytes((self.root / str(update["path"])).read_bytes())
        with self.assertRaisesRegex(CatalogError, "generated source drift"):
            check_checkout(self.root)

    def test_deleting_authority_cannot_replace_a_reviewed_release(self) -> None:
        self.apply(self.request(activate=["foundation"]))
        self.commit("activate")
        _ = git(self.root, "rm", "-r", "--cached", "--", "evidence-catalog")
        check_checkout(self.root)
        with self.assertRaisesRegex(CatalogError, "missing catalog manifest"):
            check_checkout(self.root, staged=True)

    def test_staged_legacy_drift_cannot_be_hidden_by_working_copy(self) -> None:
        self.apply(self.request(activate=["foundation"]))
        self.commit("activate")
        path = self.root / FINDING
        expected = path.read_bytes()
        _ = path.write_bytes(expected + b"\n")
        _ = git(self.root, "add", "--", FINDING)
        _ = path.write_bytes(expected)
        check_checkout(self.root)
        with self.assertRaisesRegex(CatalogError, "generated source drift"):
            check_checkout(self.root, staged=True)


if __name__ == "__main__":
    _ = unittest.main()
