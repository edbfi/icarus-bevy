# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory parity --rev REV --journal DIR``: the gate between a migration journal and the legacy system.

``parity(root, rev, journal)`` replays the journal under ``journal/memory/journal`` and compares it with
the typed catalog parse of ``rev`` (``resolved_catalog`` and ``Index``), not with the exporter's code.
Every mismatch is a line naming its ID; the gate passes only with none. It checks:

- **Every F, U, N and M record**: present with its type; ``legacy`` path and cells byte-identical to
  the catalog's fields (and, rendered back, to the line in the Git blob); parsed fields against the
  catalog (claim, kind, question, severity, trigger, change; evidence path, sha256, class, kind, status,
  date, label, note, records, members in listed order); derived status; supersession; no resolver of a
  migrated unknown, and its Findings column listed (``state.legacy_listed``) exactly as the catalog reads it.
- **Links**: per record, the IDs a journal record links (cites, refs, legacy supersession, with each
  selector's evidence ID) equal the catalog's link targets; the unknown/finding split; every selector
  resolving to one listed member (or a legacy case note on a legacy row). Compatibility, lead and report
  rows (``table_edges``): each record links exactly the IDs and selectors its verbatim cells state (ranges
  over defined IDs), and the total equals a scan of the raw table lines. A tracked evidence row whose path
  a REF of the journal migrates links that REF, with the note, and nothing else changes.
- **Rendering** (``rendering_problems``): every non-empty legacy cell of a migrated record appears
  verbatim in its text, stored and with its derived status (an enum or status cell may differ in case
  only); a release's cells are each named by its ``sources``; a REF's ``heading`` is its body's first line.
- **Released model** (``released_model_problems``): every revision of the journal parses with the model of
  ``origin/main`` (``tools/memory/model.py`` exported by ``git archive`` into a temporary directory), so
  the tool already released reads the migration's journal.
- **Counts** per type against a raw scan of the Git tree (``raw_counts``), the five-or-so unallocated
  finding numbers among them; edge counts (distinct targets per record) from the journal against the
  catalog's, and the catalog's raw link tuples (one per target and cell) for reference.
- **Replay**: no replay problem, dangling link, supersession cycle or fork.
- **``check --ci``** on a scratch clone of ``rev`` holding the journal, so the citation scan covers the
  repository's code and documents; the census of citations to retired records is reported.
- **Bytes**: every byte of every file under ``docs/ledgers/``, ``docs/specs/``, ``docs/research/`` (not
  ``archive/``), ``docs/implementation-status.md`` and ``evidence/`` is in a record's verbatim legacy
  cells, a REF body, or a structural line (``tools.memory.legacy_prose.account``).
- **``protected-paths``** from the journal alone equals the legacy ``docs_lint`` local list for the
  same revision's shards, both resolved against the primary checkout.
"""

from __future__ import annotations

import importlib.util
import io
import re
import shutil
import sys
import tarfile
import tempfile
from collections import Counter, defaultdict
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Protocol, cast

from tools.docs_lint import Linter, parse_shard
from tools.evidence_catalog_model import ID_IN_TEXT, PIPE, RANGE, Entry, Index, references
from tools.evidence_catalog_publish import resolved_catalog
from tools.evidence_catalog_store import commit_id
from tools.memory.check import CITATION_EXCLUDED, CITATION_ROOTS, citations, run_check
from tools.memory.gitio import GitError, git, listed_files, read_blobs, tree_files
from tools.memory.canonical import RenderError, render_text
from tools.memory.journal import JOURNAL, read_tree
from tools.memory.legacy import TRACKED_NOTE
from tools.memory.legacy_prose import account
from tools.memory.legacy_rules import PROSE_SUPERSESSION, SELECTORS, UNSTATED, vocabulary
from tools.memory.model import (
    CONFIDENCES, EVIDENCE_CLASSES, EVIDENCE_KINDS, FINDING_KINDS, SELECTOR, SEVERITIES, TYPES, Record, as_mapping, id_key, load_json,
)
from tools.memory.paths import primary_checkout, protected_paths_for
from tools.memory.state import State, legacy_listed, replay

CODE_ROOTS = ("src/", "tests/", "examples/", "tools/", "config/")


@dataclass
class Parity:
    mismatches: list[str] = field(default_factory=list[str])
    measured: dict[str, str] = field(default_factory=dict[str, str])

    def fail(self, message: str) -> None:
        self.mismatches.append(message)


def raw_counts(root: Path, revision: str) -> tuple[dict[str, int], list[str]]:
    """Record counts from a plain scan of the legacy files, and the finding numbers that have no row."""
    files = {**tree_files(root, revision, "docs/ledgers"), **tree_files(root, revision, "evidence"), **tree_files(root, revision, "docs/specs"),
             **tree_files(root, revision, "docs/research"), **tree_files(root, revision, "docs/implementation-status.md")}
    paths = sorted(files)
    texts = {path: (data or b"").decode() for path, data in zip(paths, read_blobs(root, [files[path] for path in paths]), strict=True)}
    counts: Counter[str] = Counter()
    numbers: set[int] = set()
    for path, text in texts.items():
        lines = text.split("\n")
        if path.startswith("docs/ledgers/findings/"):
            found = [int(match[1]) for line in lines if (match := re.match(r"\|\s*F-([0-9]+)\s*\|", line))]
            counts["finding"] += len(found)
            numbers.update(found)
        elif path == "docs/ledgers/unknowns.md":
            counts["unknown"] += sum(1 for line in lines if re.match(r"\|\s*U-[0-9]+\s*\|", line))
        elif path == "docs/ledgers/bugs.md":
            counts["bug"] += sum(1 for line in lines if re.match(r"\|\s*N-[0-9]+\s*\|", line))
            counts["lead"] += sum(1 for line in lines if re.match(r"\|\s*SL-[0-9]+:", line))
            counts["report"] += sum(1 for line in lines if re.match(r"\|\s*[0-9]+\s*\|", line))
        elif path == "docs/ledgers/compatibility.md":
            separators = [index for index, line in enumerate(lines) if re.fullmatch(r"\|(?: *-+ *\|)+", line)]
            counts["compatibility"] += sum(1 for line in lines if line.startswith("|")) - 2 * len(separators)
        elif path.startswith("evidence/") and path.endswith(".jsonl"):
            counts["evidence"] += sum(1 for line in lines if line.strip())
        if path.startswith("docs/ledgers/") and any(line.strip() and not line.startswith("|") for line in lines):
            counts["reference"] += 1
        elif path.startswith("docs/specs/") and path.endswith(".md"):
            counts["reference"] += 1
        elif path.startswith("docs/research/") and not path.startswith("docs/research/archive/") and path.count("/") == 3 and path.endswith(".md"):
            counts["reference"] += 1
        elif path in ("docs/implementation-status.md", "evidence/README.md"):
            counts["reference"] += 1
    unallocated = [f"F-{number:03d}" for number in range(1, max(numbers, default=0) + 1) if number not in numbers]
    return dict(counts), unallocated


def selector_base(citation: str) -> str:
    return citation.split("/", 1)[0]


def journal_targets(record: Record) -> set[str]:
    legacy = record.legacy
    found = {*record.links.cites, *record.links.refs, *(selector_base(cite) for cite in record.links.cites)}
    if legacy is not None:
        found.update(legacy.superseded_by)
    return found


def compare_row(result: Parity, state: State, index: Index, entry: Entry, record: Record, blob_lines: list[str], prose: dict[str, set[str]],
                sources: dict[str, str]) -> None:
    identifier = entry.record.identifier
    legacy = record.legacy
    if legacy is None or legacy.path != entry.path:
        result.fail(f"{identifier}: legacy path {legacy.path if legacy else None!r} is not {entry.path}")
        return
    source_line = blob_lines[entry.line - 1] if entry.line <= len(blob_lines) else None
    if entry.record.render() != source_line:
        result.fail(f"{identifier}: the catalog's rendering differs from {entry.path}:{entry.line}")
    fields = {name: str(value) for name, value in entry.record.fields.items()}
    if entry.record.kind == "M":
        if legacy.cells.get("row") != source_line or set(legacy.cells) != {"row"}:
            result.fail(f"{identifier}: legacy row is not the shard line byte for byte")
    else:
        expected = dict(fields) | ({"original": entry.record.original} if entry.record.original is not None else {})
        kept = {name: cell for name, cell in legacy.cells.items() if not name.startswith("prose ") and name != "table"}
        if kept != expected:
            result.fail(f"{identifier}: legacy cells differ from the catalog fields: {sorted(set(kept.items()) ^ set(expected.items()))[:2]}")
    status = state.status(identifier)
    match entry.record.kind:
        case "F":
            expect(result, identifier, record, {
                "claim": fields["claim"], "kind": vocabulary(fields["kind"], FINDING_KINDS),
                "confidence": vocabulary(confidence, CONFIDENCES) if (confidence := fields["scope_and_limits"].split(";", 1)[0].strip()) else None,
            })
            retired_to = {link.target for link in index.links[identifier] if link.kind == "supersedes_to"} | prose.get(identifier, set())
            if set(state.superseded_by.get(identifier, ())) != retired_to:
                result.fail(f"{identifier}: superseded by {sorted(state.superseded_by.get(identifier, ()))}, legacy states {sorted(retired_to)}")
            cell = fields["status"]
            wanted = "superseded" if retired_to or re.search(r"\bsuperseded\b", cell) else "retired" if cell.startswith("retired") else cell
            if status != wanted:
                result.fail(f"{identifier}: derived status {status}, legacy status cell {cell!r}")
        case "U":
            expect(result, identifier, record, {
                "question": fields["question"], "severity": vocabulary(fields["severity"], SEVERITIES) if fields["severity"] else None,
                "next_probe": fields["next_probe"] or None,
            })
            if resolvers := state.resolved_by.get(identifier, ()):
                result.fail(f"{identifier}: resolved by {list(resolvers)}; the legacy table names no resolver")
            listed = {link.target for link in index.links[identifier] if link.field == "findings"}
            if set(legacy_listed(record, "findings")) != listed:
                result.fail(f"{identifier}: lists {list(legacy_listed(record, 'findings'))}, the Findings cell states {sorted(listed, key=id_key)}")
            if status != fields["status"]:
                result.fail(f"{identifier}: derived status {status}, legacy {fields['status']!r}")
        case "N":
            expect(result, identifier, record, {"trigger": fields["trigger_and_former_behavior"], "change": fields["change"]})
            # The native table has no status column: the status is unstated, the framing verbatim ledger text.
            if record.status != UNSTATED or legacy.cells.get("table", "\0") not in "\n".join(blob_lines):
                result.fail(f"{identifier}: status {record.status} or its table cell is not the ledger's framing")
        case _:
            compare_evidence(result, state, entry, record, sources)
    if entry.record.kind != "M":
        catalog_targets = {link.target for link in index.links[identifier]} | prose.get(identifier, set())
        if journal_targets(record) != catalog_targets:
            result.fail(f"{identifier}: links {sorted(journal_targets(record) ^ catalog_targets, key=id_key)} differ from the catalog")


def compare_evidence(result: Parity, state: State, entry: Entry, record: Record, sources: dict[str, str]) -> None:
    row = entry.record.evidence(entry.path, entry.line)
    identifier = row.identifier
    expect(result, identifier, record, {
        "path": row.path, "sha256": row.sha256, "class": vocabulary(row.klass, EVIDENCE_CLASSES),
        "kind": vocabulary(row.kind, EVIDENCE_KINDS), "date": row.date, "label": row.label, "records": row.records,
    })
    if record.status != vocabulary(row.status, TYPES["evidence"].statuses):
        result.fail(f"{identifier}: status {record.status}, legacy {row.status}")
    if record.items("members") != (*row.ev, *row.unk):
        result.fail(f"{identifier}: members differ from the listed ev and unk IDs")
    reference = sources.get(row.path.partition("#")[0]) if row.klass == "tracked" else None
    if record.notes != (*((row.note,) if row.note is not None else ()), *((TRACKED_NOTE.format(reference),) if reference else ())):
        result.fail(f"{identifier}: notes differ from the legacy note and the tracked-file note")
    if record.links.refs != ((reference,) if reference else ()) or record.links.cites:
        result.fail(f"{identifier}: links {list(record.links.targets())}; the legacy row and its migrated file state {reference or 'none'}")
    wanted = (row.superseded_by,) if row.superseded_by is not None else ()
    if (record.legacy.superseded_by if record.legacy else ()) != wanted or state.superseded_by.get(identifier, ()) != wanted:
        result.fail(f"{identifier}: superseded_by differs from the legacy {row.superseded_by!r}")


def expect(result: Parity, identifier: str, record: Record, wanted: dict[str, str | int | None]) -> None:
    for name, value in wanted.items():
        if record.fields.get(name) != value:
            result.fail(f"{identifier}: {name} is {record.fields.get(name)!r}, legacy {value!r}")


def edge_class(source: str, target: str) -> str:
    return f"{source.split('-', 1)[0]}->{selector_base(target).split('-', 1)[0]}" + (" selector" if "/" in target else "")


def edge_counts(state: State, index: Index, prose: int) -> dict[str, tuple[int, int]]:
    """(journal, catalog) count per edge class: ledger links by source and target type (supersession apart),
    supersessions (catalog plus prose), evidence supersessions, and finding evidence-cell support."""
    journal: Counter[str] = Counter()
    catalog: Counter[str] = Counter()
    for identifier, entry in state.entries.items():
        record, prefix = entry.record, identifier.split("-", 1)[0]
        legacy = record.legacy
        if prefix == "M":
            journal["M->M superseded_by"] += len(legacy.superseded_by) if legacy else 0
            continue
        if prefix not in ("F", "U", "N"):
            continue
        targets = journal_targets(record) - (set(legacy.superseded_by) if legacy and prefix == "F" else set[str]())
        journal.update(edge_class(identifier, target) for target in targets)
        if prefix == "F":
            journal["supersession"] += len(state.superseded_by.get(identifier, ()))
            cell = legacy.cells.get("evidence", "") if legacy else ""
            stated = set(references(cell, index.entries)) | {match[0] for pattern in SELECTORS for match in pattern.finditer(cell)}
            linked = {*record.links.cites, *(selector_base(cite) for cite in record.links.cites)}
            journal["F->M support"] += len({target for target in linked & stated if target.startswith("M-")})
    for identifier, links in index.links.items():
        prefix = identifier.split("-", 1)[0]
        retired = {link.target for link in links if link.kind == "supersedes_to"}
        if prefix == "M":
            catalog["M->M superseded_by"] += len(retired)
            continue
        targets = {link.target for link in links} - (retired if prefix == "F" else set[str]())
        catalog.update(edge_class(identifier, target) for target in targets)
        catalog["supersession"] += len(retired) if prefix == "F" else 0
        catalog["F->M support"] += sum(1 for link in links if link.kind == "support" and prefix == "F")
    catalog["supersession"] += prose
    return {name: (journal[name], catalog[name]) for name in sorted(set(journal) | set(catalog))}


def texts_of(state: State, identifier: str, record: Record) -> list[str]:
    """The record's text as stored and, when replay derives another status, as the bank shows it."""
    found = [render_text(record)]
    if (status := state.status(identifier)) != record.status:
        try:
            found.append(render_text(replace(record, status=status)))
        except RenderError:
            pass
    return found


def cell_shown(record: Record, cell: str, text: str) -> bool:
    """A cell is verbatim in the text, or an enum or status value equal to it ignoring case is."""
    if cell in text:
        return True
    values = [record.status, *(value for name, value in record.fields.items() if record.spec.fields[name].kind == "enum" and isinstance(value, str))]
    return any(value.lower() == cell.lower() and value in text for value in values)


def rendering_problems(state: State, revision: str) -> list[str]:
    """Every legacy cell a reader does not see (module docstring), one line per ID and cell."""
    problems: list[str] = []
    for identifier in sorted(state.entries, key=id_key):
        record = state.entries[identifier].record
        legacy = record.legacy
        if legacy is None:
            continue
        if record.type == "reference":
            heading = (record.body or "").split("\n", 1)[0]
            if legacy.cells != {"heading": heading, "revision": revision}:
                problems.append(f"{identifier}: cells are not the body's first line and the export revision")
            continue
        if record.type == "release":
            sources = "\n".join(record.items("sources"))
            problems.extend(f"{identifier}: cell {name!r} is named by no source" for name in legacy.cells
                            if name.split(" ", 1)[0] not in sources and name.split(" ", 1)[-1] not in sources)
            continue
        cells = dict(legacy.cells)
        if record.type == "evidence":
            row = as_mapping(load_json(cells.pop("row", "{}").encode()), f"{identifier}: row")
            cells = {name: str(value) for name, value in row.items()
                     if name not in ("id", "legacy", "ev", "unk") and value is not None and not isinstance(value, (bool, list, dict))}
        for text in texts_of(state, identifier, record):
            problems.extend(f"{identifier}: legacy cell {name!r} not in its text: {cell[:80]!r}" for name, cell in cells.items()
                            if name != "original" and cell and not cell_shown(record, cell, text))
    return list(dict.fromkeys(problems))


def stated_targets(text: str, known: Index) -> set[str]:
    """IDs and selectors a cell states, by plain regular expressions: selectors, then ranges over defined
    IDs, then single defined IDs."""
    found = {match[0] for match in SELECTOR.finditer(text)}
    rest = SELECTOR.sub(" ", text)
    for match in RANGE.finditer(rest):
        first, last = match[1], match[2]
        if first[0] == last[0]:
            found.update(key for key in known.entries if key[0] == first[0] and int(first[2:]) <= int(key[2:]) <= int(last[2:]))
    found.update(match[0] for match in ID_IN_TEXT.finditer(RANGE.sub(" ", rest)) if match[0] in known.entries)
    return found


TABLE_TYPES = ("compatibility", "lead", "report")


def table_edges(result: Parity, state: State, index: Index, documents: dict[str, list[str]]) -> None:
    """Compatibility, lead and report links against their cells, and their total against the raw table lines."""
    journal = 0
    for identifier in sorted(state.entries, key=id_key):
        record = state.entries[identifier].record
        if record.type not in TABLE_TYPES or record.legacy is None:
            continue
        linked = {*record.links.cites, *record.links.refs}
        journal += len(linked)
        stated = {target for name, cell in record.legacy.cells.items() if name not in ("original", "table") for target in stated_targets(cell, index)}
        if linked != stated:
            result.fail(f"{identifier}: links {sorted(linked ^ stated, key=id_key)} differ from its cells")
    raw = 0
    for path in ("docs/ledgers/compatibility.md", "docs/ledgers/bugs.md"):
        lines = documents.get(path, [])
        for number, line in enumerate(lines):
            following = lines[number + 1] if number + 1 < len(lines) else ""
            header = re.fullmatch(r"\|(?:\s*:?-+:?\s*\|)+", following) is not None
            if not line.startswith("|") or header or re.fullmatch(r"\|(?:\s*:?-+:?\s*\|)+", line) or re.match(r"\|\s*N-[0-9]+\s*\|", line):
                continue
            cells = [cell.strip().replace(r"\|", "|") for cell in PIPE.split(line)[1:-1]]
            raw += len({target for cell in cells for target in stated_targets(cell, index)})
    result.measured["edges compatibility, lead and report rows"] = f"{journal} (raw table lines {raw})"
    if journal != raw:
        result.fail(f"edges of compatibility, lead and report rows: journal {journal}, raw table lines {raw}")


class Parser(Protocol):
    def __call__(self, data: object, body: str | None = None, /) -> object: ...


def released_model_problems(root: Path, journal: Path, ref: str = "origin/main") -> tuple[str, int, list[str]]:
    """Parse every revision under ``journal`` with ``ref``'s ``tools/memory/model.py``; returns the commit, the
    count parsed and one problem per rejected revision. A model that imports other project modules cannot be
    loaded alone and is a problem itself."""
    commit = git(root, "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}").decode().strip()
    archive = git(root, "archive", "--format=tar", commit, "tools/memory/model.py")
    with tempfile.TemporaryDirectory(prefix="memory-released-model-") as scratch:
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(scratch, filter="data")
        path = Path(scratch) / "tools/memory/model.py"
        source = path.read_text(encoding="utf-8")
        if re.search(r"^\s*(?:from|import)\s+tools\b", source, re.MULTILINE):
            return commit, 0, [f"{ref}:tools/memory/model.py imports project modules; it cannot be loaded alone"]
        name = f"memory_released_model_{commit[:12]}"
        spec = importlib.util.spec_from_file_location(name, path)
        if spec is None or spec.loader is None:
            return commit, 0, [f"{ref}:tools/memory/model.py cannot be loaded"]
        module = importlib.util.module_from_spec(spec)
        sys.modules[name] = module
        try:
            spec.loader.exec_module(module)
            parse = cast(Parser, getattr(cast(object, getattr(module, "Record")), "from_json"))
            problems: list[str] = []
            parsed = 0
            for segment in sorted((journal / JOURNAL).rglob("*.jsonl")):
                for number, line in enumerate(segment.read_bytes().split(b"\n")[1:], 2):
                    if not line:
                        continue
                    raw = as_mapping(load_json(line), f"{segment}:{number}")
                    body = raw.get("body")
                    try:
                        _ = parse(raw.get("record"), body if isinstance(body, str) else None)
                        parsed += 1
                    except ValueError as error:
                        problems.append(f"{segment.relative_to(journal)}:{number}: {error}")
        finally:
            del sys.modules[name]
    return commit, parsed, problems


def legacy_local(primary: Path, root: Path, revision: str) -> tuple[set[str], list[str]]:
    """``docs_lint --list-local`` over the shards of ``revision``, resolved against ``primary``."""
    linter = Linter(primary, local=True)
    files = tree_files(root, revision, "evidence")
    shards = sorted(path for path in files if path.endswith(".jsonl"))
    for path, data in zip(shards, read_blobs(root, [files[path] for path in shards]), strict=True):
        rows, problems = parse_shard(path, (data or b"").decode())
        if problems:
            raise GitError(problems[0].render())
        for row in rows:
            _ = linter.manifest.setdefault(row.identifier, row)
    paths = set(linter.local_paths())
    return paths, list(linter.local_problems)


def census(root: Path, state: State) -> dict[str, set[str]]:
    """Distinct retired IDs cited from code and from prose (the ``check --ci`` citation roots)."""
    found: dict[str, set[str]] = {"code": set(), "prose": set()}
    for path in listed_files(root, *CITATION_ROOTS):
        if path.startswith(CITATION_EXCLUDED) or not (root / path).is_file() or (root / path).is_symlink():
            continue
        try:
            text = (root / path).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for _line, citation in citations(text):
            target = selector_base(citation)
            if target in state.entries and state.retired(target):
                found["code" if path.startswith(CODE_ROOTS) else "prose"].add(target)
    return found


def scratch_clone(primary: Path, revision: str, journal: Path, directory: Path) -> Path:
    """A shared, detached clone of ``revision`` with the journal copied in (untracked); a Git failure is ``GitError``."""
    tree = directory / "tree"
    _ = git(directory, "clone", "-q", "--shared", "--no-checkout", "--", str(primary), str(tree))
    _ = git(tree, "checkout", "-q", "--detach", revision)
    _ = shutil.copytree(journal / JOURNAL, tree / JOURNAL)
    return tree


def parity(root: Path, rev: str, journal: Path, released: str = "origin/main") -> Parity:
    """The gate (module docstring); ``released`` names the ref whose model must parse the journal."""
    result = Parity()
    revision = commit_id(root, rev)
    segments = read_tree(journal)
    state = replay(segments)
    catalog = resolved_catalog(root, revision)
    index = Index(catalog.documents)
    result.measured["revision"] = revision
    result.measured["segments"] = f"{len(segments)} ({sum(len(segment.revisions) for segment in segments)} revisions)"
    result.mismatches.extend(state.problems)
    result.mismatches.extend(f"{identifier}: fork (heads {', '.join(entry.heads)})" for identifier, entry in state.entries.items() if entry.conflict)
    result.mismatches.extend(f"supersession cycle: {cycle}" for cycle in state.supersession_cycles())
    result.mismatches.extend(state.link_problems())
    prose: dict[str, set[str]] = defaultdict(set)
    prose_count = 0
    for path, document in catalog.documents.items():
        for line in document.lines:
            if path.startswith("docs/ledgers/") and isinstance(line, str) and not line.startswith("|"):
                for match in PROSE_SUPERSESSION.finditer(line):
                    prose[match[1]].add(match[2])
                    prose_count += 1
    blobs = {path: (data or b"").decode().split("\n") for path, data in zip(
        sorted(catalog.documents), read_blobs(root, [f"{revision}:{path}" for path in sorted(catalog.documents)]), strict=True)}
    sources = {entry.record.legacy.path: identifier for identifier, entry in state.entries.items()
               if entry.record.type == "reference" and entry.record.legacy is not None}
    for identifier in sorted(index.entries, key=id_key):
        entry = index.entries[identifier]
        record = state.record(identifier)
        if record is None:
            result.fail(f"{identifier}: missing from the journal")
            continue
        compare_row(result, state, index, entry, record, blobs[entry.path], prose, sources)
    tracked = sum(1 for entry in state.entries.values() if entry.record.type == "evidence" and entry.record.links.refs)
    result.measured["tracked evidence linked to the REF of its file"] = str(tracked)
    table_edges(result, state, index, blobs)
    rendering = rendering_problems(state, revision)
    result.mismatches.extend(rendering)
    texts = [len(render_text(entry.record)) for entry in state.entries.values() if entry.record.body is None]
    result.measured["rendering"] = f"{len(rendering)} legacy cell(s) unseen; longest text {max(texts, default=0)} characters"

    counts, unallocated = raw_counts(root, revision)
    journal_counts = Counter(entry.record.type for entry in state.entries.values())
    for kind in sorted(set(counts) | set(journal_counts)):
        if kind != "release" and counts.get(kind, 0) != journal_counts.get(kind, 0):
            result.fail(f"{kind}: {journal_counts.get(kind, 0)} records in the journal, {counts.get(kind, 0)} rows in the legacy files")
    result.measured["counts"] = ", ".join(f"{kind} {journal_counts[kind]}" for kind in sorted(journal_counts))
    result.measured["unallocated finding numbers"] = ", ".join(unallocated)
    for identifier in unallocated:
        if identifier in state.entries:
            result.fail(f"{identifier}: an unallocated number has a record")
    tuples = Counter(
        "supersedes_to" if link.kind == "supersedes_to" else edge_class(identifier, link.target)
        for identifier, links in index.links.items() for link in links
    )
    result.measured["catalog link tuples (target, field)"] = ", ".join(f"{name} {count}" for name, count in sorted(tuples.items()))
    for name, (in_journal, in_catalog) in edge_counts(state, index, prose_count).items():
        result.measured[f"edges {name}"] = f"{in_journal} (catalog {in_catalog})"
        if in_journal != in_catalog:
            result.fail(f"edges {name}: journal {in_journal}, catalog {in_catalog}")
    selectors = Counter[str]()
    for identifier, entry in state.entries.items():
        for citation in entry.record.links.citations():
            if "/" in citation:
                resolution = state.resolve(citation)
                kind = "case note" if resolution.member is None else citation.split("/", 1)[1].split("_", 1)[0]
                selectors[kind] += 1
                if resolution.problem is not None:
                    result.fail(f"{identifier}: {citation}: {resolution.problem}")
    result.measured["selectors"] = ", ".join(f"{kind} {count}" for kind, count in sorted(selectors.items())) + f" (total {sum(selectors.values())})"
    accounting = account(root, revision, {identifier: entry.record for identifier, entry in state.entries.items()})
    result.measured["byte accounting"] = (f"{accounting.files} files: " + ", ".join(f"{kind} {count}" for kind, count in sorted(accounting.bytes.items()))
                                          + f" (total {sum(accounting.bytes.values())})")
    result.mismatches.extend(accounting.problems)
    primary = primary_checkout(root)
    with tempfile.TemporaryDirectory(prefix="memory-parity-") as scratch:
        tree = scratch_clone(primary, revision, journal, Path(scratch))
        report = run_check(tree, "HEAD")
        result.mismatches.extend(f"check --ci: {problem}" for problem in report.problems)
        result.measured["check --ci"] = f"{len(report.problems)} problem(s), {len(report.notices)} notice(s)"
        retired = census(tree, state)
        cited = sum(len(citations((tree / path).read_text(encoding="utf-8", errors="replace"))) for path in listed_files(tree, *CITATION_ROOTS)
                    if not path.startswith(CITATION_EXCLUDED) and (tree / path).is_file() and not (tree / path).is_symlink())
        result.measured["citations scanned"] = str(cited)
        for where, identifiers in retired.items():
            result.measured[f"retired IDs cited from {where}"] = f"{len(identifiers)}: {', '.join(sorted(identifiers, key=id_key))}"
    if (journal / "evidence").exists():
        result.fail(f"{journal}: holds an evidence/ directory; protected-paths would read it")
    protected = protected_paths_for(primary, [journal])
    legacy_paths, legacy_problems = legacy_local(primary, root, revision)
    result.measured["protected paths"] = f"{len(protected.paths)} from the journal, {len(legacy_paths)} from docs_lint ({len(legacy_problems)} local problem(s))"
    for path in sorted(set(protected.paths) ^ legacy_paths):
        result.fail(f"protected-paths: {path} only {'in the journal' if path in protected.paths else 'in the legacy list'}")
    commit, parsed, rejected = released_model_problems(root, journal, released)
    result.mismatches.extend(f"released model: {problem}" for problem in rejected)
    result.measured["released model"] = f"{released} {commit[:12]}: {parsed} revision(s) parsed, {len(rejected)} rejected"
    return result
