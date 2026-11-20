# SPDX-License-Identifier: AGPL-3.0-or-later
"""The online commands against an in-memory fake bank, in invented Git repositories. No service, no network."""

from __future__ import annotations

import json
import os
import re
import socket
import tempfile
import unittest
from collections.abc import Sequence
from datetime import datetime, timedelta
from pathlib import Path
from typing import cast, override
from unittest.mock import patch

from tools.memory import online
from tools.memory.bank import Applied, Bank, BankError, Desired, Universe, desired, gather
from tools.memory.hindsight import (
    AllOf, AnyOf, Client, ConfigError, Connection, Document, Group, Hit, Item, Not, Tags, Unavailable, Unit, connection, group_json,
)
from tools.memory.journal import Revision, read_tree, write_change_set
from tools.memory.model import JSON, Record, load_json, sha256_hex
from tools.memory.online import Changes, Refused
from tools.tests.memory_fixtures import (
    CREATED, MEMBER, NEW_CLIENT, NEW_SERVER, OTHER_MEMBER, chain, clean_git_environment, commit_all, evidence, finding, new_repository,
    run_git, unknown, write,
)

TODAY = CREATED.replace(hour=13)


def matches(group: Group, labels: set[str]) -> bool:
    """Hindsight's strict semantics: an untagged document matches no leaf, and a ``not`` excludes it too."""
    match group:
        case Tags(names, every):
            return bool(labels) and (all(name in labels for name in names) if every else any(name in labels for name in names))
        case Not(inner):
            return bool(labels) and not matches(inner, labels)
        case AnyOf(groups):
            return any(matches(item, labels) for item in groups)
        case AllOf(groups):
            return all(matches(item, labels) for item in groups)


class FakeBank:
    """An in-memory bank with Hindsight's document semantics; ``fail_retains`` makes retains raise."""

    def __init__(self) -> None:
        self.held: dict[str, Document] = {}
        self.queued: dict[str, list[Item]] = {}
        self.fail_retains: bool = False
        self.extra_banks: tuple[str, ...] = ()
        self.objects: dict[str, int] = {}
        self.settings: dict[str, object] = dict(online.EXPECTED_CONFIG)
        self.extra_units: list[Unit] = []
        self.async_calls: int = 0

    @property
    def bank(self) -> str:
        return "icarus"

    def put(self, item: Item) -> None:
        self.held[item.document_id] = Document(
            item.document_id, item.content, sha256_hex(item.content.encode()), tuple(sorted(item.tags)), dict(item.metadata),
            tuple(sorted(item.entities)), None if item.timestamp == "unset" else item.timestamp,
        )

    def retain(self, items: Sequence[Item]) -> None:
        if self.fail_retains:
            raise Unavailable("POST /memories: service unavailable (fake)")
        for item in items:
            self.put(item)

    def retain_async(self, items: Sequence[Item]) -> str:
        self.async_calls += 1
        name = f"op-{len(self.queued)}"
        self.queued[name] = list(items)
        return name

    def wait(self, operations: Sequence[str], deadline: float = 0.0) -> None:
        del deadline
        for name in operations:
            for item in self.queued.pop(name):
                self.put(item)

    def document(self, document_id: str) -> Document | None:
        return self.held.get(document_id)

    def delete(self, document_id: str) -> None:
        _ = self.held.pop(document_id, None)

    def retag(self, document_id: str, tags: Sequence[str]) -> None:
        current = self.held[document_id]
        self.held[document_id] = Document(current.id, current.text, current.content_hash, tuple(sorted(tags)), current.metadata,
                                          current.entities, current.event_date)

    def documents(self, tags: Tags | None = None) -> list[Document]:
        found = [item for item in self.held.values() if tags is None or matches(tags, set(item.tags))]
        return [Document(item.id, None, item.content_hash, item.tags, item.metadata, item.entities, item.event_date) for item in found]

    def units(self, *, text: str | None = None, document_id: str | None = None) -> list[Unit]:
        units = [Unit(f"u-{item.id}", item.id, item.text or "", "world", item.tags) for item in self.held.values()] + self.extra_units
        return [unit for unit in units if (text is None or text.casefold() in unit.text.casefold())
                and (document_id is None or unit.document_id == document_id)]

    def recall(self, query: str, groups: Sequence[Group], *, query_timestamp: str, max_tokens: int = 4096) -> list[Hit]:
        del max_tokens
        assert re.fullmatch(r"[0-9]{4}-[0-9]{2}-[0-9]{2}T23:59:59Z", query_timestamp)
        words = {word for word in words_of(query) if len(word) > 2}
        scored: list[tuple[int, str, Document]] = []
        for item in self.held.values():
            if all(matches(group, set(item.tags)) for group in groups):
                score = len(words & words_of(item.text or ""))
                if score:
                    scored.append((-score, item.id, item))
        return [Hit(item.id, item.text or "", item.tags, item.metadata, float(-score)) for score, _, item in sorted(scored)]

    def config(self) -> tuple[dict[str, object], dict[str, object]]:
        return dict(self.settings), {}

    def banks(self) -> tuple[str, ...]:
        return tuple(sorted(("icarus", *self.extra_banks)))

    def count(self, kind: str) -> int:
        return self.objects.get(kind, 0)


def words_of(text: str) -> set[str]:
    return {match[0] for match in re.finditer(r"[a-z0-9]+", text.lower())}


def ignore_retains(_items: Sequence[Item]) -> None:
    return None


def no_reconcile(_bank: Bank, _universe: Universe, *, bulk: bool = False, orphan: bool = False) -> Applied:
    del bulk, orphan
    return Applied()


def changes(**values: JSON) -> Changes:
    return Changes(dict(values))


class Online(unittest.TestCase):
    top: Path = Path()
    root: Path = Path()
    bank: FakeBank = FakeBank()
    second: int = 0

    @override
    def setUp(self) -> None:
        self.top = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.root = self.top / "primary"
        self.root.mkdir()
        self.enterContext(clean_git_environment())
        self.enterContext(patch.dict(os.environ, {"ICARUS_MEMORY_HOME": str(self.top / "home")}))
        new_repository(self.root)
        self.put("config/release.toml", f"[release]\nbuild_id = {NEW_SERVER}\n")
        self.bank = FakeBank()
        self.second = 0

    def put(self, path: str, text: str, root: Path | None = None) -> None:
        target = (root or self.root) / path
        target.parent.mkdir(parents=True, exist_ok=True)
        _ = target.write_text(text, encoding="utf-8")

    def tick(self) -> datetime:
        self.second += 1
        return TODAY + timedelta(seconds=self.second)

    def accepted_chain(self) -> None:
        """The fixture chain, merged into ``origin/main``."""
        _ = write(self.root, chain())
        commit_all(self.root)

    def new_finding(self, claim: str, **extra: JSON) -> str:
        values: dict[str, JSON | None] = {"claim": claim, "kind": "observed", "confidence": "medium", "standing": "confirmed", "area": "listing",
                                          "scope": {"basis": "explicit", "subjects": ["original-server"], "builds": [f"server:{NEW_SERVER}"]}}
        values.update(extra)
        output = online.propose(self.bank, self.root, "finding", Changes(values), now=self.tick())
        ids = output.data["ids"]
        assert isinstance(ids, list) and isinstance(ids[0], str)
        return ids[0]

    def doc(self, document_id: str) -> Document:
        found = self.bank.held.get(document_id)
        assert found is not None, f"{document_id} not in the bank: {sorted(self.bank.held)}"
        return found

    def wanted(self) -> dict[str, Desired]:
        return desired(gather(self.root))


class PublicationTests(Online):
    def test_propose_writes_applies_and_tags(self) -> None:
        self.accepted_chain()
        identifier = self.new_finding("Invented: the lock follows the key on the new build", cites=["M-102"])
        self.assertEqual("F-107", identifier)
        document = self.doc("F-107")
        self.assertTrue(document.text is not None and document.text.startswith("F-107 finding active\nInvented: the lock follows"))
        segment = document.metadata["segment"]
        self.assertTrue((self.root / segment).is_file())
        stem = Path(segment).stem
        self.assertEqual(
            ("area:listing", "basis:explicit", f"build:server:{NEW_SERVER}", "id:F-107", f"seg:{stem}", "state:proposed", "status:active",
             "subject:original-server", "type:finding"), document.tags)
        self.assertEqual({"record", "sha256", "prev", "segment"}, set(document.metadata))
        self.assertEqual(sha256_hex(document.metadata["record"].encode()), document.metadata["sha256"])
        self.assertEqual("2026-11-20T00:00:00Z", document.event_date)
        self.assertIn("M-102", document.entities)
        self.assertEqual("accepted", self.doc("F-101").tags[[tag.split(":")[0] for tag in self.doc("F-101").tags].index("state")].split(":")[1])

    def test_validation_refuses_before_writing(self) -> None:
        self.accepted_chain()
        before = len(read_tree(self.root))
        with self.assertRaisesRegex(Refused, "no record M-999"):
            _ = self.new_finding("Invented: cites a ghost", cites=["M-999"])
        with self.assertRaisesRegex(Refused, "matches 0 listed members"):
            _ = self.new_finding("Invented: cites a missing member", cites=["M-101/ev_ffffffffffff"])
        with self.assertRaisesRegex(Refused, "allocates"):
            _ = online.propose(self.bank, self.root, "finding", changes(id="F-500", claim="x"), now=self.tick())
        self.assertEqual(before, len(read_tree(self.root)))

    def test_killed_between_segment_and_bank_is_healed_by_the_next_command(self) -> None:
        self.accepted_chain()
        _ = online.status(self.bank, self.root)
        self.bank.fail_retains = True
        with self.assertRaises(Unavailable):
            _ = self.new_finding("Invented: written but never applied")
        segment = [item for item in read_tree(self.root) if any(revision.record.id == "F-107" for revision in item.revisions)]
        self.assertEqual(1, len(segment))
        self.assertNotIn("F-107", self.bank.held)
        self.bank.fail_retains = False
        output = online.status(self.bank, self.root)
        self.assertIn("bank: 1 retained", "\n".join(output.lines))
        self.assertEqual(self.wanted()["F-107"].item.content, self.doc("F-107").text)

    def test_reconcile_detects_a_bank_that_does_not_take_writes(self) -> None:
        self.accepted_chain()
        with patch.object(self.bank, "retain", ignore_retains), self.assertRaisesRegex(BankError, "still differs"):
            _ = online.status(self.bank, self.root)

    def test_allocation_spans_bank_refs_worktrees_and_legacy(self) -> None:
        self.put("docs/ledgers/findings.md", "| F-120 | invented |\n")
        self.put("evidence/area/001.jsonl", json.dumps({"id": "M-130"}) + "\n")
        commit_all(self.root)
        _ = run_git(self.root, "checkout", "-q", "-b", "other")
        self.put("docs/ledgers/findings.md", "| F-120 | invented |\n| F-1285 | invented, other branch |\n")
        _ = run_git(self.root, "commit", "-qam", "other")
        _ = run_git(self.root, "checkout", "-q", "main")
        linked = self.top / "linked"
        _ = run_git(self.root, "worktree", "add", "-q", "-b", "side", str(linked))
        self.put("evidence/area/001.jsonl", json.dumps({"id": "M-1641"}) + "\n", linked)
        self.assertEqual("F-1286", self.new_finding("Invented: above the other branch's legacy row"))
        output = online.propose(self.bank, self.root, "unknown", changes(question="Invented?", severity="low", next_probe="Look", area="listing"), now=self.tick())
        self.assertEqual(["U-001"], output.data["ids"])
        self.put("work/case/r.txt", "bytes")
        added = online.evidence_add(self.bank, self.root, "work/case/r.txt", "receipt", "Invented receipt", now=self.tick())
        self.assertEqual(["M-1642"], added.data["ids"])
        self.bank.put(Item("F-2000", "x", ("type:finding",), {}, "unset", ()))
        self.assertEqual("F-2001", self.new_finding("Invented: above the bank's highest"))


class RevisionTests(Online):
    def test_supersede_refreshes_the_old_document_without_revising_it(self) -> None:
        self.accepted_chain()
        new = self.new_finding("Invented: the lock follows the account, not the key")
        before = sum(len(item.revisions) for item in read_tree(self.root) if any(revision.record.id == "F-106" for revision in item.revisions))
        _ = online.supersede(self.bank, self.root, "F-106", new, now=self.tick())
        after = sum(len(item.revisions) for item in read_tree(self.root) if any(revision.record.id == "F-106" for revision in item.revisions))
        self.assertEqual(before, after)
        old = self.doc("F-106")
        self.assertIn("status:superseded", old.tags)
        self.assertTrue(old.text is not None and old.text.startswith("F-106 finding superseded\n"))
        self.assertIn('"status":"active"', old.metadata["record"])
        with self.assertRaisesRegex(Refused, "is accepted"):
            _ = online.supersede(self.bank, self.root, "F-104", "F-106", now=self.tick())

    def test_resolve_on_a_different_release(self) -> None:
        self.accepted_chain()
        _ = online.propose(self.bank, self.root, "unknown", changes(question="Which invented build drops the lock?", severity="low",
                                                                      next_probe="Diff the invented handlers", area="listing"), now=self.tick())
        fixer = self.new_finding("Invented: build two drops the lock", scope={"basis": "explicit", "subjects": ["original-server"],
                                                                              "builds": [f"server:{NEW_SERVER}", f"client:{NEW_CLIENT}"]})
        _ = online.resolve(self.bank, self.root, "U-102", fixer, now=self.tick())
        self.assertIn("status:resolved", self.doc("U-102").tags)
        got = online.get(self.bank, self.root, "U-102")
        self.assertEqual(0, got.status)
        self.assertEqual([fixer], got.data["resolved_by"])
        with self.assertRaisesRegex(Refused, "only a finding"):
            _ = online.resolve(self.bank, self.root, "U-102", "U-101", now=self.tick())

    def test_revise_allow_list_pending_and_acceptance(self) -> None:
        self.accepted_chain()
        with self.assertRaisesRegex(Refused, "claim is not revisable"):
            _ = online.revise(self.bank, self.root, "F-104", changes(claim="Invented: changed"), now=self.tick())
        with self.assertRaisesRegex(Refused, "label is not revisable"):
            _ = online.revise(self.bank, self.root, "M-102", changes(label="Changed"), now=self.tick())
        with self.assertRaisesRegex(Refused, "appends a note"):
            _ = online.revise(self.bank, self.root, "M-102", changes(status="retired"), now=self.tick())
        output = online.revise(self.bank, self.root, "U-101", Changes({"status": "investigating", "next_probe": "Read the invented table"}), now=self.tick())
        sha = output.data["sha256"]
        assert isinstance(sha, list) and isinstance(sha[0], str)
        pending = [name for name in self.bank.held if name.startswith("U-101@")]
        self.assertEqual(1, len(pending))
        self.assertIn("state:pending", self.doc(pending[0]).tags)
        self.assertIn("state:accepted", self.doc("U-101").tags)
        self.assertIn('"status":"open"', self.doc("U-101").metadata["record"])
        got = online.get(self.bank, self.root, "U-101")
        self.assertIn(f"pending revision: {pending[0]}", "\n".join(got.lines))
        commit_all(self.root, "merge the revision")
        _ = online.status(self.bank, self.root)
        self.assertNotIn(pending[0], self.bank.held)
        self.assertIn("status:resolved", self.doc("U-101").tags)
        self.assertIn('"status":"investigating"', self.doc("U-101").metadata["record"])
        self.assertEqual(sha[0], self.doc("U-101").metadata["sha256"])

    def test_fork_shows_conflict_and_revise_reconciles(self) -> None:
        self.accepted_chain()
        head = next(item for segment in read_tree(self.root) for item in segment.revisions if item.record.id == "U-101")
        one = Revision.of(Record.from_json(unknown("U-101", "Which invented key prompts for a password?", status="investigating")), [head.sha256])
        two = Revision.of(Record.from_json(unknown("U-101", "Which invented key prompts for a password?", status="blocked")), [head.sha256])
        _ = write_change_set(self.root, CREATED.replace(second=10), [one])
        _ = write_change_set(self.root, CREATED.replace(second=11), [two])
        commit_all(self.root, "both PRs merged")
        _ = online.status(self.bank, self.root)
        self.assertIn("state:conflict", self.doc("U-101").tags)
        output = online.revise(self.bank, self.root, "U-101", Changes({"status": "investigating"}, note="Reconciled"), now=self.tick())
        merged = next(item for segment in read_tree(self.root) for item in segment.revisions
                      if item.record.id == "U-101" and len(item.prev) == 2)
        self.assertEqual(sorted([one.sha256, two.sha256]), list(merged.prev))
        self.assertEqual(0, output.status)
        commit_all(self.root, "reconciled")
        _ = online.status(self.bank, self.root)
        self.assertIn("state:accepted", self.doc("U-101").tags)


class EvidenceTests(Online):
    def test_add_bundle_and_verify_outcomes(self) -> None:
        bundle = {"records": [{"evidence_id": MEMBER}, {"evidence_id": MEMBER}], "unknowns": [{"unknown_id": OTHER_MEMBER}]}
        self.put("work/case/rea-bundle.json", json.dumps(bundle))
        self.put("work/case/receipt.txt", "invented receipt")
        self.put("work/case/dir/x.txt", "x")
        output = online.evidence_add(self.bank, self.root, "work/case/rea-bundle.json", "rea-bundle", "Invented bundle", now=self.tick())
        record = Record.from_json(load_json(self.doc("M-001").metadata["record"]))
        self.assertEqual((1, (MEMBER, OTHER_MEMBER)), (record.number("records"), record.items("members")))
        self.assertEqual("local-private", record.text("class"))
        self.assertEqual(["M-001"], output.data["ids"])
        _ = online.evidence_add(self.bank, self.root, str(self.root / "work/case/receipt.txt"), "receipt", "Invented receipt", now=self.tick())
        with self.assertRaisesRegex(Refused, "already"):
            _ = online.evidence_add(self.bank, self.root, "work/case/receipt.txt", "receipt", "Again", now=self.tick())
        with self.assertRaisesRegex(Refused, "sha256 manifest"):
            _ = online.evidence_add(self.bank, self.root, "work/case/dir", "receipt", "A directory", now=self.tick())
        with self.assertRaisesRegex(Refused, "primary checkout"):
            _ = online.evidence_add(self.bank, self.root, str(self.top / "elsewhere.txt"), "receipt", "Outside", now=self.tick())
        legacy = evidence("M-003", "work/case/legacy/", legacy={"path": "evidence/x.jsonl", "cells": {"row": "{}"}}, gaps=["sha256"])
        del legacy["sha256"]
        _ = write(self.root, [legacy], second=40)
        result = online.verify(self.root, (), every=True)
        self.assertEqual(0, result.status)
        self.assertEqual(["verified", "verified", "legacy-unverifiable"], [line.split(": ")[1].split(" ")[0] for line in result.lines[:3]])
        self.put("work/case/receipt.txt", "tampered")
        self.assertEqual((1, "mismatched"), (online.verify(self.root, ["M-002"]).status, online.verify(self.root, ["M-002"]).lines[0].split(": ")[1].split(" ")[0]))
        (self.root / "work/case/receipt.txt").unlink()
        self.assertIn("unavailable", online.verify(self.root, ["M-002"]).lines[0])
        self.assertEqual(1, online.verify(self.root, ["M-002"]).status)
        commit_all(self.root)
        linked = self.top / "linked"
        _ = run_git(self.root, "worktree", "add", "-q", "--detach", str(linked))
        self.assertIn("M-001: verified", online.verify(linked, ["M-001"]).lines[0])


class ReadingTests(Online):
    def test_get_verifies_against_the_journal(self) -> None:
        self.accepted_chain()
        got = online.get(self.bank, self.root, "M-101/ev_a1b2c3d4e5f6")
        self.assertEqual(0, got.status)
        self.assertEqual(MEMBER, got.data["member"])
        self.bank.held["F-102"] = Document("F-102", "tampered", sha256_hex(b"tampered"), self.doc("F-102").tags, self.doc("F-102").metadata, (), None)
        with patch.object(online, "reconcile", no_reconcile):
            tampered = online.get(self.bank, self.root, "F-102")
        self.assertEqual(1, tampered.status)
        self.assertIn("problem: F-102: text differs from the journal", tampered.lines)
        healed = online.get(self.bank, self.root, "F-102", history=True)
        self.assertEqual(0, healed.status)
        self.assertIn("history:", healed.lines)

    def test_recall_partitions_successors_and_visibility(self) -> None:
        self.accepted_chain()
        mine = self.new_finding("Invented: the listing lock lives in the handler table")
        other = self.top / "other"
        _ = run_git(self.root, "worktree", "add", "-q", "-b", "elsewhere", str(other))
        _ = write_change_set(other, CREATED.replace(second=50), [Revision.of(Record.from_json(finding("F-150", "Invented: other listing lock theory")))])
        _ = online.revise(self.bank, self.root, "U-101", changes(status="investigating"), now=self.tick())
        output = online.recall(self.bank, self.root, "listing lock key F-104", all_proposals=True)
        text = "\n".join(output.lines)
        self.assertTrue(output.lines[0].startswith("named: F-104 [accepted, active]"))
        self.assertIn("== current ==", text)
        self.assertIn(f"{mine} [proposed, active; current against REL-9.0.2", text)
        self.assertIn("F-106 [accepted, active; other-build against REL-9.0.2", text)
        self.assertIn("retired under it: F-101 [accepted, superseded]", text)
        self.assertIn("retired under it: F-102 [accepted, superseded]", text)
        self.assertNotRegex(text, r"(?m)^F-10[12] \[")
        self.assertNotIn("M-102 [accepted, active;", text)
        self.assertNotIn("U-101@", text)
        self.assertIn("== proposals of other worktrees ==\nF-150 [proposed, active]", text)
        partitions = output.data["partitions"]
        assert isinstance(partitions, dict)
        self.assertEqual({"current", "other-build"}, set(partitions) - {"native", "unscoped"})
        default = "\n".join(online.recall(self.bank, self.root, "listing lock").lines)
        self.assertNotIn("F-150", default)
        evidence_hits = "\n".join(online.recall(self.bank, self.root, "invented report receipt", include_evidence=True).lines)
        self.assertIn("M-102 [accepted, active", evidence_hits)
        texts = "\n".join(online.recall(self.bank, self.root, "nothing ranks this", text="handler table").lines)
        self.assertIn(f"{mine} [proposed", texts)

    def test_area_and_status_views(self) -> None:
        self.accepted_chain()
        self.put("src/lib.rs", "// relies on F-101 and F-103\n")
        commit_all(self.root)
        listing = online.area(self.bank, self.root, "listing")
        text = "\n".join(listing.lines)
        self.assertIn("superseded:", text)
        self.assertIn("open unknowns: none", text)
        self.assertIn("M-101", text)
        state = online.status(self.bank, self.root)
        summary = "\n".join(state.lines)
        self.assertIn("pin: REL-9.0.2", summary)
        self.assertIn("F-101: superseded (first cited at src/lib.rs:1)", summary)
        self.assertNotIn("F-103: ", summary)
        self.assertIn("M-101: missing: work/invented-case/rea-bundle.json", summary)
        self.assertEqual(state.data, online.status(self.bank, self.root).data)


class SyncAndRebuildTests(Online):
    def remote(self) -> None:
        origin = self.top / "origin.git"
        _ = run_git(self.top, "init", "-q", "--bare", "-b", "main", str(origin))
        _ = run_git(self.root, "remote", "add", "origin", str(origin))
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "init")
        _ = run_git(self.root, "push", "-q", "origin", "main")

    def test_sync_accepts_orphans_only_after_a_fetch_and_unorphans(self) -> None:
        self.remote()
        _ = write(self.root, chain())
        _ = online.status(self.bank, self.root)
        self.assertIn("state:proposed", self.doc("F-101").tags)
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "records")
        _ = run_git(self.root, "push", "-q", "origin", "main")
        result = online.sync(self.bank, self.root)
        self.assertEqual(0, result.status)
        self.assertIn("state:accepted", self.doc("F-101").tags)
        stray = Revision.of(Record.from_json(finding("F-300", "Invented: from a deleted worktree")))
        path = write_change_set(self.root, CREATED.replace(second=30), [stray])[0]
        _ = online.status(self.bank, self.root)
        saved = path.read_bytes()
        path.unlink()
        _ = run_git(self.root, "remote", "set-url", "origin", str(self.top / "missing.git"))
        failed = online.sync(self.bank, self.root)
        self.assertEqual(1, failed.status)
        self.assertIn("state:proposed", self.doc("F-300").tags)
        _ = run_git(self.root, "remote", "set-url", "origin", str(self.top / "origin.git"))
        self.assertEqual(["F-300"], online.sync(self.bank, self.root).data["orphaned"])
        self.assertIn("state:orphaned", self.doc("F-300").tags)
        self.assertEqual(0, online.check_bank(self.bank, self.root).status)
        recall = "\n".join(online.recall(self.bank, self.root, "invented deleted worktree").lines)
        self.assertNotIn("F-300", recall)
        _ = path.write_bytes(saved)
        _ = online.status(self.bank, self.root)
        self.assertIn("state:proposed", self.doc("F-300").tags)

    def test_rebuild_into_an_empty_bank_and_check_bank(self) -> None:
        self.accepted_chain()
        for number in range(60):
            _ = write(self.root, [finding(f"F-{200 + number}", f"Invented bulk finding {number}")], second=number)
        _ = self.enterContext(patch.object(online, "DISK_FLOOR", 0))
        result = online.rebuild(self.bank, self.root)
        self.assertEqual((0, 70), (result.status, result.data["retained"]))
        self.assertEqual(2, self.bank.async_calls)
        self.assertEqual(0, online.check_bank(self.bank, self.root).status)
        self.assertEqual(0, online.rebuild(self.bank, self.root).data["retained"])

    def test_check_bank_finds_drift(self) -> None:
        self.accepted_chain()
        _ = online.status(self.bank, self.root)
        self.assertEqual(0, online.check_bank(self.bank, self.root).status)
        self.bank.settings["retain_chunk_size"] = 3000
        self.bank.extra_banks = ("typo",)
        self.bank.objects["mental-models"] = 1
        self.bank.put(Item("loose", "untagged text", (), {}, "unset", ()))
        self.bank.retag("F-104", ("type:finding", "id:F-104", "state:accepted"))
        self.bank.extra_units.append(Unit("obs", "F-104", "an observation", "observation", ()))
        problems = "\n".join(online.check_bank(self.bank, self.root).lines)
        for expected in ("config retain_chunk_size is 3000", "banks are icarus, typo", "1 mental-models exist", "loose: untagged",
                         "loose: not explained", "F-104: tags differ", "of type observation", "F-104: 2 memory unit(s)"):
            self.assertIn(expected, problems)


class ClientTests(unittest.TestCase):
    def test_groups_are_strict_only(self) -> None:
        tree = AllOf((AnyOf((Tags(("a",)), Tags(("b", "c"), every=False))), Not(Tags(("d",)))))
        self.assertEqual({"and": [{"or": [{"tags": ["a"], "match": "all_strict"}, {"tags": ["b", "c"], "match": "any_strict"}]},
                                  {"not": {"tags": ["d"], "match": "all_strict"}}]}, group_json(tree))
        self.assertNotRegex(json.dumps(group_json(tree)), r'"(any|all|exact)"|fuzzy')

    def test_key_never_shown(self) -> None:
        key = "k" * 64
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 0))
            port = cast(tuple[str, int], probe.getsockname())[1]  # an AF_INET socket's address
        connected = Connection(f"http://127.0.0.1:{port}", key, "icarus")
        client = Client(connected, timeout=2)
        self.assertNotIn(key, repr(connected) + repr(client))
        with self.assertRaises(Unavailable) as caught:
            _ = client.documents()
        self.assertNotIn(key, str(caught.exception))

    def test_connection_file_rules(self) -> None:
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"ICARUS_MEMORY_HOME": directory}):
            path = Path(directory) / "connection.json"
            with self.assertRaisesRegex(ConfigError, "unreadable"):
                _ = connection()
            _ = path.write_text(json.dumps({"url": "http://127.0.0.1:1", "key": "secret-value", "bank": "icarus"}))
            path.chmod(0o644)
            with self.assertRaisesRegex(ConfigError, "0600"):
                _ = connection()
            path.chmod(0o600)
            self.assertEqual("icarus", connection().bank)
            _ = path.write_text(json.dumps({"url": "http://example.invalid:1", "key": "secret-value", "bank": "icarus"}))
            with self.assertRaisesRegex(ConfigError, "loopback") as caught:
                _ = connection()
            self.assertNotIn("secret-value", str(caught.exception))
