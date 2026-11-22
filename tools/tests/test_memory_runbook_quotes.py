"""The salvage text the runbook quotes, pinned on both sides.

``docs/runbooks/memory-service.md`` ("Worktrees that predate the cut-over") tells the operator what each line
``memory salvage`` prints means. The two drifted apart in several review rounds, so every fragment it quotes is listed
here and must occur in the runbook (whitespace-normalised, since it wraps lines) and in what salvage prints: the
document facts are produced by ``Pending.facts``; refusals and held-row reasons, built inside ``salvage``, are matched
in its source. A wording changed on either side fails this test.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path

from tools.memory import salvage
from tools.memory.salvage import InForce, Pending

ROOT = Path(__file__).resolve().parents[2]

FACTS = ("body written to", "in force:", "no accepted reference names this path", "this branch changed it since its base: no",
         "rows held because they cite it", "scope tokens", "the exporter reads as its scope", "this text newly names: nothing",
         "at this branch's base its reference was REF-N: retired", "superseded by", "none the accepted journal holds",
         "none (the document is new on this branch)", "main changed or replaced its reference since this branch's base: yes",
         "--note 'salvaged document: <path>'")
SOURCE = ("--out must name a new directory", "could not remove", "remove it by hand", "internal error, nothing written",
          "needs a superseding record", "needs a decision by hand", "holds other content under this ID",
          "which no accepted REF names (by legacy path or a note", "that record must be accepted first", "propose it alone first",
          "as this branch has it", "which holds other text of", "which no longer stands for", "several REFs in force name",
          ": settle the document first", "which the accepted journal holds", "records sharing the",
          "a rename needs a superseding record", "which would carry the link")


def flat(text: str) -> str:
    return re.sub(r"\s+", " ", text)


def printed() -> str:
    """Fact lines for every shape a pending document takes."""
    one = InForce("REF-N", ("3.0.37",), False, ())
    shapes = [
        Pending("<path>", "", ("3.0.37",), ("3.0.36",), (one,), True, ("F-1",), False, (("REF-N", "retired"),)),
        Pending("<path>", "", (), (), (), False, (), True, (("REF-N", "superseded by REF-M"),), from_sections=False),
        Pending("<path>", "", (), (), (), False, (), True, ()),
        Pending("<path>", "", (), (), (), False, (), True, None),
    ]
    return flat("\n".join(line for shape in shapes for line in shape.facts("<file>")))


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

    def test_the_runbook_section_keeps_the_heading_salvage_names(self) -> None:
        self.assertIn('section "Worktrees that predate the cut-over"', flat(Path(salvage.__file__).read_text(encoding="utf-8")))
        self.assertIn("\n## Worktrees that predate the cut-over\n", (ROOT / "docs/runbooks/memory-service.md").read_text(encoding="utf-8"))


if __name__ == "__main__":
    _ = unittest.main()
