# SPDX-License-Identifier: AGPL-3.0-or-later
"""The legacy exporter, parity gate and salvage, on an invented miniature legacy repository.

Every row below is invented; builds are fictitious ``2999xxxx`` numbers and releases ``3.0.9x``. The
fixture carries each hazard of the legacy data: free-text and role-less builds, glued builds, digits
inside selectors, tool versions, commits in claims, several commits in one row, a confidence slot
holding a build, ``unknown`` kind, bare ``retired``/``superseded`` and compound statuses, cross-file
supersession, prose supersessions, ranges over an unallocated number and ending in a selector, a
resolved unknown, compatibility/bug/lead tables whose citations stay text, a non-canonically spaced
row, legacy evidence without digest or date, directory and descriptive paths, a bundle whose record
count differs from its listed members, shared members, IDs in labels, and a client build stored under
the server app in a release profile.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import subprocess
import tempfile
import unittest
from unittest import mock
from datetime import UTC, datetime
from pathlib import Path
from typing import override

from tools.memory.journal import Revision, Segment, read_tree, write_change_set
from tools.memory.legacy import Export, export, write
from tools.memory.legacy_rules import extract, standing
from tools.memory.legacy_prose import account
from tools.memory.legacy_report import inbound, render, residue
from tools.memory.model import JSON, Legacy, Record, RecordError
from tools.memory import migrate
from tools.memory.parity import parity
from tools.memory.paths import ProtectedPathsError
from tools.memory import salvage as salvage_command
from tools.memory.salvage import salvage
from tools.memory.state import partition, replay
from tools.tests.memory_fixtures import clean_git_environment, new_repository, run_git

MEMBER_A = "ev_29990001aaaa" + "1" * 52
MEMBER_B = "ev_bbbbbbbbbbbb" + "2" * 52
MEMBER_U = "unk_cccccccccccc" + "3" * 52

ACTIVE = """# Findings: Widgets

Active rows. F-104 superseded-by F-105.

Last reconciled: 2026-01-01

F-106 is retired in favor of F-107.

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-102 | Widget server 29990001 frames gadgets; client build 29990002 agrees | observed | static; M-101/ev_29990001aaaa, M-101/ev_bbbbbbbbbbbb; c0ffee1 | high; build 29990003; build29990004 too | U-101 | active |
| F-105 | Gadget 3.0.91 run ends; tool 0.20.0 used; commit-like deadbee9 in claim | measured | live; M-102..M-104/ev_bbbbbbbbbbbb; a1b2c3d | 29990001; one run; 1,299,900,010 bytes | U-101, U-102 | active |
| F-107 | Decision text without tokens | derived | review | bounded; candidate; owner said so | | active |
| F-108 | Native loop at abc1234 then def5678 | observed | authored-test; abc1234, def5678 | medium; loopback |  | active |
|  F-109 | Spaced row | observed | review | low; none | | active |
"""
RETIRED = """# Retired findings: Widgets

Rows retired from the active ledger.

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-101 | Old widget claim citing F-102 | derived | static; M-101 | medium; old | U-101 | superseded-by F-102 |
| F-103 | Receipt row | unknown | tool-receipt; M-102 | high; receipt | | retired: receipt |
| F-104 | Bare superseded row | observed | review; M-102 | high; x | | superseded |
| F-106 | Bare retired row | observed | review; M-102 | high; x | | retired |
| F-110 | Compound row | observed | review | high; x | | retired: historical intake; deferral superseded-by F-107 |
"""
UNKNOWNS = """# Unknowns

Status: open, investigating, blocked, contradicted, resolved.

| ID | Question | Severity | Status | Next probe | Findings | REA id |
| --- | --- | --- | --- | --- | --- | --- |
| U-101 | Which gadget? | medium | resolved | Answered by F-102 | F-101 to F-105, F-107 | M-101/ev_bbbbbbbbbbbb |
| U-102 | Open gadget? | high | open | Probe F-105 | F-105 | M-101/unk_cccccccccccc |
"""
BUGS = """# Bug ledger

Leads. SL-001 belongs to M-103.

## Reported campaigns

| Priority | Reported problem | Status | Next evidence |
| --- | --- | --- | --- |
| 1 | Crashes | Unreproduced | Dumps (F-102) |
| 2 | Slow | Unreproduced | Profiles |

## Static leads on the original

| Lead | Static evidence | What would establish or refute a problem |
| --- | --- | --- |
| SL-001: Reader differs | M-103/E-010; F-102 (M-101); c0ffee2 | Test `0x14000beef` at abcdef9 |

## Native foundation defects

| ID | Trigger and former behavior | Change | Findings |
| --- | --- | --- | --- |
| N-101 | Saves dropped | Keep them | F-108 |
"""
COMPATIBILITY = """# Compatibility matrix

Server observations apply to build 29990001 (M-101).

## Surfaces

| Surface | Status | Required proof |
| --- | --- | --- |
| Discovery | Unknown | Browser behavior |

## Bounded observations

| Subsurface | Status | Evidence | Remaining gate |
| --- | --- | --- | --- |
| Widgets | Observed; mirrored | F-102, F-105; [spec](../specs/net/widget.md) | Session (U-101) |
"""
SPEC = """# Widget framing

## Status

Candidate; observed with client build 29990002 only.

## Scope

Server 29990001 and 3.0.91 at c0ffee3.

## Confirmed

- Widget `0x140001234` and `GadgetFrame` (F-102).
"""
DIGEST = """# Widgets

## Current answer

Frames take 42.5 ms and `NeverRecordedName` (F-102).
"""
RELEASE = """# Pin
[release]
app_id = 1234
build_id = 29990010
project_version = "3.0.92.100-rel-Test"
network_checksum = 1
"""
FIXTURE_CLIENT = """# Frozen profile: 3.0.91 pinned by its client build 29990002.

[release]
app_id = 1234
build_id = 29990002
project_version = "3.0.91.50-rel-Test"
network_checksum = 1
"""
FIXTURE_SERVER = """# Frozen profile: 3.0.91 (server 29990001, client 29990002).

[release]
app_id = 1234
build_id = 29990001
project_version = "3.0.91.50-rel-Test"
network_checksum = 1
"""


def shard_rows() -> list[dict[str, object]]:
    return [
        {"id": "M-101", "path": "work/case/rea-bundle.json", "sha256": "a" * 64, "class": "local-private", "kind": "rea-bundle",
         "date": "2026-01-01", "label": "Bundle for server 29990001 and F-102", "status": "active", "records": 7, "ev": [MEMBER_A, MEMBER_B], "unk": [MEMBER_U]},
        {"id": "M-102", "path": "work/case/", "sha256": None, "class": "local-private", "date": None, "label": "Legacy directory",
         "kind": "input", "status": "active", "legacy": True},
        {"id": "M-103", "path": "unstated", "sha256": None, "class": "external", "kind": "report", "date": "2026-01-02",
         "label": "Descriptive", "status": "retired", "superseded_by": "M-104", "note": "see M-104 and F-105"},
        {"id": "M-104", "path": "docs/specs/net/widget.md", "sha256": "b" * 64, "class": "tracked", "kind": "rea-bundle", "date": "2026-01-03",
         "label": "Second bundle", "status": "active", "records": 1, "ev": [MEMBER_B]},
    ]


def build_repository(root: Path) -> None:
    new_repository(root)
    files = {
        "README.md": "The official dedicated server (Steam app 1234); nothing else.\n",
        "docs/ledgers/findings/widgets.md": ACTIVE,
        "docs/ledgers/findings/retired/widgets.md": RETIRED,
        "docs/ledgers/unknowns.md": UNKNOWNS,
        "docs/ledgers/bugs.md": BUGS,
        "docs/ledgers/compatibility.md": COMPATIBILITY,
        "docs/specs/net/widget.md": SPEC,
        "docs/specs/net/another.md": "# Another\n\n## Scope\n\nNothing.\n",
        "docs/research/net/widgets.md": DIGEST,
        "docs/implementation-status.md": "# Status\n\nSee [the spec](specs/net/widget.md).\n",
        "src/lib.rs": "//! See `docs/specs/net/widget.md` (F-102, F-101).\n",
        "evidence/README.md": "# Evidence index\n\nOne row per artifact.\n",
        "config/release.toml": RELEASE,
        "tests/fixtures/releases/3.0.91-client.toml": FIXTURE_CLIENT,
        "tests/fixtures/releases/3.0.91.toml": FIXTURE_SERVER,
        "evidence/widgets/001.jsonl": "".join(json.dumps({k: v for k, v in row.items() if v is not None or k in ("sha256", "date")},
                                                         separators=(",", ":")) + "\n" for row in shard_rows()),
    }
    for path, text in files.items():
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        _ = target.write_text(text, encoding="utf-8")
    _ = run_git(root, "add", "-A")
    _ = run_git(root, "commit", "-q", "-m", "legacy")


class LegacyRepository(unittest.TestCase):
    top: Path = Path()
    root: Path = Path()
    result: Export = Export("", datetime.now(UTC))
    records: dict[str, Record] = {}

    @override
    def setUp(self) -> None:
        self.top = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.enterContext(clean_git_environment())
        self.root = self.top / "repo"
        self.root.mkdir()
        build_repository(self.root)
        self.result = export(self.root, "HEAD")
        self.records = self.result.records()

    def record(self, identifier: str) -> Record:
        return self.records[identifier]


class ExtractionTests(unittest.TestCase):
    def test_roles_need_the_word_next_to_the_number(self) -> None:
        scope = extract({"claim": "server 29990001, client build 29990002, build 29990003, build29990004, X 29990005"}).scope
        self.assertEqual(scope, {"basis": "legacy-text", "builds": ["any:29990003", "any:29990004", "any:29990005", "client:29990002", "server:29990001"]})

    def test_no_build_inside_numbers_selectors_or_words(self) -> None:
        text = "M-1/ev_29990001aaaa, 1,299,900,010 B, 129990001, 29990001.5, x29990001, 0.20.0, 3.0.91.0.1"
        self.assertIsNone(extract({"claim": text}).scope)

    def test_a_role_tag_replaces_the_role_less_token(self) -> None:
        scope = extract({"claim": "29990001 here; server 29990001 there"}).scope
        self.assertEqual(scope, {"basis": "legacy-text", "builds": ["server:29990001"]})

    def test_commits_only_from_the_named_cells_and_only_when_single(self) -> None:
        self.assertIsNone(extract({"claim": "pawn c0ffee1"}, ()).scope)
        self.assertEqual(extract({"evidence": "x; c0ffee1"}, ("evidence",)).scope, {"basis": "legacy-text", "commit": "c0ffee1"})
        several = extract({"evidence": "abc1234, def5678"}, ("evidence",))
        self.assertIsNone(several.scope)
        self.assertEqual(several.commits, ("abc1234", "def5678"))
        self.assertIsNone(extract({"evidence": "deadbeef cafebabe 1234567"}, ("evidence",)).scope)

    def test_standing_only_in_so_many_words(self) -> None:
        self.assertEqual(standing(["high; candidate; x"]), "candidate")
        self.assertIsNone(standing(["one candidate", "unconfirmed"]))
        self.assertIsNone(standing(["confirmed", "refuted"]))


class LegacyResolverTests(unittest.TestCase):
    def test_only_an_unknown_lead_or_report_names_resolvers(self) -> None:
        data: dict[str, JSON] = {"id": "U-101", "type": "unknown", "status": "legacy:resolved", "question": "Which?",
                                 "legacy": {"path": "x.md", "cells": {"status": "resolved"}, "resolved_by": ["F-101"], "implied_status": "resolved"},
                                 "gaps": ["area", "next_probe", "severity"]}
        self.assertEqual(Record.from_json(data).legacy, Legacy("x.md", {"status": "resolved"}, resolved_by=("F-101",), implied_status="resolved"))
        data.update({"id": "F-102", "type": "finding", "claim": "x", "gaps": ["area", "confidence", "kind", "scope", "standing"]})
        _ = data.pop("question")
        with self.assertRaisesRegex(RecordError, "only an unknown, lead or report is resolved"):
            _ = Record.from_json(data)


class GapTests(unittest.TestCase):
    @staticmethod
    def finding(*gaps: str, scope: JSON = None) -> dict[str, JSON]:
        data: dict[str, JSON] = {"id": "F-101", "type": "finding", "status": "active", "claim": "x", "kind": "observed", "confidence": "high",
                                 "area": "a", "legacy": {"path": "x.md", "cells": {"claim": "x"}}, "gaps": list[JSON](gaps)}
        if scope is not None:
            data["scope"] = scope
        return data

    def test_a_gap_may_name_a_scope_part_with_detail(self) -> None:
        record = Record.from_json(self.finding("scope.commit: several commits in row (abc1234, def5678)", "standing",
                                               scope={"basis": "legacy-text", "releases": ["3.0.91"]}))
        self.assertEqual(record.gaps, ("scope.commit: several commits in row (abc1234, def5678)", "standing"))
        _ = Record.from_json(self.finding("scope", "scope.commit: several commits in row (a, b)", "standing"))

    def test_a_gap_names_an_absent_field(self) -> None:
        with self.assertRaisesRegex(RecordError, "listed as missing but present"):
            _ = Record.from_json(self.finding("scope.commit: x", "standing", scope={"basis": "legacy-text", "commit": "abc1234"}))
        with self.assertRaisesRegex(RecordError, "is not a field"):
            _ = Record.from_json(self.finding("scope.other: x", "scope", "standing"))
        with self.assertRaisesRegex(RecordError, "standing: required field missing"):
            _ = Record.from_json(self.finding("scope"))


class ExportTests(LegacyRepository):
    def test_counts_and_ids(self) -> None:
        kinds = sorted(record.id for record in self.records.values())
        self.assertEqual(kinds, sorted([
            *(f"F-{n}" for n in range(101, 111)), "U-101", "U-102", "N-101", "M-101", "M-102", "M-103", "M-104", "SL-001",
            "RP-001", "RP-002", "CP-001", "CP-002", *(f"REF-{n:03d}" for n in range(1, 11)), "REL-3.0.91", "REL-3.0.92",
        ]))

    def test_cells_are_verbatim_and_gaps_list_what_is_missing(self) -> None:
        record = self.record("F-102")
        assert record.legacy is not None
        self.assertEqual(record.legacy.path, "docs/ledgers/findings/widgets.md")
        self.assertEqual(record.legacy.cells["scope_and_limits"], "high; build 29990003; build29990004 too")
        self.assertEqual(record.gaps, ("standing",))
        spaced = self.record("F-109")
        assert spaced.legacy is not None
        self.assertEqual(spaced.legacy.cells["original"], "|  F-109 | Spaced row | observed | review | low; none | | active |")

    def test_vocabulary_and_legacy_values(self) -> None:
        self.assertEqual(self.record("F-103").text("kind"), "legacy:unknown")
        self.assertEqual(self.record("F-105").text("confidence"), "legacy:29990001")
        self.assertEqual(self.record("F-107").text("confidence"), "legacy:bounded")
        self.assertEqual(self.record("F-107").text("standing"), "candidate")
        self.assertEqual(self.record("CP-001").status, "unknown")
        self.assertEqual(self.record("CP-002").status, "legacy:Observed; mirrored")
        self.assertEqual(self.record("U-101").status, "legacy:resolved")

    def test_scope_tokens(self) -> None:
        f102 = self.record("F-102").scope
        assert f102 is not None
        self.assertEqual((f102.basis, f102.builds, f102.commit, f102.subjects),
                         ("legacy-text", ("any:29990003", "any:29990004", "client:29990002", "server:29990001"), "c0ffee1", ()))
        f105 = self.record("F-105").scope
        assert f105 is not None
        self.assertEqual((f105.builds, f105.releases, f105.commit), (("any:29990001",), ("3.0.91",), "a1b2c3d"))
        self.assertIsNone(self.record("F-108").scope)
        self.assertEqual(self.result.commits["F-108"], ("abc1234", "def5678"))
        self.assertEqual(self.record("F-108").gaps, ("scope", "scope.commit: several commits in row (abc1234, def5678)", "standing"))
        self.assertIn("scope", self.record("F-107").gaps)
        # M-101's label names server 29990001; F-101 cites M-101 but states no build.
        self.assertIn("scope", self.record("F-101").gaps)

    def test_supersession_from_cells_and_prose(self) -> None:
        state = replay(self.segments())
        self.assertEqual(state.superseded_by["F-101"], ("F-102",))
        self.assertEqual(state.superseded_by["F-104"], ("F-105",))
        self.assertEqual(state.superseded_by["F-106"], ("F-107",))
        self.assertEqual(state.superseded_by["F-110"], ("F-107",))
        self.assertEqual([(link.old, link.new) for link in self.result.prose_links], [("F-104", "F-105"), ("F-106", "F-107")])
        f104 = self.record("F-104")
        assert f104.legacy is not None
        self.assertIn("prose docs/ledgers/findings/widgets.md:3", f104.legacy.cells)
        self.assertEqual({identifier: state.status(identifier) for identifier in ("F-103", "F-104", "F-106", "F-110")},
                         {"F-103": "retired", "F-104": "superseded", "F-106": "superseded", "F-110": "superseded"})

    def test_links_ranges_and_resolution(self) -> None:
        state = replay(self.segments())
        u101 = self.record("U-101")
        assert u101.legacy is not None
        # F-101 to F-105 expands over defined IDs only; F-103 is defined, an unallocated number would not be.
        self.assertEqual(u101.legacy.resolved_by, ("F-101", "F-102", "F-103", "F-104", "F-105", "F-107"))
        self.assertEqual(u101.links.cites, ("M-101/ev_bbbbbbbbbbbb",))
        self.assertEqual(state.status("U-101"), "resolved")
        self.assertEqual(self.record("U-102").links.refs, ("F-105",))
        self.assertEqual(state.status("U-102"), "open")
        f105 = self.record("F-105")
        self.assertEqual(f105.links.cites, ("M-102", "M-103", "M-104/ev_bbbbbbbbbbbb"))
        self.assertEqual(self.record("F-102").links.cites, ("M-101/ev_29990001aaaa", "M-101/ev_bbbbbbbbbbbb"))
        self.assertEqual(self.record("F-101").links.refs, ("F-102", "U-101"))
        self.assertEqual(state.link_problems(), ())

    def test_tables_keep_citations_as_text(self) -> None:
        cp = self.record("CP-002")
        self.assertTrue(cp.assigned_at_migration)
        self.assertEqual((cp.text("name"), cp.links.targets()), ("Widgets", ()))
        lead = self.record("SL-001")
        self.assertEqual((lead.text("lead"), lead.links.targets(), lead.status), ("Reader differs", (), "open"))
        scope = lead.scope
        assert scope is not None
        self.assertEqual(scope.commit, "c0ffee2")
        report = self.record("RP-001")
        self.assertEqual((report.number("priority"), report.status, report.links.targets()), (1, "unreproduced", ()))
        self.assertEqual((self.record("N-101").status, self.record("N-101").gaps), ("fixed", ("tests",)))

    def test_references_are_verbatim_specs(self) -> None:
        widget = self.record("REF-002")
        self.assertEqual((widget.text("title"), widget.body, widget.text("area")), ("Widget framing", SPEC, "net"))
        assert widget.scope is not None and widget.legacy is not None
        # Status and Scope both state applicability: their builds and releases are unioned; a spec's hex word is never a commit.
        self.assertEqual((widget.scope.builds, widget.scope.releases, widget.scope.commit), (("client:29990002", "server:29990001"), ("3.0.91",), None))
        self.assertEqual(self.record("REF-001").text("title"), "Another")
        assert widget.legacy is not None
        self.assertEqual(widget.legacy.cells["revision"], run_git(self.root, "rev-parse", "HEAD").strip())

    def test_documents_the_cut_over_deletes_are_references(self) -> None:
        paths = {record.legacy.path: record for record in self.records.values() if record.type == "reference" and record.legacy}
        self.assertEqual([paths[path].id for path in (
            "docs/specs/net/another.md", "docs/specs/net/widget.md", "docs/research/net/widgets.md", "docs/ledgers/bugs.md",
            "docs/ledgers/compatibility.md", "docs/ledgers/findings/retired/widgets.md", "docs/ledgers/findings/widgets.md",
            "docs/ledgers/unknowns.md", "docs/implementation-status.md", "evidence/README.md")], [f"REF-{n:03d}" for n in range(1, 11)])
        self.assertEqual(paths["docs/research/net/widgets.md"].body, DIGEST)
        unknowns = paths["docs/ledgers/unknowns.md"]
        self.assertEqual(unknowns.body, "# Unknowns\n\nStatus: open, investigating, blocked, contradicted, resolved.\n\n"
                         + "<!-- table: its rows are migrated as records -->\n")
        self.assertEqual((unknowns.text("title"), unknowns.text("area")), ("Unknowns", "unknowns"))
        compatibility = paths["docs/ledgers/compatibility.md"]
        scope = compatibility.scope
        assert scope is not None and compatibility.body is not None
        # "Server observations apply to build N": the role is not next to the number.
        self.assertEqual((scope.builds, scope.commit), (("any:29990001",), None))
        self.assertEqual(compatibility.body.count("<!-- table:"), 2)

    def test_releases_from_explicit_statements(self) -> None:
        pinned = self.record("REL-3.0.92")
        self.assertEqual((pinned.status, pinned.text("server_build"), pinned.gaps), ("current", "29990010", ("client_build", "dates")))
        old = self.record("REL-3.0.91")
        self.assertEqual((old.status, old.text("client_build"), old.text("server_build")), ("past", "29990002", None))
        self.assertTrue(any("server_build not set" in note for note in old.notes))
        self.assertTrue(any("both as server and as client" in note for note in old.notes))

    def test_evidence_rows(self) -> None:
        bundle = self.record("M-101")
        self.assertEqual(bundle.items("members"), (MEMBER_A, MEMBER_B, MEMBER_U))
        self.assertEqual(bundle.number("records"), 7)
        directory = self.record("M-102")
        self.assertEqual((directory.text("path"), directory.gaps), ("work/case/", ("date", "sha256", "size")))
        retired = self.record("M-103")
        assert retired.legacy is not None
        self.assertEqual((retired.status, retired.legacy.superseded_by, retired.notes, retired.links.targets()),
                         ("retired", ("M-104",), ("see M-104 and F-105",), ()))
        line = (self.root / "evidence/widgets/001.jsonl").read_text().split("\n")[2]
        self.assertEqual(retired.legacy.cells, {"row": line})

    def test_segments_are_deterministic_and_bounded(self) -> None:
        out = self.top
        first, second = write(self.result, out / "a"), write(export(self.root, "HEAD"), out / "b")
        self.assertEqual([path.read_bytes() for path in first], [path.read_bytes() for path in second])
        self.assertEqual(write(self.result, out / "a"), first)
        stamp = datetime.fromtimestamp(int(run_git(self.root, "show", "-s", "--format=%ct", "HEAD")), UTC).strftime("%Y%m%dT%H%M%SZ")
        self.assertTrue(all(path.name.startswith(f"{stamp}-") for path in first))

    def test_nothing_is_current_and_partitions_show_tokens(self) -> None:
        state = replay(self.segments())
        release = state.record("REL-3.0.92")
        assert release is not None
        names = {identifier: partition(entry.record, release, lambda _commit: None).name for identifier, entry in state.entries.items()}
        self.assertNotIn("current", names.values())
        self.assertEqual(names["F-102"], "legacy-scope")
        self.assertEqual(names["F-107"], "unscoped")

    def test_report_residue_and_inbound(self) -> None:
        self.assertEqual(residue(self.result, self.root), [])
        without = Export(self.result.revision, self.result.created, [item for item in self.result.migrated if item.record.id != "REF-003"],
                         catalog=self.result.catalog)
        values = {item.value for item in residue(without, self.root)}
        self.assertIn("NeverRecordedName", values)
        self.assertIn("42.5", values)
        found = {(item.path, item.target) for item in inbound(self.result, self.root)}
        self.assertIn(("src/lib.rs", "docs/specs/net/widget.md"), found)
        self.assertIn(("docs/implementation-status.md", "docs/specs/net/widget.md"), found)
        self.assertIn(("evidence/widgets/001.jsonl", "docs/specs/net/widget.md"), found)
        text = render(self.result, self.root)
        self.assertIn("- F-106 superseded by F-107: docs/ledgers/findings/widgets.md:7: F-106 is retired in favor of F-107.", text)
        self.assertIn("finding `standing` set from the row's own words on 1 of 10: F-107", text)
        self.assertIn("no active finding is `current` for REL-3.0.92", text)
        self.assertIn("- REL-3.0.92 (current): states version_string 3.0.92.100-rel-Test, server_build 29990010; lacks client_build, dates", text)
        self.assertIn("finding `scope.commit`: 1: F-108 (several commits in row (abc1234, def5678))", text)

    def segments(self) -> tuple[Segment, ...]:
        out = self.top / "segments"
        _ = write(self.result, out)
        return read_tree(out)


class ParityTests(LegacyRepository):
    def test_the_export_passes_and_a_changed_record_fails(self) -> None:
        out = self.top / "journal"
        _ = write(self.result, out)
        result = parity(self.root, "HEAD", out)
        self.assertEqual(result.mismatches, [])
        self.assertIn("finding 10", result.measured["counts"])
        self.assertIn("reference 10", result.measured["counts"])
        accounting = account(self.root, "HEAD", self.records)
        self.assertEqual(accounting.problems, [])
        self.assertEqual(sum(accounting.bytes.values()), sum(
            (self.root / path).stat().st_size for path in run_git(self.root, "ls-files", "docs/ledgers", "docs/specs", "docs/research",
                                                                  "docs/implementation-status.md", "evidence").split()))
        self.assertGreater(accounting.bytes["structural"], 0)
        lost = {identifier: record for identifier, record in self.records.items() if identifier not in ("REF-008", "M-103")}
        problems = "\n".join(account(self.root, "HEAD", lost).problems)
        self.assertIn("docs/ledgers/unknowns.md:1:", problems)
        self.assertIn("evidence/widgets/001.jsonl:3:", problems)
        tampered = self.top / "tampered"
        revisions = [item for change_set in self.result.change_sets() for item in change_set.revisions]
        changed: list[Revision] = []
        for revision in revisions:
            data = revision.record.to_json()
            if revision.record.id == "F-102":
                data["claim"] = "Something the row never said"
            elif revision.record.id == "U-102":
                data["refs"] = ["F-102"]
            elif revision.record.id == "M-102":
                data["path"] = "work/elsewhere/"
            changed.append(Revision.of(Record.from_json(data, revision.record.body)))
        _ = write_change_set(tampered, datetime(2026, 1, 5, tzinfo=UTC), changed)
        mismatches = "\n".join(parity(self.root, "HEAD", tampered).mismatches)
        self.assertIn("F-102: claim", mismatches)
        self.assertIn("U-102: links", mismatches)
        self.assertIn("M-102: path", mismatches)

    def test_an_unreadable_manifest_is_a_reported_failure_not_a_traceback(self) -> None:
        options = argparse.Namespace(rev="HEAD", journal=self.top / "journal")
        stderr = io.StringIO()
        with (mock.patch("tools.memory.parity.parity", side_effect=ProtectedPathsError("M-101: unreadable sha256 manifest work/x/fixity.sha256")),
              mock.patch("tools.memory.migrate.repository_root", return_value=self.root), contextlib.redirect_stderr(stderr)):
            self.assertEqual(migrate.run_parity(options), 1)
        self.assertIn("memory parity: M-101: unreadable sha256 manifest", stderr.getvalue())

    def test_a_failing_clone_is_a_reported_failure_not_a_traceback(self) -> None:
        out = self.top / "journal"
        _ = write(self.result, out)
        options = argparse.Namespace(rev="HEAD", journal=out)
        stderr = io.StringIO()
        with (mock.patch("tools.memory.parity.primary_checkout", return_value=self.top / "no-such-repository"),
              mock.patch("tools.memory.migrate.repository_root", return_value=self.root), contextlib.redirect_stderr(stderr)):
            self.assertEqual(migrate.run_parity(options), 1)
        self.assertRegex(stderr.getvalue(), r"^memory parity: git clone: exit status [0-9]+: .*no-such-repository")
        self.assertNotIn("Traceback", stderr.getvalue())


class SalvageTests(LegacyRepository):
    def publish(self) -> None:
        """Point ``origin/main``, the acceptance ref salvage classifies against, at ``main``."""
        _ = run_git(self.root, "update-ref", "refs/remotes/origin/main", "main")

    def branch_and_main(self) -> None:
        """``feature`` edits F-107 and adds F-111, F-112 and M-105; ``main`` meanwhile takes F-112 for its own row."""
        root = self.root
        _ = run_git(root, "checkout", "-q", "-b", "feature")
        ledger = root / "docs/ledgers/findings/widgets.md"
        _ = ledger.write_text(ledger.read_text().replace("| F-107 | Decision text", "| F-107 | Edited decision text")
                              .replace("# Findings: Widgets", "# Findings: Widgets and gadgets")
                              + "| F-111 | New branch claim | observed | review; M-105 | high; server 29990001 | | active |\n"
                              + "| F-112 | Branch takes a number main also takes | observed | review | high; x | | active |\n")
        shard = root / "evidence/widgets/001.jsonl"
        _ = shard.write_text(shard.read_text() + json.dumps({"id": "M-105", "path": "work/new/report.md", "sha256": "c" * 64, "class": "local-private",
                                                             "kind": "report", "date": "2026-01-06", "label": "New", "status": "active"},
                                                            separators=(",", ":")) + "\n")
        _ = run_git(root, "commit", "-q", "-am", "branch rows")
        _ = run_git(root, "checkout", "-q", "main")
        _ = ledger.write_text(ledger.read_text() + "| F-112 | Main's own F-112 | observed | review | high; y | | active |\n")
        _ = run_git(root, "commit", "-q", "-am", "main row")
        self.publish()

    def journal_on_main(self, *, remove_legacy: bool) -> dict[str, Record]:
        """Commit ``main``'s export as its journal, as the cut-over does, optionally deleting the legacy trees."""
        _ = run_git(self.root, "checkout", "-q", "main")
        migrated = export(self.root, "main")
        _ = write(migrated, self.root)
        if remove_legacy:
            _ = run_git(self.root, "rm", "-r", "-q", "docs/ledgers", "evidence")
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "cut-over")
        self.publish()
        return migrated.records()

    def run_salvage(self, ref: str, *, out: Path | None = None, main: str = "origin/main") -> tuple[int, str, str]:
        stdout, stderr = io.StringIO(), io.StringIO()
        with (mock.patch("tools.memory.salvage.repository_root", return_value=self.root), contextlib.redirect_stdout(stdout),
              contextlib.redirect_stderr(stderr)):
            status = salvage_command.run(argparse.Namespace(ref=ref, check=False, out=out, main=main))
        return status, stdout.getvalue(), stderr.getvalue()

    def test_branch_rows_become_proposals_and_conflicts_are_kept_apart(self) -> None:
        self.branch_and_main()
        result = salvage(self.root, "feature")
        self.assertEqual("legacy", result.source)
        self.assertEqual({revision.record.id: bool(revision.prev) for revision in result.proposals}, {"F-111": False, "M-105": False})
        # A changed row changes its verbatim legacy cells, which never change: it needs a superseding record and is never proposed.
        self.assertEqual(result.unrevisable, {"F-107": ("claim", "legacy")})
        self.assertEqual("supersede", result.kinds["F-107"])
        self.assertIn("docs/ledgers/findings/widgets.md: changed on feature; revise the body of REF-007 by hand", result.notices)
        self.assertFalse(any("unknowns.md" in notice or "evidence" in notice for notice in result.notices))
        self.assertEqual([new.id for _old, new in result.conflicts], ["F-112"])
        self.assertEqual(sorted(result.unsalvaged), ["F-107", "F-111", "F-112", "M-105"])
        proposed = {revision.record.id: revision.record for revision in result.proposals}
        f111 = proposed["F-111"].scope
        assert f111 is not None
        self.assertEqual(f111.builds, ("server:29990001",))

    def test_an_unrevisable_edit_is_never_written_and_exits_apart_from_conflicts(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "claim-edit")
        ledger = self.root / "docs/ledgers/findings/widgets.md"
        _ = ledger.write_text(ledger.read_text().replace("| F-107 | Decision text", "| F-107 | Edited decision text")
                              + "| F-111 | New branch claim | observed | review | high; x | | active |\n")
        _ = run_git(self.root, "commit", "-q", "-am", "edit a claim")
        self.publish()
        out = self.top / "salvaged"
        status, stdout, stderr = self.run_salvage("claim-edit", out=out)
        self.assertEqual((salvage_command.EXIT_SUPERSEDE, ""), (status, stdout))
        self.assertIn("needs a superseding record: F-107: changes claim, legacy, which a revision may not change; not proposed", stderr)
        self.assertIn("1 proposal(s), 1 needing a superseding record, 0 conflict(s)", stderr)
        written = {revision.record.id for segment in read_tree(out) for revision in segment.revisions}
        self.assertEqual({"F-111"}, written)

    def test_after_the_cut_over_main_s_journal_gives_the_same_answer(self) -> None:
        self.branch_and_main()
        before = salvage(self.root, "feature")
        _ = self.journal_on_main(remove_legacy=True)
        after = salvage(self.root, "feature")
        self.assertEqual(("legacy", "journal"), (before.source, after.source))
        self.assertEqual([revision.line() for revision in before.proposals], [revision.line() for revision in after.proposals])
        self.assertEqual(before.conflicts, after.conflicts)
        self.assertEqual((before.kinds, before.unrevisable, before.notices, before.unsalvaged),
                         (after.kinds, after.unrevisable, after.notices, after.unsalvaged))
        status, stdout, stderr = self.run_salvage("feature")
        self.assertEqual(1, status, "a conflict fails the command")
        self.assertIn("memory salvage: feature: 2 proposal(s), 1 needing a superseding record, 1 conflict(s), 0 row(s) already accepted "
                      + "(against origin/main ", stderr)
        self.assertIn("conflict: F-112: origin/main holds other content under this ID", stderr)
        self.assertIn("# memory/journal/", stdout)

    def test_the_accepted_ref_is_origin_main_whatever_local_main_says(self) -> None:
        self.branch_and_main()
        expected = salvage(self.root, "feature")
        records = self.journal_on_main(remove_legacy=True)
        f111 = records.get("F-111")
        self.assertIsNone(f111)
        accepted = salvage(self.root, "feature")
        # A stale local main (behind origin/main) plays no part.
        _ = run_git(self.root, "checkout", "-q", "feature")
        _ = run_git(self.root, "branch", "-f", "main", "main~2")
        stale = salvage(self.root, "feature")
        self.assertEqual(("journal", accepted.main, accepted.base), (stale.source, stale.main, stale.base))
        self.assertEqual([revision.line() for revision in accepted.proposals], [revision.line() for revision in stale.proposals])
        self.assertEqual(expected.kinds, stale.kinds)
        # Nor does a missing one.
        _ = run_git(self.root, "branch", "-D", "main")
        missing = salvage(self.root, "feature")
        self.assertEqual((accepted.kinds, accepted.main), (missing.kinds, missing.main))
        # --main overrides the acceptance ref: against the pre-cut-over commit the legacy export stands in.
        status, _stdout, stderr = self.run_salvage("feature", main="origin/main~1")
        self.assertEqual(1, status)
        self.assertIn("legacy export)", stderr)

    def test_a_record_main_s_journal_revised_is_a_conflict_and_otherwise_prev_is_the_journal_head(self) -> None:
        self.branch_and_main()
        records = self.journal_on_main(remove_legacy=False)
        f107 = records["F-107"]
        noted = f107.to_json() | {"notes": ["Invented: main revised this"]}
        first = Revision.of(f107)
        second = Revision.of(Record.from_json(noted, f107.body), [first.rev])
        _ = write_change_set(self.root, datetime(2026, 1, 7, tzinfo=UTC), [second])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "revise F-107 on main")
        self.publish()
        # The journal wins over the legacy trees main still has: they would have made this a plain revision.
        revised = salvage(self.root, "feature")
        self.assertEqual("journal", revised.source)
        self.assertEqual(["F-107", "F-112"], [new.id for _old, new in revised.conflicts])
        self.assertEqual("conflict", revised.kinds["F-107"])
        self.assertNotIn("F-107", {revision.record.id for revision in revised.proposals})
        # Main returns F-107 to its merge-base content: a new revision, whose rev the branch's row would have to name.
        third = Revision.of(f107, [second.rev])
        _ = write_change_set(self.root, datetime(2026, 1, 8, tzinfo=UTC), [third])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "revert F-107 on main")
        self.publish()
        restored = salvage(self.root, "feature")
        self.assertEqual("supersede", restored.kinds["F-107"], "no longer a conflict; its claim edit still needs a superseding record")
        self.assertNotIn("F-107", {revision.record.id for revision in restored.proposals})
        self.assertNotEqual(third.rev, first.rev)

    def merged_cut_over(self, *, keep_legacy: bool) -> str:
        """``feature`` merges the cut-over, resolving the modify/delete conflicts by deleting or by keeping its files; its pre-merge tip."""
        self.branch_and_main()
        _ = self.journal_on_main(remove_legacy=True)
        _ = run_git(self.root, "checkout", "-q", "feature")
        tip = run_git(self.root, "rev-parse", "HEAD").strip()
        with self.assertRaises(subprocess.CalledProcessError):
            _ = run_git(self.root, "merge", "-q", "--no-edit", "main")
        if keep_legacy:
            _ = run_git(self.root, "add", "-A", "docs/ledgers", "evidence")
        else:
            _ = run_git(self.root, "rm", "-r", "-q", "--ignore-unmatch", "docs/ledgers", "evidence")
        _ = run_git(self.root, "commit", "-q", "--no-edit")
        return tip

    def test_after_merging_the_cut_over_salvage_names_the_pre_merge_tip(self) -> None:
        for keep in (False, True):
            with self.subTest(keep_legacy=keep):
                self.setUp()
                tip = self.merged_cut_over(keep_legacy=keep)
                result = salvage(self.root, "feature")
                self.assertEqual((tip, [], [], {}), (result.premerge, result.proposals, result.conflicts, result.kinds))
                status, stdout, stderr = self.run_salvage("feature")
                self.assertEqual((salvage_command.EXIT_PREMERGE, ""), (status, stdout))
                self.assertIn(f"run memory salvage {tip}", stderr)
                self.assertNotIn("nothing to salvage", stderr)
                again = salvage(self.root, tip)
                self.assertEqual(({"F-107": "supersede", "F-111": "new", "F-112": "conflict", "M-105": "new"}, None), (again.kinds, again.premerge))

    def test_a_merge_of_the_cut_over_without_branch_rows_has_nothing_to_salvage(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "quiet")
        _ = (self.root / "src/lib.rs").write_text("//! Branch work without legacy rows.\n", encoding="utf-8")
        _ = run_git(self.root, "commit", "-q", "-am", "work")
        _ = self.journal_on_main(remove_legacy=True)
        _ = run_git(self.root, "checkout", "-q", "quiet")
        _ = run_git(self.root, "merge", "-q", "--no-edit", "main")
        status, stdout, stderr = self.run_salvage("quiet")
        self.assertEqual((0, ""), (status, stdout))
        self.assertIn("changes no legacy rows; nothing to salvage", stderr)

    def test_a_branch_without_legacy_trees_has_nothing_to_salvage(self) -> None:
        _ = self.journal_on_main(remove_legacy=True)
        _ = run_git(self.root, "checkout", "-q", "-b", "after-cut-over")
        _ = (self.root / "src/lib.rs").write_text("//! Changed after the cut-over.\n", encoding="utf-8")
        _ = run_git(self.root, "commit", "-q", "-am", "work")
        result = salvage(self.root, "after-cut-over")
        self.assertEqual(([], [], []), (result.proposals, result.conflicts, result.unsalvaged))
        status, stdout, stderr = self.run_salvage("after-cut-over")
        self.assertEqual((0, ""), (status, stdout))
        self.assertIn("notice: after-cut-over has no legacy ledgers or evidence index at its tip; nothing to salvage", stderr)
        self.assertNotIn("Traceback", stderr)


if __name__ == "__main__":
    _ = unittest.main()
