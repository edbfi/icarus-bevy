# SPDX-License-Identifier: AGPL-3.0-or-later
"""Record model and canonical form of the memory tool, on invented records."""

from __future__ import annotations

import hashlib
import random
import unittest

from tools.memory.canonical import TEXT_LIMIT, RenderError, canonical_json, entities, record_sha256, render_text, tags
from tools.memory.model import JSON, Record, RecordError, load_json, string_problem
from tools.tests.memory_fixtures import MEMBER, chain, evidence, explicit, finding

MIGRATED: dict[str, JSON] = {
    "id": "F-201", "type": "finding", "status": "legacy:superseded-by F-202", "claim": "Invented legacy claim",
    "kind": "legacy:unknown", "confidence": "legacy:bounded", "area": "listing",
    "scope": {"basis": "legacy-text", "builds": ["any:90000001", "client:90000003"], "releases": ["9.0.1"], "commit": "a1b2c3d4"},
    "gaps": ["standing"], "cites": ["M-170/E-010"],
    "legacy": {"path": "docs/ledgers/findings/invented.md", "cells": {"status": "superseded-by F-202", "unknowns": ""},
               "superseded_by": ["F-202"], "implied_status": "superseded"},
}


def parse(data: dict[str, JSON], body: str | None = None) -> Record:
    return Record.from_json(data, body)


def shuffled(value: JSON, rng: random.Random) -> JSON:
    if isinstance(value, dict):
        keys = list(value)
        rng.shuffle(keys)
        return {key: shuffled(value[key], rng) for key in keys}
    if isinstance(value, list):
        return [shuffled(item, rng) for item in value]
    return value


class RecordTests(unittest.TestCase):
    def assert_rejected(self, data: dict[str, JSON], *fragments: str, body: str | None = None) -> None:
        with self.assertRaises(RecordError) as caught:
            _ = parse(data, body)
        for fragment in fragments:
            self.assertIn(fragment, str(caught.exception))

    def test_every_chain_record_round_trips(self) -> None:
        for item in chain():
            record = parse(item)
            self.assertEqual(record, parse(record.to_json()))
            self.assertEqual(canonical_json(record.to_json()), canonical_json(parse(record.to_json()).to_json()))

    def test_messages_name_the_record_and_field(self) -> None:
        data = finding("F-101", "claim")
        del data["claim"]
        self.assert_rejected(data, "F-101: claim: required field missing")
        self.assert_rejected(finding("F-101", "claim", kind="guess"), "F-101: kind:", "'guess'")
        self.assert_rejected(finding("F-101", "claim", kind="legacy:guess"), "F-101: kind:", "migrated")
        self.assert_rejected(finding("F-101", "claim", colour="red"), "F-101: colour: unknown field")
        self.assert_rejected(finding("F-101", "claim", type="unknown"), "F-101: type:")
        self.assert_rejected(finding("F-101", "claim", status="superseded"), "F-101: status:")
        self.assert_rejected(finding("F-101", " padded"), "F-101: claim: must be one trimmed line")
        self.assert_rejected(finding("F-101", "claim", refs=["F-101"]), "F-101: refs: a record cannot link to itself")
        self.assert_rejected(finding("F-101", "claim", supersedes=["U-101"]), "F-101: supersedes: U-101 has the wrong type")
        self.assert_rejected(finding("F-101", "claim", refs=[]), "F-101: refs: must be a non-empty array")
        self.assert_rejected(finding("F-101", "claim", refs=["F-102", "F-102"]), "F-101: refs: duplicate entries")
        self.assert_rejected(finding("F-0001", "claim"), "record: id:")

    def test_links_and_scope_sort_naturally(self) -> None:
        record = parse(finding("F-101", "claim", refs=["F-1000", "F-999", "U-101"], scope=explicit("server:2", "client:1")))
        self.assertEqual(("F-999", "F-1000", "U-101"), record.links.refs)
        self.assertEqual(("client:1", "server:2"), record.scope.builds if record.scope else ())

    def test_new_record_rules(self) -> None:
        self.assert_rejected(finding("F-101", "claim", kind="derived"), "F-101: refs: a derived or inferred finding names its sources")
        self.assert_rejected(finding("F-101", "claim", kind="decision", scope=explicit(subjects=("decision",))), "F-101: instruction")
        decision = parse(finding(
            "F-101", "Owner decided invented policy", kind="decision", scope=explicit(subjects=("decision",)),
            instruction="Use the invented policy.", instruction_date="2026-11-20", instruction_source="invented thread",
        ))
        self.assertEqual("Use the invented policy.", decision.text("instruction"))
        self.assert_rejected(finding("F-101", "claim", instruction="x"), "F-101: instruction: only a decision")
        self.assert_rejected(finding("F-101", "claim", scope=explicit(subjects=("native",))), "F-101: scope.commit")
        self.assert_rejected(finding("F-101", "claim", scope=explicit("any:1")), "F-101: scope.builds")
        self.assert_rejected(finding("F-101", "claim", scope={"basis": "legacy-text"}), "F-101: scope.basis")
        self.assert_rejected(finding("F-101", "claim", gaps=["standing"]), "F-101: gaps: only a migrated record")

    def test_migrated_record_keeps_legacy_values_and_gaps(self) -> None:
        record = parse(MIGRATED)
        self.assertTrue(record.migrated)
        self.assertEqual("legacy:bounded", record.text("confidence"))
        self.assertEqual(("standing",), record.gaps)
        self.assertEqual(("F-202",), record.legacy.superseded_by if record.legacy else ())
        self.assertEqual(MIGRATED, record.to_json())
        present = dict(MIGRATED, standing="candidate")
        self.assert_rejected(present, "F-201: gaps: 'standing' is listed as missing but present")
        missing = dict(MIGRATED)
        del missing["gaps"]
        self.assert_rejected(missing, "F-201: standing: required field missing (list it in gaps")

    def test_legacy_evidence_shapes_are_carried(self) -> None:
        row = evidence("M-151", "reference-data/", kind="input", gaps=["date", "sha256", "size"],
                       legacy={"path": "evidence/invented/001.jsonl", "cells": {"row": '{"id":"M-151","sha256":null}'}})
        for name in ("sha256", "size", "date"):
            del row[name]
        record = parse(row)
        self.assertEqual("reference-data/", record.text("path"))
        bundle = parse(evidence("M-103", "work/x/rea-bundle.json", kind="rea-bundle", members=["ev_0123456789ab", MEMBER]))
        self.assertEqual(("ev_0123456789ab", MEMBER), bundle.items("members"))
        self.assert_rejected(evidence("M-103", "work/x", members=[MEMBER]), "M-103: members: only a rea-bundle")
        self.assert_rejected(evidence("M-103", "work/x", members=["ev_0123"], kind="rea-bundle"), "M-103: members: invalid entry")

    def test_assigned_at_migration_marks_exactly_migrated_cp_and_rp(self) -> None:
        legacy: JSON = {"path": "docs/ledgers/compatibility.md", "cells": {"surface": "Invented surface"}}
        surface: dict[str, JSON] = {"id": "CP-001", "type": "compatibility", "status": "legacy:Unknown", "name": "Invented surface",
                                    "gaps": ["qualifier", "required_proof", "scope"], "legacy": legacy}
        self.assert_rejected(surface, "CP-001: assigned_at_migration")
        self.assertTrue(parse(dict(surface, assigned_at_migration=True)).assigned_at_migration)
        self.assert_rejected(dict(MIGRATED, assigned_at_migration=True), "F-201: assigned_at_migration")

    def test_run_evidence(self) -> None:
        run: dict[str, JSON] = {"workload": "invented", "targets": ["native"], "matched_reference": "none", "identities": ["M-102"],
                                "start": "2026-11-20T10:00:00Z", "end": "2026-11-20T10:05:00Z", "environment": "loopback",
                                "thresholds": ["p99 < 5 ms"], "outcome": "pass"}
        record = parse(evidence("M-104", "work/x/run.json", kind="run", run=run))
        self.assertEqual("pass", record.run["outcome"] if record.run else None)
        self.assert_rejected(evidence("M-104", "work/x/run.json", kind="run"), "M-104: run: present exactly on run evidence")
        self.assert_rejected(evidence("M-104", "work/x/run.json", kind="run", run=dict(run, outcome="flaky")), "M-104: run.outcome:")
        self.assert_rejected(evidence("M-104", "work/x/run.json", kind="run", run=dict(run, start="today")), "M-104: run.start:")

    def test_strings_refuse_what_hindsight_would_strip(self) -> None:
        for code in range(0x00, 0xA0):
            character = chr(code)
            refused = string_problem(f"a{character}b") is not None
            self.assertEqual(code < 0x20 and character not in "\t\n" or 0x7F <= code <= 0x9F, refused, hex(code))
        self.assert_rejected(finding("F-101", "a\x7fb"), "F-101: claim: control character U+007F")
        self.assert_rejected(finding("F-101", "a\rb"), "F-101: claim: control character U+000D")
        self.assert_rejected(finding("F-101", "café"), "F-101: claim: not NFC-normalised")
        with self.assertRaises(RecordError):
            _ = load_json('{"a":1.5}')
        with self.assertRaises(RecordError):
            _ = load_json('{"a":1,"a":2}')


class CanonicalTests(unittest.TestCase):
    def test_canonical_json_is_sorted_compact_utf8_and_order_independent(self) -> None:
        value: JSON = {"b": [1, {"z": "é", "a": True}], "a": "x"}
        self.assertEqual('{"a":"x","b":[1,{"a":true,"z":"é"}]}'.encode(), canonical_json(value))
        rng = random.Random(7)
        for item in chain():
            expected = canonical_json(item)
            for _ in range(5):
                self.assertEqual(expected, canonical_json(shuffled(item, rng)))
            once = canonical_json(parse(item).to_json())
            self.assertEqual(once, canonical_json(Record.from_json(shuffled(parse(item).to_json(), rng)).to_json()))

    def test_digest_ignores_input_key_order_and_follows_content(self) -> None:
        item = chain()[0]
        record = parse(item)
        self.assertEqual(record_sha256(record), record_sha256(Record.from_json(shuffled(item, random.Random(3)))))
        self.assertNotEqual(record_sha256(record), record_sha256(parse(dict(item, claim="Invented: different"))))

    def test_canonical_json_refuses_control_characters_and_non_nfc(self) -> None:
        for text in ("a\x00", "a\x1b", "a\x7f", "a\r", "é"):
            with self.assertRaises(RenderError):
                _ = canonical_json({"text": text})
        self.assertEqual(b'{"t":"a\\tb\\nc"}', canonical_json({"t": "a\tb\nc"}))

    def test_text_leads_with_id_type_status_and_claim_and_omits_members(self) -> None:
        bundle = parse(chain()[6])
        text = render_text(bundle)
        self.assertTrue(text.startswith("M-101 evidence active\nInvented receipt\n"))
        self.assertNotIn(MEMBER[:15], text)
        claim = render_text(parse(chain()[1]))
        self.assertEqual(["F-102 finding active", "Invented: any `L_x` marks a listing locked"], claim.split("\n")[:2])
        self.assertNotIn("\r", claim)
        self.assertEqual(render_text(parse(chain()[1])), claim)
        with self.assertRaises(RenderError):
            _ = render_text(parse(finding("F-101", "x" * TEXT_LIMIT)))
        self.assertEqual(TEXT_LIMIT, 1800)

    def test_reference_text_is_its_body_and_json_holds_only_its_digest(self) -> None:
        body = "# Invented spec\n\nThe `Handshake` reads `Cookie`.\n"
        data: dict[str, JSON] = {"id": "REF-001", "type": "reference", "status": "current", "title": "Invented spec",
                                 "scope": explicit(subjects=("tooling",)),
                                 "body_sha256": "8a2e3e5a1f04f1f0b06c0e8de6d77b1a2e1f2a3b4c5d6e7f8091a2b3c4d5e6f7"}
        self.assert_rejected_body(data, body)
        record = Record.from_json(dict(data, body_sha256=hashlib.sha256(body.encode()).hexdigest()), body)
        self.assertEqual(body, render_text(record))
        self.assertNotIn("body", record.to_json())
        self.assertIn("Handshake", entities(record))
        long_body = body + "x" * (TEXT_LIMIT * 2)
        long_record = Record.from_json(dict(data, body_sha256=hashlib.sha256(long_body.encode()).hexdigest()), long_body)
        self.assertEqual(long_body, render_text(long_record))

    def assert_rejected_body(self, data: dict[str, JSON], body: str) -> None:
        with self.assertRaises(RecordError) as caught:
            _ = Record.from_json(data, body)
        self.assertIn("REF-001: body_sha256: does not match the body", str(caught.exception))
        with self.assertRaises(RecordError):
            _ = Record.from_json(data)

    def test_tags_and_entities(self) -> None:
        record = parse(chain()[2])
        self.assertEqual(
            ("area:listing", "basis:explicit", "build:client:90000003", "build:server:90000002", "id:F-103", "seg:s1",
             "state:accepted", "status:active", "subject:original-server", "type:finding"),
            tags(record, state="accepted", status="active", segment="s1"),
        )
        with self.assertRaises(RenderError):
            _ = tags(record, state="prior", status="active", segment="s1")
        found = entities(parse(chain()[1]))
        for expected in ("F-101", "U-101", "M-101", "M-101/ev_a1b2c3d4e5f6", "M-102", "L_x", "server:90000001"):
            self.assertIn(expected, found)
        self.assertEqual(sorted(found), list(found))


if __name__ == "__main__":
    _ = unittest.main()
