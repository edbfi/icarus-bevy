# SPDX-License-Identifier: AGPL-3.0-or-later
from __future__ import annotations

from subprocess import CompletedProcess
import tempfile
import unittest
from http.client import HTTPConnection
from pathlib import Path
from typing import final, override
from unittest.mock import patch
from zipfile import ZipFile

from tools.evidence_catalog_model import CatalogError, Record, digest, json_line
from tools.evidence_catalog_store import commit_id, git
from tools.evidence_hindsight import (
    Publication, current_publication, exact_rows, prepare, publish, query, read_publication,
)
from tools.evidence_hindsight_client import (
    CONTROL_TIMEOUT, RECALL_TIMEOUT, REFLECT_TIMEOUT, Hit, Hindsight, HindsightError, HindsightTimeout, verify_hit,
)
from tools.evidence_hindsight_projection import Projection, committed, require_ancestor, review_file
from tools.tests.test_evidence_catalog import fixture_git_environment

FINDINGS = "docs/ledgers/findings/foundation.md"
EVIDENCE = "evidence/foundation/001.jsonl"
SELECTOR = "M-001/ev_123456789abc"


class FakeHindsight:
    def __init__(self, url: str, bank: str) -> None:
        self.url: str = url
        self.bank: str = bank

    def require_authenticated_service(self) -> None:
        if not self.url:
            raise HindsightError("missing fixture URL")

    def transfer(self, archive: bytes, count: int) -> dict[str, object]:
        if not archive:
            raise HindsightError("missing synthetic archive")
        return {"documents_imported": count}

    def verify_bank(self, projection: Projection) -> None:
        if self.bank != projection.review.bank:
            raise HindsightError("wrong synthetic bank")

    def recall(self, _projection: Projection, _question: str) -> tuple[Hit, ...]:
        raise HindsightError("simulated stale or malformed response")


class BadHindsight(FakeHindsight):
    @override
    def verify_bank(self, projection: Projection) -> None:
        raise HindsightError(f"cannot verify {projection.review.topic}")


@final
class EvidenceHindsightTests(unittest.TestCase):
    @override
    def __init__(self, methodName: str = "runTest") -> None:
        super().__init__(methodName)
        self.parent = Path()
        self.repo = Path()
        self.revision = ""
        self.allowlist = Path()
        self.state = Path()

    @override
    def setUp(self) -> None:
        self.enterContext(fixture_git_environment())
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.parent = Path(temporary.name)
        self.repo = self.parent / "repo"
        self.repo.mkdir()
        _ = git(self.repo, "init", "--quiet", "--template=")
        _ = (self.repo / ".gitignore").write_text("/work/\n")
        (self.repo / FINDINGS).parent.mkdir(parents=True)
        (self.repo / EVIDENCE).parent.mkdir(parents=True)
        self.finding("Historical claim")
        evidence = {
            "id": "M-001", "class": "local-private", "kind": "rea-bundle", "date": "2026-11-20",
            "status": "active", "path": "work/private-case/private-artifact",
            "sha256": "a" * 64, "label": "fixture-unique-sensitive-label",
            "note": "fixture-unique-sensitive-note", "records": 1, "ev": ["ev_123456789abc"],
        }
        _ = (self.repo / EVIDENCE).write_text(json_line(evidence) + "\n")
        self.revision = self.commit()
        self.allowlist = self.parent / "allowlist.json"
        _ = self.allowlist.write_text('["F-001", "M-001", "F-002"]')
        (self.repo / "work").mkdir()
        self.state = self.repo / "work" / "active"

    def finding(self, claim: str, status: str = "superseded-by F-002") -> None:
        fields: dict[str, object] = {
            "claim": claim, "kind": "observed", "evidence": SELECTOR,
            "scope_and_limits": "bounded trial", "unknowns": "", "status": status,
        }
        header = (
            "# Foundation\n\n| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |\n"
            + "| --- | --- | --- | --- | --- | --- | --- |\n"
        )
        corrected = {**fields, "claim": "Current claim", "status": "active", "evidence": ""}
        _ = (self.repo / FINDINGS).write_text(
            header + Record("F-001", fields).render() + "\n" + Record("F-002", corrected).render() + "\n"
        )

    def commit(self) -> str:
        _ = git(self.repo, "add", ".")
        _ = git(
            self.repo, "-c", "user.name=Fixture", "-c", "user.email=fixture@example.org",
            "commit", "--quiet", "--no-gpg-sign", "-m", "fixture",
        )
        return commit_id(self.repo, "HEAD")

    def prepare_topic(self, suffix: str, rev: str | None = None) -> Path:
        out = self.repo / "work" / suffix
        _ = prepare(self.repo, "foundation", self.allowlist, out, rev)
        return out

    def make_publication(self) -> Publication:
        directory = self.prepare_topic("ready", self.revision)
        _data, review, archive = review_file(directory)
        return Publication(self.revision, committed(self.repo, self.revision, review), archive)

    def test_deterministic_archive_excludes_local_private_payload(self) -> None:
        first = self.prepare_topic("first", self.revision)
        _ = self.allowlist.write_text('["F-002", "M-001", "F-001"]')
        second = self.prepare_topic("second", self.revision)
        self.assertEqual((first / "review.json").read_bytes(), (second / "review.json").read_bytes())
        archive = (first / "projection.zip").read_bytes()
        self.assertEqual(archive, (second / "projection.zip").read_bytes())
        for marker in (b"private-artifact", b"fixture-unique-sensitive-label", b"fixture-unique-sensitive-note",
                       b"a" * 64, b'"ev":', b'"records":'):
            self.assertNotIn(marker, archive)
        with ZipFile(first / "projection.zip") as zipped:
            self.assertNotIn(SELECTOR.encode(), zipped.read("documents/M-001.json"))
            self.assertIn(SELECTOR.encode(), zipped.read("documents/F-001.json"))
        self.finding("Private body under work/private-case/credential.txt")
        with self.assertRaisesRegex(CatalogError, "potential private material"):
            _ = self.prepare_topic("unsafe", self.commit())
        self.assertFalse((self.repo / "work" / "unsafe").exists())

    def test_committed_source_must_match_exact_staging_parity(self) -> None:
        self.finding("Approved staged correction")
        _ = git(self.repo, "add", FINDINGS)
        self.finding("Unstaged and unreviewed body")
        directory = self.prepare_topic("staged")
        _data, review, expected = review_file(directory)
        with self.assertRaisesRegex(CatalogError, "differ from the reviewed snapshot"):
            _ = committed(self.repo, self.revision, review)
        self.finding("Approved staged correction")
        _ = (self.repo / "other.txt").write_text("Unrelated changes may travel with the source")
        revised = self.commit()
        self.assertEqual(expected, digest(committed(self.repo, revised, review).archive()))
        self.finding("Drift outside the prepared catalog")
        with self.assertRaisesRegex(CatalogError, "differ from the reviewed snapshot"):
            _ = committed(self.repo, self.commit(), review)

    def test_selector_membership_and_independent_statuses_on_fallback(self) -> None:
        publication = self.make_publication()
        verified, unresolved, rows = exact_rows(publication.projection, (SELECTOR, "F-001", "M-001/ev_unknown"))
        self.assertEqual([SELECTOR, "F-001"], verified)
        self.assertEqual(["M-001/<invalid-selector>"], unresolved)
        by_id = {row["item_id"]: row for row in rows}
        self.assertEqual("active", by_id["M-001"]["status"])
        self.assertEqual("superseded-by F-002", by_id["F-001"]["status"])
        self.assertIn({"handle": SELECTOR, "status": "active"}, by_id["F-001"]["references"])
        self.assertIn("F-002", by_id)
        with patch("tools.evidence_hindsight.Hindsight", FakeHindsight):
            result = query(publication, "What was approved?", ("F-001",), "http://127.0.0.1:8080", False)
        self.assertEqual("exact-fallback", result["retrieval"])
        self.assertEqual(["F-001"], result["verified_handles"])
        exact = query(publication, "Explain F-002", ("F-001",), None, False)
        self.assertEqual(["F-002"], exact["verified_handles"])

    def test_recall_requires_exact_document_metadata_text_and_chunk(self) -> None:
        projection = self.make_publication().projection
        row = projection.rows["F-001"]
        document = projection.document(row)
        metadata = document["facts"][0]["metadata"]
        hit: dict[str, object] = {
            "metadata": metadata, "id": "fact", "chunk_id": "chunk", "document_id": "F-001", "type": "world",
            "text": row.text,
        }
        self.assertEqual(row, verify_hit(projection, hit, {"chunk": {"text": row.text}}).row)
        with self.assertRaises(HindsightError):
            _ = verify_hit(projection, hit, {"chunk": {"text": "stale body"}})
        with self.assertRaises(HindsightError):
            _ = verify_hit(projection, {**hit, "metadata": {**metadata, "ledger_status": "active"}},
                           {"chunk": {"text": row.text}})

    def test_failed_verification_does_not_replace_active_pointer(self) -> None:
        ready = self.prepare_topic("ready", self.revision)
        approved = digest((ready / "review.json").read_bytes())
        url = "http://127.0.0.1:8080"
        with patch("tools.evidence_hindsight.Hindsight", FakeHindsight):
            self.assertEqual(3, publish(self.repo, ready, approved, self.revision, self.state, url)["documents_imported"])
        pointer = self.state / "foundation.json"
        before = read_publication(self.repo, pointer, "foundation")
        _ = self.allowlist.write_text('["F-002"]')
        revised = self.prepare_topic("replacement", self.revision)
        approved = digest((revised / "review.json").read_bytes())
        with patch("tools.evidence_hindsight.Hindsight", BadHindsight), self.assertRaises(HindsightError):
            _ = publish(self.repo, revised, approved, self.revision, self.state, url)
        self.assertEqual(before.projection.review.bank, read_publication(self.repo, pointer, "foundation").projection.review.bank)
        with patch("tools.evidence_hindsight.Hindsight", FakeHindsight):
            _ = publish(self.repo, revised, approved, self.revision, self.state, url)
        after = read_publication(self.repo, pointer, "foundation")
        self.assertNotEqual(before.projection.review.bank, after.projection.review.bank)
        self.assertEqual({"F-002"}, set(after.projection.rows))

    def test_divergent_commit_keeps_active_pointer_and_refuses_staged_replay(self) -> None:
        _ = (self.repo / "branch-marker.txt").write_text("feature")
        feature = self.commit()
        first = self.prepare_topic("first", feature)
        approved = digest((first / "review.json").read_bytes())
        url = "http://127.0.0.1:8080"
        with patch("tools.evidence_hindsight.Hindsight", FakeHindsight):
            _ = publish(self.repo, first, approved, feature, self.state, url)
        pointer = self.state / "foundation.json"
        existing = pointer.read_bytes()
        staged = self.prepare_topic("staged")
        _data, review, _archive = review_file(staged)

        # A sibling commit with identical catalog blobs still cannot replace this pointer.
        _ = git(self.repo, "switch", "--quiet", "--detach", self.revision)
        _ = (self.repo / "branch-marker.txt").write_text("squashed")
        sibling = self.commit()
        replacement = self.prepare_topic("sibling", sibling)
        approved = digest((replacement / "review.json").read_bytes())
        with (
            patch("tools.evidence_hindsight.Hindsight", side_effect=AssertionError("opened unrelated bank")),
            self.assertRaisesRegex(CatalogError, "active publication commit .* is not an ancestor"),
        ):
            _ = publish(self.repo, replacement, approved, sibling, self.state, url)
        self.assertEqual(existing, pointer.read_bytes())
        with self.assertRaisesRegex(CatalogError, "staged review base .* is not an ancestor"):
            _ = committed(self.repo, sibling, review)

    def test_git_ancestry_error_reports_operational_failure(self) -> None:
        result = CompletedProcess(["git"], 128, b"", b"fatal: unable to read commit")
        with (
            patch("tools.evidence_hindsight_projection.subprocess.run", return_value=result),
            self.assertRaisesRegex(CatalogError, r"git merge-base ancestry check failed \(exit 128\): fatal"),
        ):
            require_ancestor(self.repo, self.revision, self.revision, "test base")

    def test_stale_active_snapshot_uses_current_git_without_sending_to_hindsight(self) -> None:
        publication = self.make_publication()
        self.finding("Verified replacement claim", "retired")
        revised = self.commit()
        current = current_publication(self.repo, publication)
        self.assertTrue(current.stale)
        with patch("tools.evidence_hindsight.Hindsight", side_effect=AssertionError("used stale bank")):
            result = query(current, "What is the current claim?", ("F-001",), "http://127.0.0.1:8080", True)
        self.assertEqual("stale-exact-fallback", result["retrieval"])
        self.assertEqual(revised, result["revision"])
        self.assertEqual(self.revision, result["published_revision"])
        self.assertIsNone(result["draft"])
        self.assertEqual("Verified replacement claim", current.projection.rows["F-001"].fields["claim"])
        self.assertEqual("retired", current.projection.rows["F-001"].status)
        self.assertEqual("active", current.projection.rows["M-001"].status)
        self.assertEqual("Historical claim", publication.projection.rows["F-001"].fields["claim"])

    def test_prepare_refuses_publication_directory_outside_repo(self) -> None:
        with self.assertRaisesRegex(CatalogError, "dedicated publication directory"):
            _ = prepare(self.repo, "fixture", self.allowlist, self.parent / "outside", self.revision)


    def test_replay_adopts_in_flight_import_before_issuing_post(self) -> None:
        bank = self.make_publication().projection.review.bank
        identifier = "12345678-abcd-0000-0000-aabbccddeeff"
        requests: list[str] = []

        def reply(route: str, data: bytes | None = None) -> dict[str, object]:
            self.assertIsNone(data)
            requests.append(route)
            if route.startswith("/operations?"):
                status = "pending" if "pending" in route else "processing"
                return {
                    "bank_id": bank, "total": 1, "operations": [
                        {"id": identifier, "task_type": "import_documents", "status": status},
                    ],
                }
            self.assertEqual(f"/operations/{identifier}", route)
            return {
                "operation_id": identifier, "operation_type": "import_documents", "status": "completed",
                "result_metadata": {
                    "documents_imported": 3, "documents_skipped": 0, "facts_imported": 3,
                    "observations_imported": 0,
                },
            }

        with patch.dict("os.environ", {"HINDSIGHT_API_KEY": "local-fixture-only"}):
            client = Hindsight("http://127.0.0.1:8080", bank)
        with patch.object(client, "request", side_effect=reply):
            result = client.transfer(b"fixture", 3)
        self.assertEqual(3, result["documents_imported"])
        self.assertEqual(3, len(requests))


    def test_http_operations_use_distinct_timeout_budgets(self) -> None:
        projection = self.make_publication().projection
        hit = Hit(projection.rows["F-001"], "fixture-fact")

        def connect(host: str, port: int, *, timeout: float) -> HTTPConnection:
            return HTTPConnection(host, port, timeout=timeout)

        with patch.dict("os.environ", {"HINDSIGHT_API_KEY": "local-fixture-only"}):
            client = Hindsight("http://127.0.0.1:8080", projection.review.bank)
        with (
            patch("tools.evidence_hindsight_client.HTTPConnection", side_effect=connect),
            patch.object(HTTPConnection, "request"),
            patch.object(HTTPConnection, "getresponse", side_effect=TimeoutError),
        ):
            with self.assertRaisesRegex(HindsightTimeout, f"after {CONTROL_TIMEOUT:g} seconds"):
                _ = client.request("/documents")
            with self.assertRaisesRegex(HindsightTimeout, f"after {RECALL_TIMEOUT:g} seconds"):
                _ = client.recall(projection, "Local recall fixture")
            with self.assertRaisesRegex(HindsightTimeout, f"after {REFLECT_TIMEOUT:g} seconds"):
                _ = client.draft("Local reflection fixture", (hit,))

    def test_recall_and_draft_timeouts_preserve_canonical_rows(self) -> None:
        publication = self.make_publication()

        def available(projection: Projection, _question: str) -> tuple[Hit, ...]:
            return (Hit(projection.rows["F-001"], "fixture-fact"),)

        with patch("tools.evidence_hindsight.Hindsight", FakeHindsight), patch.object(
            FakeHindsight, "recall", side_effect=HindsightTimeout(RECALL_TIMEOUT),
        ):
            fallback = query(publication, "What was approved?", ("F-001",), "http://127.0.0.1:8080", False)
        self.assertEqual("exact-fallback", fallback["retrieval"])
        self.assertEqual("timeout", fallback["retrieval_detail"])
        self.assertEqual(["F-001"], fallback["verified_handles"])

        with (
            patch("tools.evidence_hindsight.Hindsight", FakeHindsight),
            patch.object(FakeHindsight, "recall", side_effect=available),
            patch.object(FakeHindsight, "draft", create=True, side_effect=HindsightTimeout(REFLECT_TIMEOUT)),
        ):
            timed_out = query(publication, "What was approved?", (), "http://127.0.0.1:8080", True)
        self.assertEqual("verified-recall", timed_out["retrieval"])
        self.assertEqual("timeout", timed_out["draft_status"])
        self.assertIsNone(timed_out["draft"])
        self.assertEqual(fallback["rows"], timed_out["rows"])
