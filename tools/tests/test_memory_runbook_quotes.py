"""The salvage text the runbook quotes, pinned on both sides.

The runbook ("Worktrees that predate the cut-over") says what each line ``memory salvage`` prints means; the two drifted
apart in several review rounds. Each quoted fragment must occur in the runbook (whitespace-normalised) and in what
salvage prints: facts produced by ``Pending.facts``, refusals and held-row reasons matched in its source. A held row
prints ``needs a decision by hand: <ID>: <reason>; not proposed`` and the runbook reads the action from the words the
reason ends with, so a fixture run checks one held row per ending. A wording changed on either side fails here.
"""
from __future__ import annotations

import json
import re
import unittest
from pathlib import Path

from tools.memory import salvage
from tools.memory.salvage import InForce, Pending
from tools.tests.memory_fixtures import run_git
from tools.tests import test_memory_legacy as legacy_tests

ROOT = Path(__file__).resolve().parents[2]

FACTS = ("body written to", "in force:", "no accepted reference names this path", "this branch changed it since its base: no",
         "rows held because they cite it", "scope tokens", "the exporter reads as its scope", "this text newly names: nothing",
         "at this branch's base its reference was REF-N: retired", "superseded by", "none the accepted journal holds",
         "none (the document is new on this branch)", "main changed or replaced its reference since this branch's base: yes",
         "--note 'salvaged document: <path>'", "references that named this path and are no longer in force")
# The runbook groups held-row reasons by the words they end with; each names its action.
ENDINGS = ("must be accepted first", "propose it alone first", "settle the document first", "decide by hand")
SOURCE = ("--out must name a new directory", "could not remove", "remove it by hand", "internal error, nothing written",
          "needs a superseding record", "needs a decision by hand", "holds other content under this ID", *ENDINGS)


def flat(text: str) -> str:
    return re.sub(r"\s+", " ", text)


def printed() -> str:
    """Fact lines for every shape a pending document takes."""
    one = InForce("REF-N", ("3.0.37",), False, ())
    shapes = [
        Pending("<path>", "", ("3.0.37",), ("3.0.36",), (one,), True, ("F-1",), False, (("REF-N", "retired"),)),
        Pending("<path>", "", (), (), (), False, (), True, (("REF-N", "superseded by REF-M"),), from_sections=False,
                withdrawn=(("REF-M", "retired"),)),
        Pending("<path>", "", (), (), (), False, (), True, ()),
        Pending("<path>", "", (), (), (), False, (), True, None),
    ]
    return flat("\n".join(line for shape in shapes for line in shape.facts("<file>")))


def held_lines(stderr: str, ref: str) -> dict[tuple[str, str], str]:
    return {(ref, line.split(": ")[1]): line for line in stderr.splitlines() if line.startswith("needs a decision by hand: ")}


class RunbookQuotes(unittest.TestCase):
    def test_every_quoted_fragment_is_in_the_runbook_and_in_what_salvage_prints(self) -> None:
        runbook = flat((ROOT / "docs/runbooks/memory-service.md").read_text(encoding="utf-8"))
        source = flat(Path(salvage.__file__).read_text(encoding="utf-8"))
        facts = printed()
        for fragment in FACTS:
            with self.subTest(fact=fragment):
                self.assertTrue(fragment in runbook, f"the runbook no longer quotes {fragment!r}")
                self.assertTrue(fragment in facts, f"salvage no longer prints {fragment!r}")
        for fragment in SOURCE:
            with self.subTest(source=fragment):
                self.assertTrue(fragment in runbook, f"the runbook no longer quotes {fragment!r}")
                self.assertTrue(fragment in source, f"salvage's source no longer says {fragment!r}")

    def test_a_held_row_ends_with_its_action_before_not_proposed(self) -> None:
        case = legacy_tests.SalvageTests("run")  # its fixtures, not its tests
        cases: list[tuple[str, str, str]] = []
        case.setUp()
        case.new_spec_branch("docs/specs/net/third.md")  # M-105 cites a document no accepted REF names yet
        cases.append(("specs", "M-105", "must be accepted first"))
        held = held_lines(case.run_salvage("specs")[2], "specs")
        case.doCleanups()
        case.setUp()  # M-105 cites the spec as the branch has it; main has since changed that spec
        _ = run_git(case.root, "checkout", "-q", "-b", "older")
        shard = case.root / "evidence/widgets/001.jsonl"
        _ = shard.write_text(shard.read_text() + json.dumps({"id": "M-105", "path": "docs/specs/net/another.md", "sha256": "d" * 64, "class": "tracked",
                                                             "kind": "report", "date": "2026-01-07", "label": "As it read", "status": "active"}) + "\n")
        _ = run_git(case.root, "commit", "-q", "-am", "evidence for the spec")
        _ = case.journal_on_main(remove_legacy=True)
        case.revise_spec_on_main(case.ref_body("main~1", "docs/specs/net/another.md") + "\nInvented: main's text.\n")
        cases.append(("older", "M-105", "settle the document first"))
        held |= held_lines(case.run_salvage("older")[2], "older")
        case.doCleanups()
        case.setUp()  # two compatibility rows share one name
        table, row = "docs/ledgers/compatibility.md", "| Discovery | Unknown | Browser behavior |\n"
        case.edited("dup", (table, row, row + "| Dup | Unknown | One |\n| Dup | Unknown | Two |\n"))
        dup = held_lines(case.run_salvage("dup")[2], "dup")
        held |= dup
        cases.append(("dup", next(name for ref, name in dup if ref == "dup"), "decide by hand"))
        case.doCleanups()
        for ref, name, ending in cases:
            with self.subTest(ending=ending):
                self.assertTrue(held[(ref, name)].endswith(f"{ending}; not proposed"), held[(ref, name)])

    def test_the_runbook_section_keeps_the_heading_salvage_names(self) -> None:
        self.assertIn('section "Worktrees that predate the cut-over"', flat(Path(salvage.__file__).read_text(encoding="utf-8")))
        self.assertIn("\n## Worktrees that predate the cut-over\n", (ROOT / "docs/runbooks/memory-service.md").read_text(encoding="utf-8"))


if __name__ == "__main__":
    _ = unittest.main()
