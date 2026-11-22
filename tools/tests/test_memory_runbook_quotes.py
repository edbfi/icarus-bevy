"""The salvage text the runbook quotes, pinned on both sides.

The runbook ("Worktrees that predate the cut-over") says what each line ``memory salvage`` prints means; the two drifted
apart in several review rounds. Each quoted fragment must occur in the runbook (whitespace-normalised) and in what
salvage prints: facts produced by ``Pending.facts``, refusals and held-row reasons matched in its source. A held row
prints ``needs a decision by hand: <ID>: <reason>; not proposed`` and the runbook reads the action from the words the
reason ends with: fixture runs check one held row for each of three endings; the fourth, ``propose it alone first``,
is checked against the run's own result by ``assert_endings`` in ``test_memory_salvage_properties.py``. A wording
changed on either side fails here.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

from tools.memory import salvage
from tools.memory.salvage import InForce, Pending
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
          "needs a superseding record", "needs a decision by hand", "holds other content under this ID", *ENDINGS,
          "its content will be recorded under a new ID", "this row's link must name that ID",
          "it is reconciled by hand, under its ID or as a new record", "refer to each other and none can be accepted alone")
# The owner causes ``owner_cause`` gives a row citing a document (rules 1 and 2).
CAUSES = ("main withdrew the document it cites", "and main has since changed it", "and main has since withdrawn it")


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
    unchanged_withdrawn = Pending("<path>", "", (), (), (), False, branch_changed=False, based_on=(("REF-N", "retired"),))
    causes = [salvage.owner_cause(shape) or "" for shape in (*shapes, unchanged_withdrawn)]
    return flat("\n".join([*(line for shape in shapes for line in shape.facts("<file>")), *causes]))


def held_lines(stderr: str, ref: str) -> dict[tuple[str, str], str]:
    return {(ref, line.split(": ")[1]): line for line in stderr.splitlines() if line.startswith("needs a decision by hand: ")}


class RunbookQuotes(unittest.TestCase):
    def test_every_quoted_fragment_is_in_the_runbook_and_in_what_salvage_prints(self) -> None:
        runbook = flat((ROOT / "docs/runbooks/memory-service.md").read_text(encoding="utf-8"))
        source = flat(Path(salvage.__file__).read_text(encoding="utf-8"))
        facts = printed()
        for fragment in (*FACTS, *CAUSES, *SOURCE):
            with self.subTest(runbook=fragment):
                self.assertTrue(fragment in runbook, f"the runbook no longer quotes {fragment!r}")
        for fragment in (*FACTS, *CAUSES):
            with self.subTest(printed=fragment):
                self.assertTrue(fragment in facts, f"salvage no longer prints {fragment!r}")
        for fragment in SOURCE:
            with self.subTest(source=fragment):
                self.assertTrue(fragment in source, f"salvage's source no longer says {fragment!r}")

    def test_a_held_row_ends_with_its_action_before_not_proposed(self) -> None:
        case = legacy_tests.SalvageTests("run")  # its fixtures, not its tests
        case.setUp()
        case.new_spec_branch("docs/specs/net/third.md")  # M-105 cites a document no accepted REF names yet
        held = held_lines(case.run_salvage("specs")[2], "specs")
        case.doCleanups()
        case.setUp()  # F-111 cites a member M-101 does not list; F-112 refers to F-111, which this run holds
        case.edited("chain", ("docs/ledgers/findings/widgets.md", "| F-107 |",
                              "| F-111 | Invented: a bad selector | observed | review; M-101/ev_000000000000 | high; server 29990001 | | active |\n"
                              + "| F-112 | Invented: refers to F-111 | observed | review; F-111 | high; server 29990001 | | active |\n| F-107 |"))
        held |= held_lines(case.run_salvage("chain")[2], "chain")
        case.doCleanups()
        for ref, name, ending in (("specs", "M-105", "settle the document first"), ("chain", "F-112", "must be accepted first"),
                                  ("chain", "F-111", "decide by hand")):
            with self.subTest(ending=ending):
                self.assertTrue(held[(ref, name)].endswith(f"{ending}; not proposed"), held[(ref, name)])

    def test_the_runbook_section_keeps_the_heading_salvage_names(self) -> None:
        self.assertIn('section "Worktrees that predate the cut-over"', flat(Path(salvage.__file__).read_text(encoding="utf-8")))
        self.assertIn("\n## Worktrees that predate the cut-over\n", (ROOT / "docs/runbooks/memory-service.md").read_text(encoding="utf-8"))


if __name__ == "__main__":
    _ = unittest.main()
