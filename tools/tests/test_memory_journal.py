# SPDX-License-Identifier: AGPL-3.0-or-later
"""Journal segments, replay and partitions of the memory tool, on invented records."""

from __future__ import annotations

import random
from hashlib import sha256
import re
import tempfile
import unittest
from pathlib import Path
from typing import override

from tools.memory.journal import (
    SEGMENT_LIMIT, JournalError, Revision, Segment, encode_segment, parse_segment, read_git, read_tree, split_change_set, write_change_set,
)
from tools.memory.model import JSON, Record
from tools.memory.state import Partition, State, partition, replay
from tools.tests.memory_fixtures import (
    CREATED, MEMBER, NEW_CLIENT, NEW_SERVER, OLD_SERVER, chain, clean_git_environment, commit_all, evidence, explicit, finding,
    new_repository, release, revisions, write,
)


def record(data: dict[str, JSON]) -> Record:
    return Record.from_json(data)


def segment(path: str, items: list[Revision]) -> Segment:
    return Segment(path, "2026-11-20T12:00:00Z", tuple(items))


def summary(state: State) -> tuple[object, ...]:
    return (
        {key: (entry.heads, sorted(entry.revisions)) for key, entry in state.entries.items()},
        dict(state.superseded_by), dict(state.resolved_by), sorted(state.problems),
    )


class JournalTests(unittest.TestCase):
    root: Path = Path()

    @override
    def setUp(self) -> None:
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory()))

    def test_write_and_read_round_trip(self) -> None:
        paths = write(self.root, chain())
        self.assertEqual(1, len(paths))
        relative = paths[0].relative_to(self.root).as_posix()
        self.assertRegex(relative, r"^memory/journal/2026/11/20261120T120000Z-[0-9a-f]{12}\.jsonl$")
        lines = paths[0].read_bytes().split(b"\n")
        self.assertEqual(b'{"created":"2026-11-20T12:00:00Z","format":"icarus-memory-journal","revisions":10,"version":1}', lines[0])
        (segment_read,) = read_tree(self.root)
        self.assertEqual(tuple(revisions(chain())), segment_read.revisions)
        self.assertEqual(paths, write(self.root, chain()))  # A retried write is a no-op.
        self.assertEqual((), read_tree(self.root / "missing"))

    def test_large_change_sets_split_under_the_limit(self) -> None:
        items = [finding(f"F-{number}", "Invented " + "y" * 1500) for number in range(300, 600)]
        parts = split_change_set(CREATED, revisions(items))
        self.assertGreater(len(parts), 1)
        self.assertTrue(all(len(data) <= SEGMENT_LIMIT for _path, data in parts))
        self.assertEqual(len(items), sum(len(parse_segment(path, data).revisions) for path, data in parts))

    def test_splitting_and_names_are_deterministic(self) -> None:
        items = revisions([finding(f"F-{number}", "Invented " + "y" * 1500) for number in range(300, 400)])
        self.assertEqual(split_change_set(CREATED, items), split_change_set(CREATED, items))
        other = Path(self.enterContext(tempfile.TemporaryDirectory()))
        first = [path.relative_to(self.root) for path in write_change_set(self.root, CREATED, items)]
        second = [path.relative_to(other) for path in write_change_set(other, CREATED, items)]
        self.assertEqual(first, second)
        self.assertTrue(all((self.root / path).read_bytes() == (other / path).read_bytes() for path in first))

    def test_half_written_and_edited_segments_are_refused(self) -> None:
        (path,) = write(self.root, chain())
        data = path.read_bytes()
        _ = path.write_bytes(data[: len(data) // 2])
        with self.assertRaisesRegex(JournalError, "half-written or edited"):
            _ = read_tree(self.root)
        relative = path.relative_to(self.root).as_posix()
        truncated = data[: data.rindex(b"\n", 0, len(data) - 1) + 1]
        with self.assertRaisesRegex(JournalError, "header counts 10 revisions; the file holds 9"):
            _ = parse_segment(rename(relative, truncated), truncated)
        unterminated = data[:-1]
        with self.assertRaisesRegex(JournalError, "ending in LF"):
            _ = parse_segment(rename(relative, unterminated), unterminated)

    def test_noncanonical_and_tampered_lines_are_refused(self) -> None:
        name, data = encode_segment(CREATED, revisions(chain()[:1]))
        header, line, _ = data.split(b"\n")
        reordered = header + b"\n" + line.replace(b'":"', b'": "', 1) + b"\n"
        with self.assertRaisesRegex(JournalError, "not canonical JSON"):
            _ = parse_segment(rename(name, reordered), reordered)
        tampered = data.replace(b"Invented: the prompt", b"Invented: a prompt")
        with self.assertRaisesRegex(JournalError, "does not match the record"):
            _ = parse_segment(rename(name, tampered), tampered)
        rev = Revision.of(Record.from_json(chain()[0])).rev
        self.assertIn(f'"rev":"{rev}"'.encode(), line)
        forged = data.replace(rev.encode(), b"0" * 64)
        with self.assertRaisesRegex(JournalError, "rev '0+' does not match prev and record"):
            _ = parse_segment(rename(name, forged), forged)
        with self.assertRaisesRegex(JournalError, "not a segment path"):
            _ = parse_segment("memory/journal/2026/12/" + name.rsplit("/", 1)[1], data)

    def test_temporary_files_are_skipped_and_strays_refused(self) -> None:
        (path,) = write(self.root, chain())
        _ = (path.parent / f".{path.name}.123.tmp").write_bytes(b"partial")
        self.assertEqual(1, len(read_tree(self.root)))
        _ = (path.parent / "notes.txt").write_text("hand-written")
        with self.assertRaisesRegex(JournalError, "notes.txt: not a segment path"):
            _ = read_tree(self.root)

    def test_git_tree_reader_matches_the_working_tree(self) -> None:
        with clean_git_environment():
            new_repository(self.root)
            _ = write(self.root, chain())
            commit_all(self.root)
            self.assertEqual(read_tree(self.root), read_git(self.root, "origin/main"))
            self.assertEqual(read_tree(self.root), read_git(self.root, "origin/main^{tree}"))


def rename(path: str, data: bytes) -> str:
    """The path a segment with ``data`` would have, so the name check passes and deeper checks run."""
    return re.sub(r"-[0-9a-f]{12}\.jsonl$", f"-{sha256(data).hexdigest()[:12]}.jsonl", path)


class ReplayTests(unittest.TestCase):
    def test_designated_chain(self) -> None:
        state = replay([segment("memory/journal/2026/11/a.jsonl", revisions(chain()))])
        self.assertEqual((), state.problems)
        self.assertEqual((), state.link_problems())
        self.assertEqual("superseded", state.status("F-101"))
        self.assertEqual(("F-102",), state.superseded_by["F-101"])
        self.assertEqual(("F-106",), state.successors("F-101"))
        self.assertEqual("resolved", state.status("U-101"))
        self.assertEqual(("F-103",), state.resolved_by["U-101"])
        self.assertEqual("active", state.status("F-103"))
        self.assertTrue(state.retired("F-102"))
        self.assertFalse(state.retired("M-102"))
        self.assertEqual(MEMBER, state.resolve("M-101/ev_a1b2c3d4e5f6").member)
        self.assertIn("matches 0 listed members", state.resolve("M-101/ev_ffffffffffff").problem or "")
        self.assertIn("migrated evidence", state.resolve("M-101/E-010").problem or "")
        self.assertEqual("no record F-999", state.resolve("F-999").problem)
        release_record = state.release(NEW_SERVER)
        self.assertEqual("REL-9.0.2", release_record.id if release_record else None)

    def test_migrated_lifecycle_unions_with_links(self) -> None:
        legacy: JSON = {"path": "docs/ledgers/findings/invented.md", "cells": {"status": "superseded-by F-202"},
                        "superseded_by": ["F-202"], "implied_status": "superseded"}
        old = finding("F-201", "Invented old", status="legacy:superseded-by F-202", gaps=["standing"], legacy=legacy)
        del old["standing"]
        resolved: dict[str, JSON] = {"id": "U-201", "type": "unknown", "status": "legacy:resolved", "question": "Invented?", "severity": "low",
                    "next_probe": "none", "area": "listing", "legacy": {"path": "docs/ledgers/unknowns.md", "cells": {"status": "resolved"},
                                                                       "implied_status": "resolved"}}
        state = replay([segment("s", revisions([old, finding("F-202", "Invented new"), resolved, *chain()[6:8]]))])
        self.assertEqual("superseded", state.status("F-201"))
        self.assertEqual(("F-202",), state.successors("F-201"))
        self.assertEqual("resolved", state.status("U-201"))
        self.assertEqual((), state.resolved_by.get("U-201", ()))

    def test_fork_and_reconciliation(self) -> None:
        first = Revision.of(record(chain()[5]))
        left = Revision.of(record(dict(chain()[5], status="investigating")), [first.rev])
        right = Revision.of(record(dict(chain()[5], next_probe="Read the other invented handler")), [first.rev])
        forked = replay([segment("a", [first]), segment("b", [left]), segment("c", [right])])
        entry = forked.entries["U-101"]
        self.assertTrue(entry.conflict)
        self.assertEqual(tuple(sorted((left.rev, right.rev))), entry.heads)
        merged = Revision.of(record(dict(chain()[5], status="investigating", next_probe="Read both")), [left.rev, right.rev])
        reconciled = replay([segment("a", [first]), segment("b", [left]), segment("c", [right]), segment("d", [merged])])
        self.assertFalse(reconciled.entries["U-101"].conflict)
        self.assertEqual((merged.rev,), reconciled.entries["U-101"].heads)
        self.assertEqual([first.rev], [item.revision.rev for item in reconciled.entries["U-101"].history()][:1])
        self.assertEqual(merged.rev, reconciled.entries["U-101"].history()[-1].revision.rev)

    def assert_reverts(self, contents: list[Record]) -> Revision:
        """Each content after the previous; the last returns to the first. Replays cleanly to one head."""
        chained = [Revision.of(contents[0])]
        for content in contents[1:]:
            chained.append(Revision.of(content, [chained[-1].rev]))
        self.assertEqual(chained[0].sha256, chained[-1].sha256)
        self.assertNotEqual(chained[0].rev, chained[-1].rev)
        state = replay([segment(f"s{index}", [item]) for index, item in enumerate(chained)])
        entry = state.entries[contents[0].id]
        self.assertEqual(((), (chained[-1].rev,), len(chained)), (state.problems, entry.heads, len(entry.revisions)))
        self.assertEqual([item.rev for item in chained], [item.revision.rev for item in entry.history()])
        self.assertEqual(contents[-1], entry.record)
        return chained[-1]

    def test_returning_to_earlier_content_is_a_new_revision(self) -> None:
        opened = record(chain()[5])
        self.assertEqual("open", self.assert_reverts([opened, record(dict(chain()[5], status="blocked")), opened]).record.status)
        active = record(chain()[0])
        self.assertEqual("active", self.assert_reverts([active, record(dict(chain()[0], status="retired")), active]).record.status)
        header: dict[str, JSON] = {"id": "REF-101", "type": "reference", "status": "current", "title": "Invented spec",
                                   "scope": explicit(subjects=("tooling",))}

        def reference(body: str) -> Record:
            return Record.from_json(dict(header, body_sha256=sha256(body.encode()).hexdigest()), body)

        self.assertEqual("first\n", self.assert_reverts([reference("first\n"), reference("second\n"), reference("first\n")]).record.body)

    def test_replay_ignores_segment_order(self) -> None:
        items = revisions(chain())
        first = Revision.of(record(chain()[5]))
        later = Revision.of(record(dict(chain()[5], status="blocked")), [first.rev])
        back = Revision.of(record(chain()[5]), [later.rev])
        segments = [segment(f"s{index}", [item]) for index, item in enumerate([*items, later, back])]
        expected = summary(replay(segments))
        rng = random.Random(11)
        for _ in range(10):
            rng.shuffle(segments)
            self.assertEqual(expected, summary(replay(segments)))

    def test_malformed_histories_are_problems(self) -> None:
        first = Revision.of(record(chain()[5]))
        orphan = Revision.of(record(dict(chain()[5], status="blocked")), ["0" * 64])
        state = replay([segment("a", [first]), segment("b", [first, orphan])])
        self.assertTrue(any("appears in a and b" in problem for problem in state.problems))
        self.assertTrue(any("unknown prev 000000000000" in problem for problem in state.problems))
        cycle = replay([segment("a", revisions([
            finding("F-301", "Invented one", supersedes=["F-302"]), finding("F-302", "Invented two", supersedes=["F-301"]), *chain()[6:8],
        ]))])
        self.assertEqual(("F-301 -> F-302 -> F-301",), cycle.supersession_cycles())


class PartitionTests(unittest.TestCase):
    release: Record = record(release("REL-9.0.2", NEW_SERVER, NEW_CLIENT))
    asked: list[str] = []

    @override
    def setUp(self) -> None:
        self.asked = []

    def place(self, data: dict[str, JSON]) -> Partition:
        def descends(commit: str) -> bool | None:
            self.asked.append(commit)
            return True

        return partition(record(data), self.release, descends)

    def test_partitions_against_the_pin(self) -> None:
        self.assertEqual("current", self.place(chain()[2]).name)
        self.assertEqual(Partition("other-build", (f"server:{OLD_SERVER}",)), self.place(chain()[1]))
        half = finding("F-301", "Invented pair", scope=explicit(f"server:{NEW_SERVER}", f"client:{OLD_SERVER}"))
        self.assertEqual("other-build", self.place(half).name)
        self.assertEqual("current", self.place(finding("F-302", "x", scope={"basis": "explicit", "releases": ["9.0.2"]})).name)
        native = self.place(chain()[3])
        self.assertEqual(("native", "c3d4e5f", True), (native.name, native.commit, native.descends))
        self.assertEqual(["c3d4e5f"], self.asked)
        decision = finding("F-303", "Owner chose invented policy", kind="decision", scope=explicit(subjects=("decision",)),
                           instruction="Do it.", instruction_date="2026-11-20", instruction_source="invented thread")
        self.assertEqual("current", self.place(decision).name)
        self.assertEqual("unscoped", self.place(finding("F-304", "x", scope=explicit(subjects=("tooling",)))).name)

    def test_verified_on_moves_an_old_record_to_current(self) -> None:
        verified = finding("F-305", "x", verified_on=[{"build": f"server:{NEW_SERVER}", "cites": ["M-102"]}])
        self.assertEqual(Partition("current", (f"server:{NEW_SERVER}",)), self.place(verified))
        pair = finding("F-306", "x", scope=explicit(f"server:{OLD_SERVER}", "client:90000000"),
                       verified_on=[{"build": f"server:{NEW_SERVER}", "cites": ["M-102"]}])
        self.assertEqual("other-build", self.place(pair).name)

    def test_legacy_scope_reports_tokens_and_the_pin(self) -> None:
        legacy: JSON = {"path": "docs/ledgers/findings/invented.md", "cells": {"scope": "high; server 90000002"}}

        def migrated(scope: JSON) -> dict[str, JSON]:
            data = finding("F-307", "x", scope=scope, legacy=legacy, gaps=["standing"])
            del data["standing"]
            return data

        named = self.place(migrated({"basis": "legacy-text", "builds": [f"any:{NEW_SERVER}"], "releases": ["9.0.1"]}))
        self.assertEqual(Partition("legacy-scope", (f"any:{NEW_SERVER}", "9.0.1"), True), named)
        other = self.place(migrated({"basis": "legacy-text", "builds": [f"server:{OLD_SERVER}"], "commit": "a93179bf"}))
        self.assertEqual(Partition("legacy-scope", (f"server:{OLD_SERVER}", "a93179bf"), False, "a93179bf"), other)
        self.assertEqual("unscoped", self.place(migrated({"basis": "legacy-text"})).name)
        self.assertEqual([], self.asked)

    def test_evidence_and_unscoped_records(self) -> None:
        self.assertEqual("unscoped", self.place(evidence("M-105", "work/x")).name)


if __name__ == "__main__":
    _ = unittest.main()
