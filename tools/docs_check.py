# SPDX-License-Identifier: AGPL-3.0-or-later
"""Checks of the documents that stay in Git (``.agents/rules/memory.md``, "Stays in Git").

``python3 tools/docs_check.py`` exits 1 on any problem, one line each:

- every tracked Markdown file is UTF-8;
- every relative link in tracked Markdown resolves inside the repository to a tracked file or directory,
  and its ``#anchor`` to a heading of the target (GitHub's slug); links in the frozen directories are
  history and are not resolved;
- every ``docs/<path>.md[#anchor]`` named in code (``src/``, ``tests/``, ``examples/``, ``tools/``,
  ``config/``, ``.github/``) or in the instructions (``CLAUDE.md``, ``.agents/rules/``) exists, with its anchor;
- the frozen directories hold exactly ``FROZEN``: nothing is added to the record of the past.

Project knowledge is checked by ``python3 -m tools.memory check --ci``, not here.
"""

from __future__ import annotations

import re
import subprocess
import sys
from collections.abc import Iterable
from pathlib import Path, PurePosixPath

FROZEN_DIRS = ("docs/history/", "docs/research/archive/")
FROZEN = frozenset({
    "docs/history/authorizations-index.md", "docs/history/overnight-work-plan.md", "docs/history/research-authorization.md",
    "docs/history/decompilation-authorization.md", "docs/history/static-analysis-authorization.md",
    "docs/history/offline-runtime-authorization.md",
    *(f"docs/research/archive/2026-10-{name}.md" for name in (
        "04-decompilation-report", "04-native-snapshot-performance", "04-offline-startup-report", "04-overnight-runtime-checkpoint",
        "04-overnight-static-checkpoint", "04-prospect-rejection", "04-reference-inventory", "04-save-failure-output", "04-static-baseline",
        "04-targeted-static-report", "12-control-caller-limit")),
})
# Vendored skills and invented test fixtures are not scanned; the frozen legacy reader names the deleted trees on purpose.
SKIPPED = (".agents/skills/", ".agents/vendor/", "tools/tests/", "tools/memory/legacy")
CODE = ("src/", "tests/", "examples/", "tools/", "config/", ".github/")
INSTRUCTIONS = ("CLAUDE.md", ".agents/rules/")
LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)\)")
INLINE_CODE = re.compile(r"`[^`\n]*`")
FENCE = re.compile(r"^\s*(```|~~~)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
DOC_PATH = re.compile(r"(?<![\w./-])(docs/[\w./-]+\.md)(?:#([\w-]+))?")


def tracked(root: Path) -> list[str]:
    """Tracked and untracked, non-ignored files present in the working tree."""
    command = ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"]
    output = subprocess.run(command, check=True, capture_output=True).stdout.decode()
    return sorted({path for path in output.split("\0") if path and ((root / path).exists() or (root / path).is_symlink())})


def slug(heading: str) -> str:
    """GitHub's anchor for a heading."""
    return re.sub(r"[^\w\- ]", "", heading.strip().lower()).replace(" ", "-")


def prose(text: str) -> Iterable[tuple[int, str]]:
    """Numbered lines outside fenced blocks, inline code removed."""
    fenced = False
    for number, line in enumerate(text.split("\n"), 1):
        if FENCE.match(line):
            fenced = not fenced
        elif not fenced:
            yield number, INLINE_CODE.sub("", line)


def anchors(text: str) -> set[str]:
    return {slug(match[2]) for _number, line in prose(text) if (match := HEADING.match(line))}


def check(root: Path) -> list[str]:
    files = tracked(root)
    known = set(files)
    directories = {str(parent) for path in files for parent in PurePosixPath(path).parents}
    texts: dict[str, str] = {}
    problems: list[str] = []
    for path in files:
        # A symlink (AGENTS.md) is checked as its target.
        if path.startswith(SKIPPED) or (root / path).is_symlink():
            continue
        if path.endswith(".md") or path.startswith(CODE) or path.startswith(INSTRUCTIONS):
            try:
                texts[path] = (root / path).read_text(encoding="utf-8")
            except UnicodeDecodeError:
                if path.endswith(".md"):
                    problems.append(f"{path}: not UTF-8")
            except OSError:
                continue

    def target_problem(target: str, anchor: str | None) -> str | None:
        if target not in known and target not in directories:
            return "does not exist"
        if anchor and target.endswith(".md") and target in texts and anchor not in anchors(texts[target]):
            return f"has no heading #{anchor}"
        return None

    for path, text in sorted(texts.items()):
        if path.endswith(".md") and not path.startswith(FROZEN_DIRS):
            for number, line in prose(text):
                for match in LINK.finditer(line):
                    link = match[1]
                    if re.match(r"[a-z][a-z0-9+.-]*:", link):
                        continue
                    location, _, anchor = link.partition("#")
                    target = PurePosixPath(path).parent / location if location else PurePosixPath(path)
                    parts: list[str] = []
                    for part in target.parts:
                        if part == "..":
                            if not parts:
                                break
                            _ = parts.pop()
                        elif part != ".":
                            parts.append(part)
                    else:
                        if problem := target_problem("/".join(parts).rstrip("/"), anchor or None):
                            problems.append(f"{path}:{number}: link {link} {problem}")
                        continue
                    problems.append(f"{path}:{number}: link {link} leaves the repository")
        if path.startswith(CODE) or path.startswith(INSTRUCTIONS):
            for number, line in enumerate(text.split("\n"), 1):
                for match in DOC_PATH.finditer(line):
                    if problem := target_problem(match[1], match[2]):
                        problems.append(f"{path}:{number}: names {match[0]}, which {problem}")
    problems.extend(f"{path}: new file in a frozen directory (history is not added to)" for path in files
                    if path.startswith(FROZEN_DIRS) and path not in FROZEN)
    return problems


def main() -> int:
    root = Path(subprocess.run(["git", "rev-parse", "--show-toplevel"], check=True, capture_output=True, text=True).stdout.strip())
    problems = check(root)
    for problem in problems:
        print(problem)
    print(f"docs_check: {len(problems)} problem(s)")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
