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

from tools.memory import bank as bank_module, durability, journal, online, service
from tools.memory.bank import Applied, Bank, BankError, Desired, Universe, desired, gather
from tools.memory.durability import Watermark
from tools.memory.gitio import GitError, run_git as run_git_tool
from tools.memory.hindsight import (
    AllOf, AnyOf, Client, Document, Group, Hit, Item, Not, Tags, Unavailable, Unit, group_json,
)
from tools.memory.journal import Revision, read_tree, write_change_set
from tools.memory.model import JSON, Record, RecordError, load_json, sha256_hex
from tools.memory.online import Changes, Output, Refused
from tools.memory.service import Connection, ServiceError, connection
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
        self.settings: dict[str, object] = dict(online.EXPECTED_CONFIG) | {"enable_reranking": False}
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
        """One unit per document, or chunks of at most 1,800 characters of a longer text, as Hindsight's chunks mode."""
        units = [Unit(f"u-{item.id}-{start}", item.id, (item.text or "")[start:start + 1800], "world", item.tags)
                 for item in self.held.values() for start in range(0, max(len(item.text or ""), 1), 1800)] + self.extra_units
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
        self.assertEqual({"record", "sha256", "rev", "prev", "segment"}, set(document.metadata))
        self.assertEqual(read_tree(self.root)[-1].revisions[0].rev, document.metadata["rev"])
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


class LegacyAllocationTests(Online):
    """Allocation reads the legacy trees of every ref: present without IDs, absent, or a mix."""

    def test_legacy_trees_without_ids(self) -> None:
        self.put("docs/ledgers/findings.md", "| ID | claim |\n| --- | --- |\n")
        self.put("evidence/area/001.jsonl", "")
        commit_all(self.root)
        self.assertEqual("F-001", self.new_finding("Invented: no legacy ID anywhere"))
        self.put("work/case/r.txt", "bytes")
        self.assertEqual(["M-001"], online.evidence_add(self.bank, self.root, "work/case/r.txt", "receipt", "Invented", now=self.tick()).data["ids"])

    def test_legacy_trees_absent_on_every_ref(self) -> None:
        commit_all(self.root)
        self.assertEqual("F-001", self.new_finding("Invented: after cut-over"))

    def test_a_mix_across_refs(self) -> None:
        self.put("docs/ledgers/findings.md", "| F-130 | invented |\n")
        commit_all(self.root)
        _ = run_git(self.root, "branch", "old-full")
        _ = run_git(self.root, "checkout", "-q", "-b", "old-empty")
        self.put("docs/ledgers/findings.md", "| ID | claim |\n")
        _ = run_git(self.root, "commit", "-qam", "emptied")
        _ = run_git(self.root, "checkout", "-q", "main")
        _ = run_git(self.root, "rm", "-rq", "docs/ledgers")
        _ = run_git(self.root, "commit", "-qm", "cut-over")
        _ = run_git(self.root, "update-ref", "refs/remotes/origin/main", "HEAD")
        self.assertEqual("F-131", self.new_finding("Invented: above the old branch's legacy row"))
        self.assertEqual(["U-001"], online.propose(self.bank, self.root, "unknown", changes(question="Invented?", severity="low", next_probe="Look",
                                                                                         area="listing"), now=self.tick()).data["ids"])

    def test_a_real_git_failure_still_raises(self) -> None:
        with self.assertRaisesRegex(GitError, "git grep: exit status 128"):
            _ = run_git_tool(self.root, "grep", "-e", "x", "no-such-tree", ok=(0, 1))


class RevisionTests(Online):
    def segments_of(self, identifier: str) -> list[str]:
        return [item.path for item in read_tree(self.root) if any(revision.record.id == identifier for revision in item.revisions)]

    def test_supersede_by_a_proposal_regenerates_it_and_flips_nothing_until_accepted(self) -> None:
        self.accepted_chain()
        new = self.new_finding("Invented: the lock follows the account, not the key")
        before_old, before_new = self.segments_of("F-106"), self.segments_of(new)
        output = online.supersede(self.bank, self.root, "F-106", new, now=self.tick())
        self.assertEqual(before_old, self.segments_of("F-106"), "the old record gets no revision")
        after_new = self.segments_of(new)
        self.assertEqual(1, len(after_new))
        self.assertNotEqual(before_new, after_new, "the never-accepted proposal's segment is regenerated")
        self.assertFalse((self.root / before_new[0]).exists())
        self.assertEqual(after_new[0], output.data["regenerated"] and self.doc(new).metadata["segment"])
        regenerated = next(segment for segment in read_tree(self.root) if segment.path == after_new[0])
        self.assertEqual(((), ("F-106",)), (regenerated.revisions[0].prev, regenerated.revisions[0].record.links.supersedes))
        old = self.doc("F-106")
        self.assertIn("status:active", old.tags, "a proposal never flips an accepted record in the bank")
        self.assertTrue(old.text is not None and old.text.startswith("F-106 finding active\n"))
        got = online.get(self.bank, self.root, "F-106")
        self.assertEqual("active", got.data["status"])
        self.assertIn(f"proposed superseder: {new} (proposed, this worktree)", got.lines)
        commit_all(self.root, "merge the superseder")
        _ = online.status(self.bank, self.root)
        old = self.doc("F-106")
        self.assertIn("status:superseded", old.tags)
        self.assertTrue(old.text is not None and old.text.startswith("F-106 finding superseded\n"))
        self.assertIn('"status":"active"', old.metadata["record"])
        self.assertNotIn("proposed superseder", "\n".join(online.get(self.bank, self.root, "F-106").lines))
        with self.assertRaisesRegex(Refused, "only a finding"):
            _ = online.resolve(self.bank, self.root, "U-101", "U-101", now=self.tick())

    def test_an_accepted_record_gains_a_link_by_an_appended_revision(self) -> None:
        self.accepted_chain()
        _ = online.supersede(self.bank, self.root, "F-104", "F-106", now=self.tick())
        self.assertEqual(2, len(self.segments_of("F-106")), "accepted: the link is a new, pending revision")
        pending = [name for name in self.bank.held if name.startswith("F-106@")]
        self.assertEqual(1, len(pending))
        self.assertIn("status:active", self.doc("F-104").tags)
        self.assertIn("proposed superseder: F-106 (proposed, this worktree)", online.get(self.bank, self.root, "F-104").lines)
        with self.assertRaisesRegex(Refused, "cites changed; it is not revisable"):
            _ = online.revise(self.bank, self.root, "F-106", changes(cites=["M-101"]), now=self.tick())
        commit_all(self.root, "merge the link")
        _ = online.status(self.bank, self.root)
        self.assertIn("status:superseded", self.doc("F-104").tags)
        self.assertNotIn(pending[0], self.bank.held)

    def test_a_proposal_shared_with_another_ref_gets_a_revision_not_a_regeneration(self) -> None:
        self.accepted_chain()
        new = self.new_finding("Invented: pushed before it was linked")
        commit_all(self.root, "pushed")
        _ = run_git(self.root, "update-ref", "refs/remotes/origin/feature", "HEAD")
        _ = run_git(self.root, "update-ref", "refs/remotes/origin/main", "HEAD~1")
        before = self.segments_of(new)
        output = online.supersede(self.bank, self.root, "F-106", new, now=self.tick())
        self.assertNotIn("regenerated", output.data)
        self.assertEqual(2, len(self.segments_of(new)))
        self.assertTrue(all((self.root / path).exists() for path in before))

    def test_an_interrupted_regeneration_is_finished_by_the_next_command(self) -> None:
        self.accepted_chain()
        new = self.new_finding("Invented: regenerated while killed")
        old_path = self.segments_of(new)[0]
        with patch.object(online, "remove_segment", side_effect=OSError("killed")), self.assertRaises(OSError):
            _ = online.supersede(self.bank, self.root, "F-106", new, now=self.tick())
        self.assertEqual(2, len(self.segments_of(new)))
        _ = online.status(self.bank, self.root)
        self.assertEqual(1, len(self.segments_of(new)))
        self.assertFalse((self.root / old_path).exists())
        self.assertIn("F-106", Record.from_json(load_json(self.doc(new).metadata["record"])).links.supersedes)

    def test_worktree_proposals_never_show_in_another_worktree(self) -> None:
        self.accepted_chain()
        other = self.top / "other"
        _ = run_git(self.root, "worktree", "add", "-q", "-b", "elsewhere", str(other))
        new = self.new_finding("Invented: the lock follows the account after all")
        _ = online.supersede(self.bank, self.root, "F-106", new, now=self.tick())
        from_b = online.get(self.bank, other, "F-106")
        self.assertEqual("active", from_b.data["status"])
        self.assertNotIn("proposed", "\n".join(from_b.lines))
        recall_b = "\n".join(online.recall(self.bank, other, "lock follows the key re-read").lines)
        self.assertIn("F-106 [finding; accepted, active;", recall_b)
        self.assertNotIn(new, recall_b)
        everything = online.get(self.bank, other, "F-106", all_proposals=True)
        self.assertIn(f"proposed superseder: {new} (proposed, another worktree or branch)", everything.lines)
        from_a = "\n".join(online.recall(self.bank, self.root, "lock follows the key re-read").lines)
        self.assertIn("F-106 [finding; accepted, active;", from_a)
        self.assertIn(f"proposed superseder: {new} (proposed, this worktree)", from_a)
        self.assertIn(f"proposed superseder: {new} (proposed, this worktree)", online.get(self.bank, self.root, "F-106").lines)

    def test_resolve_on_a_different_release(self) -> None:
        self.accepted_chain()
        _ = online.propose(self.bank, self.root, "unknown", changes(question="Which invented build drops the lock?", severity="low",
                                                                      next_probe="Diff the invented handlers", area="listing"), now=self.tick())
        fixer = self.new_finding("Invented: build two drops the lock", scope={"basis": "explicit", "subjects": ["original-server"],
                                                                              "builds": [f"server:{NEW_SERVER}", f"client:{NEW_CLIENT}"]})
        output = online.resolve(self.bank, self.root, "U-102", fixer, now=self.tick())
        self.assertIn("regenerated", output.data)
        self.assertIn("status:open", self.doc("U-102").tags)
        got = online.get(self.bank, self.root, "U-102")
        self.assertEqual(0, got.status)
        self.assertIn(f"proposed resolver: {fixer} (proposed, this worktree)", got.lines)
        commit_all(self.root, "merge the resolution")
        _ = online.status(self.bank, self.root)
        self.assertIn("status:resolved", self.doc("U-102").tags)
        self.assertEqual([fixer], online.get(self.bank, self.root, "U-102").data["resolved_by"])
        with self.assertRaisesRegex(Refused, "only a finding"):
            _ = online.resolve(self.bank, self.root, "U-102", "U-101", now=self.tick())

    def test_revise_allow_list_pending_and_acceptance(self) -> None:
        self.accepted_chain()
        with self.assertRaisesRegex(Refused, "claim changed; it is not revisable"):
            _ = online.revise(self.bank, self.root, "F-104", changes(claim="Invented: changed"), now=self.tick())
        with self.assertRaisesRegex(Refused, "label changed; it is not revisable"):
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
        one = Revision.of(Record.from_json(unknown("U-101", "Which invented key prompts for a password?", status="investigating")), [head.rev])
        two = Revision.of(Record.from_json(unknown("U-101", "Which invented key prompts for a password?", status="blocked")), [head.rev])
        _ = write_change_set(self.root, CREATED.replace(second=10), [one])
        _ = write_change_set(self.root, CREATED.replace(second=11), [two])
        commit_all(self.root, "both PRs merged")
        _ = online.status(self.bank, self.root)
        self.assertIn("state:conflict", self.doc("U-101").tags)
        output = online.revise(self.bank, self.root, "U-101", Changes({"status": "investigating"}), now=self.tick())
        merged = next(item for segment in read_tree(self.root) for item in segment.revisions
                      if item.record.id == "U-101" and len(item.prev) == 2)
        self.assertEqual(sorted([one.rev, two.rev]), list(merged.prev))
        self.assertEqual((one.record, one.sha256), (merged.record, merged.sha256), "identical in content to one head")
        self.assertNotEqual(one.rev, merged.rev)
        self.assertEqual(0, output.status)
        self.assertEqual(merged.rev, self.doc(f"U-101@{merged.rev[:12]}").metadata["rev"])
        commit_all(self.root, "reconciled")
        _ = online.status(self.bank, self.root)
        self.assertIn("state:accepted", self.doc("U-101").tags)


class QualificationTests(Online):
    """CP proofs (rules, "Qualification"), enforced by ``propose`` and ``revise``."""

    def compatibility(self, status: str, scope: JSON, **links: JSON) -> Output:
        values: dict[str, JSON | None] = {"name": "Invented: listing round trip", "qualifier": "invented qualifier",
                                          "required_proof": "an invented run", "scope": scope, "status": status, **links}
        return online.propose(self.bank, self.root, "compatibility", Changes(values), now=self.tick())

    def run_evidence(self, name: str, *, outcome: str = "pass", environment: str = "two hosts", targets: list[JSON] | None = None) -> str:
        self.put(f"work/case/{name}.txt", f"invented run {name}")
        run: dict[str, JSON] = {
            "workload": "invented listing", "targets": targets or [f"server:{NEW_SERVER}", f"client:{NEW_CLIENT}"],
            "matched_reference": "invented reference", "identities": ["invented identity"], "start": "2026-11-20T10:00:00Z",
            "end": "2026-11-20T10:05:00Z", "environment": environment, "thresholds": ["invented threshold"], "outcome": outcome,
        }
        output = online.evidence_add(self.bank, self.root, f"work/case/{name}.txt", "run", f"Invented run {name}", run=run, now=self.tick())
        ids = output.data["ids"]
        assert isinstance(ids, list) and isinstance(ids[0], str)
        return ids[0]

    def test_specified_needs_accepted_active_findings_in_scope(self) -> None:
        self.accepted_chain()
        scope: JSON = {"basis": "explicit", "subjects": ["original-server"], "builds": [f"server:{NEW_SERVER}"]}
        with self.assertRaisesRegex(Refused, "specified needs refs naming the accepted findings"):
            _ = self.compatibility("specified", scope)
        with self.assertRaisesRegex(Refused, "specified needs accepted active findings; F-101 is superseded"):
            _ = self.compatibility("specified", scope, refs=["F-101"])
        proposal = self.new_finding("Invented: not yet accepted")
        with self.assertRaisesRegex(Refused, f"{proposal} is not accepted"):
            _ = self.compatibility("specified", scope, refs=[proposal])
        with self.assertRaisesRegex(Refused, "F-106 is scoped to server:90000001"):
            _ = self.compatibility("specified", scope, refs=["F-106"])
        self.assertEqual(0, self.compatibility("specified", scope, refs=["F-103"]).status)

    def test_implemented_needs_a_commit_and_its_tests(self) -> None:
        self.accepted_chain()
        native: JSON = {"basis": "explicit", "subjects": ["native"], "commit": "c3d4e5f"}
        with self.assertRaisesRegex(Refused, "implemented needs refs naming its tests"):
            _ = self.compatibility("implemented", native, refs=["F-103"])
        with self.assertRaisesRegex(Refused, "needs scope.commit naming the native code"):
            _ = self.compatibility("implemented", {"basis": "explicit", "subjects": ["original-server"]}, refs=["F-104"])
        measured = self.new_finding("Invented: the native listing round trip passes", kind="measured",
                                    scope={"basis": "explicit", "subjects": ["native"], "commit": "c3d4e5f"})
        self.assertEqual(0, self.compatibility("implemented", native, refs=[measured]).status)

    def test_validated_needs_a_passing_verified_run_matching_the_scope(self) -> None:
        self.accepted_chain()
        stock: JSON = {"basis": "explicit", "subjects": ["original-client", "native"], "commit": "c3d4e5f",
                       "builds": [f"server:{NEW_SERVER}", f"client:{NEW_CLIENT}"]}
        failed = self.run_evidence("failed", outcome="fail")
        loopback = self.run_evidence("loopback", environment="loopback on one host")
        narrow = self.run_evidence("narrow", targets=[f"server:{NEW_SERVER}"])
        with self.assertRaisesRegex(Refused, "validated needs cites naming a passing run"):
            _ = self.compatibility("validated", stock, cites=["M-102"])
        with self.assertRaisesRegex(Refused, f"{failed} outcome is fail"):
            _ = self.compatibility("validated", stock, cites=[failed])
        with self.assertRaisesRegex(Refused, f"{loopback} ran on loopback .*never validates stock-client interoperability"):
            _ = self.compatibility("validated", stock, cites=[loopback])
        with self.assertRaisesRegex(Refused, f"{narrow} does not name client:{NEW_CLIENT}"):
            _ = self.compatibility("validated", stock, cites=[narrow])
        good = self.run_evidence("good")
        _ = (self.root / "work/case/good.txt").write_text("tampered", encoding="utf-8")
        with self.assertRaisesRegex(Refused, f"{good} artifacts are mismatched"):
            _ = self.compatibility("validated", stock, cites=[good])
        _ = (self.root / "work/case/good.txt").write_text("invented run good", encoding="utf-8")
        server_only: JSON = {"basis": "explicit", "subjects": ["native"], "commit": "c3d4e5f", "builds": [f"server:{NEW_SERVER}"]}
        self.assertEqual(0, self.compatibility("validated", server_only, cites=[loopback]).status, "loopback validates a server-only claim")
        identifier = self.compatibility("observed", stock, cites=[failed]).data["ids"]
        assert isinstance(identifier, list) and isinstance(identifier[0], str)
        with self.assertRaisesRegex(Refused, f"{identifier[0]}: validated needs a passing run .*outcome is fail"):
            _ = online.revise(self.bank, self.root, identifier[0], changes(status="validated"), now=self.tick())
        noted = online.revise(self.bank, self.root, "CP-001", Changes({}, note="Invented: re-read"), now=self.tick())
        self.assertEqual(0, noted.status, "a revision that keeps a proven status is not re-qualified")


class EvidenceTests(Online):
    def test_a_bundle_above_the_cap_is_refused_unless_members_are_waived(self) -> None:
        bundle = {"records": [{"evidence_id": MEMBER}], "unknowns": []}
        self.put("work/case/big-bundle.json", json.dumps(bundle))
        size = (self.root / "work/case/big-bundle.json").stat().st_size
        _ = self.enterContext(patch.object(online, "BUNDLE_PARSE_CAP", size - 1))
        parsed: list[bytes] = []

        def recorded(data: bytes) -> tuple[int, list[str]]:
            parsed.append(data)
            return 0, []

        _ = self.enterContext(patch.object(online, "bundle_members", recorded))
        with self.assertRaisesRegex(Refused, f"REA bundle of {size} bytes exceeds the {size - 1}-byte parse cap; its members would not be listed"):
            _ = online.evidence_add(self.bank, self.root, "work/case/big-bundle.json", "rea-bundle", "Invented big bundle", now=self.tick())
        self.assertEqual((), read_tree(self.root))
        output = online.evidence_add(self.bank, self.root, "work/case/big-bundle.json", "rea-bundle", "Invented big bundle", without_members=True,
                                     now=self.tick())
        record = Record.from_json(load_json(self.doc("M-001").metadata["record"]))
        self.assertEqual(((), None), (record.items("members"), record.number("records")))
        self.assertEqual((f"members not listed: the bundle's {size} bytes exceed the {size - 1}-byte parse cap",), record.notes)
        self.assertEqual([], parsed, "a bundle above the cap is never read whole")
        self.assertEqual(0, output.status)
        with self.assertRaisesRegex(Refused, "matches 0 listed members"):
            _ = self.new_finding("Invented: cites a member of the unlisted bundle", cites=["M-001/ev_a1b2c3d4e5f6"])
        self.assertEqual("verified", online.verify_record(self.root, record).outcome)

    def test_add_bundle_and_verify_outcomes(self) -> None:
        bundle = {"records": [{"evidence_id": MEMBER, "score": 4.0947}, {"evidence_id": MEMBER}], "unknowns": [{"unknown_id": OTHER_MEMBER}]}
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

    def test_a_retired_record_whose_bytes_changed_is_reported_but_does_not_fail(self) -> None:
        self.put("work/case/receipt.txt", "invented receipt")
        _ = online.evidence_add(self.bank, self.root, "work/case/receipt.txt", "receipt", "Invented receipt", now=self.tick())
        _ = online.revise(self.bank, self.root, "M-001", Changes({"status": "retired"}, note="Invented: superseded"), now=self.tick())
        self.put("work/case/receipt.txt", "changed after retirement")
        result = online.verify(self.root, ["M-001"])
        self.assertEqual((0, "M-001: mismatched (sha256 differs: work/case/receipt.txt)"), (result.status, result.lines[0]))
        self.assertTrue(result.lines[-1].endswith("; 0 failing"))
        with self.assertRaisesRegex(RecordError, "duplicate key"):
            _ = online.bundle_members(b'{"records": [], "records": [], "unknowns": []}')


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
        self.assertIn(f"{mine} [finding; proposed, active; current against REL-9.0.2", text)
        self.assertIn("F-106 [finding; accepted, active; other-build against REL-9.0.2", text)
        self.assertIn("retired under it: F-101 [accepted, superseded]", text)
        self.assertIn("retired under it: F-102 [accepted, superseded]", text)
        self.assertNotRegex(text, r"(?m)^F-10[12] \[")
        self.assertNotIn("M-102 [evidence; accepted, active;", text)
        self.assertNotIn("U-101@", text)
        self.assertIn("== proposals of other worktrees ==\nF-150 [proposed, active]", text)
        partitions = output.data["partitions"]
        assert isinstance(partitions, dict)
        self.assertEqual({"current", "other-build"}, set(partitions) - {"native", "unscoped"})
        default = "\n".join(online.recall(self.bank, self.root, "listing lock").lines)
        self.assertNotIn("F-150", default)
        evidence_hits = "\n".join(online.recall(self.bank, self.root, "invented report receipt", include_evidence=True).lines)
        self.assertIn("M-102 [evidence; accepted, active", evidence_hits)
        texts = "\n".join(online.recall(self.bank, self.root, "nothing ranks this", text="handler table").lines)
        self.assertIn(f"{mine} [finding; proposed", texts)
        others = online.recall(self.bank, self.root, "nothing ranks this", text="listing lock", all_proposals=True)
        listed = others.data["other_proposals"]
        assert isinstance(listed, list)
        self.assertEqual(["F-150"], [item["id"] for item in listed if isinstance(item, dict)], "a substring search lists only other worktrees' proposals there")

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
        self.assertIn("cited records not current: 1 (1 superseded); memory check --pin lists each", summary)
        self.assertEqual([{"id": "F-101", "why": "superseded", "where": "src/lib.rs:1"}], state.data["cited_not_current"])
        self.assertIn("F-101: superseded (first cited at src/lib.rs:1)", "\n".join(online.check_pin(self.root).lines))
        self.assertIn("M-101: missing: work/invented-case/rea-bundle.json", summary)
        self.assertEqual(state.data, online.status(self.bank, self.root).data)


class CompactReadTests(Online):
    def reference(self) -> str:
        body = "# Invented reference\n\n" + "".join(f"## Section {number}\n\nInvented filler paragraph {number} about nothing in particular.\n\n" for number in range(60))
        body += "## Lock rules\n\nInvented: the listing lock key is checked here.\n"
        output = online.propose(self.bank, self.root, "reference", Changes({"title": "Invented long reference", "scope": {
            "basis": "explicit", "subjects": ["tooling"]}}, body=body), now=self.tick())
        self.assertEqual(0, output.status)
        return body

    def test_recall_prints_pointers_a_passage_and_capped_links_within_a_budget(self) -> None:
        self.accepted_chain()
        body = self.reference()
        compact = online.recall(self.bank, self.root, "listing lock key")
        text = "\n".join(compact.lines)
        self.assertIn("REF-001 [reference; proposed, current;", text)
        self.assertIn("REF-001 [reference; proposed, current; unscoped against REL-9.0.2]\n  Invented long reference\n  passage (under 'Lock rules'):", text)
        self.assertIn("passage", text)
        self.assertIn("listing lock key", text)
        self.assertNotIn("Invented filler paragraph 30", text, "a REF hit never prints its body")
        self.assertLessEqual(len(text.encode()), online.RECALL_BUDGET + 1024)
        partitions = compact.data["partitions"]
        assert isinstance(partitions, dict)
        self.assertFalse(any("text" in item for items in partitions.values() if isinstance(items, list) for item in items if isinstance(item, dict)))
        full = "\n".join(online.recall(self.bank, self.root, "listing lock key", full=True).lines)
        self.assertIn("Invented filler paragraph 30", full)
        self.assertIn(body.split("\n")[0], full)
        _ = self.enterContext(patch.object(online, "RECALL_BUDGET", 200))
        tight = online.recall(self.bank, self.root, "listing lock key")
        self.assertTrue(tight.data["not_shown"])
        self.assertIn("not shown (", "\n".join(tight.lines))
        ranked = online.recall(self.bank, self.root, "listing lock key", limit=1).data["documents"]
        assert isinstance(ranked, list)
        self.assertEqual(1, len(ranked))

    def test_records_come_before_at_most_three_references_and_refs_lists_only_references(self) -> None:
        self.accepted_chain()
        for number in range(4):
            body = f"# Invented reference {number}\n\n## Rules\n\nInvented: the listing lock note number {number}.\n"
            _ = online.propose(self.bank, self.root, "reference", Changes({"title": f"Invented reference {number}", "scope": {
                "basis": "explicit", "subjects": ["tooling"]}}, body=body), now=self.tick())
        compact = online.recall(self.bank, self.root, "listing lock")
        text = "\n".join(compact.lines)
        self.assertEqual(3, text.count("[reference;"))
        self.assertRegex(text, r"\+1 more reference documents: REF-00[1-4]; use --refs or get")
        unscoped = text.split("== unscoped ==", 1)[1]
        self.assertLess(unscoped.index("U-101 ["), unscoped.index("REF-"), "claims before references within a partition")
        only = "\n".join(online.recall(self.bank, self.root, "listing lock", refs=True).lines)
        self.assertEqual(4, only.count("[reference;"))
        self.assertNotIn("[finding;", only)
        first = online.recall(self.bank, self.root, "nothing ranks this", text="note number 3").data["documents"]
        assert isinstance(first, list)
        self.assertEqual("REF-004", first[0], "substring hits rank first")

    def test_evidence_is_not_partitioned(self) -> None:
        self.accepted_chain()
        got = online.get(self.bank, self.root, "M-102")
        self.assertFalse(any(line.startswith("partition:") for line in got.lines))
        self.assertIn("M-102 [evidence; accepted, active]", "\n".join(online.recall(self.bank, self.root, "invented report receipt", include_evidence=True).lines))

    def test_the_passage_is_trimmed_around_the_query_under_its_heading(self) -> None:
        unit = "x " * 1000 + "the listing lock key is here " + "y " * 1000
        excerpt, heading = online.passage("listing lock", [unit], "# Doc\n\n## Lock rules\n\n" + unit)
        self.assertIn("listing lock", excerpt)
        self.assertLessEqual(len(excerpt), online.PASSAGE_CAP + 2)
        self.assertEqual("Lock rules", heading)
        self.assertEqual("A, B (+2 more)", online.capped(["A", "B", "C", "D"], 2))

    def test_area_and_history_are_bounded_unless_full(self) -> None:
        self.accepted_chain()
        _ = self.enterContext(patch.object(online, "AREA_BUDGET", 400))
        compact = "\n".join(online.area(self.bank, self.root, "listing").lines)
        self.assertIn("more record(s) not shown; memory area listing --full", compact)
        self.assertIn("by status:", compact)
        full = "\n".join(online.area(self.bank, self.root, "listing", full=True).lines)
        self.assertIn("linked records outside the area (", full)
        self.assertGreater(len(full), len(compact))
        _ = online.revise(self.bank, self.root, "U-101", changes(status="investigating"), now=self.tick())
        history = online.get(self.bank, self.root, "U-101", history=True)
        self.assertEqual(2, sum(1 for line in history.lines if line.startswith("  ") and " in memory/journal/" in line))
        self.assertGreater(len("\n".join(online.get(self.bank, self.root, "U-101", history=True, full=True).lines)), len("\n".join(history.lines)))


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
        _ = self.enterContext(patch.object(service, "FLOOR", 0))
        result = online.rebuild(self.bank, self.root)
        self.assertEqual((0, 70), (result.status, result.data["retained"]))
        self.assertEqual(2, self.bank.async_calls)
        self.assertEqual(0, online.check_bank(self.bank, self.root).status)
        self.assertEqual(0, online.rebuild(self.bank, self.root).data["retained"])

    def test_a_long_reference_body_is_held_in_chunks(self) -> None:
        self.accepted_chain()
        body = "# Invented reference\n\n" + "".join(f"Invented paragraph {number} about the listing lock.\n\n" for number in range(120))
        output = online.propose(self.bank, self.root, "reference", Changes({"title": "Invented long reference", "scope": {
            "basis": "explicit", "subjects": ["tooling"]}}, body=body), now=self.tick())
        self.assertEqual(0, output.status)
        self.assertGreater(len(self.bank.units(document_id="REF-001")), 2)
        self.assertEqual(0, online.check_bank(self.bank, self.root).status)
        self.bank.extra_units.append(Unit("stray", "REF-001", "Invented text the body never held", "world", ()))
        self.assertIn("REF-001: ", "\n".join(online.check_bank(self.bank, self.root).lines))

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

    def test_the_client_reads_the_service_connection_file(self) -> None:
        """One connection file (the service's, 0600 enforced), found through ``ICARUS_MEMORY_HOME``."""
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"ICARUS_MEMORY_HOME": directory}):
            path = Path(directory) / "config" / "connection.env"
            with self.assertRaisesRegex(ServiceError, "install"):
                _ = connection()
            settings = service.ensure_settings(service.home())
            path.chmod(0o644)
            with self.assertRaisesRegex(ServiceError, "0600"):
                _ = connection()
            path.chmod(0o600)
            found = connection()
            self.assertEqual(("icarus", f"http://127.0.0.1:{service.API_PORT}"), (found.bank, found.url))
            self.assertNotIn(settings.key, repr(Client(found)) + repr(found))
            _ = path.write_text(path.read_text().replace("HINDSIGHT_API_HOST=127.0.0.1", "HINDSIGHT_API_HOST=0.0.0.0"))
            with self.assertRaisesRegex(ServiceError, "loopback") as caught:
                _ = connection()
            self.assertNotIn(settings.key, str(caught.exception))


class DurabilitySeamTests(Online):
    """The journal side of backup, restore and drill, against the fake bank."""

    def patched(self) -> None:
        bank = self.bank

        def client(_target: Connection) -> FakeBank:
            return bank

        _ = self.enterContext(patch.object(durability, "Client", client))
        _ = self.enterContext(patch.object(durability, "connection", lambda: Connection("http://127.0.0.1:1", "k" * 64, "icarus")))

    def test_watermark_names_origin_main_and_every_applied_segment(self) -> None:
        self.patched()
        self.accepted_chain()
        _ = write(self.root, [finding("F-120", "Invented: proposed after the merge")], second=20)
        mark = durability.watermark(self.root)
        self.assertEqual(run_git(self.root, "rev-parse", "origin/main").strip(), mark.origin_main)
        files = sorted(path for path in (self.root / "memory/journal").rglob("*.jsonl"))
        self.assertEqual(sorted(sha256_hex(path.read_bytes()) for path in files), list(mark.segments))
        self.assertIn("F-120", self.bank.held, "replay first")
        self.assertEqual(mark, Watermark.decode(mark.encode()))

    def test_restore_replays_forward_from_the_watermark_and_refuses_to_go_back(self) -> None:
        self.patched()
        self.accepted_chain()
        mark = durability.watermark(self.root)
        self.bank.held.clear()
        _ = write(self.root, [finding("F-121", "Invented: written after the backup")], second=21)
        commit_all(self.root, "later")
        durability.replay_forward(self.root)(Connection("http://127.0.0.1:1", "k" * 64, "icarus"), mark)
        self.assertIn("state:accepted", self.doc("F-121").tags)
        self.assertEqual(0, online.check_bank(self.bank, self.root).status)
        later = durability.watermark(self.root)
        _ = run_git(self.root, "update-ref", "refs/remotes/origin/main", "HEAD~1")
        with self.assertRaisesRegex(ServiceError, "does not contain the backup's"):
            durability.replay_forward(self.root)(Connection("http://127.0.0.1:1", "k" * 64, "icarus"), later)

    def test_the_drill_compares_the_restored_bank_with_the_journal(self) -> None:
        self.patched()
        self.accepted_chain()
        mark = durability.watermark(self.root)
        check = durability.journal_comparison(self.root)
        self.assertEqual([], check(Connection("http://127.0.0.1:1", "k" * 64, "icarus"), mark))
        self.bank.settings["retain_chunk_size"] = 3000
        self.assertEqual(["journal: config retain_chunk_size is 3000, expected 1800"], check(Connection("http://127.0.0.1:1", "k" * 64, "icarus"), mark))
        self.assertIn("no origin/main", durability.watermark_problems(self.top, Watermark("a" * 40, ()))[0])


class StampTests(Online):
    """Reads skip the bank diff only when the reconcile stamp shows nothing new; check --bank never trusts it."""

    listings: int = 0

    def counted(self) -> None:
        documents = self.bank.documents

        def counting(tags: Tags | None = None) -> list[Document]:
            self.listings += 1
            return documents(tags)

        _ = self.enterContext(patch.object(self.bank, "documents", counting))

    def test_an_unchanged_journal_skips_the_bank_diff(self) -> None:
        self.accepted_chain()
        _ = online.get(self.bank, self.root, "F-101")
        self.counted()
        for _ in range(3):
            self.assertEqual(0, online.get(self.bank, self.root, "F-102").status)
            _ = online.recall(self.bank, self.root, "listing lock")
        self.assertEqual(0, self.listings, "no document listing, so no diff")
        self.assertIn(bank_module.MARKER, self.bank.held)

    def test_a_new_local_segment_or_commit_reconciles(self) -> None:
        self.accepted_chain()
        _ = online.get(self.bank, self.root, "F-101")
        self.counted()
        _ = write(self.root, [finding("F-140", "Invented: written without the tool")], second=40)
        self.assertEqual(0, online.get(self.bank, self.root, "F-140").status)
        self.assertGreater(self.listings, 0, "a new local segment reconciles")
        self.assertIn("F-140", self.bank.held)
        self.listings = 0
        commit_all(self.root, "accepted")
        _ = online.get(self.bank, self.root, "F-140")
        self.assertGreater(self.listings, 0, "a new origin/main commit reconciles")
        self.assertIn("state:accepted", self.doc("F-140").tags)

    def test_a_bank_whose_marker_differs_is_reconciled(self) -> None:
        self.accepted_chain()
        _ = online.get(self.bank, self.root, "F-101")
        del self.bank.held["F-104"]
        self.bank.put(Item(bank_module.MARKER, "memory stamp restored", ("marker:stamp",), {"stamp": "0" * 64}, "unset", ()))
        self.counted()
        _ = online.status(self.bank, self.root)
        self.assertGreater(self.listings, 0)
        self.assertIn("F-104", self.bank.held, "a restored bank is replayed forward")

    def test_a_trusted_read_that_finds_drift_reconciles_and_check_bank_never_trusts(self) -> None:
        self.accepted_chain()
        _ = online.get(self.bank, self.root, "F-101")
        held = self.doc("F-104")
        self.bank.held["F-104"] = Document("F-104", "tampered", sha256_hex(b"tampered"), held.tags, held.metadata, (), None)
        checked = online.check_bank(self.bank, self.root)
        self.assertEqual(["F-104: not applied (missing or differing content)", "F-104: 1 memory unit(s); expected one holding the document text"],
                         checked.data_problems())
        self.assertNotIn(bank_module.MARKER, "\n".join(checked.lines))
        healed = online.get(self.bank, self.root, "F-104")
        self.assertEqual(0, healed.status, "the trusted read heals what it verifies")
        self.assertEqual(["F-104: text differs from the journal"], healed.data["repaired"])
        self.assertIn("repaired: F-104: text differs from the journal; rewritten from the journal", "\n".join(healed.lines))
        self.assertEqual(0, online.check_bank(self.bank, self.root).status)
        self.assertEqual([], online.get(self.bank, self.root, "F-104").data["repaired"], "nothing to repair the second time")
        self.bank.held["F-104"] = Document("F-104", "tampered", sha256_hex(b"tampered"), held.tags, held.metadata, (), None)
        hit = Hit("F-104", "tampered", held.tags, held.metadata, 1.0)

        def only_the_tampered_hit(*_args: object, **_kwargs: object) -> list[Hit]:
            return [hit]

        _ = self.enterContext(patch.object(self.bank, "recall", only_the_tampered_hit))
        recalled = online.recall(self.bank, self.root, "invented")
        self.assertEqual(["F-104"], recalled.data["repaired"])
        self.assertIn("repaired: F-104: differed from the journal", "\n".join(recalled.lines))


class ParseCacheTests(unittest.TestCase):
    def test_the_cache_round_trips_and_refuses_what_it_did_not_write(self) -> None:
        with tempfile.TemporaryDirectory() as directory, clean_git_environment():
            top = Path(directory)
            root = top / "repo"
            root.mkdir()
            new_repository(root)
            _ = write(root, chain())
            cache = top / "cache.pickle"
            _ = journal.forget_cache()
            first = read_tree(root)
            journal.save_cache(cache)
            self.assertEqual(0o600, cache.stat().st_mode & 0o777)
            _ = journal.forget_cache()
            journal.load_cache(cache)
            self.assertEqual(first, read_tree(root))
            self.assertEqual(len(first), journal.forget_cache(), "every segment came from the cache")
            cache.chmod(0o644)
            journal.load_cache(cache)
            self.assertEqual(0, journal.forget_cache(), "a cache others could write is ignored")
            cache.chmod(0o600)
            _ = cache.write_bytes(b"not a pickle")
            journal.load_cache(cache)
            self.assertEqual(0, journal.forget_cache())
