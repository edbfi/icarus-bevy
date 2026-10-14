#!/usr/bin/env python3
"""Documentation lint: keep the current documentation surface bounded and consistent.

The repository documentation has two surfaces (see ``.agents/rules/docs.md``):

* the *current surface*: ``README.md`` and everything under ``docs/`` except the
  frozen directories; bounded in bytes, fixed heading sets, no evidence detail
  in prose, resolvable links and ledger IDs;
* *retained history*: ``docs/history/`` and ``docs/research/archive/``; verbatim,
  append-only, and frozen to the allowlist below.

``CLAUDE.md`` and ``.agents/rules/`` join the link and document-path checks
without becoming part of the current surface.

The script reads files only. ``--warn`` reports every problem but exits 0, for
use while a migration is in flight. ``--local`` additionally verifies that
``local-private`` manifest rows still exist on disk with matching digests.
``--base REF`` names the revision whose ledger IDs must survive (default
``HEAD``); its findings may still use the legacy single ``findings.md``.

Usage::

    python3 tools/docs_lint.py                  # strict, repository root found from cwd
    python3 tools/docs_lint.py --warn           # report only
    python3 tools/docs_lint.py --local          # also verify local-private evidence
    python3 tools/docs_lint.py --root P         # explicit repository root
    python3 tools/docs_lint.py --base origin/main
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable, Iterator, Sequence
from dataclasses import dataclass, field
from datetime import date
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
# Agent instructions: link and document-path checks only, never the current surface.
INSTRUCTION_ROOTS: tuple[str, ...] = ("CLAUDE.md", ".agents/rules")
FROZEN_DIRS: tuple[str, ...] = ("docs/history", "docs/research/archive")
FROZEN_ALLOWLIST: frozenset[str] = frozenset(
    {
        "docs/history/authorizations-index.md",
        "docs/history/overnight-work-plan.md",
        "docs/history/research-authorization.md",
        "docs/history/decompilation-authorization.md",
        "docs/history/static-analysis-authorization.md",
        "docs/history/offline-runtime-authorization.md",
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
    "docs/ledgers/unknowns.md": 64 * KIB,
    "docs/ledgers/bugs.md": 64 * KIB,
    "docs/ledgers/compatibility.md": 64 * KIB,
}
# Directory caps; the first matching prefix wins.
PREFIX_CAPS: tuple[tuple[str, int], ...] = (
    ("docs/ledgers/findings/retired/", 64 * KIB),
    ("docs/ledgers/findings/", 48 * KIB),
)
DIGEST_CAP = 8 * KIB
ACTIVE_FINDINGS_CAP = 96 * KIB
LEDGER_ROW_CAP = 400
MANIFEST_ROW_CAP = 300
AGGREGATE_CAP = 160 * KIB

# Read tiers: files every session loads, and per-task entry sets mirroring the
# routing table in CLAUDE.md.
STARTUP_FILES: tuple[str, ...] = ("CLAUDE.md", "docs/README.md", "docs/implementation-status.md")
STARTUP_BUDGET = 20 * KIB
TASK_BUDGET = 64 * KIB
_RE_SKILL = ".agents/skills/reverse-engineer-anything/SKILL.md"
_RESEARCH = (".agents/rules/project.md", "docs/gates.md")
TASK_TIERS: dict[str, tuple[str, ...]] = {
    "save-format research": (*_RESEARCH, "docs/research/save-format/save-class-audits.md", "docs/specs/save-format/prospect-loading.md", _RE_SKILL),
    "C001 research": (*_RESEARCH, "docs/research/save-format/c001-constructor.md", "docs/specs/save-format/prospect-loading.md", _RE_SKILL),
    "network research": (*_RESEARCH, "docs/research/network/control-stores.md", "docs/specs/network/control-negotiation.md", _RE_SKILL),
    "native server": (
        ".agents/rules/project.md",
        "docs/architecture.md",
        "docs/research/foundation/foundation.md",
        "docs/runbooks/native-server.md",
        ".agents/skills/bevy/SKILL.md",
    ),
    "documentation": (".agents/rules/docs.md", "docs/README.md"),
}
STALE_DAYS = 30

STATUS_FILE = "docs/implementation-status.md"
STATUS_H2: tuple[str, ...] = ("Summary", "Milestones", "Next tasks")
SPEC_H2: tuple[str, ...] = ("Status", "Scope", "Confirmed", "Candidate", "Open questions", "Evidence")
DIGEST_H2: tuple[str, ...] = ("Question cluster", "Current answer", "Open", "Artifacts")
RECONCILED_REQUIRED_PREFIXES: tuple[str, ...] = (STATUS_FILE, "docs/specs/", "docs/ledgers/")

PROSE_EXEMPT_PREFIXES: tuple[str, ...] = ("docs/runbooks/", "docs/gates.md")
# The single findings ledger that preceded the split tree; read only at --base.
LEGACY_FINDINGS_FILE = "docs/ledgers/findings.md"
FINDINGS_DIR = "docs/ledgers/findings"
RETIRED_DIR = "docs/ledgers/findings/retired"
UNKNOWNS_FILE = "docs/ledgers/unknowns.md"
MANIFEST_FILE = "evidence/manifest.md"
CODE_REFERENCE_ROOTS: tuple[str, ...] = ("src", "tools", ".github")
# Execution receipts belong in the evidence manifest, not in new finding rows.
RECEIPT_CLASSES: tuple[str, ...] = ("tool-receipt", "tooling-receipt")

HEX64 = re.compile(r"(?<![0-9a-fA-F])[0-9a-f]{64}(?![0-9a-fA-F])")
UTC_TIME = re.compile(r"\b\d{1,2}:\d{2}\s*UTC\b")
DATED_HEADING = re.compile(r"\d{4}-\d{2}-\d{2}|\d{1,2}:\d{2}\s*UTC")
DATED_NAME = re.compile(r"\d{4}-\d{2}-\d{2}")
PRIVATE_PATH = re.compile(r"(?<![\w/.-])(work|reference-data)/[\w./-]*")
MD_LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)\)")
LINK_TARGET = re.compile(r"\]\([^)\s]*\)")
FENCE = re.compile(r"^\s*(```|~~~)")
INLINE_CODE = re.compile(r"`[^`\n]*`")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
FINDING_ID = re.compile(r"\bF-\d{3,}\b")
UNKNOWN_ID = re.compile(r"\bU-\d{3,}\b")
MANIFEST_ID = re.compile(r"\bM-\d{3,}\b")
EVIDENCE_ID = re.compile(r"\bev_[0-9a-f]{64}\b")
SUPERSEDED_BY = re.compile(r"superseded-by\s+(F-\d{3,})")
RECONCILED = re.compile(r"Last reconciled:\s*(\d{4}-\d{2}-\d{2})")
CODE_DOC_REF = re.compile(r"docs/[\w./-]+\.md")
# Words glued to numbers by whitespace stripping, e.g. "passes128authored".
GLUE = re.compile(r"\b[a-z]{2,}\d+[a-z]{2,}\b|\b\d+[a-z]{4,}\b")
HEXISH = re.compile(r"[0-9a-f]{7,}")
PROVENANCE = re.compile(r"\b[0-9a-f]{7,40}:docs/\S+")


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


def is_findings(rel: str) -> bool:
    """Any file of the findings tree, active or retired."""
    return is_under(rel, (FINDINGS_DIR,))


def is_digest(rel: str) -> bool:
    return rel.startswith("docs/research/") and not rel.endswith("/README.md") and not is_under(rel, FROZEN_DIRS)


def table_cells(line: str) -> list[str]:
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def ledger_ids(doc: Document, pattern: re.Pattern[str]) -> list[tuple[int, str]]:
    """IDs defined in the first cell of each table row."""
    defined: list[tuple[int, str]] = []
    for number, line, fenced in iter_lines(doc.lines):
        if fenced or not line.startswith("|"):
            continue
        cells = table_cells(line)
        if cells and pattern.fullmatch(cells[0]):
            defined.append((number, cells[0]))
    return defined


def glued_tokens(line: str) -> list[str]:
    """Glue-regex hits outside inline code and link targets; hex identifiers are exempt."""
    stripped = LINK_TARGET.sub("]()", INLINE_CODE.sub("", line))
    tokens = (match.group(0) for match in GLUE.finditer(stripped))
    return [token for token in tokens if not HEXISH.fullmatch(token)]


@final
class Linter:
    root: Path
    local: bool
    base: str
    today: date
    problems: list[Problem]
    warnings: list[Problem]
    docs: dict[str, Document]
    current: dict[str, Document]

    def __init__(self, root: Path, *, local: bool, base: str = "HEAD", today: date | None = None) -> None:
        self.root = root
        self.local = local
        self.base = base
        self.today = today or date.today()
        self.problems = []
        self.warnings = []
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
        for root in CURRENT_ROOTS + INSTRUCTION_ROOTS + (MANIFEST_FILE,):
            path = self.root / root
            if path.is_file() and path.suffix == ".md":
                tracked.add(root)
            elif path.is_dir():
                for file in path.rglob("*.md"):
                    tracked.add(file.relative_to(self.root).as_posix())
        return tracked

    @staticmethod
    def is_current(rel: str) -> bool:
        if rel == MANIFEST_FILE or is_under(rel, INSTRUCTION_ROOTS):
            return False
        return not is_under(rel, FROZEN_DIRS)

    def report(self, rel: str, line: int, message: str) -> None:
        self.problems.append(Problem(rel, line, message))

    def warn(self, rel: str, line: int, message: str) -> None:
        self.warnings.append(Problem(rel, line, message))

    def findings_docs(self) -> list[Document]:
        return [doc for rel, doc in sorted(self.docs.items()) if is_findings(rel)]

    # -- checks -----------------------------------------------------------

    def run(self) -> int:
        self.load()
        self.check_frozen()
        self.check_sizes()
        self.check_headings()
        self.check_prose()
        self.check_links()
        self.check_code_references()
        self.check_instruction_references()
        self.check_ids()
        self.check_new_findings()
        self.check_manifest()
        self.check_reconciled()
        self.check_staleness()
        self.check_glue()
        self.check_filenames()
        self.check_provenance()
        self.check_read_tiers()
        return sum(doc.size for rel, doc in self.current.items() if not rel.startswith("docs/ledgers/"))

    def check_frozen(self) -> None:
        for rel in self.docs:
            if is_under(rel, FROZEN_DIRS) and rel not in FROZEN_ALLOWLIST:
                self.report(rel, 0, "frozen directory accepts no new files; add to FROZEN_ALLOWLIST deliberately")

    def file_cap(self, rel: str) -> int:
        if rel in FILE_CAPS:
            return FILE_CAPS[rel]
        for prefix, cap in PREFIX_CAPS:
            if rel.startswith(prefix):
                return cap
        if is_digest(rel):
            return DIGEST_CAP
        return DEFAULT_FILE_CAP

    def check_sizes(self) -> None:
        total = 0
        active_findings = 0
        for rel, doc in self.current.items():
            if not rel.startswith("docs/ledgers/"):
                total += doc.size
            if is_under(rel, (FINDINGS_DIR,)) and not is_under(rel, (RETIRED_DIR,)):
                active_findings += doc.size
            cap = self.file_cap(rel)
            if doc.size > cap:
                self.report(rel, 0, f"{doc.size} bytes exceeds the {cap}-byte cap")
            if rel.startswith("docs/ledgers/"):
                for number, line, fenced in iter_lines(doc.lines):
                    if not fenced and line.startswith("|") and len(line.encode("utf-8")) > LEDGER_ROW_CAP:
                        self.report(rel, number, f"ledger row exceeds {LEDGER_ROW_CAP} bytes")
        if total > AGGREGATE_CAP:
            self.report("docs", 0, f"current prose surface is {total} bytes; cap is {AGGREGATE_CAP}")
        if active_findings > ACTIVE_FINDINGS_CAP:
            self.report(FINDINGS_DIR, 0, f"active findings total {active_findings} bytes; cap is {ACTIVE_FINDINGS_CAP}")

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
        if is_digest(rel):
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
                rel_file = file.relative_to(self.root).as_posix()
                if rel_file == "tools/docs_lint.py" or rel_file.startswith("tools/tests/"):
                    continue
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

    def check_instruction_references(self) -> None:
        """Agent instructions must not name document paths that no longer exist."""
        for rel, doc in self.docs.items():
            if not is_under(rel, INSTRUCTION_ROOTS):
                continue
            for number, line, _fenced in iter_lines(doc.lines):
                for match in CODE_DOC_REF.finditer(line):
                    if not (self.root / match.group(0)).exists():
                        self.report(rel, number, f"references missing document {match.group(0)}")

    def check_ids(self) -> None:
        unknowns = self.docs.get(UNKNOWNS_FILE)
        manifest = self.docs.get(MANIFEST_FILE)
        findings = self.findings_docs()
        defined_f: set[str] = self.unique_ids(findings, FINDING_ID)
        defined_u: set[str] = self.unique_ids([unknowns], UNKNOWN_ID) if unknowns else set()
        defined_m: set[str] = self.unique_ids([manifest], MANIFEST_ID) if manifest else set()
        self.check_no_removed_ids(self.base_findings_texts(), defined_f, FINDING_ID)
        for doc in findings:
            for number, line, fenced in iter_lines(doc.lines):
                if fenced:
                    continue
                for match in SUPERSEDED_BY.finditer(line):
                    if match.group(1) not in defined_f:
                        self.report(doc.rel, number, f"superseded-by target {match.group(1)} is not defined")
        if unknowns:
            self.check_no_removed_ids(self.base_texts([UNKNOWNS_FILE]), defined_u, UNKNOWN_ID)
        for rel, doc in self.current.items():
            for number, line, fenced in iter_lines(doc.lines):
                if fenced:
                    continue
                self.check_references(rel, number, line, FINDING_ID, defined_f, "finding")
                self.check_references(rel, number, line, UNKNOWN_ID, defined_u, "unknown")
                self.check_references(rel, number, line, MANIFEST_ID, defined_m, "manifest row")
                if EVIDENCE_ID.search(line) and not MANIFEST_ID.search(line):
                    self.report(rel, number, "ev_ identifier cited without a manifest row (M-NNN) on the same line")

    def unique_ids(self, docs: Sequence[Document], pattern: re.Pattern[str]) -> set[str]:
        """IDs defined across the given files; each must be defined exactly once."""
        seen: dict[str, tuple[str, int]] = {}
        for doc in docs:
            for number, identifier in ledger_ids(doc, pattern):
                if identifier in seen:
                    first_rel, first_line = seen[identifier]
                    self.report(doc.rel, number, f"duplicate ID {identifier} (first at {first_rel}:{first_line})")
                    continue
                seen[identifier] = (doc.rel, number)
        return set(seen)

    def check_references(self, rel: str, number: int, line: str, pattern: re.Pattern[str], defined: set[str], kind: str) -> None:
        for match in pattern.finditer(line):
            if match.group(0) not in defined:
                self.report(rel, number, f"{kind} {match.group(0)} is not defined in its ledger")

    def base_texts(self, rels: Iterable[str]) -> list[Document]:
        found: list[Document] = []
        for rel in rels:
            text = self.git_show(rel)
            if text is not None:
                found.append(Document(rel, text))
        return found

    def base_findings_texts(self) -> list[Document]:
        """Findings files at the base revision: the legacy file plus the split tree."""
        return self.base_texts([LEGACY_FINDINGS_FILE, *self.git_ls(FINDINGS_DIR)])

    def base_finding_ids(self) -> set[str] | None:
        """F- IDs defined at the base revision, or None when the base is unreadable."""
        if not self.base_exists():
            return None
        return {identifier for doc in self.base_findings_texts() for _n, identifier in ledger_ids(doc, FINDING_ID)}

    def check_no_removed_ids(self, previous: Sequence[Document], defined: set[str], pattern: re.Pattern[str]) -> None:
        for doc in previous:
            for _number, identifier in ledger_ids(doc, pattern):
                if identifier not in defined:
                    self.report(doc.rel, 0, f"ID {identifier} existed at {self.base} and was removed; mark it superseded instead")

    def check_new_findings(self) -> None:
        """Rows added since the base revision must not be execution receipts."""
        base_ids = self.base_finding_ids() or set()
        for doc in self.findings_docs():
            if not is_under(doc.rel, (FINDINGS_DIR,)):
                continue
            evidence_column: int | None = None
            for number, line, fenced in iter_lines(doc.lines):
                if fenced or not line.startswith("|"):
                    continue
                cells = table_cells(line)
                if "Evidence" in cells and cells[0] == "ID":
                    evidence_column = cells.index("Evidence")
                    continue
                if evidence_column is None or not cells or not FINDING_ID.fullmatch(cells[0]) or cells[0] in base_ids:
                    continue
                evidence = cells[evidence_column] if evidence_column < len(cells) else ""
                if evidence.startswith(RECEIPT_CLASSES):
                    self.report(doc.rel, number, f"new finding {cells[0]} is an execution receipt; record it as a manifest row")

    def git(self, *args: str) -> subprocess.CompletedProcess[str] | None:
        try:
            return subprocess.run(["git", "-C", str(self.root), *args], capture_output=True, text=True, check=False)
        except OSError:
            return None

    def git_show(self, rel: str) -> str | None:
        result = self.git("show", f"{self.base}:{rel}")
        return result.stdout if result is not None and result.returncode == 0 else None

    def git_ls(self, directory: str) -> list[str]:
        result = self.git("ls-tree", "-r", "--name-only", self.base, "--", directory)
        if result is None or result.returncode != 0:
            return []
        return [line for line in result.stdout.split("\n") if line.endswith(".md")]

    def base_exists(self) -> bool:
        result = self.git("rev-parse", "--verify", "--quiet", f"{self.base}^{{commit}}")
        return result is not None and result.returncode == 0

    def check_manifest(self) -> None:
        manifest = self.docs.get(MANIFEST_FILE)
        if manifest is None:
            return
        for number, line, fenced in iter_lines(manifest.lines):
            if fenced or not line.startswith("|"):
                continue
            cells = table_cells(line)
            if len(cells) < 6 or not MANIFEST_ID.fullmatch(cells[0]):
                continue
            if len(line.encode("utf-8")) > MANIFEST_ROW_CAP:
                self.report(MANIFEST_FILE, number, f"manifest row exceeds {MANIFEST_ROW_CAP} bytes")
            _identifier, path, digest, _date, _session, klass = cells[:6]
            if klass not in {"local-private", "external"}:
                self.report(MANIFEST_FILE, number, f"class must be local-private or external, not {klass!r}")
            if digest == "unstated":
                continue
            if not re.fullmatch(r"[0-9a-f]{64}", digest):
                self.report(MANIFEST_FILE, number, "sha256 column must be a 64-hex digest or 'unstated'")
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

    def check_staleness(self) -> None:
        status = self.current.get(STATUS_FILE)
        match = RECONCILED.search(status.text) if status else None
        if match is None:
            return
        try:
            reconciled = date.fromisoformat(match.group(1))
        except ValueError:
            self.report(STATUS_FILE, 0, f"invalid Last reconciled date {match.group(1)}")
            return
        age = (self.today - reconciled).days
        if age > STALE_DAYS:
            self.warn(STATUS_FILE, 0, f"last reconciled {age} days ago; run the checkpoint reconcile in .agents/rules/docs.md")

    # -- layout checks ----------------------------------------------------

    def check_glue(self) -> None:
        targets = dict(self.current)
        if MANIFEST_FILE in self.docs:
            targets[MANIFEST_FILE] = self.docs[MANIFEST_FILE]
        for rel, doc in targets.items():
            for number, line, fenced in iter_lines(doc.lines):
                if fenced:
                    continue
                tokens = glued_tokens(line)
                if tokens:
                    self.report(rel, number, f"glued tokens {', '.join(tokens)}; restore the spaces or reword")

    def check_filenames(self) -> None:
        for rel in self.current:
            if DATED_NAME.search(rel.rsplit("/", 1)[-1]):
                self.report(rel, 0, "dated filename on the current surface; name the topic, not the session")

    def check_provenance(self) -> None:
        for rel, doc in self.current.items():
            found: list[int] = []
            for number, line, fenced in iter_lines(doc.lines):
                if fenced or line.startswith("|"):
                    continue
                found.extend(number for _match in PROVENANCE.finditer(line))
            for number in found[1:]:
                self.report(rel, number, "more than one commit:path provenance line; keep only 'Before restructure: <sha>:<path>'")

    def tier_size(self, rels: Iterable[str]) -> tuple[int, list[str]]:
        """Total bytes of the files that exist, and the names that do not."""
        total = 0
        missing: list[str] = []
        for rel in rels:
            path = self.root / rel
            if path.is_file():
                total += path.stat().st_size
            else:
                missing.append(rel)
        return total, missing

    def check_read_tiers(self) -> None:
        """Startup tier fails over budget; task tiers are reported, never failed."""
        startup, _missing = self.tier_size(STARTUP_FILES)
        if startup > STARTUP_BUDGET:
            self.report("docs", 0, f"startup read tier is {startup} bytes; budget is {STARTUP_BUDGET}")
        for task, rels in TASK_TIERS.items():
            size, missing = self.tier_size(rels)
            for rel in missing:
                self.warn(rel, 0, f"task read tier {task!r} names a missing file")
            if size > TASK_BUDGET:
                self.warn("docs", 0, f"task read tier {task!r} is {size} bytes; budget is {TASK_BUDGET}")


def find_root(start: Path) -> Path:
    for candidate in (start, *start.parents):
        if (candidate / ".git").exists() and (candidate / "docs").is_dir():
            return candidate
    raise SystemExit("docs_lint: repository root with docs/ not found")


class Options(argparse.Namespace):
    root: Path | None = None
    warn: bool = False
    local: bool = False
    base: str = "HEAD"


def parse_options(argv: Sequence[str] | None) -> Options:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    _ = parser.add_argument("--root", type=Path, default=None)
    _ = parser.add_argument("--warn", action="store_true", help="report problems but exit 0")
    _ = parser.add_argument("--local", action="store_true", help="verify local-private manifest rows on disk")
    _ = parser.add_argument("--base", default="HEAD", help="revision whose ledger IDs must survive (default HEAD)")
    options = Options()
    _ = parser.parse_args(argv, namespace=options)
    return options


def main(argv: Sequence[str] | None = None) -> int:
    options = parse_options(argv)
    root = options.root.resolve() if options.root else find_root(Path.cwd().resolve())
    linter = Linter(root, local=options.local, base=options.base)
    total = linter.run()
    for problem in linter.problems:
        print(problem.render())
    for warning in linter.warnings:
        print(f"warning: {warning.render()}")
    startup, _missing = linter.tier_size(STARTUP_FILES)
    print(
        f"docs_lint: current prose surface {total} bytes of {AGGREGATE_CAP}; startup tier {startup} bytes of {STARTUP_BUDGET}; "
        + f"{len(linter.problems)} problem(s), {len(linter.warnings)} warning(s)"
    )
    if linter.problems and not options.warn:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
