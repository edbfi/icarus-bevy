# SPDX-License-Identifier: AGPL-3.0-or-later
"""``tools/docs_check.py`` on an invented repository."""

from __future__ import annotations

import os
import tempfile
import unittest
from pathlib import Path
from typing import override

from tools import docs_check
from tools.tests.memory_fixtures import clean_git_environment, new_repository


class DocsCheckTests(unittest.TestCase):
    root: Path = Path()

    @override
    def setUp(self) -> None:
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory())) / "repo"
        self.root.mkdir()
        self.enterContext(clean_git_environment())
        new_repository(self.root)
        self.put("docs/plan.md", "# Plan\n\n## Release tracking\n\nSee [gates](gates.md) and [notes](runbooks/).\n")
        self.put("docs/gates.md", "# Gates\n")
        self.put("docs/runbooks/a.md", "# A\n\n[plan](../plan.md#release-tracking), `[not a link](missing.md)`\n\n```\n[fenced](missing.md)\n```\n")
        self.put("CLAUDE.md", "Read `docs/plan.md` and docs/plan.md#release-tracking.\n")
        os.symlink("CLAUDE.md", self.root / "AGENTS.md")
        self.put("src/lib.rs", "// see docs/gates.md\n")
        self.put("docs/history/authorizations-index.md", "[gone](../ledgers/x.md)\n")

    def put(self, path: str, text: str) -> None:
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        _ = target.write_text(text, encoding="utf-8")

    def test_a_consistent_tree_passes(self) -> None:
        self.assertEqual([], docs_check.check(self.root), "links in frozen history are not resolved")

    def test_broken_links_anchors_and_escapes_are_reported(self) -> None:
        self.put("docs/runbooks/b.md", "[x](../specs/gone.md) [y](../plan.md#no-such-heading) [z](../../../outside.md) [w](https://example.invalid/)\n")
        self.assertEqual([
            "docs/runbooks/b.md:1: link ../specs/gone.md does not exist",
            "docs/runbooks/b.md:1: link ../plan.md#no-such-heading has no heading #no-such-heading",
            "docs/runbooks/b.md:1: link ../../../outside.md leaves the repository",
        ], docs_check.check(self.root))

    def test_anchors_follow_github_for_inline_code_and_repeated_headings(self) -> None:
        self.put("docs/runbooks/c.md", "# C\n\n## The `memory get` command\n\n## Notes\n\n## Notes\n\n```\n## Not a heading\n```\n")
        self.put("docs/runbooks/d.md", "[a](c.md#the-memory-get-command) [b](c.md#notes) [c](c.md#notes-1) [d](c.md#notes-2) [e](c.md#not-a-heading)\n")
        self.assertEqual(["docs/runbooks/d.md:1: link c.md#notes-2 has no heading #notes-2",
                          "docs/runbooks/d.md:1: link c.md#not-a-heading has no heading #not-a-heading"], docs_check.check(self.root))

    def test_document_paths_named_in_code_and_instructions_must_exist(self) -> None:
        self.put("tools/x.py", "# docs/specs/old.md and docs/plan.md#nowhere\n")
        self.put(".agents/rules/r.md", "Read docs/research/digest.md.\n")
        self.assertEqual([
            ".agents/rules/r.md:1: names docs/research/digest.md, which does not exist",
            "tools/x.py:1: names docs/specs/old.md, which does not exist",
            "tools/x.py:1: names docs/plan.md#nowhere, which has no heading #nowhere",
        ], docs_check.check(self.root))

    def test_nothing_is_added_to_the_frozen_directories_and_markdown_is_utf8(self) -> None:
        self.put("docs/research/archive/2026-12-01-new.md", "# New\n")
        _ = (self.root / "docs/bad.md").write_bytes(b"\xff\xfe")
        self.assertEqual(["docs/bad.md: not UTF-8", "docs/research/archive/2026-12-01-new.md: new file in a frozen directory (history is not added to)"],
                         docs_check.check(self.root))


if __name__ == "__main__":
    _ = unittest.main()
