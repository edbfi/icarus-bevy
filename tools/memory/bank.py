# SPDX-License-Identifier: AGPL-3.0-or-later
"""The bank as a deterministic function of the journal: gather, desired documents, diff, apply, verify.

``gather(root)`` collects every segment the host knows: ``origin/main`` (the acceptance ref), the
working-tree journal of every linked worktree, and every local branch and remote-tracking ref, read
without a checkout. A malformed segment outside the current worktree is skipped with a notice; one in
the current worktree, or in ``origin/main``, is an error. Segments are content-named, so they are
de-duplicated by name.

``desired(universe)`` maps every document the bank should hold to its retain item, per the rules'
Storage section. Over the union replay, for each record ID:

- some revision accepted (its sha256 is in an ``origin/main`` segment, compared by content so squash
  merges count): ``<ID>`` holds the head of the accepted revisions, ``state:accepted`` (``conflict``
  when ``origin/main`` itself has two heads), and every union head that is not accepted is a pending
  revision ``<ID>@<12 hex>``, ``state:pending``;
- none accepted: ``<ID>`` holds the union head, ``state:proposed`` (``conflict`` on a fork).

Tags carry the *derived* status, and the text renders it, so superseding or resolving a record
refreshes the old record's document although no revision of it is written. Metadata: ``record``
(canonical JSON), ``sha256``, ``prev`` (comma-separated) and ``segment`` (repository path).

*Applied* means the bank holds exactly the desired document: same metadata (so the same revision,
``sha256`` and segment), the same text (``content_hash``), entities and tags. ``diff`` compares that;
``apply`` retains what differs, re-tags what differs only in tags and deletes documents of known
revisions that are no longer desired (a pending revision since accepted, an older proposal). A
document of a revision no gathered segment holds is *unexplained*: it is left alone, and only
``memory sync`` after a successful fetch re-tags it ``state:orphaned``. The same diff is publication,
the replay of a half-applied change set, ``rebuild`` and the repair path.
"""

from __future__ import annotations

import fcntl
import os
import re
import time
from collections.abc import Generator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, field, replace
from pathlib import Path, PurePosixPath
from typing import Protocol

from tools.memory.canonical import RenderError, canonical_json, entities, render_text, tags
from tools.memory.gitio import GitError, changed_files, git, read_blobs, tree_files
from tools.memory.hindsight import Document, Group, Hit, Item, Tags, Unit
from tools.memory.journal import JOURNAL, JournalError, Revision, Segment, parse_segment, read_git, read_tree
from tools.memory.model import BUILD, ID, SELECTOR, Record, id_key, sha256_hex
from tools.memory.paths import ProtectedPathsError, worktrees
from tools.memory.state import Entry, State, replay

ACCEPTANCE_REF = "origin/main"
LOCK_FILE = "memory.lock"
LOCK_TIMEOUT = 300.0
REF_NAMESPACES = ("refs/heads", "refs/remotes")
LEGACY_TREES = ("docs/ledgers", "evidence")
LEGACY_ID = re.compile(r"(?<![\w-])(F|U|N|SL|RP|CP|REF|M)-([0-9]+)(?![\w-])")
BULK = 50
BATCH = 25


class BankError(RuntimeError):
    """The bank does not hold what the journal requires after applying it."""


class Bank(Protocol):
    """What the memory tool needs from a bank; ``hindsight.Client`` and the tests' fake implement it."""

    @property
    def bank(self) -> str: ...
    def retain(self, items: Sequence[Item]) -> None: ...
    def retain_async(self, items: Sequence[Item]) -> str: ...
    def wait(self, operations: Sequence[str], deadline: float = ...) -> None: ...
    def document(self, document_id: str) -> Document | None: ...
    def delete(self, document_id: str) -> None: ...
    def retag(self, document_id: str, tags: Sequence[str]) -> None: ...
    def documents(self, tags: Tags | None = None) -> list[Document]: ...
    def units(self, *, text: str | None = None, document_id: str | None = None) -> list[Unit]: ...
    def recall(self, query: str, groups: Sequence[Group], *, query_timestamp: str, max_tokens: int = ...) -> list[Hit]: ...
    def config(self) -> tuple[dict[str, object], dict[str, object]]: ...
    def banks(self) -> tuple[str, ...]: ...
    def count(self, kind: str) -> int: ...


def state_directory() -> Path:
    """The service's state directory: ``$ICARUS_MEMORY_HOME`` or ``$XDG_STATE_HOME/icarus-memory``."""
    override = os.environ.get("ICARUS_MEMORY_HOME")
    if override:
        return Path(override)
    return Path(os.environ.get("XDG_STATE_HOME") or str(Path.home() / ".local" / "state")) / "icarus-memory"


@contextmanager
def host_lock(timeout: float = LOCK_TIMEOUT) -> Generator[None]:
    """The host-wide lock every bank write, ID allocation and sync holds."""
    directory = state_directory()
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(directory / LOCK_FILE, os.O_RDWR | os.O_CREAT, 0o600)
    try:
        stop = time.monotonic() + timeout
        while True:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() > stop:
                    raise TimeoutError(f"memory host lock {directory / LOCK_FILE} held for over {timeout:.0f} s") from None
                time.sleep(0.1)
        yield
    finally:
        os.close(descriptor)


@dataclass(frozen=True)
class Universe:
    """Every known segment, the acceptance set and the two replays commands read.

    ``state`` replays every segment (the bank's function); ``view`` replays ``origin/main`` plus this
    worktree, de-duplicated as ``own`` (what this checkout reads and revises). ``accepted`` holds the revision identities (the
    keys of ``Entry.revisions``) of ``origin/main``; ``main_known`` is false when that ref is missing.
    ``known`` holds ``(sha256, prev)`` of every gathered revision, as a bank document's metadata names it.
    """

    root: Path
    segments: tuple[Segment, ...]
    main: tuple[Segment, ...]
    local: tuple[Segment, ...]
    own: tuple[Segment, ...]
    accepted: frozenset[str]
    main_known: bool
    state: State
    view: State
    notices: tuple[str, ...] = ()

    @property
    def local_stems(self) -> frozenset[str]:
        return frozenset(PurePosixPath(segment.path).stem for segment in self.local)

    @property
    def known(self) -> frozenset[tuple[str, str]]:
        return frozenset((located.revision.sha256, ",".join(located.revision.prev))
                         for entry in self.state.entries.values() for located in entry.revisions.values())


def _batch_check(root: Path, names: Sequence[str]) -> list[str | None]:
    """The tree object ID each ``<commit>:<path>`` names, or ``None``."""
    if not names:
        return []
    output = git(root, "cat-file", "--batch-check", stdin="".join(f"{name}\n" for name in names).encode()).decode()
    found: list[str | None] = []
    for line in output.splitlines():
        parts = line.split()
        found.append(parts[0] if len(parts) == 3 and parts[1] == "tree" else None)
    if len(found) != len(names):
        raise GitError("cat-file --batch-check answered an unexpected number of lines")
    return found


def ref_commits(root: Path) -> tuple[str, ...]:
    output = git(root, "for-each-ref", "--format=%(objectname) %(objecttype)", *REF_NAMESPACES).decode()
    return tuple(sorted({line.split()[0] for line in output.splitlines() if line.endswith(" commit")}))


def gather(root: Path, base: str = ACCEPTANCE_REF) -> Universe:
    """Every segment known on this host; see the module docstring for what is fatal."""
    notices: list[str] = []
    local = read_tree(root)
    try:
        main, main_known = read_git(root, base), True
    except GitError as error:
        main, main_known = (), False
        notices.append(f"{base} unreadable ({error}); nothing counts as accepted")
    by_name: dict[str, Segment] = {segment.name: segment for segment in (*main, *local)}
    try:
        checkouts, skipped = worktrees(root)
    except ProtectedPathsError as error:
        checkouts, skipped = (), (f"worktrees unknown: {error}",)
    notices.extend(skipped)
    own = root.resolve()
    for checkout in checkouts:
        if checkout.resolve() == own:
            continue
        try:
            segments = read_tree(checkout)
        except (JournalError, OSError) as error:
            notices.append(f"skipped the journal of worktree {checkout}: {error}")
            continue
        for segment in segments:
            _ = by_name.setdefault(segment.name, segment)
    commits = ref_commits(root)
    trees = _batch_check(root, [f"{commit}:{JOURNAL}" for commit in commits])
    for tree in sorted({tree for tree in trees if tree is not None}):
        files = {f"{JOURNAL}/{path}": oid for path, oid in tree_files(root, tree, ".").items()}
        wanted = sorted(path for path in files if PurePosixPath(path).name not in by_name)
        for path, data in zip(wanted, read_blobs(root, [files[path] for path in wanted]), strict=True):
            try:
                if data is None:
                    raise JournalError(f"{path}: blob missing")
                segment = parse_segment(path, data)
            except JournalError as error:
                notices.append(f"skipped a segment of a ref: {error}")
                continue
            by_name[segment.name] = segment
    segments = tuple(sorted(by_name.values(), key=lambda item: item.path))
    accepted = frozenset(key for entry in replay(main).entries.values() for key in entry.revisions)
    own = tuple(sorted({segment.name: segment for segment in (*main, *local)}.values(), key=lambda item: item.path))
    return Universe(root, segments, main, local, own, accepted, main_known, replay(segments), replay(own), tuple(notices))


@dataclass(frozen=True)
class Desired:
    """One document the bank should hold."""

    document_id: str
    record_id: str
    state: str
    status: str
    revision: Revision
    segment: str
    item: Item


def derived_status(state: State, record: Record) -> str:
    """The status ``record`` (a revision of a known ID) shows: superseded, resolved, an implied legacy
    retirement, or its stored status."""
    shown = state.status(record.id)
    head = state.entries[record.id].record
    return record.status if shown == head.status else shown


def document_text(record: Record, status: str) -> str:
    """The rendered text with the derived status; the stored rendering when that would not fit."""
    if status == record.status or record.body is not None:
        return render_text(record)
    try:
        return render_text(replace(record, status=status))
    except RenderError:
        return render_text(record)


def entity_types(names: Sequence[str]) -> dict[str, str]:
    return {name: "record" if ID.fullmatch(name) or SELECTOR.fullmatch(name) else "build" if BUILD.fullmatch(name) else "identifier"
            for name in names}


def item_for(document_id: str, revision: Revision, segment: str, state: str, status: str) -> Item:
    record = revision.record
    names = entities(record)
    date = record.text("date")
    return Item(
        document_id=document_id,
        content=document_text(record, status),
        tags=tags(record, state=state, status=status, segment=PurePosixPath(segment).stem),
        metadata={"record": canonical_json(record.to_json()).decode(), "sha256": revision.sha256,
                  "prev": ",".join(revision.prev), "segment": segment},
        timestamp=f"{date}T00:00:00Z" if date else "unset",
        entities=names,
        entity_types=entity_types(names),
    )


def accepted_heads(entry: Entry, accepted: frozenset[str]) -> tuple[str, ...]:
    """Heads among the accepted revisions of ``entry``: accepted identities no accepted revision names as ``prev``."""
    mine = {sha for sha in entry.revisions if sha in accepted}
    named = {prev for sha in mine for prev in entry.revisions[sha].revision.prev}
    return tuple(sorted(mine - named))


def desired(universe: Universe) -> dict[str, Desired]:
    """Every document the bank should hold, by document ID (see the module docstring)."""
    state = universe.state
    found: dict[str, Desired] = {}

    def add(document_id: str, entry: Entry, sha: str, document_state: str) -> None:
        located = entry.revisions[sha]
        status = derived_status(state, located.revision.record)
        item = item_for(document_id, located.revision, located.segment, document_state, status)
        found[document_id] = Desired(document_id, entry.id, document_state, status, located.revision, located.segment, item)

    for identifier in sorted(state.entries, key=id_key):
        entry = state.entries[identifier]
        heads = accepted_heads(entry, universe.accepted)
        if heads:
            add(identifier, entry, heads[0], "conflict" if len(heads) > 1 else "accepted")
            for sha in entry.heads:
                if sha not in universe.accepted:
                    add(f"{identifier}@{sha[:12]}", entry, sha, "pending")
        else:
            add(identifier, entry, entry.heads[0], "conflict" if entry.conflict else "proposed")
    return found


@dataclass(frozen=True)
class Plan:
    """What ``apply`` does: retain, re-tag (document ID, tags) and delete; ``unexplained`` is left alone."""

    retain: tuple[Item, ...] = ()
    retag: tuple[tuple[str, tuple[str, ...]], ...] = ()
    delete: tuple[str, ...] = ()
    unexplained: tuple[Document, ...] = ()

    @property
    def empty(self) -> bool:
        return not (self.retain or self.retag or self.delete)


def matches(have: Document, want: Item) -> tuple[bool, bool]:
    """(content matches, tags match) of a bank document against a desired item."""
    content = (
        have.metadata == dict(want.metadata)
        and have.content_hash == sha256_hex(want.content.encode())
        and (have.text is None or have.text == want.content)
        and have.entities == tuple(sorted(want.entities))
    )
    return content, have.tags == tuple(sorted(want.tags))


def diff(wanted: Mapping[str, Desired], held: Sequence[Document], known: frozenset[tuple[str, str]]) -> Plan:
    by_id = {document.id: document for document in held}
    retain: list[Item] = []
    retag: list[tuple[str, tuple[str, ...]]] = []
    for document_id in sorted(wanted, key=id_key):
        want = wanted[document_id].item
        have = by_id.get(document_id)
        if have is None:
            retain.append(want)
            continue
        content, labelled = matches(have, want)
        if not content:
            retain.append(want)
        elif not labelled:
            retag.append((document_id, want.tags))
    delete: list[str] = []
    unexplained: list[Document] = []
    for document in sorted(held, key=lambda item: id_key(item.id)):
        if document.id in wanted:
            continue
        if (document.metadata.get("sha256", ""), document.metadata.get("prev", "")) in known:
            delete.append(document.id)
        else:
            unexplained.append(document)
    return Plan(tuple(retain), tuple(retag), tuple(delete), tuple(unexplained))


@dataclass
class Applied:
    """What one reconciliation changed and what it left alone."""

    retained: int = 0
    retagged: int = 0
    deleted: int = 0
    orphaned: list[str] = field(default_factory=list[str])
    unexplained: list[str] = field(default_factory=list[str])
    notices: list[str] = field(default_factory=list[str])


def apply(bank: Bank, plan: Plan, *, bulk: bool = False) -> None:
    """Carry out ``plan``; ``bulk`` submits large retains as async operations of ``BULK`` items."""
    items = list(plan.retain)
    if bulk and len(items) > BULK:
        operations = [bank.retain_async(items[start:start + BULK]) for start in range(0, len(items), BULK)]
        bank.wait(operations)
    else:
        for start in range(0, len(items), BATCH):
            bank.retain(items[start:start + BATCH])
    for document_id, labels in plan.retag:
        bank.retag(document_id, labels)
    for document_id in plan.delete:
        bank.delete(document_id)


def orphan_tags(document: Document) -> tuple[str, ...] | None:
    """The tags of ``document`` with ``state:orphaned``, or ``None`` when it has no record metadata or is orphaned already."""
    if "sha256" not in document.metadata or "record" not in document.metadata or "state:orphaned" in document.tags:
        return None
    return tuple(sorted({*(tag for tag in document.tags if not tag.startswith("state:")), "state:orphaned"}))


def reconcile(bank: Bank, universe: Universe, *, bulk: bool = False, orphan: bool = False) -> Applied:
    """Make the bank hold exactly ``desired(universe)``; with ``orphan`` (only after a successful fetch)
    re-tag unexplained documents ``state:orphaned``. Raises ``BankError`` if a desired document still differs."""
    wanted = desired(universe)
    plan = diff(wanted, bank.documents(), universe.known)
    apply(bank, plan, bulk=bulk)
    result = Applied(len(plan.retain), len(plan.retag), len(plan.delete))
    for document in plan.unexplained:
        labels = orphan_tags(document) if orphan else None
        if labels is not None:
            bank.retag(document.id, labels)
            result.orphaned.append(document.id)
        else:
            result.unexplained.append(document.id)
    if not plan.empty or result.orphaned:
        after = diff(wanted, bank.documents(), universe.known)
        if not after.empty:
            missing = [item.document_id for item in after.retain] + [name for name, _ in after.retag] + list(after.delete)
            raise BankError(f"bank still differs from the journal after applying it: {', '.join(missing[:10])}")
    result.notices.extend(universe.notices)
    return result


def read_back(bank: Bank, wanted: Mapping[str, Desired], revisions: Sequence[Revision]) -> list[str]:
    """Exact GET of each published revision's document; problems if its text, sha256 or metadata differ."""
    problems: list[str] = []
    for revision in revisions:
        target = next((item for item in wanted.values() if item.revision == revision), None)
        if target is None:
            continue
        document = bank.document(target.document_id)
        if document is None:
            problems.append(f"{target.document_id}: missing after publication")
            continue
        if document.text != target.item.content or document.content_hash != sha256_hex(target.item.content.encode()):
            problems.append(f"{target.document_id}: text differs after publication")
        if document.metadata != dict(target.item.metadata):
            problems.append(f"{target.document_id}: metadata (sha256 {revision.sha256[:12]}) differs after publication")
    return problems


def legacy_numbers(root: Path, notices: list[str]) -> dict[str, int]:
    """Highest number per prefix in the legacy ledgers and shards of every ref and every worktree's working tree."""
    best: dict[str, int] = {}

    def take(text: str) -> None:
        for match in LEGACY_ID.finditer(text):
            best[match[1]] = max(best.get(match[1], 0), int(match[2]))

    commits = ref_commits(root)
    try:
        checkouts, _skipped = worktrees(root)
    except ProtectedPathsError as error:
        checkouts = ()
        notices.append(f"worktrees unknown for allocation: {error}")
    heads: list[str] = []
    for checkout in checkouts:
        try:
            heads.append(git(checkout, "rev-parse", "HEAD").decode().strip())
        except GitError as error:
            notices.append(f"skipped worktree {checkout} for allocation: {error}")
    names = [f"{commit}:{tree}" for commit in sorted({*commits, *heads}) for tree in LEGACY_TREES]
    trees = sorted({tree for tree in _batch_check(root, names) if tree is not None})
    if trees:
        try:
            take(git(root, "-c", "grep.threads=4", "grep", "-h", "-o", "-I", "-E",
                     r"(F|U|N|SL|RP|CP|REF|M)-[0-9]+", *trees).decode("utf-8", "replace"))
        except GitError as error:
            if "exit status 1" not in str(error) and str(error).strip() != "git grep:":
                raise
    for checkout in checkouts:
        try:
            changed = changed_files(checkout, "HEAD", *LEGACY_TREES)
        except GitError as error:
            notices.append(f"skipped the legacy working files of {checkout}: {error}")
            continue
        for path in changed:
            try:
                take((checkout / path).read_text(encoding="utf-8", errors="replace"))
            except OSError:
                continue
    return best


def allocate(universe: Universe, held: Sequence[Document], prefix: str, notices: list[str]) -> str:
    """The next ID for ``prefix``, above every one in the bank, every gathered journal and the legacy trees.
    Call it under ``host_lock``."""
    numbers = [int(identifier.split("-", 1)[1]) for identifier in universe.state.entries if identifier.split("-", 1)[0] == prefix]
    numbers.extend(int(document.id.split("@", 1)[0].split("-", 1)[1]) for document in held
                   if ID.fullmatch(document.id.split("@", 1)[0]) and document.id.split("-", 1)[0] == prefix)
    numbers.append(legacy_numbers(universe.root, notices).get(prefix, 0))
    return f"{prefix}-{max(numbers, default=0) + 1:03d}"
