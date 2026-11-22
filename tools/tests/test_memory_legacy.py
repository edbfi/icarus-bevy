# SPDX-License-Identifier: AGPL-3.0-or-later
"""The legacy exporter, parity gate and salvage, on an invented miniature legacy repository.

Every row below is invented; builds are fictitious ``2999xxxx`` numbers and releases ``3.0.9x``. The
fixture carries each hazard of the legacy data: free-text and role-less builds, glued builds, digits
inside selectors, tool versions, commits in claims, source revisions in evidence cells, several
commits in one row, an all-digit commit candidate beside a commit, a confidence slot holding a build,
``unknown`` kind, bare ``retired``/``superseded`` and compound statuses, cross-file supersession, prose
supersessions, ranges over an unallocated number and ending in a selector, a resolved unknown,
compatibility/bug/lead tables whose cells link IDs and whose framing states no status, a tracked
evidence row whose file becomes a REF, a non-canonically spaced row, legacy evidence without digest or date, directory and descriptive paths, a bundle whose record
count differs from its listed members, shared members, IDs in labels, and a client build stored under
the server app in a release profile.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import shlex
import subprocess
import tempfile
import unittest
from collections.abc import Iterator, Mapping, Sequence
from datetime import UTC, datetime
from pathlib import Path
from typing import override
from unittest import mock

from tools.memory import cli, migrate, online, salvage as salvage_command
from tools.memory.canonical import render_text
from tools.memory.check import revision_problems
from tools.memory.journal import Revision, Segment, read_git, read_tree, write_change_set
from tools.memory.legacy import Export, export, write
from tools.memory.legacy_prose import account
from tools.memory.legacy_report import inbound, render, residue
from tools.memory.legacy_rules import extract, standing
from tools.memory.model import JSON, Record, RecordError
from tools.memory.parity import parity
from tools.memory.paths import ProtectedPathsError
from tools.memory.salvage import Salvage, salvage
from tools.memory.state import State, legacy_listed, partition, replay
from tools.tests.memory_fixtures import clean_git_environment, finding, new_repository, run_git

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
|  F-109 | Spaced row | observed | review | low; build 1234567, c0ffee4 | | active |
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

    def test_a_commit_only_from_a_scope_cell_and_only_when_alone(self) -> None:
        self.assertIsNone(extract({"claim": "pawn c0ffee1"}, ()).scope)
        self.assertEqual(extract({"scope_and_limits": "x; c0ffee1"}, ("scope_and_limits",)).scope, {"basis": "legacy-text", "commit": "c0ffee1"})
        several = extract({"scope_and_limits": "abc1234, def5678"}, ("scope_and_limits",))
        self.assertIsNone(several.scope)
        self.assertEqual(several.unplaced, ("abc1234", "def5678"))
        self.assertEqual(several.notes, ("scope commit not set: the limits cell names abc1234, def5678 (several commits)",))

    def test_an_all_digit_candidate_is_named_and_keeps_any_commit_out_of_scope(self) -> None:
        # 1234567 may be a commit or a number; 29990001 is a build, not a candidate.
        beside = extract({"scope_and_limits": "high; builds 1234567, 591d40f; server 29990001"}, ("scope_and_limits",))
        self.assertEqual(beside.scope, {"basis": "legacy-text", "builds": ["server:29990001"]})
        self.assertEqual(beside.unplaced, ("1234567", "591d40f"))
        self.assertEqual(beside.notes, ("scope commit not set: the limits cell names 1234567, 591d40f; all-digit 1234567 is a commit or a number,"
                                        + " not decided at migration",))
        alone = extract({"scope_and_limits": "branch build 6286752"}, ("scope_and_limits",))
        self.assertIsNone(alone.scope)
        self.assertEqual(alone.unplaced, ("6286752",))

    def test_an_evidence_cell_revision_is_a_note_never_scope(self) -> None:
        found = extract({"evidence": "static; M-101; c0ffee1"}, (), ("evidence",))
        self.assertIsNone(found.scope)
        self.assertEqual((found.revisions, found.notes), (("c0ffee1",), ("source revision (evidence cell): c0ffee1",)))
        mixed = extract({"evidence": "authored-test; M-101; ebd38a67, 12760573; 1178518/b1e1697; M-1545; 2026-01-01"}, (), ("evidence",))
        self.assertEqual(mixed.notes, ("source revision candidates (evidence cell): ebd38a67, 12760573, 1178518, b1e1697; all-digit 12760573,"
                                       + " 1178518 are a commit or a number, not decided at migration",))

    def test_standing_only_in_so_many_words(self) -> None:
        self.assertEqual(standing(["high; candidate; x"]), "candidate")
        self.assertIsNone(standing(["one candidate", "unconfirmed"]))
        self.assertIsNone(standing(["confirmed", "refuted"]))


class ReleasedModelTests(unittest.TestCase):
    def test_a_legacy_resolver_list_and_a_scope_part_gap_are_refused_as_main_refuses_them(self) -> None:
        data: dict[str, JSON] = {"id": "U-101", "type": "unknown", "status": "legacy:resolved", "question": "Which?",
                                 "legacy": {"path": "x.md", "cells": {"status": "resolved"}, "resolved_by": ["F-101"], "implied_status": "resolved"},
                                 "gaps": ["area", "next_probe", "severity"]}
        with self.assertRaisesRegex(RecordError, "legacy: unknown fields resolved_by"):
            _ = Record.from_json(data)
        finding: dict[str, JSON] = {"id": "F-101", "type": "finding", "status": "active", "claim": "x", "kind": "observed", "confidence": "high",
                                    "area": "a", "legacy": {"path": "x.md", "cells": {"claim": "x"}}, "gaps": ["scope", "scope.commit: x", "standing"]}
        with self.assertRaisesRegex(RecordError, "is not a field"):
            _ = Record.from_json(finding)

    def test_a_gap_names_an_absent_required_field(self) -> None:
        finding: dict[str, JSON] = {"id": "F-101", "type": "finding", "status": "active", "claim": "x", "kind": "observed", "confidence": "high",
                                    "area": "a", "legacy": {"path": "x.md", "cells": {"claim": "x"}}, "gaps": ["area", "scope", "standing"]}
        with self.assertRaisesRegex(RecordError, "listed as missing but present"):
            _ = Record.from_json(finding)
        finding["gaps"] = ["scope"]
        with self.assertRaisesRegex(RecordError, "standing: required field missing"):
            _ = Record.from_json(finding)

    def test_limits_belong_to_new_findings(self) -> None:
        finding: dict[str, JSON] = {"id": "F-101", "type": "finding", "status": "active", "claim": "x", "kind": "observed", "confidence": "high",
                                    "area": "a", "legacy": {"path": "x.md", "cells": {"claim": "x"}}, "gaps": ["scope", "standing"], "limits": "one run"}
        with self.assertRaisesRegex(RecordError, "limits: a migrated finding keeps its limits in its legacy cell"):
            _ = Record.from_json(finding)


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
        self.assertEqual(spaced.legacy.cells["original"], "|  F-109 | Spaced row | observed | review | low; build 1234567, c0ffee4 | | active |")

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
        # The Evidence cell's hash is the revision of the sources read: a note, never scope.
        self.assertEqual((f102.basis, f102.builds, f102.commit, f102.subjects),
                         ("legacy-text", ("any:29990003", "any:29990004", "client:29990002", "server:29990001"), None, ()))
        self.assertEqual(self.record("F-102").notes, ("source revision (evidence cell): c0ffee1",))
        f105 = self.record("F-105").scope
        assert f105 is not None
        self.assertEqual((f105.builds, f105.releases, f105.commit), (("any:29990001",), ("3.0.91",), None))
        self.assertIsNone(self.record("F-108").scope)
        self.assertEqual(self.record("F-108").notes, ("source revision (evidence cell): abc1234, def5678",))
        self.assertEqual(self.record("F-108").gaps, ("scope", "standing"))
        f109 = self.record("F-109")
        self.assertEqual((f109.scope, f109.gaps, self.result.unplaced), (None, ("scope", "standing"), {"F-109": ("1234567", "c0ffee4")}))
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
        # The Findings column lists related findings, never a resolver: they are refs, and U-101 is resolved by its
        # legacy status alone. F-101 to F-105 expands over defined IDs only.
        self.assertEqual(u101.links.refs, ("F-101", "F-102", "F-103", "F-104", "F-105", "F-107"))
        self.assertEqual(legacy_listed(u101, "findings"), ("F-101", "F-102", "F-103", "F-104", "F-105", "F-107"))
        self.assertEqual(legacy_listed(u101, "next_probe"), ("F-102",))
        self.assertEqual(u101.links.cites, ("M-101/ev_bbbbbbbbbbbb",))
        self.assertEqual((state.status("U-101"), state.resolved_by.get("U-101")), ("resolved", None))
        self.assertEqual(self.record("U-102").links.refs, ("F-105",))
        self.assertEqual(state.status("U-102"), "open")
        f105 = self.record("F-105")
        self.assertEqual(f105.links.cites, ("M-102", "M-103", "M-104/ev_bbbbbbbbbbbb"))
        self.assertEqual(self.record("F-102").links.cites, ("M-101/ev_29990001aaaa", "M-101/ev_bbbbbbbbbbbb"))
        self.assertEqual(self.record("F-101").links.refs, ("F-102", "U-101"))
        self.assertEqual(state.link_problems(), ())

    def test_tables_link_their_ids_and_state_no_status_they_lack(self) -> None:
        cp = self.record("CP-002")
        self.assertTrue(cp.assigned_at_migration)
        self.assertEqual((cp.text("name"), cp.links.refs, cp.links.cites), ("Widgets", ("F-102", "F-105", "U-101"), ()))
        lead = self.record("SL-001")
        self.assertEqual((lead.text("lead"), lead.links.cites, lead.links.refs, lead.status),
                         ("Reader differs", ("M-101", "M-103/E-010"), ("F-102",), "legacy:unstated"))
        self.assertIsNone(lead.scope)
        self.assertEqual(lead.notes[-1], "source revision (static evidence cell): c0ffee2")
        assert lead.legacy is not None
        self.assertEqual(lead.legacy.cells["table"], "## Static leads on the original")
        report = self.record("RP-001")
        self.assertEqual((report.number("priority"), report.status, report.links.refs), (1, "unreproduced", ("F-102",)))
        bug = self.record("N-101")
        assert bug.legacy is not None
        self.assertEqual((bug.status, bug.gaps, bug.legacy.cells["table"]), ("legacy:unstated", ("tests",), "## Native foundation defects"))
        self.assertTrue(bug.notes[0].startswith("status unstated"))

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
        # M-104 is tracked and its file becomes REF-002 at cut-over: linked, its locator kept.
        tracked = self.record("M-104")
        self.assertEqual((tracked.text("path"), tracked.links.refs, tracked.notes),
                         ("docs/specs/net/widget.md", ("REF-002",), ("tracked file migrated to REF-002 at cut-over (link assigned at migration)",)))

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
        self.assertIn("- F-109: scope commit not set: the limits cell names 1234567, c0ffee4; all-digit 1234567 is a commit or a number", text)
        self.assertIn("- M-104 (active): docs/specs/net/widget.md -> REF-002", text)
        self.assertIn("- lead status `legacy:unstated` on 1", text)
        self.assertIn("- compatibility refs: 3", text)

    def segments(self) -> tuple[Segment, ...]:
        out = self.top / "segments"
        _ = write(self.result, out)
        return read_tree(out)


class ParityTests(LegacyRepository):
    def release_model(self, source: str) -> None:
        """Point ``origin/main`` at a commit whose ``tools/memory/model.py`` is ``source``."""
        released = self.top / f"released-{len(list(self.top.glob('released-*')))}"
        released.mkdir()
        new_repository(released)
        target = released / "tools/memory/model.py"
        target.parent.mkdir(parents=True)
        _ = target.write_text(source, encoding="utf-8")
        _ = run_git(released, "add", "-A")
        _ = run_git(released, "commit", "-q", "-m", "released")
        _ = run_git(self.root, "fetch", "-q", "-f", str(released), "main:refs/remotes/origin/main")

    def test_an_anchored_tracked_row_links_the_ref_of_its_file(self) -> None:
        shard = self.root / "evidence/widgets/001.jsonl"
        row = {"id": "M-105", "path": "docs/specs/net/widget.md#scope", "sha256": None, "class": "tracked", "kind": "report",
               "date": None, "label": "Anchored", "status": "active"}
        _ = shard.write_text(shard.read_text() + json.dumps(row, separators=(",", ":")) + "\n")
        _ = run_git(self.root, "commit", "-q", "-am", "anchored tracked row")
        exported = export(self.root, "HEAD")
        anchored = exported.records()["M-105"]
        self.assertEqual((("REF-002",), ("tracked file migrated to REF-002 at cut-over (link assigned at migration)",)),
                         (anchored.links.refs, anchored.notes))
        self.release_model((Path(__file__).parents[1] / "memory/model.py").read_text(encoding="utf-8"))
        out = self.top / "anchored"
        _ = write(exported, out)
        self.assertEqual([], parity(self.root, "HEAD", out).mismatches)

    def test_the_export_passes_and_a_changed_record_fails(self) -> None:
        model = (Path(__file__).parents[1] / "memory/model.py").read_text(encoding="utf-8")
        self.release_model(model)
        out = self.top / "journal"
        _ = write(self.result, out)
        result = parity(self.root, "HEAD", out)
        self.assertEqual(result.mismatches, [])
        self.assertRegex(result.measured["released model"], r"^origin/main [0-9a-f]{12}: 34 revision\(s\) parsed, 0 rejected$")
        self.assertEqual(result.measured["edges compatibility, lead and report rows"], "7 (raw table lines 7)")
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
            elif revision.record.id == "CP-002":
                data["refs"] = ["F-102"]
            changed.append(Revision.of(Record.from_json(data, revision.record.body)))
        _ = write_change_set(tampered, datetime(2026, 1, 5, tzinfo=UTC), changed)
        mismatches = "\n".join(parity(self.root, "HEAD", tampered).mismatches)
        self.assertIn("F-102: claim", mismatches)
        self.assertIn("U-102: links", mismatches)
        self.assertIn("M-102: path", mismatches)
        self.assertIn("CP-002: links ['F-105', 'U-101'] differ from its cells", mismatches)
        self.assertIn("edges of compatibility, lead and report rows: journal 5, raw table lines 7", mismatches)

    def test_an_unseen_legacy_cell_and_a_record_main_rejects_fail(self) -> None:
        out = self.top / "journal"
        _ = write(self.result, out)
        model = (Path(__file__).parents[1] / "memory/model.py").read_text(encoding="utf-8")
        self.release_model(model.replace('{"path", "cells", "superseded_by", "implied_status"}', '{"path", "cells", "implied_status"}'))

        def without_limits(record: Record) -> str:
            return "\n".join(line for line in render_text(record).split("\n") if not line.startswith("limits: "))

        with mock.patch("tools.memory.parity.render_text", without_limits):
            mismatches = "\n".join(parity(self.root, "HEAD", out).mismatches)
        self.assertIn("F-102: legacy cell 'scope_and_limits' not in its text: 'high; build 29990003; build29990004 too'", mismatches)
        self.assertRegex(mismatches, r"released model: memory/journal/[0-9/]+/[^:]+:[0-9]+: F-101: legacy: unknown fields superseded_by")

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


def surface_record(identifier: str, name: str) -> Record:
    """A compatibility row as proposed on main (not migrated)."""
    return Record.from_json({"id": identifier, "type": "compatibility", "status": "unknown", "name": name, "qualifier": "Invented",
                             "required_proof": "Invented proof", "scope": {"basis": "explicit", "subjects": ["original-server"]}})


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

    def run_salvage(self, ref: str, *, out: Path | None = None, main: str = "origin/main", check: bool = False) -> tuple[int, str, str]:
        stdout, stderr = io.StringIO(), io.StringIO()
        with (mock.patch("tools.memory.salvage.repository_root", return_value=self.root), contextlib.redirect_stdout(stdout),
              contextlib.redirect_stderr(stderr)):
            status = salvage_command.run(argparse.Namespace(ref=ref, check=check, out=out, main=main))
        return status, stdout.getvalue(), stderr.getvalue()

    def test_branch_rows_become_proposals_and_conflicts_are_kept_apart(self) -> None:
        self.branch_and_main()
        result = salvage(self.root, "feature")
        self.assertEqual("legacy", result.source)
        self.assertEqual({revision.record.id: bool(revision.prev) for revision in result.proposals}, {"F-111": False, "M-105": False})
        # A changed claim is not revisable: the row needs a superseding record and is never proposed.
        self.assertEqual(result.unrevisable, {"F-107": ("claim",)})
        self.assertEqual("supersede", result.kinds["F-107"])
        self.assertIn("docs/ledgers/findings/widgets.md: pending document; in force: REF-007 (scope: none; other text; this text newly names: nothing); this branch changed it since its base: yes; the exporter reads as its scope, from its whole text:"
                      + " nothing; at this branch's base its reference was REF-007: now in force; main changed or replaced its reference since this branch's base: no;"
                      + " a new reference for it carries --note 'salvaged document: docs/ledgers/findings/widgets.md'", result.notices)
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
        self.assertIn("needs a superseding record: F-107: a revision may not change claim; not proposed", stderr)
        self.assertIn("1 proposal(s), 1 needing a superseding record, 0 needing a decision by hand, 0 conflict(s)", stderr)
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
        self.assertIn("memory salvage: feature: 2 proposal(s), 1 needing a superseding record, 0 needing a decision by hand, 1 conflict(s), 0 row(s) already accepted "
                      + "(against origin/main ", stderr)
        self.assertIn("conflict: F-112: origin/main holds other content under this ID", stderr)
        self.assertIn("# memory/journal/", stdout)

    def status_branch(self, name: str, status: str, *, extra: str = "") -> None:
        """A branch that changes only F-107's Status cell (and may add rows), after the cut-over journalled main."""
        _ = self.journal_on_main(remove_legacy=False)
        _ = run_git(self.root, "checkout", "-q", "-b", name, "main")
        ledger = self.root / "docs/ledgers/findings/widgets.md"
        row = "| F-107 | Decision text without tokens | derived | review | bounded; candidate; owner said so | |"
        text = ledger.read_text()
        self.assertIn(f"{row} active |", text)
        _ = ledger.write_text(text.replace(f"{row} active |", f"{row} {status} |") + extra)
        _ = run_git(self.root, "commit", "-q", "-am", "status edit")

    def legal(self, result: Salvage) -> State:
        """The accepted journal plus the proposals, replayed: no illegal revision, no dangling link."""
        out = self.top / "legal"
        _ = write_change_set(out, datetime(2026, 2, 1, tzinfo=UTC), result.proposals)
        state = replay([*read_git(self.root, "origin/main"), *read_tree(out)])
        self.assertEqual(([], (), ()), (revision_problems(state), state.link_problems(), state.problems))
        return state

    def test_a_status_cell_superseded_by_a_new_row_is_a_supersedes_link_on_it(self) -> None:
        self.status_branch("supersede", "superseded-by F-111", extra="| F-111 | Replacement claim | observed | review | high; x | | active |\n")
        result = salvage(self.root, "supersede")
        self.assertEqual(({}, {}), (result.unrevisable, result.undecided))
        self.assertEqual({"F-107": "superseded", "F-111": "new"}, result.kinds)
        self.assertEqual(["F-111"], [revision.record.id for revision in result.proposals], "no revision of F-107")
        self.assertEqual(("F-107",), result.proposals[0].record.links.supersedes)
        state = self.legal(result)
        self.assertEqual("superseded", state.status("F-107"))
        status, _stdout, _stderr = self.run_salvage("supersede")
        self.assertEqual(0, status)

    def test_a_status_cell_superseded_by_an_accepted_record_appends_to_it(self) -> None:
        self.status_branch("by-accepted", "superseded-by F-105")
        result = salvage(self.root, "by-accepted")
        self.assertEqual({"F-107": "superseded", "F-105": "revision"}, result.kinds)
        (revision,) = result.proposals
        self.assertEqual(("F-105", ("F-107",)), (revision.record.id, revision.record.links.supersedes))
        _ = self.legal(result)

    def test_a_retired_status_cell_is_a_status_revision_quoting_the_cell(self) -> None:
        self.status_branch("retire", "retired: replaced by the invented design")
        result = salvage(self.root, "retire")
        (revision,) = result.proposals
        self.assertEqual(("F-107", "retired"), (revision.record.id, revision.record.status))
        self.assertEqual("status cell (salvaged legacy row): retired: replaced by the invented design", revision.record.notes[-1])
        self.assertEqual(self.records["F-107"].legacy, revision.record.legacy, "main's verbatim cells are kept")
        _ = self.legal(result)

    def test_a_status_cell_outside_the_vocabulary_or_without_a_known_successor_is_undecided(self) -> None:
        self.status_branch("odd", "parked for later")
        odd = salvage(self.root, "odd")
        self.assertEqual(([], {"F-107": "its status cell on odd is outside the finding vocabulary: 'parked for later'"}), (odd.proposals, odd.undecided))
        status, _stdout, stderr = self.run_salvage("odd")
        self.assertEqual(salvage_command.EXIT_SUPERSEDE, status)
        self.assertIn("needs a decision by hand: F-107:", stderr)

    def edited(self, name: str, *edits: tuple[str, str, str]) -> None:
        """After the cut-over journalled main (legacy trees kept), a branch whose only change is each (path, old, new) replacement."""
        _ = self.journal_on_main(remove_legacy=False)
        _ = run_git(self.root, "checkout", "-q", "-b", name, "main")
        for path, old, new in edits:
            target = self.root / path
            text = target.read_text()
            self.assertIn(old, text)
            _ = target.write_text(text.replace(old, new, 1))
        _ = run_git(self.root, "commit", "-q", "-am", "edit")

    def accept(self, result: Salvage) -> None:
        """Accept salvage's proposals on main, as merging them would."""
        _ = run_git(self.root, "checkout", "-q", "main")
        _ = write_change_set(self.root, datetime(2026, 3, 1, tzinfo=UTC), result.proposals)
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "accept the salvaged proposals")
        self.publish()

    def salvaged_twice(self, name: str) -> Salvage:
        """Salvage, check the change set replays legally, accept it on main, salvage again: nothing left."""
        first = salvage(self.root, name)
        self.assertTrue(first.proposals)
        _ = self.legal(first)
        self.accept(first)
        again = salvage(self.root, name)
        self.assertEqual(([], [], {}, {}, []), (again.proposals, again.conflicts, again.unrevisable, again.undecided, again.unsalvaged))
        status, stdout, _stderr = self.run_salvage(name)
        self.assertEqual((0, ""), (status, stdout), "the second run is clean")
        return first

    def test_a_stored_status_overrides_a_legacy_status_and_the_second_run_is_clean(self) -> None:
        retired = "docs/ledgers/findings/retired/widgets.md"
        cases = [
            ("F-103", (retired, "receipt | | retired: receipt |", "receipt | | active |"), "active"),
            ("F-101", (retired, "| U-101 | superseded-by F-102 |", "| U-101 | retired: replaced |"), "retired"),
            ("U-101", ("docs/ledgers/unknowns.md", "| medium | resolved |", "| medium | open |"), "open"),
            ("F-107", ("docs/ledgers/findings/widgets.md", "owner said so | | active |", "owner said so | | retired: dropped |"), "retired"),
        ]
        for identifier, edit, status in cases:
            with self.subTest(identifier):
                self.setUp()
                self.edited("status", edit)
                first = salvage(self.root, "status")
                self.assertEqual([(identifier, status)], [(item.record.id, item.record.status) for item in first.proposals])
                self.assertEqual(status, self.legal(first).status(identifier), "the replay honours the edit")
                self.assertEqual(self.records[identifier].legacy, first.proposals[0].record.legacy, "main's verbatim cells are kept")
                _ = self.salvaged_twice("status")

    def test_an_evidence_row_s_new_successor_is_a_link_and_a_withdrawn_one_is_undecided(self) -> None:
        shard = "evidence/widgets/001.jsonl"
        self.edited("evidence", (shard, '"kind":"input","status":"active","legacy":true}',
                                 '"kind":"input","status":"retired","legacy":true,"superseded_by":"M-104","note":"replaced by M-104"}'))
        result = salvage(self.root, "evidence")
        proposed = {item.record.id: item.record for item in result.proposals}
        self.assertEqual(("M-102",), proposed["M-104"].links.supersedes)
        self.assertEqual("superseded", self.legal(result).status("M-102"))
        _ = self.salvaged_twice("evidence")
        self.setUp()
        self.edited("retarget", (shard, '"superseded_by":"M-104"', '"superseded_by":"M-101"'))
        retarget = salvage(self.root, "retarget")
        self.assertEqual([], retarget.proposals)
        self.assertIn("withdraws the legacy successor M-104", retarget.undecided["M-103"])

    def test_a_next_probe_cleared_or_filled_is_a_revision_with_its_gap(self) -> None:
        self.edited("probe", ("docs/ledgers/unknowns.md", "| open | Probe F-105 |", "| open |  |"))
        cleared = salvage(self.root, "probe")
        (revision,) = cleared.proposals
        self.assertEqual((None, ("area", "next_probe")), (revision.record.text("next_probe"), revision.record.gaps))
        _ = self.salvaged_twice("probe")
        # The reverse: a probe the accepted row lacks, filled on a branch, fills the gap.
        self.setUp()
        unknowns = self.root / "docs/ledgers/unknowns.md"
        _ = unknowns.write_text(unknowns.read_text().replace("| open | Probe F-105 |", "| open |  |"))
        _ = run_git(self.root, "commit", "-q", "-am", "no probe on main")
        self.edited("fill", ("docs/ledgers/unknowns.md", "| open |  |", "| open | Probe again |"))
        filled = salvage(self.root, "fill")
        self.assertEqual([("U-102", "Probe again", ("area",))], [(item.record.id, item.record.text("next_probe"), item.record.gaps) for item in filled.proposals])
        _ = self.salvaged_twice("fill")

    def test_a_line_above_a_prose_supersession_changes_no_row(self) -> None:
        self.edited("prose", ("docs/ledgers/findings/widgets.md", "Active rows.", "Intro line.\n\nActive rows."))
        result = salvage(self.root, "prose")
        self.assertEqual(([], {}, {}), (result.proposals, result.unrevisable, result.undecided))
        self.assertTrue(all(kind == "cosmetic" for kind in result.kinds.values()), result.kinds)

    def test_a_successor_main_already_links_is_accepted(self) -> None:
        self.edited("linked", ("docs/ledgers/findings/widgets.md", "owner said so | | active |", "owner said so | | superseded-by F-105 |"))
        first = salvage(self.root, "linked")
        self.assertEqual({"F-107": "superseded", "F-105": "revision"}, first.kinds)
        self.accept(first)
        again = salvage(self.root, "linked")
        self.assertEqual(([], {"F-107": "accepted"}, []), (again.proposals, again.kinds, again.unsalvaged))

    def test_a_link_from_a_conflicting_row_is_never_attached(self) -> None:
        records = self.journal_on_main(remove_legacy=False)
        _ = run_git(self.root, "checkout", "-q", "-b", "supersede", "main")
        ledger = self.root / "docs/ledgers/findings/widgets.md"
        _ = ledger.write_text(ledger.read_text().replace("owner said so | | active |", "owner said so | | superseded-by F-111 |")
                              + "| F-111 | Replacement claim | observed | review | high; x | | active |\n")
        _ = run_git(self.root, "commit", "-q", "-am", "supersede F-107")
        _ = run_git(self.root, "checkout", "-q", "main")
        f107 = records["F-107"]
        retired = Revision.of(Record.from_json({**f107.to_json(), "status": "retired", "notes": ["Invented: retired on main"]}), [Revision.of(f107).rev])
        _ = write_change_set(self.root, datetime(2026, 2, 2, tzinfo=UTC), [retired])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "main retires F-107")
        self.publish()
        result = salvage(self.root, "supersede")
        self.assertEqual({"F-107": "conflict", "F-111": "new"}, result.kinds)
        self.assertEqual([("F-111", ())], [(item.record.id, item.record.links.supersedes) for item in result.proposals])
        self.assertIn("F-107: F-111 would supersede it, but F-107 is conflict; F-111 is proposed without that link", result.notices)

    def test_a_link_from_a_row_held_for_a_stuck_successor_is_never_attached(self) -> None:
        # F-107's legacy row names F-105 (prose) and the new F-111 (status cell); F-105's claim edit holds it, so F-107 is stuck.
        ledger = "docs/ledgers/findings/widgets.md"
        self.edited("stuck", (ledger, "Last reconciled", "F-107 superseded-by F-105.\n\nLast reconciled"),
                    (ledger, "owner said so | | active |", "owner said so | | superseded-by F-111 |\n| F-111 | Replacement claim | observed | review | high; x | | active |"),
                    (ledger, "| F-105 | Gadget 3.0.91 run ends;", "| F-105 | Gadget 3.0.91 run ended;"))
        result = salvage(self.root, "stuck")
        self.assertEqual(("supersede", "undecided", "new"), (result.kinds["F-105"], result.kinds["F-107"], result.kinds["F-111"]))
        self.assertIn("names successor F-105", result.undecided["F-107"])
        self.assertEqual([("F-111", ())], [(item.record.id, item.record.links.supersedes) for item in result.proposals])
        self.assertIn("F-107: F-111 would supersede it, but F-107 is undecided; F-111 is proposed without that link", result.notices)

    def test_a_row_the_accepted_journal_does_not_hold_is_never_revised(self) -> None:
        ledger = self.root / "docs/ledgers/findings/widgets.md"
        _ = run_git(self.root, "checkout", "-q", "-b", "retire")
        _ = ledger.write_text(ledger.read_text().replace("owner said so | | active |", "owner said so | | retired: dropped |"))
        _ = run_git(self.root, "commit", "-q", "-am", "retire F-107")
        _ = run_git(self.root, "checkout", "-q", "main")
        _ = ledger.write_text("".join(line + "\n" for line in ledger.read_text().splitlines() if not line.startswith(("| F-107 ", "F-106 is retired"))))
        _ = run_git(self.root, "commit", "-q", "-am", "main drops F-107")
        self.publish()
        result = salvage(self.root, "retire")
        self.assertEqual(([], "undecided"), (result.proposals, result.kinds["F-107"]))
        self.assertIn("the accepted journal has no record F-107 to revise", result.undecided["F-107"])

    def test_a_proposal_that_does_not_replay_cleanly_is_never_returned(self) -> None:
        self.status_branch("dangling", "retired: dropped")
        real = salvage_command.apply

        def dangling(edit: salvage_command.Edit, base: Record) -> Record | None:
            built = real(edit, base)
            return None if built is None else Record.from_json({**built.to_json(), "refs": ["F-999"]})

        with mock.patch.object(salvage_command, "apply", dangling):
            result = salvage(self.root, "dangling")
        self.assertEqual([], result.proposals)
        self.assertIn("would not replay cleanly: F-107: revision ", result.undecided["F-107"])

    def forced(self, branch: str, identifier: str) -> Salvage:
        """Salvage ``branch`` with the first replay check also failing ``identifier``; the final set must replay cleanly."""
        real = salvage_command.honoured
        calls: list[int] = []

        def failing(on_accepted: salvage_command.Accepted, proposals: Sequence[Revision], edits: Mapping[str, salvage_command.Edit]) -> dict[str, str]:
            found = real(on_accepted, proposals, edits)
            calls.append(len(calls))
            return {**found, identifier: "invented failure"} if len(calls) == 1 else found

        with mock.patch.object(salvage_command, "honoured", failing):
            result = salvage(self.root, branch)
        _ = self.legal(result)
        return result

    def superseded_by_new_row(self) -> None:
        self.edited("pair", ("docs/ledgers/findings/widgets.md", "owner said so | | active |",
                             "owner said so | | superseded-by F-111 |\n| F-111 | Replacement claim | observed | review | high; x | | active |"))

    def test_a_status_note_is_the_same_by_branch_name_and_by_its_merged_tip(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "named")
        ledger = self.root / "docs/ledgers/findings/widgets.md"
        _ = ledger.write_text(ledger.read_text().replace("owner said so | | active |", "owner said so | | retired: dropped |"))
        _ = run_git(self.root, "commit", "-q", "-am", "retire F-107")
        _ = self.journal_on_main(remove_legacy=True)
        first = salvage(self.root, "named")
        self.assertEqual(["F-107"], [item.record.id for item in first.proposals])
        self.accept(first)
        _ = run_git(self.root, "checkout", "-q", "named")
        with self.assertRaises(subprocess.CalledProcessError):
            _ = run_git(self.root, "merge", "-q", "--no-edit", "main")
        _ = run_git(self.root, "rm", "-r", "-q", "--ignore-unmatch", "docs/ledgers", "evidence")
        _ = run_git(self.root, "commit", "-q", "--no-edit")
        status, stdout, stderr = self.run_salvage("named")
        self.assertEqual((0, ""), (status, stdout), stderr)
        self.assertIn("leaves nothing to salvage", stderr)

    def test_a_status_revision_is_the_same_by_name_by_sha_and_after_an_unrelated_commit(self) -> None:
        self.status_branch("spelled", "retired: dropped")
        by_name = salvage(self.root, "spelled")
        sha = run_git(self.root, "rev-parse", "spelled").strip()
        by_sha = salvage(self.root, sha)
        _ = run_git(self.root, "checkout", "-q", "spelled")
        _ = run_git(self.root, "commit", "-q", "--allow-empty", "-m", "unrelated")
        later = salvage(self.root, "spelled")
        lines = [[revision.line() for revision in result.proposals] for result in (by_name, by_sha, later)]
        self.assertEqual(lines[0], lines[1])
        self.assertEqual(lines[0], lines[2])
        self.accept(by_name)
        _ = run_git(self.root, "checkout", "-q", "spelled")
        _ = run_git(self.root, "commit", "-q", "--allow-empty", "-m", "another unrelated")
        again = salvage(self.root, "spelled")
        self.assertEqual(([], "accepted"), (again.proposals, again.kinds["F-107"]))

    def test_a_dropped_old_row_strips_its_link_from_the_successor(self) -> None:
        self.superseded_by_new_row()
        result = self.forced("pair", "F-107")
        self.assertEqual([("F-111", ())], [(item.record.id, item.record.links.supersedes) for item in result.proposals])
        self.assertEqual(("undecided", "invented failure"), (result.kinds["F-107"], result.undecided["F-107"]))
        self.assertIn("F-107: F-111 would supersede it, but F-107 is undecided; F-111 is proposed without that link", result.notices)

    def test_a_dropped_successor_holds_the_old_row(self) -> None:
        self.superseded_by_new_row()
        result = self.forced("pair", "F-111")
        self.assertEqual([], result.proposals)
        self.assertEqual("undecided", result.kinds["F-107"])
        self.assertIn("names successor F-111, which cannot carry the link (invented failure)", result.undecided["F-107"])
        self.assertIn("F-107", result.unsalvaged)

    def test_a_row_held_for_its_second_successor_leaves_no_link_on_its_first(self) -> None:
        # F-107 names the new F-111 (status cell) and the accepted F-105 (prose); F-105's link fails replay.
        ledger = "docs/ledgers/findings/widgets.md"
        self.edited("two", (ledger, "Last reconciled", "F-107 superseded-by F-105.\n\nLast reconciled"),
                    (ledger, "owner said so | | active |", "owner said so | | superseded-by F-111 |\n| F-111 | Replacement claim | observed | review | high; x | | active |"))
        result = self.forced("two", "F-105")
        self.assertEqual([("F-111", ())], [(item.record.id, item.record.links.supersedes) for item in result.proposals])
        self.assertEqual("undecided", result.kinds["F-107"])
        self.assertIn("names successor F-105, which cannot carry the link (invented failure)", result.undecided["F-107"])
        self.assertIn("F-107: F-111 would supersede it, but F-107 is undecided; F-111 is proposed without that link", result.notices)
        self.assertIn("F-107: F-105 would supersede it, but F-107 is undecided; F-105 is not revised", result.notices)

    def test_a_notice_about_a_held_row_follows_the_successor_s_actual_links(self) -> None:
        # Main already links F-105 -> F-107. The branch names F-105 and F-109 as F-107's successors (F-109's claim edit
        # holds it, so F-107 is held) and F-108 superseded by F-105, so F-105 is revised to carry F-108's link and keeps
        # the accepted link to F-107.
        records = self.journal_on_main(remove_legacy=False)
        f105 = records["F-105"]
        linked = Revision.of(Record.from_json({**f105.to_json(), "supersedes": ["F-107"]}, f105.body), [Revision.of(f105).rev])
        _ = write_change_set(self.root, datetime(2026, 2, 3, tzinfo=UTC), [linked])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "main links F-105 to F-107")
        self.publish()
        _ = run_git(self.root, "checkout", "-q", "-b", "kept", "main~1")
        ledger = self.root / "docs/ledgers/findings/widgets.md"
        text = ledger.read_text()
        text = text.replace("Last reconciled", "F-107 superseded-by F-109.\n\nLast reconciled")
        text = text.replace("owner said so | | active |", "owner said so | | superseded-by F-105 |")
        text = text.replace("| medium; loopback |  | active |", "| medium; loopback |  | superseded-by F-105 |")
        text = text.replace("|  F-109 | Spaced row |", "|  F-109 | Spaced row edited |")
        _ = ledger.write_text(text)
        _ = run_git(self.root, "commit", "-q", "-am", "branch edits")
        result = salvage(self.root, "kept")
        self.assertEqual("undecided", result.kinds["F-107"])
        f105_proposal = next(item.record for item in result.proposals if item.record.id == "F-105")
        self.assertEqual(("F-107", "F-108"), f105_proposal.links.supersedes)
        self.assertIn("F-107: F-105 would supersede it, but F-107 is undecided; F-105 keeps the link its accepted record already holds", result.notices)
        self.assertFalse(any("F-105 is proposed without that link" in notice for notice in result.notices))

    def test_invariants_refuse_a_notice_its_proposal_contradicts(self) -> None:
        self.superseded_by_new_row()
        result = salvage(self.root, "pair")
        result.notices.append("F-107: F-111 would supersede it, but F-107 is undecided; F-111 is not revised")
        with self.assertRaisesRegex(salvage_command.SalvageError, "F-111: a notice says it is not revised"):
            salvage_command.invariants(result, ["F-107", "F-111"], salvage_command.accepted_on(self.root, "origin/main"))

    def test_compatibility_rows_sharing_a_name_are_each_held_and_none_is_lost(self) -> None:
        table = "docs/ledgers/compatibility.md"
        for case, added, shared in (("new names", "| Dup | Unknown | One |\n| Dup | Unknown | Two |\n", "compatibility row 'Dup'"),
                                    ("an accepted name", "| Discovery | Unknown | Again |\n", "compatibility row 'Discovery'")):
            with self.subTest(case):
                self.setUp()
                self.edited("dup", (table, "| Discovery | Unknown | Browser behavior |\n", "| Discovery | Unknown | Browser behavior |\n" + added))
                result = salvage(self.root, "dup")
                held = sorted(name for name, reason in result.undecided.items() if f"records sharing the {shared}" in reason)
                self.assertEqual(2, len(held), result.undecided)
                self.assertEqual(2, len(set(held)), "each row under its own number")
                self.assertFalse([item for item in result.proposals if item.record.type == "compatibility"])
                self.assertFalse(any("removed on dup" in notice for notice in result.notices), result.notices)

    def test_a_row_whose_identity_main_holds_twice_is_held_never_new(self) -> None:
        twin = "| Discovery | Unknown | Twin |\n"
        cases = (("changed", "docs/ledgers/compatibility.md", twin, "| Discovery | Observed | Twin |\n"),
                 ("one dropped", "docs/ledgers/compatibility.md", twin, ""),
                 ("both kept", "docs/ledgers/findings/widgets.md", "owner said so | | active |", "owner said so | | retired: dropped |"))
        for case, path, old, replacement in cases:
            with self.subTest(case):
                self.setUp()
                table = self.root / "docs/ledgers/compatibility.md"
                _ = table.write_text(table.read_text().replace("| Discovery | Unknown | Browser behavior |\n", "| Discovery | Unknown | Browser behavior |\n" + twin))
                _ = run_git(self.root, "commit", "-q", "-am", "main holds Discovery twice")
                self.edited("twin", (path, old, replacement))
                result = salvage(self.root, "twin")
                self.assertFalse([item for item in result.proposals if item.record.type == "compatibility"], result.kinds)
                self.assertFalse([name for name, kind in result.kinds.items() if name.startswith("CP-") and kind == "new"], result.kinds)
                held = [name for name, reason in result.undecided.items() if "which the accepted journal holds 2 times (CP-001, CP-002)" in reason]
                # A changed twin is held; the twin left as it was when the other is dropped is unchanged (the other a removal).
                self.assertEqual(1 if case == "changed" else 0, len(held), result.undecided)

    def test_a_renamed_compatibility_row_is_held_with_the_row_it_may_replace(self) -> None:
        self.edited("rename", ("docs/ledgers/compatibility.md", "| Discovery | Unknown |", "| Discovery renamed | Unknown |"))
        result = salvage(self.root, "rename")
        cps = {name: kind for name, kind in result.kinds.items() if name.startswith("CP-")}
        self.assertEqual(2, len(cps), cps)
        self.assertEqual({"undecided"}, set(cps.values()))
        self.assertIn("CP-001", cps)
        self.assertIn("is gone from the branch (accepted as CP-001) and new compatibility rows appeared ('Discovery renamed' as", result.undecided["CP-001"])
        new = next(name for name in cps if name != "CP-001")
        self.assertIn("new compatibility row 'Discovery renamed', and the branch lost CP-001; a rename", result.undecided[new])
        self.assertFalse([item for item in result.proposals if item.record.type == "compatibility"])
        self.assertFalse(any("CP-001: removed" in notice for notice in result.notices), result.notices)

    def test_a_successor_with_only_a_cosmetic_change_carries_the_link(self) -> None:
        ledger = "docs/ledgers/findings/widgets.md"
        self.edited("cosmetic", (ledger, "| F-105 | Gadget", "|  F-105  | Gadget"), (ledger, "owner said so | | active |", "owner said so | | superseded-by F-105 |"))
        result = salvage(self.root, "cosmetic")
        self.assertEqual({"F-105": "revision", "F-107": "superseded"}, {name: kind for name, kind in result.kinds.items() if name in ("F-105", "F-107")})
        self.assertEqual(("F-107",), next(item.record for item in result.proposals if item.record.id == "F-105").links.supersedes)
        self.assertEqual({}, result.undecided)
        self.assertFalse(any(notice.startswith("F-105: only verbatim") for notice in result.notices), result.notices)
        _ = self.legal(result)

    def test_a_rename_of_a_name_main_holds_twice_is_held_naming_both(self) -> None:
        table = self.root / "docs/ledgers/compatibility.md"
        _ = table.write_text(table.read_text().replace("| Discovery | Unknown | Browser behavior |\n",
                                                       "| Discovery | Unknown | Browser behavior |\n| Discovery | Unknown | Twin |\n"))
        _ = run_git(self.root, "commit", "-q", "-am", "main holds Discovery twice")
        self.edited("rename-twin", ("docs/ledgers/compatibility.md", "| Discovery | Unknown | Twin |", "| Discovery renamed | Unknown | Twin |"))
        result = salvage(self.root, "rename-twin")
        self.assertFalse([item for item in result.proposals if item.record.type == "compatibility"], result.kinds)
        self.assertFalse(any("removed on" in notice and notice.startswith("CP-") for notice in result.notices), result.notices)
        renamed = [reason for reason in result.undecided.values() if "new compatibility row 'Discovery renamed'" in reason]
        self.assertEqual(1, len(renamed), result.undecided)
        self.assertIn("the branch lost CP-001, CP-002; a rename", renamed[0])
        # Only real IDs are reported: the accepted twins and the branch's row, never a merge-base provisional number.
        self.assertEqual({"CP-001", "CP-002"}, {name for name, reason in result.undecided.items() if reason.startswith("it is gone from the branch")})

    def test_a_rename_to_a_name_main_added_meanwhile_is_held(self) -> None:
        _ = self.journal_on_main(remove_legacy=False)
        _ = run_git(self.root, "checkout", "-q", "-b", "rename-new", "main")
        table = self.root / "docs/ledgers/compatibility.md"
        _ = table.write_text(table.read_text().replace("| Discovery | Unknown |", "| Gadgets | Unknown |"))
        _ = run_git(self.root, "commit", "-q", "-am", "rename Discovery to Gadgets")
        _ = run_git(self.root, "checkout", "-q", "main")
        gadgets = surface_record("CP-900", "Gadgets")
        _ = write_change_set(self.root, datetime(2026, 2, 4, tzinfo=UTC), [Revision.of(gadgets)])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "main adds Gadgets")
        self.publish()
        result = salvage(self.root, "rename-new")
        self.assertFalse([item for item in result.proposals if item.record.type == "compatibility"], result.kinds)
        self.assertEqual("undecided", result.kinds.get("CP-900"), result.kinds)
        self.assertIn("the branch lost CP-001; a rename", result.undecided["CP-900"])
        self.assertIn("it is gone from the branch (accepted as CP-001)", result.undecided["CP-001"])

    def test_a_supersession_cycle_among_proposals_is_never_returned(self) -> None:
        # Two new rows superseding each other: each replays alone, the pair is a cycle check --ci rejects.
        self.edited("cycle", ("docs/ledgers/findings/widgets.md", "owner said so | | active |",
                              "owner said so | | active |\n| F-111 | One | observed | review | high; x | | superseded-by F-112 |"
                              + "\n| F-112 | Two | observed | review | high; x | | superseded-by F-111 |"))
        result = salvage(self.root, "cycle")
        self.assertEqual([], result.proposals)
        self.assertEqual({"F-111", "F-112"}, {name for name, reason in result.undecided.items() if "supersession cycle" in reason}, result.undecided)
        _ = self.legal(result)

    def test_one_bad_row_never_aborts_the_run(self) -> None:
        self.status_branch("two", "retired: dropped", extra="| F-111 | Unrelated new row | observed | review | high; x | | active |\n")

        def failing(origin: Record, record: Record, ref: str) -> salvage_command.Edit:
            raise RecordError(f"{record.id}: invented failure on {ref} ({origin.id})")

        with mock.patch.object(salvage_command, "plan", failing):
            result = salvage(self.root, "two")
        self.assertEqual(["F-111"], [item.record.id for item in result.proposals])
        self.assertIn("invented failure", result.undecided["F-107"])

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
        self.assertIn("leaves nothing to salvage", stderr)

    def test_a_merged_tip_whose_pre_merge_parent_changed_only_a_document_is_refused(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "spec-edit")
        spec = self.root / "docs/specs/net/another.md"
        _ = spec.write_text(spec.read_text() + "\nInvented: one more line.\n", encoding="utf-8")
        _ = run_git(self.root, "commit", "-q", "-am", "edit a spec")
        tip = run_git(self.root, "rev-parse", "HEAD").strip()
        _ = self.journal_on_main(remove_legacy=True)
        _ = run_git(self.root, "checkout", "-q", "spec-edit")
        _ = run_git(self.root, "merge", "-q", "--no-edit", "main")
        status, stdout, stderr = self.run_salvage("spec-edit")
        self.assertEqual((salvage_command.EXIT_PREMERGE, ""), (status, stdout))
        self.assertIn(f"run memory salvage {tip}", stderr)

    def revise_spec_on_main(self, body: str) -> None:
        """Main revises the REF holding docs/specs/net/another.md to ``body``, as memory revise --body would."""
        _ = run_git(self.root, "checkout", "-q", "main")
        state = replay(read_tree(self.root))
        entry = next(entry for entry in state.entries.values() if entry.record.type == "reference" and entry.record.legacy is not None
                     and entry.record.legacy.path == "docs/specs/net/another.md")
        data = {**entry.record.to_json(), "body_sha256": hashlib.sha256(body.encode()).hexdigest()}
        _ = write_change_set(self.root, datetime(2026, 4, 1, tzinfo=UTC), [Revision.of(Record.from_json(data, body), entry.heads)])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "revise the spec's REF")
        self.publish()

    def ref_body(self, ref: str, path: str) -> str:
        body = next(record.body for record in export(self.root, ref).records().values()
                    if record.type == "reference" and record.legacy is not None and record.legacy.path == path)
        assert body is not None
        return body

    def test_check_lists_a_pending_document_and_out_writes_the_body_that_clears_it(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "spec-only")
        spec = self.root / "docs/specs/net/another.md"
        _ = spec.write_text(spec.read_text() + "\nInvented: one more line.\n", encoding="utf-8")
        _ = run_git(self.root, "commit", "-q", "-am", "edit a spec")
        _ = self.journal_on_main(remove_legacy=True)
        status, stdout, _stderr = self.run_salvage("spec-only", check=True)
        self.assertEqual(1, status)
        self.assertRegex(stdout, r"spec-only: docs/specs/net/another.md: pending document; in force: REF-[0-9]+ \(scope: none; other text; this text newly names: nothing\); this branch changed it since its base: yes; the exporter "
                         + r"reads as its scope, from its Status and Scope sections: nothing; its body also names: nothing; at this branch's base its reference was REF-[0-9]+: now in force; main changed or replaced its reference since this "
                         + r"branch's base: no; a new reference for it carries --note 'salvaged document: docs/specs/net/another.md'\n")
        self.assertNotIn("--body", stdout, "no path that does not exist")
        self.assertIn("0 unsalvaged row(s), 1 pending document(s) on spec-only", stdout)
        out = self.top / "out"
        status, _stdout, stderr = self.run_salvage("spec-only", out=out)
        written = out / "documents/docs/specs/net/another.md"
        self.assertEqual(self.ref_body("spec-only", "docs/specs/net/another.md").encode(), written.read_bytes(), "the exporter's body at the tip")
        self.assertIn(f"memory salvage: docs/specs/net/another.md: body written to {written}\n", stderr)
        self.assertEqual([], [line for line in stderr.splitlines() if line.split(": ", 2)[-1].startswith("memory ")], "facts, never a command")
        self.revise_spec_on_main(written.read_text(encoding="utf-8").replace("one more line", "a different line"))
        self.assertEqual(1, self.run_salvage("spec-only", check=True)[0], "a body merged into other text stays listed")
        self.revise_spec_on_main(written.read_text(encoding="utf-8"))
        status, stdout, _stderr = self.run_salvage("spec-only", check=True)
        self.assertEqual((0, True), (status, "0 unsalvaged row(s), 0 pending document(s)" in stdout))

    def accept_reference(self, changes: online.Changes, identifier: str) -> None:
        """Accept on main the REF ``memory propose reference`` would publish from ``changes``."""
        data: dict[str, JSON] = {name: value for name, value in changes.values.items() if value is not None}
        assert changes.body is not None
        data |= {"id": identifier, "type": "reference", "status": "current", "body_sha256": hashlib.sha256(changes.body.encode()).hexdigest()}
        if changes.note is not None:
            data["notes"] = [changes.note]
        _ = run_git(self.root, "checkout", "-q", "main")
        _ = write_change_set(self.root, datetime(2026, 4, 2, tzinfo=UTC), [Revision.of(Record.from_json(data, changes.body))])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", f"accept {identifier}")
        self.publish()

    def new_spec_branch(self, *specs: str, cite: bool = True) -> None:
        """A branch adding ``specs`` (each body "# Same\\n") and, with ``cite``, a tracked row on the first; main cut over."""
        _ = run_git(self.root, "checkout", "-q", "-b", "specs")
        for spec in specs:
            _ = (self.root / spec).write_text("# Same\n", encoding="utf-8")
        if cite:
            shard = self.root / "evidence/widgets/001.jsonl"
            _ = shard.write_text(shard.read_text() + json.dumps({"id": "M-105", "path": specs[0], "sha256": "d" * 64, "class": "tracked", "kind": "report",
                                                                 "date": "2026-01-07", "label": "A new spec", "status": "active"}, separators=(",", ":")) + "\n")
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "add specs")
        _ = self.journal_on_main(remove_legacy=True)

    def test_an_unrelated_accepted_ref_with_the_same_body_is_not_the_new_document(self) -> None:
        self.new_spec_branch("docs/specs/net/third.md")
        self.accept_reference(online.Changes({"title": "Unrelated", "scope": {"basis": "explicit", "subjects": ["tooling"]}}, None, "# Same\n"), "REF-900")
        result = salvage(self.root, "specs")
        self.assertEqual("undecided", result.kinds["M-105"])
        self.assertIn("docs/specs/net/third.md, which no accepted REF names", result.undecided["M-105"])
        self.assertEqual([("docs/specs/net/third.md", ())], [(item.path, item.in_force) for item in result.documents])

    def test_two_branch_documents_with_one_body_are_each_provisional(self) -> None:
        self.new_spec_branch("docs/specs/net/third.md", "docs/specs/net/fourth.md")
        # An accepted REF of that one body (proposed by hand, no note) once made both map to it and abort the run.
        self.accept_reference(online.Changes({"title": "Same", "scope": {"basis": "explicit"}}, None, "# Same\n"), "REF-900")
        result = salvage(self.root, "specs")
        self.assertEqual(["docs/specs/net/fourth.md", "docs/specs/net/third.md"], sorted(item.path for item in result.documents))
        self.assertEqual("undecided", result.kinds["M-105"])

    def test_a_ref_proposed_without_the_note_leaves_the_document_and_its_row_listed(self) -> None:
        self.new_spec_branch("docs/specs/net/third.md")
        self.accept_reference(online.Changes({"title": "Third", "scope": {"basis": "explicit"}}, None, "# Same\n"), "REF-900")
        status, stdout, _stderr = self.run_salvage("specs", check=True)
        self.assertEqual(1, status)
        self.assertIn("specs: docs/specs/net/third.md: pending document; no accepted reference names this path; this branch changed it since its base: yes; the exporter reads as its scope, from its whole text:"
                      + " nothing; at this branch's base its reference was none (the document is new on this branch); main changed or replaced its reference since this branch's base: no;"
                      + " rows held because they cite it: M-105; a new reference for it carries --note 'salvaged document: docs/specs/net/third.md'\n",
                      stdout)
        self.assertIn("specs: M-105: undecided legacy row not yet salvaged", stdout)

    def test_a_changed_document_naming_other_tokens_lists_both_scopes(self) -> None:
        # The widget spec's text names client:29990002 and server:29990001; the branch re-observes it on build 29990003.
        self.edited("retoken", ("docs/specs/net/widget.md", "Candidate; observed with client build 29990002 only.",
                                "Candidate; observed with client build 29990003 only."))
        (document,) = salvage(self.root, "retoken").documents
        (held,) = document.in_force
        self.assertIn("client:29990003", document.names)
        self.assertIn("client:29990002", held.scope)
        # The spec's later sections name a commit its Status and Scope sections do not.
        self.assertEqual((False, False, ("c0ffee3",)), (held.holds, document.main_changed, document.more))
        self.assertEqual(("client:29990003",), held.newly)
        self.assertIn(f"in force: {held.id} (scope: {', '.join(held.scope)}; other text; this text newly names: client:29990003); this branch changed it since its base: yes; the exporter reads as its scope, from its Status and Scope sections: {', '.join(document.names)};",
                      document.line())
        self.setUp()
        self.edited("same-tokens", ("docs/specs/net/widget.md", "Candidate; observed", "Candidate, still; observed"))
        (document,) = salvage(self.root, "same-tokens").documents
        self.assertEqual((document.in_force[0].scope, ("c0ffee3",), False), (document.names, document.more, document.main_changed))

    def test_a_body_naming_more_tokens_than_its_sections_says_so(self) -> None:
        # The scope reads Status and Scope only; the branch adds an observation on another build in a later section.
        self.edited("newly", ("docs/specs/net/widget.md", "## Confirmed\n", "## Confirmed\n\n- Seen again on build 29990009.\n"))
        (document,) = salvage(self.root, "newly").documents
        self.assertEqual((("any:29990009", "c0ffee3"), document.in_force[0].scope), (document.more, document.names))
        # REF-008's shape: the sections equal, the body adds one build among tokens the accepted text already names.
        self.assertEqual(("any:29990009",), document.in_force[0].newly)
        self.assertIn("; other text; this text newly names: any:29990009);", document.line())
        self.assertIn("; its body also names: any:29990009, c0ffee3; at this branch's base its reference was REF-002: now in force; main changed or replaced its reference since this branch's base: no;", document.line())

    def test_a_digest_s_scope_is_read_from_its_whole_text(self) -> None:
        # A digest has no Status or Scope sections the exporter reads: its scope comes from its whole text, and nothing lies beyond it.
        self.edited("digest", ("docs/research/net/widgets.md", "Frames take 42.5 ms", "Frames take 42.5 ms on server build 29990004"))
        (document,) = salvage(self.root, "digest").documents
        self.assertEqual((False, ("server:29990004",), ()), (document.from_sections, document.names, document.more))
        self.assertIn("; the exporter reads as its scope, from its whole text: server:29990004; at this branch's base", document.line())
        self.assertNotIn("its body also names", document.line())
        self.assertNotIn("Status and Scope", document.line())

    def test_a_row_citing_a_document_main_has_since_changed_waits_for_it(self) -> None:
        # An older branch adds tracked evidence for the spec as it then read; main later holds other text for that path.
        _ = run_git(self.root, "checkout", "-q", "-b", "older")
        shard = self.root / "evidence/widgets/001.jsonl"
        _ = shard.write_text(shard.read_text() + json.dumps({"id": "M-105", "path": "docs/specs/net/another.md", "sha256": "d" * 64, "class": "tracked",
                                                             "kind": "report", "date": "2026-01-07", "label": "The spec as it read", "status": "active"},
                                                            separators=(",", ":")) + "\n")
        _ = run_git(self.root, "commit", "-q", "-am", "evidence for the spec")
        _ = self.journal_on_main(remove_legacy=True)
        self.revise_spec_on_main(self.ref_body("main~1", "docs/specs/net/another.md") + "\nInvented: main's successor text.\n")
        result = salvage(self.root, "older")
        # Not proposed citing main's text: held, and the document it cites listed although the branch never changed it.
        self.assertEqual("undecided", result.kinds["M-105"])
        self.assertNotIn("M-105", [revision.record.id for revision in result.proposals])
        (document,) = result.documents
        self.assertRegex(result.undecided["M-105"], r"^it cites docs/specs/net/another\.md as this branch has it \(REF-[0-9]+, a provisional number\); "
                         + rf"{document.in_force[0].id} holds other text: settle the document first$")
        self.assertEqual((("M-105",), True, False, False), (document.citing, document.main_changed, document.in_force[0].holds, document.branch_changed))
        self.assertIn("; this branch changed it since its base: no;", document.line())
        self.assertIn("main changed or replaced its reference since this branch's base: yes; rows held because they cite it: M-105;", document.line())
        status, stdout, _stderr = self.run_salvage("older", check=True)
        self.assertEqual((1, True), (status, "1 unsalvaged row(s), 1 pending document(s)" in stdout), stdout)

    def test_a_document_main_also_changed_says_so(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "both-edit")
        spec = self.root / "docs/specs/net/another.md"
        _ = spec.write_text(spec.read_text() + "\nInvented: the branch's line.\n", encoding="utf-8")
        _ = run_git(self.root, "commit", "-q", "-am", "branch edits the spec")
        _ = self.journal_on_main(remove_legacy=True)
        self.revise_spec_on_main(self.ref_body("main~1", "docs/specs/net/another.md") + "\nInvented: main's line.\n")
        (document,) = salvage(self.root, "both-edit").documents
        (held,) = document.in_force
        self.assertEqual(["in force: " + held.id + " (scope: none; other text; this text newly names: nothing)", "this branch changed it since its base: yes", "the exporter reads as its scope, from its Status and Scope sections: nothing",
                          "its body also names: nothing", "at this branch's base its reference was " + held.id + ": now in force",
                          "main changed or replaced its reference since this branch's base: yes",
                          "a new reference for it carries --note 'salvaged document: docs/specs/net/another.md'"], document.facts())

    def test_a_document_main_replaced_with_the_same_text_says_main_changed_it(self) -> None:
        # Main re-homed the document after the base: a new REF with the same body supersedes the migrated one.
        _ = run_git(self.root, "checkout", "-q", "-b", "rehomed")
        spec = self.root / "docs/specs/net/another.md"
        _ = spec.write_text(spec.read_text() + "\nInvented: the branch's line.\n", encoding="utf-8")
        _ = run_git(self.root, "commit", "-q", "-am", "branch edits the spec")
        records = self.journal_on_main(remove_legacy=True)
        old = next(record for record in records.values() if record.type == "reference" and record.legacy is not None
                   and record.legacy.path == "docs/specs/net/another.md")
        body = old.body or ""
        self.accept_reference(online.Changes({"title": "Another", "scope": {"basis": "explicit"}, "supersedes": [old.id]},
                                             salvage_command.DOCUMENT_NOTE.format("docs/specs/net/another.md"), body), "REF-900")
        (document,) = salvage(self.root, "rehomed").documents
        self.assertEqual((("REF-900",), True), (tuple(item.id for item in document.in_force), document.main_changed))
        self.assertIn("in force: REF-900 (scope: none; other text; this text newly names: nothing);", document.line())
        self.assertIn("main changed or replaced its reference since this branch's base: yes", document.line())
        self.assertIn(f"; at this branch's base its reference was {old.id}: superseded by REF-900;", document.line())

    def test_a_document_main_superseded_by_a_reference_of_another_path_says_by_which(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "moved")
        spec = self.root / "docs/specs/net/another.md"
        _ = spec.write_text(spec.read_text() + "\nInvented: the branch's line.\n", encoding="utf-8")
        _ = run_git(self.root, "commit", "-q", "-am", "branch edits the spec")
        records = self.journal_on_main(remove_legacy=True)
        old = next(record for record in records.values() if record.type == "reference" and record.legacy is not None
                   and record.legacy.path == "docs/specs/net/another.md")
        self.accept_reference(online.Changes({"title": "Elsewhere", "scope": {"basis": "explicit"}, "supersedes": [old.id]},
                                             salvage_command.DOCUMENT_NOTE.format("docs/specs/net/elsewhere.md"), "# Elsewhere\n"), "REF-900")
        (document,) = salvage(self.root, "moved").documents
        self.assertEqual(((), ((old.id, "superseded by REF-900"),)), (document.in_force, document.based_on))
        self.assertIn(f"pending document; no accepted reference names this path; this branch changed it since its base: yes;", document.line())
        self.assertIn(f"; at this branch's base its reference was {old.id}: superseded by REF-900; main changed or replaced its reference"
                      + " since this branch's base: yes;", document.line())

    def test_a_superseding_ref_is_done_only_once_the_supersession_is_accepted(self) -> None:
        self.edited("retoken", ("docs/specs/net/widget.md", "Candidate; observed with client build 29990002 only.",
                                "Candidate; observed with client build 29990003 only."))
        (document,) = salvage(self.root, "retoken").documents
        old = document.in_force[0].id
        body = self.ref_body("retoken", "docs/specs/net/widget.md")
        changes = online.Changes({"title": "Widget framing", "scope": {"basis": "explicit", "builds": ["client:29990003"]}},
                                 salvage_command.DOCUMENT_NOTE.format("docs/specs/net/widget.md"), body)
        self.accept_reference(changes, "REF-900")
        # The new REF accepted, the old one not superseded: two REFs in force for one document is not done.
        _ = run_git(self.root, "checkout", "-q", "retoken")
        (listed,) = salvage(self.root, "retoken").documents
        self.assertEqual([(old, False), ("REF-900", True)], [(item.id, item.holds) for item in listed.in_force])
        self.assertRegex(listed.line(), rf"in force: {old} \(scope: [^;]*; other text; this text newly names: client:29990003\), REF-900 \(scope: client:29990003; this text\);")
        self.assertEqual(1, self.run_salvage("retoken", check=True)[0])
        # The supersession accepted (a revision of REF-900 appending supersedes): done.
        _ = run_git(self.root, "checkout", "-q", "main")
        entry = replay(read_tree(self.root)).entries["REF-900"]
        revised = Record.from_json({**entry.record.to_json(), "supersedes": [old]}, entry.record.body)
        _ = write_change_set(self.root, datetime(2026, 4, 3, tzinfo=UTC), [Revision.of(revised, entry.heads)])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "REF-900 supersedes the old widget REF")
        self.publish()
        status, stdout, _stderr = self.run_salvage("retoken", check=True)
        self.assertEqual((0, True), (status, "0 unsalvaged row(s), 0 pending document(s)" in stdout), stdout)

    def test_a_document_whose_only_ref_is_retired_has_none_in_force(self) -> None:
        records = self.journal_on_main(remove_legacy=False)
        ref = next(record for record in records.values() if record.type == "reference" and record.legacy is not None
                   and record.legacy.path == "docs/specs/net/another.md")
        retired = Record.from_json({**ref.to_json(), "status": "retired", "notes": ["Invented: retired"]}, ref.body)
        _ = write_change_set(self.root, datetime(2026, 4, 4, tzinfo=UTC), [Revision.of(retired, [Revision.of(ref).rev])])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "retire the REF")
        self.publish()
        _ = run_git(self.root, "checkout", "-q", "-b", "after-retire", "main~1")
        spec = self.root / "docs/specs/net/another.md"
        _ = spec.write_text(spec.read_text() + "\nInvented: more.\n", encoding="utf-8")
        _ = run_git(self.root, "commit", "-q", "-am", "edit the spec")
        (document,) = salvage(self.root, "after-retire").documents
        self.assertEqual(((), True, ((ref.id, "retired"),)), (document.in_force, document.main_changed, document.based_on))
        self.assertIn(f"; at this branch's base its reference was {ref.id}: retired;", document.line())
        self.assertIn("pending document; no accepted reference names this path;", document.line())

    def test_a_document_main_added_by_hand_says_main_changed_it(self) -> None:
        # Main added the document (by REF-900) since the branch's base: salvage cannot tell main's text from an earlier state of the branch's.
        scopes: list[tuple[JSON, tuple[str, ...]]] = [({"basis": "explicit", "builds": ["server:29990001"]}, ("server:29990001",)), ({"basis": "explicit"}, ())]
        for scope, tokens in scopes:
            with self.subTest(tokens):
                self.setUp()
                _ = run_git(self.root, "checkout", "-q", "-b", "hand")
                spec = self.root / "docs/specs/net/third.md"
                _ = spec.write_text("# Third\n\n## Status\n\nObserved with server build 29990001.\n", encoding="utf-8")
                _ = run_git(self.root, "add", "-A")
                _ = run_git(self.root, "commit", "-q", "-m", "add a spec")
                _ = self.journal_on_main(remove_legacy=True)
                self.accept_reference(online.Changes({"title": "Third", "scope": scope}, salvage_command.DOCUMENT_NOTE.format("docs/specs/net/third.md"),
                                                     self.ref_body("hand", "docs/specs/net/third.md")), "REF-900")
                _ = run_git(self.root, "checkout", "-q", "hand")
                _ = spec.write_text(spec.read_text() + "\nMore text.\n", encoding="utf-8")
                _ = run_git(self.root, "commit", "-q", "-am", "more text")
                (document,) = salvage(self.root, "hand").documents
                self.assertEqual(((salvage_command.InForce("REF-900", tokens, False),), ("server:29990001",), True),
                                 (document.in_force, document.names, document.main_changed))

    def test_every_ref_in_force_is_listed_with_whether_it_holds_this_text(self) -> None:
        cases: list[tuple[list[tuple[str, JSON, str]], str]] = [
            ([("REF-900", {"basis": "explicit"}, "# Other REF-900\n"), ("REF-901", {"basis": "explicit"}, "# Other REF-901\n")],
             "in force: REF-900 (scope: none; other text; this text newly names: nothing), REF-901 (scope: none; other text; this text newly names: nothing);"),
            ([("REF-900", {"basis": "explicit", "releases": ["3.0.91"]}, "# Same\n"), ("REF-901", {"basis": "explicit", "releases": ["3.0.91", "3.0.92"]}, "# Same\n")],
             "in force: REF-900 (scope: 3.0.91; this text), REF-901 (scope: 3.0.91, 3.0.92; this text);"),
            ([("REF-900", {"basis": "explicit"}, "# Other\n"), ("REF-901", {"basis": "explicit", "releases": ["3.0.91"]}, "# Same\n"),
              ("REF-902", {"basis": "explicit"}, "# Other still\n")],
             "in force: REF-900 (scope: none; other text; this text newly names: nothing), REF-901 (scope: 3.0.91; this text), REF-902 (scope: none; other text; this text newly names: nothing);"),
        ]
        for refs, expected in cases:
            with self.subTest(len(refs)):
                self.setUp()
                self.new_spec_branch("docs/specs/net/third.md", cite=False)
                for identifier, scope, body in refs:
                    self.accept_reference(online.Changes({"title": "Third", "scope": scope}, salvage_command.DOCUMENT_NOTE.format("docs/specs/net/third.md"),
                                                         body), identifier)
                (document,) = salvage(self.root, "specs").documents
                self.assertIn(expected, document.line())
                self.assertTrue(document.main_changed)

    def test_a_row_citing_a_document_several_refs_stand_for_says_which_holds_its_text(self) -> None:
        self.new_spec_branch("docs/specs/net/third.md")
        for identifier, body in (("REF-900", "# Other\n"), ("REF-901", "# Same\n")):
            self.accept_reference(online.Changes({"title": "Third", "scope": {"basis": "explicit"}}, salvage_command.DOCUMENT_NOTE.format("docs/specs/net/third.md"),
                                                 body), identifier)
        result = salvage(self.root, "specs")
        self.assertRegex(result.undecided["M-105"], r"^it cites docs/specs/net/third\.md as this branch has it \(REF-[0-9]+, a provisional number\); "
                         + r"several REFs in force name docs/specs/net/third\.md: REF-900, REF-901 \(REF-901 holds this text\): settle the document first$")
        (document,) = result.documents
        self.assertEqual(("M-105",), document.citing)

    def test_a_revision_keeps_a_citation_only_when_the_ref_in_force_holds_the_branch_s_text(self) -> None:
        # The branch retires the tracked row M-104 (it cites the widget spec's REF); main meanwhile supersedes that REF.
        for body_of, kind in (("same", "revision"), ("other", "undecided")):
            with self.subTest(body_of):
                self.setUp()
                self.edited("retire-m104", ("evidence/widgets/001.jsonl", '"label":"Second bundle","status":"active"', '"label":"Second bundle","status":"retired"'))
                old = next(record.id for record in (entry.record for entry in replay(read_tree(self.root)).entries.values()) if salvage_command.document_path(record) == "docs/specs/net/widget.md")
                body = self.ref_body("retire-m104", "docs/specs/net/widget.md") if body_of == "same" else "# Widget, other text\n"
                self.accept_reference(online.Changes({"title": "Widget", "scope": {"basis": "explicit"}, "supersedes": [old]},
                                                     salvage_command.DOCUMENT_NOTE.format("docs/specs/net/widget.md"), body), "REF-900")
                result = salvage(self.root, "retire-m104")
                self.assertEqual(kind, result.kinds["M-104"], result.undecided)
                if kind == "revision":
                    # Main's record cites the superseded REF; resolved in force it is REF-900, which holds the branch's text.
                    (revision,) = [item for item in result.proposals if item.record.id == "M-104"]
                    self.assertEqual(((old,), "retired"), (revision.record.links.refs, revision.record.status))
                    _ = self.legal(result)

    def test_a_record_citing_a_ref_and_its_successor_is_a_conflict_not_a_crash(self) -> None:
        self.edited("retire-m104", ("evidence/widgets/001.jsonl", '"label":"Second bundle","status":"active"', '"label":"Second bundle","status":"retired"'))
        old = next(record.id for record in (entry.record for entry in replay(read_tree(self.root)).entries.values())
                   if salvage_command.document_path(record) == "docs/specs/net/widget.md")
        self.accept_reference(online.Changes({"title": "Widget", "scope": {"basis": "explicit"}, "supersedes": [old]},
                                             salvage_command.DOCUMENT_NOTE.format("docs/specs/net/widget.md"),
                                             self.ref_body("retire-m104", "docs/specs/net/widget.md")), "REF-900")
        # Main's M-104 now cites both the superseded REF and its successor: equating them would leave a duplicate citation.
        entry = replay(read_tree(self.root)).entries["M-104"]
        both = Record.from_json({**entry.record.to_json(), "refs": [*entry.record.links.refs, "REF-900"]}, entry.record.body)
        _ = write_change_set(self.root, datetime(2026, 4, 5, tzinfo=UTC), [Revision.of(both, entry.heads)])
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "M-104 cites both")
        self.publish()
        result = salvage(self.root, "retire-m104")
        self.assertEqual("conflict", result.kinds["M-104"])

    def test_invariant_f_refuses_a_stored_citation_whose_ref_in_force_holds_other_text(self) -> None:
        old = Record.from_json({"id": "REF-001", "type": "reference", "status": "current", "title": "Spec", "scope": {"basis": "explicit"},
                                "body_sha256": hashlib.sha256(b"# Old\n").hexdigest(), "notes": [salvage_command.DOCUMENT_NOTE.format("docs/specs/x.md")]}, "# Old\n")
        new = Record.from_json({**old.to_json(), "id": "REF-002", "supersedes": ["REF-001"], "body_sha256": hashlib.sha256(b"# New\n").hexdigest()}, "# New\n")
        segments = (Segment("memory/journal/a.jsonl", "", (Revision.of(old), Revision.of(new))),)
        state = replay(segments)
        accepted = salvage_command.Accepted({name: entry.record for name, entry in state.entries.items()},
                                            {name: entry.heads for name, entry in state.entries.items()}, "journal", segments,
                                            frozenset(name for name in state.entries if state.retired(name)))
        data = finding("F-900", "Invented: cites the old REF")
        del data["cites"]
        row = Record.from_json({**data, "refs": ["REF-001"]})
        branch_document = Record.from_json({**old.to_json(), "id": "REF-003", "body_sha256": hashlib.sha256(b"# Branch\n").hexdigest()}, "# Branch\n")
        result = Salvage("branch", "a" * 40, "b" * 40, "c" * 40)
        result.proposals.append(Revision.of(row))
        result.kinds["F-900"] = "new"
        with self.assertRaisesRegex(salvage_command.SalvageError, "F-900: it cites REF-001, in force as REF-002, which holds other text of docs/specs/x.md"):
            salvage_command.invariants(result, ["F-900"], accepted, {"F-900": row, "REF-003": branch_document})

    def test_the_note_line_round_trips_a_path_with_a_space(self) -> None:
        pending = salvage_command.Pending("docs/specs/net/two words.md", "# Two words\n", (), (), (), False)
        written = self.top / "out dir/documents/docs/specs/net/two words.md"
        written.parent.mkdir(parents=True)
        _ = written.write_text("# Two words\n", encoding="utf-8")
        facts = pending.facts(str(written))
        self.assertEqual(f"body written to {shlex.quote(str(written))}", facts[0])
        note = facts[-1].removeprefix("a new reference for it carries ")
        stated = ["propose", "reference", "--set", "title=Two words", "--set-json", 'scope={"basis": "explicit"}', *shlex.split(note),
                  "--body", *shlex.split(facts[0].removeprefix("body written to "))]
        changes = online.changes_from(vars(cli.parse_options(stated)))
        assert changes.note is not None and changes.body is not None
        record = Record.from_json({"id": "REF-900", "type": "reference", "status": "current", "title": "Two words", "scope": {"basis": "explicit"},
                                   "body_sha256": hashlib.sha256(changes.body.encode()).hexdigest(), "notes": [changes.note]}, changes.body)
        self.assertEqual("docs/specs/net/two words.md", salvage_command.document_path(record))

    def test_a_new_document_clears_once_a_ref_carrying_the_note_is_accepted(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "new-doc")
        _ = (self.root / "docs/specs/net/third.md").write_text("# Third spec\n\n## Status\n\nObserved with server build 29990001.\n\n"
                                                               + "## Scope\n\nInvented: new.\n\n## Notes\n\nSeen again on build 29990009.\n",
                                                               encoding="utf-8")
        # A tracked evidence row on the branch points at the new document: it cites the REF that document becomes.
        shard = self.root / "evidence/widgets/001.jsonl"
        _ = shard.write_text(shard.read_text() + json.dumps({"id": "M-105", "path": "docs/specs/net/third.md", "sha256": "d" * 64, "class": "tracked",
                                                             "kind": "report", "date": "2026-01-07", "label": "The new spec", "status": "active"},
                                                            separators=(",", ":")) + "\n")
        _ = run_git(self.root, "add", "-A")
        _ = run_git(self.root, "commit", "-q", "-m", "add a spec")
        _ = self.journal_on_main(remove_legacy=True)
        first = salvage(self.root, "new-doc")
        self.assertEqual("undecided", first.kinds["M-105"])
        self.assertIn("it cites the document docs/specs/net/third.md, which no accepted REF names", first.undecided["M-105"])
        status, stdout, _stderr = self.run_salvage("new-doc", check=True)
        self.assertEqual(1, status)
        facts = ["no accepted reference names this path", "this branch changed it since its base: yes", "the exporter reads as its scope, from its Status and Scope sections: server:29990001", "its body also names: any:29990009",
                 "at this branch's base its reference was none (the document is new on this branch)", "main changed or replaced its reference since this branch's base: no", "rows held because they cite it: M-105",
                 "a new reference for it carries --note 'salvaged document: docs/specs/net/third.md'"]
        self.assertIn("new-doc: docs/specs/net/third.md: pending document; " + "; ".join(facts) + "\n", stdout)
        out = self.top / "out"
        _, _stdout, stderr = self.run_salvage("new-doc", out=out)
        written = out / "documents/docs/specs/net/third.md"
        self.assertEqual(self.ref_body("new-doc", "docs/specs/net/third.md").encode(), written.read_bytes())
        lines = [line.split(": ", 2)[2] for line in stderr.splitlines() if line.startswith("memory salvage: docs/specs/net/third.md: ")]
        self.assertEqual([f"body written to {written}", *facts], lines)
        # The operator states a scope and proposes the body with the note: the document is done and the row cites the new REF.
        stated = ["propose", "reference", "--set", "title=Third", "--set-json", '{"basis": "explicit", "builds": ["server:29990001"]}'.join(("scope=", "")),
                  *shlex.split(facts[-1].removeprefix("a new reference for it carries ")), "--body", str(written)]
        self.accept_reference(online.changes_from(vars(cli.parse_options(stated))), "REF-900")
        status, stdout, _stderr = self.run_salvage("new-doc", check=True)
        self.assertIn("0 pending document(s)", stdout)
        # Run 2: the document maps to the REF proposed by hand (by its "salvaged document:" note), so the row citing it is proposed with that ID.
        second = salvage(self.root, "new-doc")
        self.assertEqual("new", second.kinds["M-105"])
        self.assertEqual(("REF-900",), next(item.record for item in second.proposals if item.record.id == "M-105").links.refs)

    def outs_branch(self) -> None:
        """A branch changing a spec and adding a row: --out writes one document body and one segment."""
        _ = run_git(self.root, "checkout", "-q", "-b", "outs")
        spec = self.root / "docs/specs/net/another.md"
        _ = spec.write_text(spec.read_text() + "\nInvented: one more line.\n", encoding="utf-8")
        ledger = self.root / "docs/ledgers/findings/widgets.md"
        _ = ledger.write_text(ledger.read_text() + "| F-111 | New branch claim | observed | review | high; x | | active |\n")
        _ = run_git(self.root, "commit", "-q", "-am", "a spec and a row")
        _ = self.journal_on_main(remove_legacy=True)

    def test_out_must_name_a_new_directory(self) -> None:
        self.outs_branch()
        for case in ("an empty directory", "a directory with content", "a file", "a dangling symlink"):
            with self.subTest(case):
                out = self.top / case.replace(" ", "-")
                if case == "a file":
                    _ = out.write_text("x", encoding="utf-8")
                elif case == "a dangling symlink":
                    out.symlink_to(self.top / "nowhere")
                else:
                    out.mkdir()
                    if case == "a directory with content":
                        _ = (out / "kept").write_text("x", encoding="utf-8")
                before = sorted(str(path) for path in self.top.rglob("*") if path.is_relative_to(out)) if out.is_dir() else []
                status, _stdout, stderr = self.run_salvage("outs", out=out)
                self.assertEqual(1, status)
                self.assertIn(f"memory salvage: {out} exists: --out must name a new directory", stderr)
                self.assertEqual(before, sorted(str(path) for path in self.top.rglob("*") if path.is_relative_to(out)) if out.is_dir() else [])
        out = self.top / "new"
        self.assertNotEqual(1, self.run_salvage("outs", out=out)[0])
        self.assertEqual(2, len([path for path in out.rglob("*") if path.is_file()]), "one document body and one segment")
        self.assertEqual(0o700, out.stat().st_mode & 0o777)

    def test_out_is_removed_when_a_step_inside_it_fails(self) -> None:
        self.outs_branch()
        real_mkdir = os.mkdir

        def failing_mkdir(path: str | os.PathLike[str], mode: int = 0o777) -> None:
            if str(path).endswith("memory"):
                raise PermissionError(13, "Permission denied", str(path))
            real_mkdir(path, mode)

        def failing_fsync(_descriptor: int) -> None:
            raise OSError(28, "No space left on device")

        for case, target, replacement in (("a subdirectory", "tools.memory.salvage.os.mkdir", failing_mkdir),
                                          ("an fsync", "tools.memory.salvage.os.fsync", failing_fsync)):
            with self.subTest(case):
                out = self.top / f"out-{case.split()[-1]}"
                with mock.patch(target, replacement):
                    status, _stdout, stderr = self.run_salvage("outs", out=out)
                self.assertEqual(1, status)
                self.assertIn(f"removed {out}, which this run had created", stderr)
                self.assertFalse(os.path.lexists(out))

    def test_out_is_removed_on_an_interrupt_and_a_failed_removal_is_reported(self) -> None:
        self.outs_branch()
        out = self.top / "interrupted"

        def interrupted(_descriptor: int) -> None:
            raise KeyboardInterrupt

        with mock.patch("tools.memory.salvage.os.fsync", interrupted), self.assertRaises(KeyboardInterrupt):
            _ = self.run_salvage("outs", out=out)
        self.assertFalse(os.path.lexists(out), "the directory is gone and the interrupt propagates")

        def no_space(_descriptor: int) -> None:
            raise OSError(28, "No space left on device")

        def stuck(path: str | os.PathLike[str]) -> None:
            raise PermissionError(13, "Permission denied", str(path))

        out = self.top / "stuck"
        with mock.patch("tools.memory.salvage.os.fsync", no_space), mock.patch("tools.memory.salvage.shutil.rmtree", stuck):
            status, _stdout, stderr = self.run_salvage("outs", out=out)
        self.assertEqual(1, status)
        self.assertIn(f"could not remove {out}: [Errno 13] Permission denied", stderr)
        self.assertIn("remove it by hand", stderr)
        self.assertNotIn(f"removed {out}", stderr)

    def test_an_interrupt_right_after_the_mkdir_removes_the_directory(self) -> None:
        class Interrupting(list[tuple[str, bytes]]):
            """What ``--out`` would write; iterating it, the first statement after the mkdir, is interrupted."""

            @override
            def __iter__(self) -> Iterator[tuple[str, bytes]]:
                raise KeyboardInterrupt

        out = self.top / "right-after"
        with self.assertRaises(KeyboardInterrupt):
            salvage_command.publish(out, Interrupting())
        self.assertFalse(os.path.lexists(out))
        # The window Python cannot close: the mkdir returned and the interrupt came before the next statement. The
        # directory is left empty, and the next run refuses it by name.
        real = os.mkdir

        def made_then_interrupted(path: str | os.PathLike[str], mode: int = 0o777) -> None:
            real(path, mode)
            raise KeyboardInterrupt

        out = self.top / "window"
        with mock.patch("tools.memory.salvage.os.mkdir", made_then_interrupted), self.assertRaises(KeyboardInterrupt):
            salvage_command.publish(out, [])
        self.assertEqual([], os.listdir(out))
        with self.assertRaises(salvage_command.PublishError) as refused:
            salvage_command.publish(out, [])
        self.assertEqual(f"{out} exists: --out must name a new directory", str(refused.exception))

    def test_after_its_salvage_is_merged_a_merged_tip_has_nothing_left(self) -> None:
        _ = run_git(self.root, "checkout", "-q", "-b", "runbook")
        ledger = self.root / "docs/ledgers/findings/widgets.md"
        _ = ledger.write_text(ledger.read_text() + "| F-111 | New branch claim | observed | review | high; x | | active |\n")
        _ = run_git(self.root, "commit", "-q", "-am", "add a row")
        _ = self.journal_on_main(remove_legacy=True)
        first = salvage(self.root, "runbook")
        self.assertEqual(["F-111"], [item.record.id for item in first.proposals])
        self.accept(first)
        _ = run_git(self.root, "checkout", "-q", "runbook")
        with self.assertRaises(subprocess.CalledProcessError):
            _ = run_git(self.root, "merge", "-q", "--no-edit", "main")
        _ = run_git(self.root, "rm", "-r", "-q", "--ignore-unmatch", "docs/ledgers", "evidence")
        _ = run_git(self.root, "commit", "-q", "--no-edit")
        status, stdout, stderr = self.run_salvage("runbook")
        self.assertEqual((0, ""), (status, stdout), stderr)
        self.assertIn("leaves nothing to salvage", stderr)

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
