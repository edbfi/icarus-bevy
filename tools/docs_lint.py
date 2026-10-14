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

The evidence index is ``evidence/<area>[/<topic>]/NNN.jsonl``: one JSON record
per line per retained artifact, fetched by ID and never loaded whole. A shard
folder joined with ``-`` names its findings ledger (``save-format/c001`` holds
the evidence of ``save-format-c001.md``). The Markdown manifest
it replaced is read only at ``--base`` so its IDs must survive.

The script reads files only. ``--warn`` reports every problem but exits 0, for
use while a migration is in flight. ``--local`` additionally verifies that
active ``local-private`` evidence exists on disk with matching digests and that
the Evidence and unknown IDs listed on a ``rea-bundle`` row are members of that
exported bundle. ``--base REF`` names the revision whose ledger and manifest IDs
must survive (default ``HEAD``); its findings may still use the legacy single
``findings.md``.

Usage::

    python3 tools/docs_lint.py                  # strict, repository root found from cwd
    python3 tools/docs_lint.py --warn           # report only
    python3 tools/docs_lint.py --local          # also verify local-private evidence
    python3 tools/docs_lint.py --root P         # explicit repository root
    python3 tools/docs_lint.py --base origin/main
    python3 tools/docs_lint.py --next-id M      # next free M-, F- or U- ID
    python3 tools/docs_lint.py --list-local     # active local-private evidence paths
"""

from __future__ import annotations

import argparse
from collections.abc import Callable, Iterable, Iterator, Sequence
from dataclasses import dataclass, field
from datetime import date
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
from typing import TypeIs, cast, final

KIB = 1024

# ---------------------------------------------------------------------------
# Policy constants. Changing these is a deliberate, reviewed act.
# ---------------------------------------------------------------------------

CURRENT_ROOTS: tuple[str, ...] = ("README.md", "docs", "evidence/README.md")
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
    "evidence/README.md": 2 * KIB,
}
# Directory caps; the first matching prefix wins.
PREFIX_CAPS: tuple[tuple[str, int], ...] = (
    ("docs/ledgers/findings/retired/", 64 * KIB),
    ("docs/ledgers/findings/", 48 * KIB),
)
DIGEST_CAP = 8 * KIB
ACTIVE_FINDINGS_CAP = 96 * KIB
LEDGER_ROW_CAP = 400
AGGREGATE_CAP = 160 * KIB

# Evidence index: retrieval-tier data, outside the prose surface and read tiers.
EVIDENCE_DIR = "evidence"
SHARD_NAME = re.compile(r"\d{3}\.jsonl")
CATEGORY_NAME = re.compile(r"[a-z][a-z0-9]*(?:-[a-z0-9]+)*")
CATEGORY_DEPTH = 2
SHARD_CAP = 64 * KIB
RECORD_CAP = 2 * KIB
LABEL_CAP = 160
NOTE_CAP = 200
LISTED_ID_LIMIT = 32
BUNDLE_PARSE_CAP = 64 * KIB * KIB
# M-001..M-270 predate the index; they may keep directory paths, unknown digests and long labels.
LEGACY_LAST = 270
RECORD_CLASSES: frozenset[str] = frozenset({"local-private", "tracked", "external"})
RECORD_KINDS: frozenset[str] = frozenset({"rea-bundle", "receipt", "review", "input", "build", "report", "revision"})
RECORD_STATUSES: frozenset[str] = frozenset({"active", "retired", "missing"})
REQUIRED_KEYS: tuple[str, ...] = ("id", "path", "sha256", "class", "kind", "date", "label", "status")
OPTIONAL_KEYS: tuple[str, ...] = ("ev", "unk", "records", "superseded_by", "note", "legacy")
LOCAL_PREFIXES: tuple[str, ...] = ("work/", "reference-data/", "icarus-client-orig/", "icarus-orig/")

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
# The Markdown manifest that preceded the JSONL index; read only at --base.
LEGACY_MANIFEST_FILE = "evidence/manifest.md"
CODE_REFERENCE_ROOTS: tuple[str, ...] = ("src", "tools", ".github")
# Execution receipts belong in the evidence index, not in new finding rows.
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
RECORD_ID = re.compile(r"M-(?:\d{3}|[1-9]\d{3,})")
EVIDENCE_ID = re.compile(r"\bev_[0-9a-f]{64}\b")
REA_UNKNOWN_ID = re.compile(r"\bunk_[0-9a-f]{64}\b")
# Citation of one Evidence or REA unknown ID through the index row that lists it.
LISTED_SELECTOR = re.compile(r"\b(M-\d{3,})/(ev|unk)_([0-9a-f]+)")
SELECTOR_HEX = 12
# Pre-REA case notes (E-NNN) number from E-001 in every case; cite them through their row.
LEGACY_NOTE = re.compile(r"\b(M-\d{3,})/(E-\d{3,})\b")
BARE_LEGACY_NOTE = re.compile(r"(?<![/\w-])E-\d{3,}\b")
JSON_RECORD_ID = re.compile(r'"id"\s*:\s*"(M-\d{3,})"')
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


# ---------------------------------------------------------------------------
# Evidence index records
# ---------------------------------------------------------------------------


class RecordError(ValueError):
    """A malformed evidence index line."""


def _object_dict(value: object) -> TypeIs[dict[object, object]]:
    return isinstance(value, dict)


def _object_list(value: object) -> TypeIs[list[object]]:
    return isinstance(value, list)


def _unique_pairs(pairs: list[tuple[str, object]]) -> dict[str, object]:
    found: dict[str, object] = {}
    for key, value in pairs:
        if key in found:
            raise RecordError(f"duplicate key {key!r}")
        found[key] = value
    return found


def parse_json_object(text: str | bytes) -> dict[str, object]:
    """One JSON object with string keys; duplicate keys are rejected."""
    try:
        # json.loads is untyped; every value is an object until narrowed.
        value = cast(object, json.loads(text, object_pairs_hook=_unique_pairs))
    except json.JSONDecodeError as error:
        raise RecordError(f"invalid JSON: {error.msg}") from error
    if not _object_dict(value):
        raise RecordError("record must be a JSON object")
    return {str(key): item for key, item in value.items()}


def _required_text(record: dict[str, object], key: str) -> str:
    value = record[key]
    if not isinstance(value, str) or not value:
        raise RecordError(f"{key} must be a nonempty string")
    return value


def _nullable_text(record: dict[str, object], key: str) -> str | None:
    value = record.get(key)
    if value is None:
        return None
    if not isinstance(value, str) or not value:
        raise RecordError(f"{key} must be a nonempty string or null")
    return value


def _text_list(record: dict[str, object], key: str) -> tuple[str, ...]:
    if key not in record:
        return ()
    value = record[key]
    if not _object_list(value) or not value:
        raise RecordError(f"{key} must be a nonempty list; omit the key instead")
    items: list[str] = []
    for item in value:
        if not isinstance(item, str):
            raise RecordError(f"{key} entries must be strings")
        items.append(item)
    return tuple(items)


@dataclass(frozen=True)
class ManifestRow:
    """One evidence index record: where an artifact lives, its identity and state."""

    rel: str
    line: int
    identifier: str
    path: str
    sha256: str | None
    klass: str
    kind: str
    date: str | None
    label: str
    status: str
    ev: tuple[str, ...] = ()
    unk: tuple[str, ...] = ()
    records: int | None = None
    superseded_by: str | None = None
    note: str | None = None
    legacy: bool = False

    @property
    def number(self) -> int:
        return int(self.identifier.removeprefix("M-"))

    @classmethod
    def from_record(cls, rel: str, line: int, record: dict[str, object]) -> ManifestRow:
        missing = [key for key in REQUIRED_KEYS if key not in record]
        if missing:
            raise RecordError(f"missing keys {', '.join(missing)}")
        unknown = sorted(set(record) - set(REQUIRED_KEYS) - set(OPTIONAL_KEYS))
        if unknown:
            raise RecordError(f"unknown keys {', '.join(unknown)}")
        absent_null = [key for key in OPTIONAL_KEYS if key in record and record[key] is None]
        if absent_null:
            raise RecordError(f"omit optional keys instead of null: {', '.join(absent_null)}")
        count = record.get("records")
        records: int | None = None
        if count is not None:
            if not isinstance(count, int) or isinstance(count, bool) or count < 0:
                raise RecordError("records must be a nonnegative integer")
            records = count
        if "legacy" in record and record["legacy"] is not True:
            raise RecordError("legacy must be true when present; omit it otherwise")
        legacy = "legacy" in record
        return cls(
            rel=rel,
            line=line,
            identifier=_required_text(record, "id"),
            path=_required_text(record, "path"),
            sha256=_nullable_text(record, "sha256"),
            klass=_required_text(record, "class"),
            kind=_required_text(record, "kind"),
            date=_nullable_text(record, "date"),
            label=_required_text(record, "label"),
            status=_required_text(record, "status"),
            ev=_text_list(record, "ev"),
            unk=_text_list(record, "unk"),
            records=records,
            superseded_by=_nullable_text(record, "superseded_by"),
            note=_nullable_text(record, "note"),
            legacy=legacy,
        )


def parse_shard(rel: str, text: str) -> tuple[list[ManifestRow], list[Problem]]:
    """Parse one JSONL shard; every malformed line is a problem at its line."""
    rows: list[ManifestRow] = []
    problems: list[Problem] = []
    lines = text.split("\n")
    if lines and lines[-1] == "":
        _ = lines.pop()
    for number, line in enumerate(lines, start=1):
        if not line.strip():
            problems.append(Problem(rel, number, "blank line in evidence shard"))
            continue
        try:
            rows.append(ManifestRow.from_record(rel, number, parse_json_object(line)))
        except RecordError as error:
            problems.append(Problem(rel, number, f"invalid evidence record: {error}"))
    return rows, problems


def legacy_markdown_rows(doc: Document) -> list[ManifestRow]:
    """Rows of the Markdown manifest that preceded the JSONL index."""
    rows: list[ManifestRow] = []
    for number, line, fenced in iter_lines(doc.lines):
        if fenced or not line.startswith("|"):
            continue
        cells = table_cells(line)
        if len(cells) < 6 or not MANIFEST_ID.fullmatch(cells[0]):
            continue
        identifier, path, digest, day, session, klass = cells[:6]
        rows.append(
            ManifestRow(
                rel=doc.rel,
                line=number,
                identifier=identifier,
                path=path,
                sha256=None if digest == "unstated" else digest,
                klass=klass,
                kind="receipt",
                date=None if day == "unstated" else day,
                label=session,
                status="active",
                legacy=True,
            )
        )
    return rows


def sha256_file(path: Path) -> str:
    with path.open("rb") as handle:
        return hashlib.file_digest(handle, "sha256").hexdigest()


def bundle_members(path: Path) -> tuple[set[str], set[str]]:
    """Evidence IDs (records[].evidence_id) and unknown IDs (unknowns[].unknown_id) of an exported REA bundle."""
    bundle = parse_json_object(path.read_bytes())
    found: list[set[str]] = []
    for section, key in (("records", "evidence_id"), ("unknowns", "unknown_id")):
        items = bundle.get(section)
        if not _object_list(items):
            raise RecordError(f"bundle {section} must be a list")
        identifiers: set[str] = set()
        for item in items:
            if _object_dict(item):
                value = item.get(key)
                if isinstance(value, str):
                    identifiers.add(value)
        found.append(identifiers)
    return found[0], found[1]


def headings_of(path: Path) -> set[str]:
    try:
        return {slugify(text) for _n, _l, text in Document(path.name, path.read_text(encoding="utf-8")).headings()}
    except (OSError, UnicodeDecodeError):
        return set()


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
    rows: list[ManifestRow]
    manifest: dict[str, ManifestRow]
    shards: dict[str, str]

    def __init__(self, root: Path, *, local: bool, base: str = "HEAD", today: date | None = None) -> None:
        self.root = root
        self.local = local
        self.base = base
        self.today = today or date.today()
        self.problems = []
        self.warnings = []
        self.docs = {}
        self.current = {}
        self.rows = []
        self.manifest = {}
        self.shards = {}

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
        self.load_manifest()

    def load_manifest(self) -> None:
        """Index rows in shard order; the first definition of an ID wins."""
        directory = self.root / EVIDENCE_DIR
        if directory.is_dir():
            for path in sorted(directory.rglob("*.jsonl")):
                rel = path.relative_to(self.root).as_posix()
                try:
                    text = path.read_text(encoding="utf-8")
                except UnicodeDecodeError:
                    self.report(rel, 0, "not valid UTF-8")
                    continue
                self.shards[rel] = text
                rows, problems = parse_shard(rel, text)
                self.problems.extend(problems)
                self.rows.extend(rows)
        for row in self.rows:
            if row.identifier in self.manifest:
                first = self.manifest[row.identifier]
                self.report(row.rel, row.line, f"duplicate ID {row.identifier} (first at {first.rel}:{first.line})")
                continue
            self.manifest[row.identifier] = row

    @property
    def index_bytes(self) -> int:
        return sum(len(text.encode("utf-8")) for text in self.shards.values())

    def tracked_markdown(self) -> set[str]:
        tracked: set[str] = set()
        for root in CURRENT_ROOTS + INSTRUCTION_ROOTS:
            path = self.root / root
            if path.is_file() and path.suffix == ".md":
                tracked.add(root)
            elif path.is_dir():
                for file in path.rglob("*.md"):
                    tracked.add(file.relative_to(self.root).as_posix())
        return tracked

    @staticmethod
    def is_current(rel: str) -> bool:
        if is_under(rel, INSTRUCTION_ROOTS):
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
        self.check_legacy_notes()
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
                    self.report(rel, number, "clock time in prose; remove it (evidence rows carry the date)")
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
        findings = self.findings_docs()
        defined_f: set[str] = self.unique_ids(findings, FINDING_ID)
        defined_u: set[str] = self.unique_ids([unknowns], UNKNOWN_ID) if unknowns else set()
        defined_m: set[str] = set(self.manifest)
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
                self.check_listed_citations(rel, number, line)

    def check_listed_citations(self, rel: str, number: int, line: str) -> None:
        """Evidence, REA unknown and legacy note IDs are cited through the index row that lists them."""
        if EVIDENCE_ID.search(line) or REA_UNKNOWN_ID.search(line):
            self.report(rel, number, f"full ev_/unk_ identifier in prose; cite M-NNN/ev_<{SELECTOR_HEX} hex> from its index row")
        for match in LISTED_SELECTOR.finditer(line):
            identifier, prefix, digits = match.groups()
            cited = f"{identifier}/{prefix}_{digits}"
            if len(digits) != SELECTOR_HEX:
                self.report(rel, number, f"{cited}: cite exactly {SELECTOR_HEX} hex digits")
                continue
            row = self.manifest.get(identifier)
            if row is None:
                continue
            listed = row.ev if prefix == "ev" else row.unk
            hits = [entry for entry in listed if entry.startswith(f"{prefix}_{digits}")]
            if len(hits) != 1:
                self.report(rel, number, f"{cited} matches {len(hits)} entries of the {prefix} list of {identifier}")
        for match in LEGACY_NOTE.finditer(line):
            row = self.manifest.get(match.group(1))
            if row is not None and not row.legacy:
                self.report(rel, number, f"{match.group(0)}: E- notes exist only under legacy rows")

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
                    self.report(doc.rel, number, f"new finding {cells[0]} is an execution receipt; record it as an evidence index row")

    def git(self, *args: str) -> subprocess.CompletedProcess[str] | None:
        try:
            return subprocess.run(["git", "-C", str(self.root), *args], capture_output=True, text=True, check=False)
        except OSError:
            return None

    def git_show(self, rel: str) -> str | None:
        result = self.git("show", f"{self.base}:{rel}")
        return result.stdout if result is not None and result.returncode == 0 else None

    def git_ls(self, directory: str, suffix: str = ".md") -> list[str]:
        result = self.git("ls-tree", "-r", "--name-only", self.base, "--", directory)
        if result is None or result.returncode != 0:
            return []
        return [line for line in result.stdout.split("\n") if line.endswith(suffix)]

    def base_exists(self) -> bool:
        result = self.git("rev-parse", "--verify", "--quiet", f"{self.base}^{{commit}}")
        return result is not None and result.returncode == 0

    # -- evidence index ----------------------------------------------------

    def check_manifest(self) -> None:
        self.check_shards()
        for row in self.manifest.values():
            self.check_row(row)
            if self.local:
                self.check_row_on_disk(row)
        numbers = {row.number for row in self.manifest.values()}
        if numbers:
            gaps = sorted(set(range(1, max(numbers) + 1)) - numbers)
            if gaps:
                self.report(EVIDENCE_DIR, 0, f"manifest IDs are not contiguous; missing {', '.join(f'M-{n:03d}' for n in gaps[:10])}")
        self.check_manifest_history()

    def check_shards(self) -> None:
        numbers: dict[str, list[int]] = {}
        for rel, text in self.shards.items():
            *folders, name = rel.split("/")[1:]
            if self.check_category(rel, folders):
                if not SHARD_NAME.fullmatch(name):
                    self.report(rel, 0, "shard name must be three digits, NNN.jsonl")
                else:
                    numbers.setdefault("/".join(folders), []).append(int(name[:3]))
            size = len(text.encode("utf-8"))
            if size > SHARD_CAP:
                self.report(rel, 0, f"{size} bytes exceeds the {SHARD_CAP}-byte shard cap; roll over to a numbered shard")
            previous = 0
            for row in self.rows:
                if row.rel != rel or not RECORD_ID.fullmatch(row.identifier):
                    continue
                if row.number <= previous:
                    self.report(rel, row.line, f"{row.identifier} is out of order; sort records by ID")
                previous = max(previous, row.number)
            for number, line in enumerate(text.split("\n"), start=1):
                if len(line.encode("utf-8")) > RECORD_CAP:
                    self.report(rel, number, f"record exceeds {RECORD_CAP} bytes")
        for folder, found in sorted(numbers.items()):
            if sorted(found) != list(range(1, len(found) + 1)):
                self.report(f"{EVIDENCE_DIR}/{folder}", 0, "shards must be numbered 001, 002, ... without gaps")

    def check_category(self, rel: str, folders: Sequence[str]) -> bool:
        """A shard folder is evidence/<area>[/<topic>] and names a findings ledger, active or retired."""
        if not folders:
            self.report(rel, 0, "shards live in a category folder: evidence/<area>[/<topic>]/NNN.jsonl")
            return False
        if len(folders) > CATEGORY_DEPTH or not all(CATEGORY_NAME.fullmatch(folder) for folder in folders):
            self.report(rel, 0, f"category folders are lowercase and at most {CATEGORY_DEPTH} deep")
            return False
        ledger = "-".join(folders) + ".md"
        if not any((self.root / directory / ledger).is_file() for directory in (FINDINGS_DIR, RETIRED_DIR)):
            self.report(rel, 0, f"category {'/'.join(folders)} has no findings ledger {ledger}")
            return False
        return True

    def check_row(self, row: ManifestRow) -> None:
        def problem(message: str) -> None:
            self.report(row.rel, row.line, f"{row.identifier}: {message}")

        if not RECORD_ID.fullmatch(row.identifier):
            problem("ID must be M- followed by at least three digits without extra leading zeros")
            return
        if row.legacy and row.number > LEGACY_LAST:
            problem(f"legacy applies only to M-001..M-{LEGACY_LAST:03d}")
        for value, allowed, name in (
            (row.klass, RECORD_CLASSES, "class"),
            (row.kind, RECORD_KINDS, "kind"),
            (row.status, RECORD_STATUSES, "status"),
        ):
            if value not in allowed:
                problem(f"{name} must be one of {', '.join(sorted(allowed))}, not {value!r}")
        if row.sha256 is not None and not re.fullmatch(r"[0-9a-f]{64}", row.sha256):
            problem("sha256 must be 64 lowercase hex digits or null")
        if row.date is not None:
            try:
                _ = date.fromisoformat(row.date)
            except ValueError:
                problem(f"invalid date {row.date!r}")
        if row.note is not None and len(row.note) > NOTE_CAP:
            problem(f"note exceeds {NOTE_CAP} characters")
        self.check_row_lists(row, problem)
        self.check_row_lifecycle(row, problem)
        if row.status != "missing":
            self.check_row_path(row, problem)
        if not row.legacy:
            self.check_new_row(row, problem)

    def check_row_lists(self, row: ManifestRow, problem: Callable[[str], None]) -> None:
        if (row.ev or row.unk or row.records is not None) and row.kind != "rea-bundle":
            problem("ev, unk and records belong only to rea-bundle rows")
        for name, values, pattern in (("ev", row.ev, EVIDENCE_ID), ("unk", row.unk, REA_UNKNOWN_ID)):
            if len(values) > LISTED_ID_LIMIT:
                problem(f"{name} lists more than {LISTED_ID_LIMIT} IDs; list only cited IDs")
            if len(set(values)) != len(values):
                problem(f"{name} lists an ID twice")
            for value in values:
                if not pattern.fullmatch(value):
                    problem(f"{name} entry {value!r} is not a full {name}_ identifier")

    def check_row_lifecycle(self, row: ManifestRow, problem: Callable[[str], None]) -> None:
        if row.superseded_by is not None:
            if row.status != "retired":
                problem("superseded_by requires status retired")
            if row.superseded_by == row.identifier or row.superseded_by not in self.manifest:
                problem(f"superseded_by {row.superseded_by} is not another defined row")
        if row.status in {"retired", "missing"} and row.note is None and row.superseded_by is None:
            problem(f"status {row.status} needs a note or superseded_by")
        if row.status == "missing" and not row.legacy and row.date is not None:
            try:
                age = (self.today - date.fromisoformat(row.date)).days
            except ValueError:
                return
            if age > STALE_DAYS:
                self.warn(row.rel, row.line, f"{row.identifier}: missing for over {STALE_DAYS} days; restore or retire it")

    def check_row_path(self, row: ManifestRow, problem: Callable[[str], None]) -> None:
        if row.klass == "local-private" and not row.path.startswith(LOCAL_PREFIXES):
            message = f"local-private path must start with {', '.join(LOCAL_PREFIXES)}: {row.path}"
            if row.legacy:
                self.warn(row.rel, row.line, f"{row.identifier}: {message}")
            else:
                problem(message)
        if row.klass == "tracked":
            path_part, _, fragment = row.path.partition("#")
            target = self.root / path_part
            if not target.exists():
                problem(f"tracked path does not exist: {path_part}")
            elif fragment and target.suffix == ".md" and fragment not in headings_of(target):
                problem(f"tracked path has no heading anchor #{fragment}")

    def check_new_row(self, row: ManifestRow, problem: Callable[[str], None]) -> None:
        if row.sha256 is None:
            problem("new rows need the artifact's sha256")
        if row.date is None:
            problem("new rows need a date")
        if row.path.endswith("/") or row.path == "unstated":
            problem("new rows name one file, never a directory or an unstated path")
        if row.kind == "rea-bundle" and row.records is None:
            problem("rea-bundle rows record the bundle's record count")
        if len(row.label) > LABEL_CAP:
            problem(f"label exceeds {LABEL_CAP} characters")
        if UTC_TIME.search(row.label) or HEX64.search(row.label) or PRIVATE_PATH.search(row.label):
            problem("label carries a clock time, digest or private path")

    def check_row_on_disk(self, row: ManifestRow) -> None:
        if row.klass != "local-private" or row.status != "active":
            return
        path = self.root / row.path
        if not path.exists():
            self.report(row.rel, row.line, f"{row.identifier}: local-private evidence missing: {row.path}")
            return
        if not path.is_file():
            if not row.legacy:
                self.report(row.rel, row.line, f"{row.identifier}: local-private evidence is not a file: {row.path}")
            return
        if row.sha256 is not None and sha256_file(path) != row.sha256:
            self.report(row.rel, row.line, f"{row.identifier}: local-private evidence digest mismatch: {row.path}")
            return
        if row.kind == "rea-bundle":
            self.check_bundle_membership(row, path)

    def check_bundle_membership(self, row: ManifestRow, path: Path) -> None:
        if path.stat().st_size > BUNDLE_PARSE_CAP:
            self.warn(row.rel, row.line, f"{row.identifier}: bundle exceeds {BUNDLE_PARSE_CAP} bytes; membership unverified")
            return
        try:
            evidence, unknowns = bundle_members(path)
        except (RecordError, UnicodeDecodeError) as error:
            self.report(row.rel, row.line, f"{row.identifier}: unreadable REA bundle: {error}")
            return
        if row.records is not None and row.records != len(evidence):
            self.report(row.rel, row.line, f"{row.identifier}: bundle holds {len(evidence)} records, row says {row.records}")
        for name, listed, members in (("ev", row.ev, evidence), ("unk", row.unk, unknowns)):
            for identifier in listed:
                if identifier not in members:
                    self.report(row.rel, row.line, f"{row.identifier}: {name} entry {identifier} is not in the bundle")

    def base_manifest_rows(self) -> dict[str, ManifestRow]:
        """Rows defined at the base revision, in either the Markdown or the JSONL layout."""
        rows: dict[str, ManifestRow] = {}
        legacy = self.git_show(LEGACY_MANIFEST_FILE)
        if legacy is not None:
            for row in legacy_markdown_rows(Document(LEGACY_MANIFEST_FILE, legacy)):
                _ = rows.setdefault(row.identifier, row)
        for rel in self.git_ls(EVIDENCE_DIR, ".jsonl"):
            text = self.git_show(rel)
            if text is None:
                continue
            parsed, _problems = parse_shard(rel, text)
            for row in parsed:
                _ = rows.setdefault(row.identifier, row)
            # A base line that no longer parses still defines its ID.
            for match in JSON_RECORD_ID.finditer(text):
                identifier = match.group(1)
                if identifier not in rows:
                    rows[identifier] = ManifestRow(rel, 0, identifier, "", None, "", "", None, "", "")
        return rows

    def check_manifest_history(self) -> None:
        """Pushed IDs survive; pushed facts change only as the rules allow."""
        for identifier, before in sorted(self.base_manifest_rows().items()):
            after = self.manifest.get(identifier)
            if after is None:
                self.report(before.rel, 0, f"ID {identifier} existed at {self.base} and was removed; retire it instead")
                continue
            if before.rel.endswith(".md") or not before.kind:
                continue
            for name, old, new in (
                ("date", before.date, after.date),
                ("ev", before.ev, after.ev),
                ("unk", before.unk, after.unk),
                ("records", before.records, after.records),
                ("kind", before.kind, after.kind if not before.legacy else before.kind),
            ):
                if old != new:
                    self.report(after.rel, after.line, f"{identifier}: {name} changed since {self.base}; it is immutable")
            for name, old, new in (("path", before.path, after.path), ("sha256", before.sha256, after.sha256)):
                if new != old and (after.note is None or str(old) not in after.note):
                    self.report(after.rel, after.line, f"{identifier}: {name} changed; keep the old value in note")

    def check_legacy_notes(self) -> None:
        """Bare E-NNN case notes are ambiguous; lines added since the base cite them as M-NNN/E-NNN."""
        if not self.base_exists():
            return
        for rel, doc in self.current.items():
            base_text = self.git_show(rel)
            base_lines = set(base_text.split("\n")) if base_text is not None else set[str]()
            for number, line, fenced in iter_lines(doc.lines):
                if fenced or line in base_lines:
                    continue
                stripped = LEGACY_NOTE.sub("", INLINE_CODE.sub("", line))
                match = BARE_LEGACY_NOTE.search(stripped)
                if match:
                    self.report(rel, number, f"bare case note {match.group(0)}; cite it as M-NNN/{match.group(0)} through its row")

    def next_id(self, prefix: str) -> str:
        if prefix == "M":
            numbers = [row.number for row in self.manifest.values() if RECORD_ID.fullmatch(row.identifier)]
        else:
            pattern = FINDING_ID if prefix == "F" else UNKNOWN_ID
            docs = self.findings_docs() if prefix == "F" else [doc for rel, doc in self.docs.items() if rel == UNKNOWNS_FILE]
            numbers = [int(identifier.split("-")[1]) for doc in docs for _n, identifier in ledger_ids(doc, pattern)]
        return f"{prefix}-{max(numbers, default=0) + 1:03d}"

    def local_paths(self) -> list[str]:
        return sorted(
            {row.path for row in self.manifest.values() if row.klass == "local-private" and row.status == "active"}
        )

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
        for rel, doc in self.current.items():
            for number, line, fenced in iter_lines(doc.lines):
                if fenced:
                    continue
                tokens = glued_tokens(line)
                if tokens:
                    self.report(rel, number, f"glued tokens {', '.join(tokens)}; restore the spaces or reword")
        for row in self.rows:
            tokens = glued_tokens(f"{row.label} {row.note or ''}")
            if tokens:
                self.report(row.rel, row.line, f"glued tokens {', '.join(tokens)}; restore the spaces or reword")

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
    next_id: str | None = None
    list_local: bool = False


def parse_options(argv: Sequence[str] | None) -> Options:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    _ = parser.add_argument("--root", type=Path, default=None)
    _ = parser.add_argument("--warn", action="store_true", help="report problems but exit 0")
    _ = parser.add_argument("--local", action="store_true", help="verify local-private manifest rows on disk")
    _ = parser.add_argument("--base", default="HEAD", help="revision whose ledger IDs must survive (default HEAD)")
    _ = parser.add_argument("--next-id", choices=("M", "F", "U"), default=None, help="print the next free ID and exit")
    _ = parser.add_argument("--list-local", action="store_true", help="print active local-private evidence paths and exit")
    options = Options()
    _ = parser.parse_args(argv, namespace=options)
    return options


def main(argv: Sequence[str] | None = None) -> int:
    options = parse_options(argv)
    root = options.root.resolve() if options.root else find_root(Path.cwd().resolve())
    linter = Linter(root, local=options.local, base=options.base)
    if options.next_id is not None or options.list_local:
        linter.load()
        lines = [linter.next_id(options.next_id)] if options.next_id is not None else linter.local_paths()
        print("\n".join(lines))
        return 0
    total = linter.run()
    for problem in linter.problems:
        print(problem.render())
    for warning in linter.warnings:
        print(f"warning: {warning.render()}")
    startup, _missing = linter.tier_size(STARTUP_FILES)
    print(
        f"docs_lint: current prose surface {total} bytes of {AGGREGATE_CAP}; startup tier {startup} bytes of {STARTUP_BUDGET}; "
        + f"evidence index {len(linter.manifest)} rows, {linter.index_bytes} bytes in {len(linter.shards)} shard(s); "
        + f"{len(linter.problems)} problem(s), {len(linter.warnings)} warning(s)"
    )
    if linter.problems and not options.warn:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
