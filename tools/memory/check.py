# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory check --ci``: every check of the rules' Checks section that needs no Hindsight.

``run_check(root, base)`` checks the working tree at ``root`` and returns a ``Report``: failing
``problems``, non-failing ``notices``, and ``no_journal`` when neither the working tree nor the
reference tree holds a segment (nothing else is checked then; with ``require_journal`` that is also a
problem).

Two trees are read. The *reference* is ``git merge-base <base> HEAD``: what this branch started from,
so a branch merely behind ``base`` is not blamed for what ``base`` gained since. It decides removed or
changed segments and which files the branch changed (new citations, the gate notice, salvage). ``base``
itself (normally ``origin/main``) is the *acceptance* set: forks are found in the union of its segments
and the working tree's, and the confidence rule's sources must be accepted there. No merge base
(unrelated histories, a shallow clone) is a problem, never a guess.

Checks, in report order: segment format, ``rev`` and ``sha256`` (the reader), replay problems
(repeated revisions, unknown ``prev``), reference segments unchanged, forks created against ``base``
(fail) or inherited from it (notice), acyclic supersession, link targets and bundle membership, fields
revised beyond ``REVISABLE``, the derived/inferred confidence rule, citations in code and documents
(every one resolves; one new to a file does not name a retired record), decision and gate notices, and,
once ``LEGACY_FROZEN`` is switched on, no added or modified legacy rows (``salvage_problems``).
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

from tools.memory.gitio import GitError, changed_files, listed_files, merge_base, read_blobs
from tools.memory.journal import JournalError, read_git, read_tree
from tools.memory.model import LEGACY, Record
from tools.memory.state import State, replay

# The cut-over commit, which deletes the legacy trees, switches both on.
LEGACY_FROZEN = False
REQUIRE_JOURNAL = False
LEGACY_DIRS = ("docs/ledgers", "evidence", "evidence-catalog")
GATES = "docs/gates.md"
CITATION_ROOTS = ("src", "tests", "examples", "tools", "config", "docs", "CLAUDE.md", "AGENTS.md", ".agents/rules")
# Never scanned for citations: the journal, frozen history, the legacy ledgers (records, not citations;
# removed at cut-over) and tool tests, whose fixtures use invented IDs on purpose.
CITATION_EXCLUDED = ("memory/journal/", "docs/history/", "docs/research/archive/", "docs/ledgers/", "tools/tests/")
# A cited ID, optionally with a selector, not glued to a word or hyphen on either side: ``f978`` in an
# identifier is no citation, and a prose range ``<ID> to <ID>`` cites its two endpoints only.
CITATION = re.compile(
    r"(?<![\w-])(?:(?:F|U|N|SL|RP|CP|REF|M)-[0-9]{3,}|REL-[0-9]+\.[0-9]+\.[0-9]+)(?:/(?:(?:ev|unk)_[0-9a-f]+|E-[0-9]{3,}))?(?![\w-])"
)
# What a revision may change in place (rules, "Revisions"); every other key of ``Record.to_json`` is
# immutable, including fields added to the model later, ``legacy`` and ``gaps``. On every type ``status``
# may change (to a vocabulary value; an evidence status change appends a note) and ``APPEND_ONLY`` lists
# may grow. A REF revises its body, which its JSON holds as ``body_sha256``.
REVISABLE: dict[str, frozenset[str]] = {"reference": frozenset({"body_sha256"}), "unknown": frozenset({"next_probe"})}
APPEND_ONLY = ("notes", "verified_on")


@dataclass
class Report:
    problems: list[str] = field(default_factory=list[str])
    notices: list[str] = field(default_factory=list[str])
    no_journal: bool = False


def citations(text: str) -> list[tuple[int, str]]:
    """``(line number, citation)`` for every citation in ``text``."""
    return [(number, match[0]) for number, line in enumerate(text.split("\n"), 1) for match in CITATION.finditer(line)]


def run_check(
    root: Path, base: str = "origin/main", *, legacy_frozen: bool = LEGACY_FROZEN, require_journal: bool = REQUIRE_JOURNAL,
) -> Report:
    report = Report()
    try:
        segments = read_tree(root)
        reference = merge_base(root, base)
        reference_segments = read_git(root, reference)
        accepted_segments = read_git(root, base)
    except (GitError, JournalError) as error:
        report.problems.append(str(error))
        return report
    if not segments and require_journal:
        report.problems.append("no journal: memory/journal/ holds no segment, and this repository requires one")
    if not segments and not reference_segments:
        report.no_journal = True
        return report
    now, before = replay(segments), replay(accepted_segments)
    report.problems.extend(now.problems)
    paths = {segment.path for segment in segments}
    report.problems.extend(
        f"{segment.path}: segment of {base} (merge base {reference[:12]}) removed or changed; accepted segments are immutable"
        for segment in reference_segments if segment.path not in paths
    )
    accepted = {rev for entry in before.entries.values() for rev in entry.revisions}
    union = replay({segment.path: segment for segment in (*accepted_segments, *segments)}.values())
    for identifier, entry in union.entries.items():
        if not entry.conflict:
            continue
        heads = ", ".join(rev[:12] for rev in entry.heads)
        if any(rev not in accepted for rev in entry.heads):
            report.problems.append(f"{identifier}: forks the record against {base} (heads {heads}); revise the current head or name every head as prev")
        else:
            report.notices.append(f"{identifier}: unreconciled fork in {base} (heads {heads}); the next change must name both as prev")
    report.problems.extend(f"supersession cycle: {cycle}" for cycle in now.supersession_cycles())
    report.problems.extend(now.link_problems())
    report.problems.extend(revision_problems(now))
    report.problems.extend(confidence_problems(now, before))
    try:
        report.problems.extend(citation_problems(root, reference, now))
        if changed_files(root, reference, GATES, kinds="ACDMRT"):
            report.notices.append(f"{GATES} changes: outside an agent's merge authority; file the PR and report it to the owner")
        if legacy_frozen:
            report.problems.extend(salvage_problems(root, reference))
    except GitError as error:
        report.problems.append(str(error))
    for identifier, entry in sorted(now.entries.items()):
        if any(rev not in accepted and item.revision.record.text("kind") == "decision"
               for rev, item in entry.revisions.items() if item.revision.record.type == "finding"):
            report.notices.append(f"{identifier}: adds or revises a decision finding; outside an agent's merge authority: file the PR and report it to the owner")
    return report


def revision_problems(state: State) -> list[str]:
    """Each revision against each of its ``prev``: changes beyond ``REVISABLE``, append-only lists, status rules."""
    problems: list[str] = []
    for identifier, entry in sorted(state.entries.items()):
        for rev, located in sorted(entry.revisions.items()):
            new = located.revision.record
            reconciles = len(located.revision.prev) > 1
            for prev in located.revision.prev:
                if prev in entry.revisions:
                    old = entry.revisions[prev].revision.record
                    problems.extend(f"{identifier}: revision {rev[:12]}: {problem}" for problem in _compare(old, new, reconciles))
    return problems


def _compare(old: Record, new: Record, reconciles: bool) -> list[str]:
    before, after = old.to_json(), new.to_json()
    allowed = {"status", *APPEND_ONLY, *REVISABLE.get(new.type, ())}
    problems = [
        f"{name} changed; it is not revisable (supersede with a new record)"
        for name in sorted(before.keys() | after.keys()) if name not in allowed and before.get(name) != after.get(name)
    ]
    for name, earlier, later in (("notes", old.notes, new.notes), ("verified_on", old.links.verified_on, new.links.verified_on)):
        kept = set(earlier) <= set(later) if reconciles else later[:len(earlier)] == earlier
        if not kept:
            problems.append(f"{name} may only be appended to")
    if old.status != new.status:
        if new.status.startswith(LEGACY):
            problems.append("a revised status takes a value from the vocabulary")
        if new.type == "evidence" and len(new.notes) <= len(old.notes):
            problems.append("an evidence status change appends a note")
    return problems


def confidence_problems(now: State, before: State) -> list[str]:
    """A new derived or inferred ``high`` finding needs every ``refs`` source accepted (in ``before``) as observed or measured."""
    problems: list[str] = []
    for identifier, entry in sorted(now.entries.items()):
        record = entry.record
        if record.type != "finding" or record.migrated or record.text("confidence") != "high" or record.text("kind") not in ("derived", "inferred"):
            continue
        for source in record.links.refs:
            accepted = before.record(source)
            if accepted is None or accepted.type != "finding" or accepted.text("kind") not in ("observed", "measured"):
                problems.append(f"{identifier}: confidence high needs every source an accepted observed or measured finding; {source} is not")
    return problems


def citation_problems(root: Path, base: str, state: State) -> list[str]:
    """Citations in ``CITATION_ROOTS`` that do not resolve, or that a file newly cites although retired."""
    files = [path for path in listed_files(root, *CITATION_ROOTS) if not path.startswith(CITATION_EXCLUDED)]
    changed = [path for path in changed_files(root, base, *CITATION_ROOTS) if not path.startswith(CITATION_EXCLUDED)]
    earlier: dict[str, set[str]] = {}
    for path, data in zip(changed, read_blobs(root, [f"{base}:{path}" for path in changed]), strict=True):
        text = data.decode("utf-8", "replace") if data is not None else ""
        earlier[path] = {citation.split("/", 1)[0] for _line, citation in citations(text)}
    problems: list[str] = []
    for path in files:
        full = root / path
        try:
            # A symlink is skipped (its target, CLAUDE.md for AGENTS.md, is scanned); binary fixtures hold no citations.
            text = full.read_text(encoding="utf-8") if full.is_file() and not full.is_symlink() else None
        except (OSError, UnicodeDecodeError):
            continue
        if text is None:
            continue
        for number, citation in citations(text):
            resolution = state.resolve(citation)
            target = resolution.target
            if resolution.problem is not None:
                problems.append(f"{path}:{number}: {citation}: {resolution.problem}")
            elif path in earlier and target not in earlier[path] and state.retired(target):
                successors = state.successors(target)
                hint = f"; cite {', '.join(successors)}" if successors else ""
                problems.append(f"{path}:{number}: new citation of {state.status(target)} {target}{hint}")
    return problems


def salvage_problems(root: Path, base: str) -> list[str]:
    """``salvage --check``: legacy files added or modified against ``base`` (deleting them is allowed)."""
    return [f"{path}: adds or modifies legacy rows; turn them into proposals with memory salvage" for path in changed_files(root, base, *LEGACY_DIRS)]
