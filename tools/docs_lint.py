#!/usr/bin/env python3
"""Documentation lint: keep the current documentation surface bounded and consistent.

The repository documentation has two surfaces (see ``.agents/rules/docs.md``):

* the *current surface*: ``README.md`` and everything under ``docs/`` except the
  frozen directories; bounded in bytes, fixed heading sets, no evidence detail
  in prose, resolvable links and ledger IDs;
* *retained history*: ``docs/history/`` and ``docs/research/archive/``; verbatim,
  append-only, and frozen to the allowlist below.

The script reads files only. ``--warn`` reports every problem but exits 0, for
use while a migration is in flight. ``--local`` additionally verifies that
``local-private`` manifest rows still exist on disk with matching digests.

Usage::

    python3 tools/docs_lint.py            # strict, repository root found from cwd
    python3 tools/docs_lint.py --warn     # report only
    python3 tools/docs_lint.py --local    # also verify local-private evidence
    python3 tools/docs_lint.py --root P   # explicit repository root
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable, Iterator, Sequence
from dataclasses import dataclass, field
import hashlib
from pathlib import Path
import re
import subprocess
import sys
from typing import final

KIB = 1024

# ---------------------------------------------------------------------------
# Policy constants. Changing these is a deliberate, reviewed act.
# ---------------------------------------------------------------------------

CURRENT_ROOTS: tuple[str, ...] = ("README.md", "docs")
FROZEN_DIRS: tuple[str, ...] = ("docs/history", "docs/research/archive")
FROZEN_ALLOWLIST: frozenset[str] = frozenset(
    {
        "docs/history/README.md",
        "docs/history/authorizations-index.md",
        "docs/history/overnight-work-plan.md",
        "docs/history/research-authorization.md",
        "docs/history/decompilation-authorization.md",
        "docs/history/static-analysis-authorization.md",
        "docs/history/offline-runtime-authorization.md",
        "docs/research/archive/README.md",
        "docs/research/archive/2026-10-04-decompilation-report.md",
        "docs/research/archive/2026-10-04-native-snapshot-performance.md",
        "docs/research/archive/2026-10-04-offline-startup-report.md",
        "docs/research/archive/2026-10-04-overnight-runtime-checkpoint.md",
        "docs/research/archive/2026-10-04-overnight-static-checkpoint.md",
        "docs/research/archive/2026-10-04-prospect-rejection.md",
        "docs/research/archive/2026-10-04-reference-inventory.md",
        "docs/research/archive/2026-10-04-save-failure-output.md",
        "docs/research/archive/2026-10-04-static-baseline.md",
        "docs/research/archive/2026-10-04-targeted-static-report.md",
        "docs/research/archive/2026-10-12-control-caller-limit.md",
    }
)

DEFAULT_FILE_CAP = 16 * KIB
FILE_CAPS: dict[str, int] = {
    "docs/gates.md": 8 * KIB,
    "docs/ledgers/findings.md": 64 * KIB,
    "docs/ledgers/unknowns.md": 64 * KIB,
    "docs/ledgers/bugs.md": 64 * KIB,
    "docs/ledgers/compatibility.md": 64 * KIB,
}
LEDGER_ROW_CAP = 400
AGGREGATE_CAP = 160 * KIB

STATUS_FILE = "docs/implementation-status.md"
STATUS_H2: tuple[str, ...] = ("Summary", "Milestones", "Works", "Does not work", "Gates", "Next tasks")
SPEC_H2: tuple[str, ...] = ("Status", "Scope", "Confirmed", "Candidate", "Open questions", "Evidence")
DIGEST_H2: tuple[str, ...] = (
    "Question cluster",
    "Answered",
    "Partial",
    "Unresolved",
    "Limitations and exclusions",
    "Receipts",
)
RECONCILED_REQUIRED_PREFIXES: tuple[str, ...] = (STATUS_FILE, "docs/specs/", "docs/ledgers/")

PROSE_EXEMPT_PREFIXES: tuple[str, ...] = ("docs/runbooks/", "docs/gates.md")
FINDINGS_FILE = "docs/ledgers/findings.md"
UNKNOWNS_FILE = "docs/ledgers/unknowns.md"
MANIFEST_FILE = "evidence/manifest.md"
CODE_REFERENCE_ROOTS: tuple[str, ...] = ("src", "tools", ".github")

HEX64 = re.compile(r"(?<![0-9a-fA-F])[0-9a-f]{64}(?![0-9a-fA-F])")
UTC_TIME = re.compile(r"\b\d{1,2}:\d{2}\s*UTC\b")
DATED_HEADING = re.compile(r"\d{4}-\d{2}-\d{2}|\d{1,2}:\d{2}\s*UTC")
PRIVATE_PATH = re.compile(r"(?<![\w/.-])(work|reference-data)/[\w./-]*")
MD_LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)\)")
FENCE = re.compile(r"^\s*(```|~~~)")
INLINE_CODE = re.compile(r"`[^`\n]*`")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
FINDING_ID = re.compile(r"\bF-\d{3,}\b")
UNKNOWN_ID = re.compile(r"\bU-\d{3,}\b")
MANIFEST_ID = re.compile(r"\bM-\d{3,}\b")
EVIDENCE_ID = re.compile(r"\bev_[0-9a-f]{64}\b")
SUPERSEDED_BY = re.compile(r"superseded-by\s+(F-\d{3,})")
RECONCILED = re.compile(r"Last reconciled:\s*\d{4}-\d{2}-\d{2}")
CODE_DOC_REF = re.compile(r"docs/[\w./-]+\.md")


@dataclass
class Problem:
    path: str
    line: int
    message: str

    def render(self) -> str:
        where = f"{self.path}:{self.line}" if self.line else self.path
        return f"{where}: {self.message}"


@dataclass
class Document:
    rel: str
    text: str
    lines: list[str] = field(init=False)

    def __post_init__(self) -> None:
        self.lines = self.text.split("\n")

    @property
    def size(self) -> int:
        return len(self.text.encode("utf-8"))

    def headings(self) -> list[tuple[int, int, str]]:
        """Return (line number, level, text) for every heading outside fenced code."""
        found: list[tuple[int, int, str]] = []
        for number, line, fenced in iter_lines(self.lines):
            if fenced:
                continue
            match = HEADING.match(line)
            if match:
                found.append((number, len(match.group(1)), match.group(2)))
        return found


def iter_lines(lines: Sequence[str]) -> Iterator[tuple[int, str, bool]]:
    """Yield (1-based number, line, inside fenced block)."""
    fenced = False
    for index, line in enumerate(lines, start=1):
        if FENCE.match(line):
            fenced = not fenced
            yield index, line, True
            continue
        yield index, line, fenced


def slugify(heading: str) -> str:
    """GitHub-style anchor for a heading."""
    text = heading.strip().lower()
    text = re.sub(r"[^\w\- ]", "", text)
    return text.replace(" ", "-")


def is_under(rel: str, prefixes: Iterable[str]) -> bool:
    return any(rel == prefix or rel.startswith(prefix.rstrip("/") + "/") for prefix in prefixes)


def ledger_ids(doc: Document, pattern: re.Pattern[str]) -> list[tuple[int, str]]:
    """IDs defined in the first cell of each table row."""
    defined: list[tuple[int, str]] = []
    for number, line, fenced in iter_lines(doc.lines):
        if fenced or not line.startswith("|"):
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        if cells and pattern.fullmatch(cells[0]):
            defined.append((number, cells[0]))
    return defined


@final
class Linter:
    root: Path
    local: bool
    problems: list[Problem]
    docs: dict[str, Document]
    current: dict[str, Document]

    def __init__(self, root: Path, *, local: bool) -> None:
        self.root = root
        self.local = local
        self.problems = []
        self.docs = {}
        self.current = {}

    # -- loading ----------------------------------------------------------

    def load(self) -> None:
        for rel in sorted(self.tracked_markdown()):
            path = self.root / rel
            try:
                text = path.read_text(encoding="utf-8")
            except UnicodeDecodeError:
                self.report(rel, 0, "not valid UTF-8")
                continue
            doc = Document(rel, text)
            self.docs[rel] = doc
            if self.is_current(rel):
                self.current[rel] = doc

    def tracked_markdown(self) -> set[str]:
        tracked: set[str] = set()
        for root in CURRENT_ROOTS + (MANIFEST_FILE,):
            path = self.root / root
            if path.is_file() and path.suffix == ".md":
                tracked.add(root)
            elif path.is_dir():
                for file in path.rglob("*.md"):
                    tracked.add(file.relative_to(self.root).as_posix())
        return tracked

    @staticmethod
    def is_current(rel: str) -> bool:
        if rel == MANIFEST_FILE:
            return False
        return not is_under(rel, FROZEN_DIRS)

    def report(self, rel: str, line: int, message: str) -> None:
        self.problems.append(Problem(rel, line, message))

    # -- checks -----------------------------------------------------------

    def run(self) -> int:
        self.load()
        self.check_frozen()
        self.check_sizes()
        self.check_headings()
        self.check_prose()
        self.check_links()
        self.check_code_references()
        self.check_ids()
        self.check_manifest()
        self.check_reconciled()
        return sum(doc.size for doc in self.current.values())

    def check_frozen(self) -> None:
        for rel in self.docs:
            if is_under(rel, FROZEN_DIRS) and rel not in FROZEN_ALLOWLIST:
                self.report(rel, 0, "frozen directory accepts no new files; add to FROZEN_ALLOWLIST deliberately")

    def check_sizes(self) -> None:
        total = 0
        for rel, doc in self.current.items():
            total += doc.size
            cap = FILE_CAPS.get(rel, DEFAULT_FILE_CAP)
            if doc.size > cap:
                self.report(rel, 0, f"{doc.size} bytes exceeds the {cap}-byte cap")
            if rel.startswith("docs/ledgers/"):
                for number, line, fenced in iter_lines(doc.lines):
                    if not fenced and line.startswith("|") and len(line.encode("utf-8")) > LEDGER_ROW_CAP:
                        self.report(rel, number, f"ledger row exceeds {LEDGER_ROW_CAP} bytes")
        if total > AGGREGATE_CAP:
            self.report("docs", 0, f"current surface is {total} bytes; cap is {AGGREGATE_CAP}")

    def check_headings(self) -> None:
        for rel, doc in self.current.items():
            headings = doc.headings()
            for number, _level, text in headings:
                if DATED_HEADING.search(text):
                    self.report(rel, number, "dated or timestamped heading; session notes belong in a digest or ledger row")
            expected = self.expected_h2(rel)
            if expected is None:
                continue
            actual = tuple(text for _n, level, text in headings if level == 2)
            if actual != expected:
                self.report(rel, 0, f"H2 set must be exactly {list(expected)}; found {list(actual)}")

    @staticmethod
    def expected_h2(rel: str) -> tuple[str, ...] | None:
        if rel == STATUS_FILE:
            return STATUS_H2
        if rel.startswith("docs/specs/") and not rel.endswith("/README.md"):
            return SPEC_H2
        if rel.startswith("docs/research/") and not rel.endswith("/README.md"):
            return DIGEST_H2
        return None

    def check_prose(self) -> None:
        for rel, doc in self.current.items():
            if is_under(rel, PROSE_EXEMPT_PREFIXES):
                continue
            ledger = rel.startswith("docs/ledgers/")
            for number, line, fenced in iter_lines(doc.lines):
                if fenced or (ledger and line.startswith("|")):
                    continue
                if HEX64.search(line):
                    self.report(rel, number, "64-hex digest in prose; cite a manifest or finding ID")
                if UTC_TIME.search(line):
                    self.report(rel, number, "clock time in prose; session timing belongs in the evidence manifest")
                stripped = INLINE_CODE.sub("", line)
                if PRIVATE_PATH.search(stripped):
                    self.report(rel, number, "work/ or reference-data/ path in prose; cite a manifest ID or use a code span")

    def check_links(self) -> None:
        anchors: dict[str, set[str]] = {
            rel: {slugify(text) for _n, _l, text in doc.headings()} for rel, doc in self.docs.items()
        }
        for rel, doc in self.docs.items():
            for number, line, fenced in iter_lines(doc.lines):
                if fenced:
                    continue
                for match in MD_LINK.finditer(line):
                    target = match.group(1)
                    if "://" in target or target.startswith("mailto:"):
                        continue
                    path_part, _, fragment = target.partition("#")
                    if path_part:
                        resolved = (self.root / rel).parent.joinpath(path_part).resolve()
                        try:
                            target_rel = resolved.relative_to(self.root.resolve()).as_posix()
                        except ValueError:
                            self.report(rel, number, f"link escapes the repository: {target}")
                            continue
                        if not resolved.exists():
                            self.report(rel, number, f"dangling link: {target}")
                            continue
                    else:
                        target_rel = rel
                    if fragment and target_rel in anchors and fragment not in anchors[target_rel]:
                        self.report(rel, number, f"missing heading anchor: {target}")

    def check_code_references(self) -> None:
        for root in CODE_REFERENCE_ROOTS:
            base = self.root / root
            if not base.is_dir():
                continue
            for file in base.rglob("*"):
                if not file.is_file() or file.suffix not in {".rs", ".py", ".cjs", ".js", ".yml", ".yaml", ".sh", ".toml", ".java"}:
                    continue
                try:
                    text = file.read_text(encoding="utf-8")
                except UnicodeDecodeError:
                    continue
                for number, line in enumerate(text.split("\n"), start=1):
                    for match in CODE_DOC_REF.finditer(line):
                        if not (self.root / match.group(0)).exists():
                            self.report(file.relative_to(self.root).as_posix(), number, f"references missing document {match.group(0)}")

    def check_ids(self) -> None:
        findings = self.docs.get(FINDINGS_FILE)
        unknowns = self.docs.get(UNKNOWNS_FILE)
        manifest = self.docs.get(MANIFEST_FILE)
        defined_f: set[str] = self.unique_ids(findings, FINDING_ID) if findings else set()
        defined_u: set[str] = self.unique_ids(unknowns, UNKNOWN_ID) if unknowns else set()
        defined_m: set[str] = self.unique_ids(manifest, MANIFEST_ID) if manifest else set()
        if findings:
            self.check_no_removed_ids(FINDINGS_FILE, defined_f, FINDING_ID)
            for number, line, fenced in iter_lines(findings.lines):
                if fenced:
                    continue
                for match in SUPERSEDED_BY.finditer(line):
                    if match.group(1) not in defined_f:
                        self.report(FINDINGS_FILE, number, f"superseded-by target {match.group(1)} is not defined")
        if unknowns:
            self.check_no_removed_ids(UNKNOWNS_FILE, defined_u, UNKNOWN_ID)
        for rel, doc in self.current.items():
            for number, line, fenced in iter_lines(doc.lines):
                if fenced:
                    continue
                self.check_references(rel, number, line, FINDING_ID, defined_f, "finding")
                self.check_references(rel, number, line, UNKNOWN_ID, defined_u, "unknown")
                self.check_references(rel, number, line, MANIFEST_ID, defined_m, "manifest row")
                if EVIDENCE_ID.search(line) and not MANIFEST_ID.search(line):
                    self.report(rel, number, "ev_ identifier cited without a manifest row (M-NNN) on the same line")

    def unique_ids(self, doc: Document, pattern: re.Pattern[str]) -> set[str]:
        seen: dict[str, int] = {}
        for number, identifier in ledger_ids(doc, pattern):
            if identifier in seen:
                self.report(doc.rel, number, f"duplicate ID {identifier} (first at line {seen[identifier]})")
            _ = seen.setdefault(identifier, number)
        return set(seen)

    def check_references(self, rel: str, number: int, line: str, pattern: re.Pattern[str], defined: set[str], kind: str) -> None:
        for match in pattern.finditer(line):
            if match.group(0) not in defined:
                self.report(rel, number, f"{kind} {match.group(0)} is not defined in its ledger")

    def check_no_removed_ids(self, rel: str, defined: set[str], pattern: re.Pattern[str]) -> None:
        previous = self.git_show(rel)
        if previous is None:
            return
        for _number, identifier in ledger_ids(Document(rel, previous), pattern):
            if identifier not in defined:
                self.report(rel, 0, f"ID {identifier} existed at HEAD and was removed; mark it superseded instead")

    def git_show(self, rel: str) -> str | None:
        try:
            result = subprocess.run(
                ["git", "-C", str(self.root), "show", f"HEAD:{rel}"],
                capture_output=True,
                text=True,
                check=False,
            )
        except OSError:
            return None
        return result.stdout if result.returncode == 0 else None

    def check_manifest(self) -> None:
        manifest = self.docs.get(MANIFEST_FILE)
        if manifest is None:
            return
        for number, line, fenced in iter_lines(manifest.lines):
            if fenced or not line.startswith("|"):
                continue
            cells = [cell.strip() for cell in line.strip("|").split("|")]
            if len(cells) < 6 or not MANIFEST_ID.fullmatch(cells[0]):
                continue
            _identifier, path, digest, _date, _session, klass = cells[:6]
            if klass not in {"local-private", "external"}:
                self.report(MANIFEST_FILE, number, f"class must be local-private or external, not {klass!r}")
            if not re.fullmatch(r"[0-9a-f]{64}", digest):
                self.report(MANIFEST_FILE, number, "sha256 column must be a 64-hex digest")
                continue
            if klass == "local-private" and self.local:
                file = self.root / path
                if not file.is_file():
                    self.report(MANIFEST_FILE, number, f"local-private evidence missing: {path}")
                elif hashlib.sha256(file.read_bytes()).hexdigest() != digest:
                    self.report(MANIFEST_FILE, number, f"local-private evidence digest mismatch: {path}")

    def check_reconciled(self) -> None:
        for rel, doc in self.current.items():
            if is_under(rel, RECONCILED_REQUIRED_PREFIXES) and not rel.endswith("/README.md"):
                if not RECONCILED.search(doc.text):
                    self.report(rel, 0, "missing 'Last reconciled: YYYY-MM-DD'")


def find_root(start: Path) -> Path:
    for candidate in (start, *start.parents):
        if (candidate / ".git").exists() and (candidate / "docs").is_dir():
            return candidate
    raise SystemExit("docs_lint: repository root with docs/ not found")


class Options(argparse.Namespace):
    root: Path | None = None
    warn: bool = False
    local: bool = False


def parse_options(argv: Sequence[str] | None) -> Options:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    _ = parser.add_argument("--root", type=Path, default=None)
    _ = parser.add_argument("--warn", action="store_true", help="report problems but exit 0")
    _ = parser.add_argument("--local", action="store_true", help="verify local-private manifest rows on disk")
    options = Options()
    _ = parser.parse_args(argv, namespace=options)
    return options


def main(argv: Sequence[str] | None = None) -> int:
    options = parse_options(argv)
    root = options.root.resolve() if options.root else find_root(Path.cwd().resolve())
    linter = Linter(root, local=options.local)
    total = linter.run()
    for problem in linter.problems:
        print(problem.render())
    print(f"docs_lint: current surface {total} bytes of {AGGREGATE_CAP}; {len(linter.problems)} problem(s)")
    if linter.problems and not options.warn:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
