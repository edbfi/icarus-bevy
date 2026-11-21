# SPDX-License-Identifier: AGPL-3.0-or-later
"""Replay: a deterministic function from journal segments to record state, and read-time partitions.

``replay(segments)`` depends only on the set of segments, never on the order they were read in.
Per ID it keeps every revision (keyed by its identity ``rev``), the heads (revisions no other revision names as ``prev``)
and the problems a malformed history shows (an unknown ``prev``, a repeated revision, a ``prev``
cycle). Two heads are a fork, shown as ``conflict`` until a reconciling revision names both.

Links are stored once, on the newer record; ``State`` derives the inverses from every head:
``superseded_by`` (``supersedes`` edges, unioned with what a migrated record's legacy row stated
about itself) and ``resolved_by``. ``State.status`` is the derived status: ``superseded``,
``resolved``, a migrated record's implied ``retired``, else the stored status.

*The latest statement wins.* A migrated record's legacy status (``legacy.implied_status`` and the
``legacy.superseded_by`` edges its row stated) is its status as of migration. It holds while the record's
stored status is the one it was migrated with (``Entry.origin``, the stored status of its first revision;
a revision may only store a vocabulary value, and every legacy-implied status was migrated beside a
``legacy:`` stored status, so a revision that changes the stored status always states something new). A
later revision that stores another status withdraws it for good (``Entry.legacy_holds``): it holds at a
revision only if every revision on the path from the migration revision to it stores the migration status,
so storing that status again later does not bring it back. At a fork each head is judged on its own path;
a reconciling revision naming both as ``prev`` is withdrawn if either path withdrew it. A ``supersedes`` or
``resolves`` link naming the record derives ``superseded`` or ``resolved`` whatever its revisions say. A
revision that leaves the stored status alone (a note, a ``verified_on``) keeps the legacy status. Every
record as migrated is at its first revision, so this changes nothing for the migration itself.

A migrated unknown whose legacy status was ``resolved`` derives ``resolved`` from that status alone: the
legacy table never said which finding resolved it, so no resolver edge exists. ``legacy_listed`` names
the linked records one of its legacy cells lists (its Findings column: ``legacy_listed(record,
"findings")``), for display as "resolved (legacy status); findings listed: ...".
"""

from __future__ import annotations

import re
import tomllib
from collections import defaultdict
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path

from tools.memory.journal import Revision, Segment
from tools.memory.model import ID, SELECTOR, Record, RecordError, as_mapping, id_key, type_of

# An ID, or a range of two IDs of one type, as legacy cells write them (``F-1``, ``F-1 to F-3``, ``F-1..F-3``).
LISTED = re.compile(rf"(?<![\w-])({ID.pattern})(?:(?:\.\.| to |–)({ID.pattern}))?(?![\w-])")
PARTITIONS = ("current", "other-build", "legacy-scope", "native", "unscoped")
RELEASE_CONFIG = "config/release.toml"


@dataclass(frozen=True)
class Located:
    """A revision and the path of the segment that holds it."""

    revision: Revision
    segment: str


@dataclass(frozen=True)
class Entry:
    """Every revision of one ID. ``revisions`` is keyed by ``rev``; ``heads`` holds ``rev`` values, sorted, never empty; ``head`` is its first."""

    id: str
    revisions: Mapping[str, Located]
    heads: tuple[str, ...]

    @property
    def conflict(self) -> bool:
        return len(self.heads) > 1

    @property
    def head(self) -> Located:
        return self.revisions[self.heads[0]]

    @property
    def record(self) -> Record:
        return self.head.revision.record

    @property
    def origin(self) -> str | None:
        """The stored status of the migration revision (a migrated first revision; ``None`` when there is none)."""
        found = sorted(located.revision.record.status for located in self.revisions.values()
                       if not located.revision.prev and located.revision.record.legacy is not None)
        return found[0] if found else None

    def legacy_holds(self, rev: str) -> bool:
        """Whether the migrated record's legacy status holds at revision ``rev``: every revision on every path
        from the migration revision to ``rev`` stores the migration status (``origin``). A ``prev`` this entry
        does not hold is not judged (the history known here decides)."""
        origin = self.origin
        known: dict[str, bool] = {}

        def holds(current: str, seen: frozenset[str]) -> bool:
            if current not in known:
                record = self.revisions[current].revision.record
                prev = [item for item in self.revisions[current].revision.prev if item in self.revisions and item not in seen]
                known[current] = (record.legacy is not None and (origin is None or record.status == origin)
                                  and all(holds(item, seen | {current}) for item in prev))
            return known[current]

        return holds(rev, frozenset())

    def history(self) -> tuple[Located, ...]:
        """Every revision, each after all of its ``prev`` (ties by ``rev``)."""
        depth: dict[str, int] = {}

        def depth_of(rev: str, seen: frozenset[str]) -> int:
            if rev not in depth:
                prev = [item for item in self.revisions[rev].revision.prev if item in self.revisions and item not in seen]
                depth[rev] = 1 + max((depth_of(item, seen | {rev}) for item in prev), default=-1)
            return depth[rev]

        return tuple(self.revisions[rev] for rev in sorted(self.revisions, key=lambda rev: (depth_of(rev, frozenset()), rev)))


@dataclass(frozen=True)
class Resolution:
    """A citation's target record, the full member a bundle selector names, or why it does not resolve."""

    target: str
    member: str | None = None
    problem: str | None = None


@dataclass(frozen=True)
class State:
    entries: Mapping[str, Entry]
    superseded_by: Mapping[str, tuple[str, ...]]
    resolved_by: Mapping[str, tuple[str, ...]]
    problems: tuple[str, ...]

    def record(self, identifier: str) -> Record | None:
        entry = self.entries.get(identifier)
        return entry.record if entry is not None else None

    def status(self, identifier: str) -> str:
        """Derived status of a known ID: superseded, resolved, an implied legacy retirement while ``legacy_holds``,
        or the stored status. ``superseded_by`` holds a head's legacy edges only while they hold."""
        entry = self.entries[identifier]
        record = entry.record
        implied = record.legacy.implied_status if record.legacy is not None and entry.legacy_holds(entry.heads[0]) else None
        if self.superseded_by.get(identifier) or implied == "superseded":
            return "superseded"
        if self.resolved_by.get(identifier) or implied == "resolved":
            return "resolved"
        return "retired" if implied == "retired" else record.status

    def retired(self, identifier: str) -> bool:
        return self.status(identifier) in ("superseded", "retired")

    def successors(self, identifier: str) -> tuple[str, ...]:
        """Terminal successors along ``superseded_by`` (empty when nothing supersedes ``identifier``)."""
        terminal: set[str] = set()
        seen = {identifier}
        pending = list(self.superseded_by.get(identifier, ()))
        while pending:
            current = pending.pop()
            if current in seen:
                continue
            seen.add(current)
            following = self.superseded_by.get(current, ())
            if following:
                pending.extend(following)
            else:
                terminal.add(current)
        return tuple(sorted(terminal, key=id_key))

    def resolve(self, citation: str) -> Resolution:
        """Resolve ``ID`` or ``M-N/ev_x``, ``M-N/unk_x`` (a listed member) or ``M-N/E-N`` (a migrated bundle)."""
        target, slash, _ = citation.partition("/")
        if not ID.fullmatch(target):
            return Resolution(target, problem=f"{target!r} is not a record ID")
        entry = self.entries.get(target)
        if entry is None:
            return Resolution(target, problem=f"no record {target}")
        if not slash:
            return Resolution(target)
        match = SELECTOR.fullmatch(citation)
        if match is None:
            return Resolution(target, problem=f"malformed selector {citation} (expected M-N/ev_<12 hex>, M-N/unk_<12 hex> or M-N/E-N)")
        record = entry.record
        if match[3]:
            return Resolution(target, problem=None if record.migrated else f"{citation}: case notes belong to migrated evidence")
        hits = [member for member in record.items("members") if member.startswith(match[2])]
        if len(hits) != 1:
            return Resolution(target, problem=f"{citation}: {match[2]} matches {len(hits)} listed members of {target}")
        return Resolution(target, member=hits[0])

    def link_problems(self) -> tuple[str, ...]:
        """Every head's links that name no record, a record of the wrong type, or an unlisted member."""
        problems: list[str] = []
        for identifier in sorted(self.entries, key=id_key):
            entry = self.entries[identifier]
            for rev in entry.heads:
                record = entry.revisions[rev].revision.record
                named = [("supersedes", target) for target in record.links.supersedes]
                named += [("resolves", target) for target in record.links.resolves]
                named += [("refs", target) for target in record.links.refs]
                if record.legacy is not None:
                    named += [("legacy.superseded_by", target) for target in record.legacy.superseded_by]
                problems.extend(f"{identifier}: {name}: no record {target}" for name, target in named if target not in self.entries)
                for citation in record.links.citations():
                    if (problem := self.resolve(citation).problem) is not None:
                        problems.append(f"{identifier}: cites: {problem}")
        return tuple(dict.fromkeys(problems))

    def supersession_cycles(self) -> tuple[str, ...]:
        """Each cycle of ``superseded_by`` edges once, as ``A -> B -> A`` starting at its smallest ID."""
        cycles: set[tuple[str, ...]] = set()
        for start in self.superseded_by:
            stack: list[tuple[str, tuple[str, ...]]] = [(start, (start,))]
            while stack:
                current, path = stack.pop()
                for following in self.superseded_by.get(current, ()):
                    if following == start:
                        cycles.add(path)
                    elif following not in path and id_key(following) > id_key(start):
                        stack.append((following, (*path, following)))
        return tuple(" -> ".join((*cycle, cycle[0])) for cycle in sorted(cycles))

    def release(self, server_build: str) -> Record | None:
        """The REL record whose ``server_build`` is ``server_build`` (the pin), smallest ID first."""
        for identifier in sorted(self.entries, key=id_key):
            record = self.entries[identifier].record
            if record.type == "release" and record.text("server_build") == server_build:
                return record
        return None


def replay(segments: Iterable[Segment]) -> State:
    """The state a set of segments describes; independent of iteration order."""
    located: dict[str, dict[str, Located]] = defaultdict(dict)
    problems: list[str] = []
    for segment in sorted(segments, key=lambda item: item.path):
        for revision in segment.revisions:
            known = located[revision.record.id].get(revision.rev)
            if known is not None:
                problems.append(f"{revision.record.id}: revision {revision.rev[:12]} appears in {known.segment} and {segment.path}")
                continue
            located[revision.record.id][revision.rev] = Located(revision, segment.path)
    entries: dict[str, Entry] = {}
    for identifier in sorted(located, key=id_key):
        revisions = located[identifier]
        named: set[str] = set()
        for rev in sorted(revisions):
            for prev in revisions[rev].revision.prev:
                named.add(prev)
                if prev not in revisions:
                    problems.append(f"{identifier}: revision {rev[:12]} names unknown prev {prev[:12]}")
        heads = tuple(sorted(rev for rev in revisions if rev not in named))
        if not heads or _has_cycle(revisions):
            problems.append(f"{identifier}: prev links form a cycle")
            heads = heads or tuple(sorted(revisions))
        entries[identifier] = Entry(identifier, dict(revisions), heads)
    superseded_by: dict[str, set[str]] = defaultdict(set)
    resolved_by: dict[str, set[str]] = defaultdict(set)
    for identifier, entry in entries.items():
        for rev in entry.heads:
            record = entry.revisions[rev].revision.record
            for target in record.links.supersedes:
                superseded_by[target].add(identifier)
            for target in record.links.resolves:
                resolved_by[target].add(identifier)
            if record.legacy is not None and entry.legacy_holds(rev):
                superseded_by[identifier].update(record.legacy.superseded_by)
    return State(
        entries,
        {key: tuple(sorted(value, key=id_key)) for key, value in sorted(superseded_by.items())},
        {key: tuple(sorted(value, key=id_key)) for key, value in sorted(resolved_by.items())},
        tuple(problems),
    )


def legacy_listed(record: Record, cell: str) -> tuple[str, ...]:
    """The records ``record`` links (``refs``, ``cites``) that its legacy cell ``cell`` names, a range
    covering the linked IDs between its ends; empty for a record without that cell."""
    text = record.legacy.cells.get(cell, "") if record.legacy is not None else ""
    linked = {*record.links.refs, *(cite.split("/", 1)[0] for cite in record.links.cites)}
    found: set[str] = set()
    for match in LISTED.finditer(text):
        first, last = match[1], match[2]
        prefix = first.split("-", 1)[0]
        if last is None or last.split("-", 1)[0] != prefix or prefix == "REL":
            found.update(item for item in (first, last) if item in linked)
            continue
        low, high = id_key(first), id_key(last)
        found.update(item for item in linked if item.split("-", 1)[0] == prefix and low <= id_key(item) <= high)
    return tuple(sorted(found, key=id_key))


def _has_cycle(revisions: Mapping[str, Located]) -> bool:
    state: dict[str, int] = {}

    def visit(rev: str) -> bool:
        state[rev] = 1
        for prev in revisions[rev].revision.prev:
            if prev in revisions and (state.get(prev) == 1 or (prev not in state and visit(prev))):
                return True
        state[rev] = 2
        return False

    return any(rev not in state and visit(rev) for rev in sorted(revisions))


@dataclass(frozen=True)
class Partition:
    """Where a record stands against a release. ``tokens`` are its literal scope (or matching verification)
    tokens; ``names_pin`` says whether a legacy token names the release; ``descends`` whether HEAD descends
    from a native record's commit (``None`` when unknown)."""

    name: str
    tokens: tuple[str, ...] = ()
    names_pin: bool = False
    commit: str | None = None
    descends: bool | None = None


def partition(record: Record, release: Record, descends: Callable[[str], bool | None]) -> Partition:
    """Classify ``record`` against REL ``release`` per the rules' Reading section, first match wins:

    a ``verified_on`` for the release covering every role the scope lists -> current; no scope, or a
    legacy-text scope without tokens -> unscoped; legacy-text -> legacy-scope; explicit builds or releases
    -> current when every one matches the release, else other-build; a commit -> native; a
    build-independent decision -> current; else unscoped.
    """
    if type_of(release.id) != "release":
        raise ValueError(f"{release.id} is not a release")
    builds = {"server": release.text("server_build"), "client": release.text("client_build")}
    version = release.id.removeprefix("REL-")

    def matches(build: str) -> bool:
        role, _, number = build.partition(":")
        return builds.get(role) == number

    def same_release(token: str) -> bool:
        return token == version or token.startswith(f"{version}.")

    scope = record.scope
    verified = tuple(item.build for item in record.links.verified_on if matches(item.build))
    listed_roles = {build.partition(":")[0] for build in scope.builds} & set(builds) if scope is not None else set[str]()
    if verified and listed_roles <= {build.partition(":")[0] for build in verified}:
        return Partition("current", verified)
    if scope is None or (scope.basis == "legacy-text" and not scope.tokens):
        return Partition("unscoped")
    if scope.basis == "legacy-text":
        names = any(
            matches(token) or (token.startswith("any:") and token[4:] in builds.values()) for token in scope.builds
        ) or any(same_release(token) for token in scope.releases)
        return Partition("legacy-scope", scope.tokens, names, scope.commit)
    if scope.builds or scope.releases:
        current = all(matches(build) for build in scope.builds) and all(same_release(token) for token in scope.releases)
        return Partition("current" if current else "other-build", scope.tokens)
    if scope.commit is not None:
        return Partition("native", scope.tokens, commit=scope.commit, descends=descends(scope.commit))
    if record.text("kind") == "decision" or "decision" in scope.subjects:
        return Partition("current")
    return Partition("unscoped")


def pinned_build(root: Path) -> str:
    """``build_id`` of the pinned release in ``config/release.toml``."""
    release = as_mapping(as_mapping(tomllib.loads((root / RELEASE_CONFIG).read_text(encoding="utf-8")), RELEASE_CONFIG).get("release"), "release")
    build = release.get("build_id")
    if not isinstance(build, int):
        raise RecordError(f"{RELEASE_CONFIG}: release.build_id must be an integer")
    return str(build)
