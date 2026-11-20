# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory parity --rev REV --journal DIR``: the gate between a migration journal and the legacy system.

``parity(root, rev, journal)`` replays the journal under ``journal/memory/journal`` and compares it with
the typed catalog parse of ``rev`` (``resolved_catalog`` and ``Index``), not with the exporter's code.
Every mismatch is a line naming its ID; the gate passes only with none. It checks:

- **Every F, U, N and M record**: present with its type; ``legacy`` path and cells byte-identical to
  the catalog's fields (and, rendered back, to the line in the Git blob); parsed fields against the
  catalog (claim, kind, question, severity, trigger, change; evidence path, sha256, class, kind, status,
  date, label, note, records, members in listed order); derived status; supersession; resolvers.
- **Links**: per record, the IDs a journal record links (cites, refs, legacy supersession and resolvers,
  with each selector's evidence ID) equal the catalog's link targets; the unknown/finding split;
  every selector resolving to one listed member (or a legacy case note on a legacy row).
- **Counts** per type against a raw scan of the Git tree (``raw_counts``), the five-or-so unallocated
  finding numbers among them; edge counts (distinct targets per record) from the journal against the
  catalog's, and the catalog's raw link tuples (one per target and cell) for reference.
- **Replay**: no replay problem, dangling link, supersession cycle or fork.
- **``check --ci``** on a scratch clone of ``rev`` holding the journal, so the citation scan covers the
  repository's code and documents; the census of citations to retired records is reported.
- **``protected-paths``** from the journal alone equals the legacy ``docs_lint`` local list for the
  same revision's shards, both resolved against the primary checkout.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import tempfile
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from pathlib import Path

from tools.docs_lint import Linter, parse_shard
from tools.evidence_catalog_model import Entry, Index, references
from tools.evidence_catalog_publish import resolved_catalog
from tools.evidence_catalog_store import commit_id
from tools.memory.check import CITATION_EXCLUDED, CITATION_ROOTS, citations, run_check
from tools.memory.gitio import GitError, listed_files, read_blobs, tree_files
from tools.memory.journal import JOURNAL, read_tree
from tools.memory.legacy_rules import PROSE_SUPERSESSION, SELECTORS, vocabulary
from tools.memory.model import CONFIDENCES, EVIDENCE_CLASSES, EVIDENCE_KINDS, FINDING_KINDS, SEVERITIES, TYPES, Record, id_key
from tools.memory.paths import primary_checkout, protected_paths_for
from tools.memory.state import State, replay

CODE_ROOTS = ("src/", "tests/", "examples/", "tools/", "config/")


@dataclass
class Parity:
    mismatches: list[str] = field(default_factory=list[str])
    measured: dict[str, str] = field(default_factory=dict[str, str])

    def fail(self, message: str) -> None:
        self.mismatches.append(message)


def raw_counts(root: Path, revision: str) -> tuple[dict[str, int], list[str]]:
    """Record counts from a plain scan of the legacy files, and the finding numbers that have no row."""
    files = tree_files(root, revision, "docs/ledgers") | tree_files(root, revision, "evidence") | tree_files(root, revision, "docs/specs")
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
        elif path.startswith("docs/specs/") and path.endswith(".md"):
            counts["reference"] += 1
    unallocated = [f"F-{number:03d}" for number in range(1, max(numbers, default=0) + 1) if number not in numbers]
    return dict(counts), unallocated


def selector_base(citation: str) -> str:
    return citation.split("/", 1)[0]


def journal_targets(record: Record) -> set[str]:
    legacy = record.legacy
    found = {*record.links.cites, *record.links.refs, *(selector_base(cite) for cite in record.links.cites)}
    if legacy is not None:
        found.update(legacy.superseded_by, legacy.resolved_by)
    return found


def compare_row(result: Parity, state: State, index: Index, entry: Entry, record: Record, blob_lines: list[str], prose: dict[str, set[str]]) -> None:
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
        kept = {name: cell for name, cell in legacy.cells.items() if not name.startswith("prose ")}
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
            resolvers = {link.target for link in index.links[identifier] if link.field == "findings" and link.target.startswith("F-")}
            wanted_resolvers = resolvers if fields["status"] == "resolved" else set[str]()
            if set(state.resolved_by.get(identifier, ())) != wanted_resolvers:
                result.fail(f"{identifier}: resolved by {sorted(state.resolved_by.get(identifier, ()))}, legacy Findings cell states {sorted(wanted_resolvers)}")
            if status != fields["status"]:
                result.fail(f"{identifier}: derived status {status}, legacy {fields['status']!r}")
        case "N":
            expect(result, identifier, record, {"trigger": fields["trigger_and_former_behavior"], "change": fields["change"]})
        case _:
            compare_evidence(result, state, entry, record)
    if entry.record.kind != "M":
        catalog_targets = {link.target for link in index.links[identifier]} | prose.get(identifier, set())
        if journal_targets(record) != catalog_targets:
            result.fail(f"{identifier}: links {sorted(journal_targets(record) ^ catalog_targets, key=id_key)} differ from the catalog")


def compare_evidence(result: Parity, state: State, entry: Entry, record: Record) -> None:
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
    if record.notes != ((row.note,) if row.note is not None else ()):
        result.fail(f"{identifier}: note differs")
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
    """A shared, detached clone of ``revision`` with the journal copied in (untracked)."""
    tree = directory / "tree"
    for command in (
        ["git", "clone", "-q", "--shared", "--no-checkout", str(primary), str(tree)],
        ["git", "-C", str(tree), "checkout", "-q", "--detach", revision],
    ):
        _ = subprocess.run(command, check=True, capture_output=True)
    _ = shutil.copytree(journal / JOURNAL, tree / JOURNAL)
    return tree


def parity(root: Path, rev: str, journal: Path) -> Parity:
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
    for identifier in sorted(index.entries, key=id_key):
        entry = index.entries[identifier]
        record = state.record(identifier)
        if record is None:
            result.fail(f"{identifier}: missing from the journal")
            continue
        compare_row(result, state, index, entry, record, blobs[entry.path], prose)
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
    return result
