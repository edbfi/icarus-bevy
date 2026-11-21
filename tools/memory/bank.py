# SPDX-License-Identifier: AGPL-3.0-or-later
"""The bank as a deterministic function of the journal: gather, desired documents, diff, apply, verify.

``gather(root)`` collects every segment the host knows: ``origin/main`` (the acceptance ref), the
working-tree journal of every linked worktree, and every local branch and remote-tracking ref, read
without a checkout. A branch checked out in a worktree is read from that worktree's working tree only,
so a regenerated unmerged segment is not resurrected from the commit it replaced. A malformed segment
outside the current worktree is skipped with a notice; one in the current worktree, or in
``origin/main``, is an error. Segments are content-named, so they are de-duplicated by name.

``desired(universe)`` maps every document the bank should hold to its retain item, per the rules'
Storage section. Over the union replay, for each record ID:

- some revision accepted (the revision is in an ``origin/main`` segment; segments are content-named and
  revisions content-addressed, so squash merges count): ``<ID>`` holds the head of the accepted
  revisions, ``state:accepted`` (``conflict`` when ``origin/main`` itself has two heads), and every
  union head that is not accepted is a pending revision ``<ID>@<12 hex>`` (the first 12 hex of the
  revision's identity, the key of ``Entry.revisions``), ``state:pending``;
- none accepted: ``<ID>`` holds the union head, ``state:proposed`` (``conflict`` on a fork).

Tags carry the *derived* status, and the text renders it, so an accepted supersession or resolution
refreshes the old record's document although no revision of it is written. The derived status comes
from ACCEPTED links only (``Universe.settled``, the replay of ``origin/main``): a proposal anywhere never
flips a record in the shared bank; read commands overlay proposals at display time. Metadata: ``record``
(canonical JSON), ``sha256`` (its digest), ``rev`` (the revision identity), ``prev`` (comma-separated
``rev`` values) and ``segment`` (repository path).

*Applied* means the bank holds exactly the desired document: same metadata (so the same ``rev`` and
segment), the same text (``content_hash``), entities and tags. ``diff`` compares that;
``apply`` retains what differs, re-tags what differs only in tags and deletes documents of known
revisions that are no longer desired (a pending revision since accepted, an older proposal). A
document whose ``rev`` no gathered segment holds is *unexplained*: it is left alone, and only
``memory sync`` after a successful fetch re-tags it ``state:orphaned``. The same diff is publication,
the replay of a half-applied change set, ``rebuild`` and the repair path.

*Rendering version.* The reconcile marker and stamp carry ``canonical.RENDERING`` (the marker's text ends
``rendering N``; a marker without it was written by a tool before version 2 and reads as 1). A tool whose
version is lower than the marker's refuses to reconcile, so it never rewrites newer documents with older
text: ``reconcile`` raises ``OlderTool`` (merge ``main``), and every write and the repair path go through
it. A tool with a higher version reconciles, which re-renders every document and raises the marker.
"""

from __future__ import annotations

import re
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field, replace
from functools import cached_property
from pathlib import Path, PurePosixPath
from typing import Protocol

from tools.memory.canonical import RENDERING, RenderError, canonical_json, entities, render_text, tags
from tools.memory.gitio import GitError, changed_files, git, read_blobs, run_git, tree_files
from tools.memory.hindsight import Document, Group, Hit, Item, Tags, Unit
from tools.memory.journal import JOURNAL, JournalError, Revision, Segment, load_cache, parse_segment, read_git, read_tree, save_cache
from tools.memory.model import BUILD, ID, SELECTOR, Record, RecordError, as_mapping, id_key, is_array, load_json, sha256_hex
from tools.memory.service import home, write_private
from tools.memory.paths import ProtectedPathsError, worktrees
from tools.memory.state import Entry, State, replay

ACCEPTANCE_REF = "origin/main"
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


@dataclass(frozen=True)
class Universe:
    """Every known segment, the acceptance set and the two replays commands read.

    ``state`` replays every segment (the bank's function); ``view`` replays ``origin/main`` plus this
    worktree, de-duplicated as ``own`` (what this checkout reads and revises); ``settled`` replays
    ``origin/main`` alone (the links that derive status in the bank). ``accepted`` holds the revision
    identities (the keys of ``Entry.revisions``) of ``origin/main``; ``main_known`` is false when that ref
    is missing. ``known`` holds the ``rev`` of every gathered revision, as a bank document's metadata names it.
    """

    root: Path
    segments: tuple[Segment, ...]
    main: tuple[Segment, ...]
    local: tuple[Segment, ...]
    own: tuple[Segment, ...]
    accepted: frozenset[str]
    main_known: bool
    main_commit: str | None = None
    notices: tuple[str, ...] = ()

    # The replays are computed when first read: a ``get`` needs ``state`` and ``settled``, never ``view``.
    @cached_property
    def state(self) -> State:
        return replay(self.segments)

    @cached_property
    def view(self) -> State:
        return replay(self.own)

    @cached_property
    def settled(self) -> State:
        return replay(self.main)

    @property
    def stamp(self) -> Stamp:
        """What reconciling the bank with this universe makes it hold (see ``Stamp``)."""
        return Stamp(self.main_commit, tuple(sorted({segment.sha256 for segment in self.segments})))

    @property
    def local_stems(self) -> frozenset[str]:
        return frozenset(PurePosixPath(segment.path).stem for segment in self.local)

    @property
    def known(self) -> frozenset[str]:
        return frozenset(rev for entry in self.state.entries.values() for rev in entry.revisions)


def _batch_check(root: Path, names: Sequence[str], kind: str = "tree") -> list[str | None]:
    """The object ID of type ``kind`` each ``<commit>:<path>`` names, or ``None``."""
    if not names:
        return []
    output = git(root, "cat-file", "--batch-check", stdin="".join(f"{name}\n" for name in names).encode()).decode()
    found: list[str | None] = []
    for line in output.splitlines():
        parts = line.split()
        found.append(parts[0] if len(parts) == 3 and parts[1] == kind else None)
    if len(found) != len(names):
        raise GitError("cat-file --batch-check answered an unexpected number of lines")
    return found


def refs(root: Path) -> tuple[tuple[str, str, str], ...]:
    """``(refname, commit, worktree path or "")`` of every local branch and remote-tracking ref that names a commit."""
    output = git(root, "for-each-ref", "--format=%(refname)%00%(objectname)%00%(objecttype)%00%(worktreepath)", *REF_NAMESPACES).decode()
    found: list[tuple[str, str, str]] = []
    for line in output.splitlines():
        name, commit, kind, checkout = line.split("\0")
        if kind == "commit":
            found.append((name, commit, checkout))
    return tuple(found)


def ref_commits(root: Path, *, checked_out: bool = True) -> tuple[str, ...]:
    """Commits of every ref; without ``checked_out``, not of branches checked out in a worktree (read from its working tree)."""
    return tuple(sorted({commit for _name, commit, checkout in refs(root) if checked_out or not checkout}))


CACHE_FILE = "journal-cache.pickle"


def gather(root: Path, base: str = ACCEPTANCE_REF) -> Universe:
    """Every segment known on this host (see the module docstring for what is fatal), parsed through the
    state directory's parse cache (``journal.load_cache``)."""
    cache = home().state / CACHE_FILE
    if cache.parent.is_dir():
        load_cache(cache)
    universe = _gather(root, base)
    if cache.parent.is_dir():
        save_cache(cache)
    return universe


def _gather(root: Path, base: str) -> Universe:
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
    commits = ref_commits(root, checked_out=False)
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
    accepted = frozenset(revision.rev for segment in main for revision in segment.revisions)
    own = tuple(sorted({segment.name: segment for segment in (*main, *local)}.values(), key=lambda item: item.path))
    commit: str | None = None
    if main_known:
        try:
            commit = git(root, "rev-parse", "--verify", "--quiet", "--end-of-options", f"{base}^{{commit}}").decode().strip()
        except GitError:
            commit = None
    return Universe(root, segments, main, local, own, accepted, main_known, commit, tuple(notices))


def shared_elsewhere(root: Path, path: str) -> bool:
    """Whether segment ``path`` of this worktree is also in a ref other than the branch checked out here
    (``origin/main``, a pushed branch, another branch) or in another worktree's working tree: then it
    cannot be regenerated, only revised."""
    own = root.resolve()
    commits = sorted({commit for _name, commit, checkout in refs(root) if not checkout or Path(checkout).resolve() != own})
    if any(found is not None for found in _batch_check(root, [f"{commit}:{path}" for commit in commits], kind="blob")):
        return True
    try:
        checkouts, _skipped = worktrees(root)
    except ProtectedPathsError:
        return True
    return any(checkout.resolve() != own and (checkout / path).exists() for checkout in checkouts)


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


def derived_status(settled: State, record: Record) -> str:
    """The status a revision shows in the bank: superseded or resolved by ACCEPTED links (``settled``, the
    replay of ``origin/main``) or by its own legacy row, an implied legacy retirement, or its stored status."""
    legacy = record.legacy
    implied = legacy.implied_status if legacy is not None else None
    if settled.superseded_by.get(record.id) or (legacy is not None and legacy.superseded_by) or implied == "superseded":
        return "superseded"
    if settled.resolved_by.get(record.id) or implied == "resolved":
        return "resolved"
    return "retired" if implied == "retired" else record.status


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
        metadata={"record": canonical_json(record.to_json()).decode(), "sha256": revision.sha256, "rev": revision.rev,
                  "prev": ",".join(revision.prev), "segment": segment},
        timestamp=f"{date}T00:00:00Z" if date else "unset",
        entities=names,
        entity_types=entity_types(names),
    )


def accepted_heads(entry: Entry, accepted: frozenset[str]) -> tuple[str, ...]:
    """Heads among the accepted revisions of ``entry``: accepted identities no accepted revision names as ``prev``."""
    mine = {rev for rev in entry.revisions if rev in accepted}
    named = {prev for rev in mine for prev in entry.revisions[rev].revision.prev}
    return tuple(sorted(mine - named))


def desired_for(universe: Universe, identifier: str) -> list[Desired]:
    """The documents one record ID should have: ``<ID>`` and its pending revisions (see the module docstring)."""
    entry = universe.state.entries[identifier]
    found: list[Desired] = []

    def add(document_id: str, rev: str, document_state: str) -> None:
        located = entry.revisions[rev]
        status = derived_status(universe.settled, located.revision.record)
        item = item_for(document_id, located.revision, located.segment, document_state, status)
        found.append(Desired(document_id, entry.id, document_state, status, located.revision, located.segment, item))

    heads = accepted_heads(entry, universe.accepted)
    if heads:
        add(identifier, heads[0], "conflict" if len(heads) > 1 else "accepted")
        for rev in entry.heads:
            if rev not in universe.accepted:
                add(f"{identifier}@{rev[:12]}", rev, "pending")
    else:
        add(identifier, entry.heads[0], "conflict" if entry.conflict else "proposed")
    return found


def desired(universe: Universe) -> dict[str, Desired]:
    """Every document the bank should hold, by document ID."""
    return {item.document_id: item for identifier in sorted(universe.state.entries, key=id_key) for item in desired_for(universe, identifier)}


class Wanted:
    """``desired`` computed per record on demand: a read that touches a few records never renders them all."""

    def __init__(self, universe: Universe) -> None:
        self.universe: Universe = universe
        self._of: dict[str, list[Desired]] = {}

    def of(self, identifier: str) -> list[Desired]:
        """The documents of record ``identifier`` (empty for an unknown ID)."""
        if identifier not in self._of:
            self._of[identifier] = desired_for(self.universe, identifier) if identifier in self.universe.state.entries else []
        return self._of[identifier]

    def get(self, document_id: str) -> Desired | None:
        return next((item for item in self.of(document_id.split("@", 1)[0]) if item.document_id == document_id), None)


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


def diff(wanted: Mapping[str, Desired], held: Sequence[Document], known: frozenset[str]) -> Plan:
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
        if document.id in wanted or document.id == MARKER:
            continue
        if document.metadata.get("rev", "") in known:
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
    if not {"record", "sha256", "rev"} <= document.metadata.keys() or "state:orphaned" in document.tags:
        return None
    return tuple(sorted({*(tag for tag in document.tags if not tag.startswith("state:")), "state:orphaned"}))


def reconcile(bank: Bank, universe: Universe, *, bulk: bool = False, orphan: bool = False) -> Applied:
    """Make the bank hold exactly ``desired(universe)``; with ``orphan`` (only after a successful fetch)
    re-tag unexplained documents ``state:orphaned``. Raises ``OlderTool`` before touching a bank a newer
    tool rendered, and ``BankError`` if a desired document still differs."""
    if (held := marker_rendering(bank)) is not None and held > RENDERING:
        raise OlderTool(held)
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
    mark(bank, universe.stamp)
    result.notices.extend(universe.notices)
    return result


# The reconcile stamp: what the bank was last reconciled to, so a read with nothing new skips the bank diff.

MARKER = "memory-stamp"
STAMP_FILE = "memory.stamp.json"
MARKER_RENDERING = re.compile(r" rendering ([0-9]+)$")


class OlderTool(BankError):
    """The bank was rendered by a newer tool; this one must not reconcile or write it."""

    def __init__(self, held: int) -> None:
        super().__init__(f"this tool renders records at version {RENDERING}, older than the bank's {held}: it does not write or repair"
                         + " the bank; merge main into this worktree (exact reads by ID still work)")
        self.held: int = held


@dataclass(frozen=True)
class Stamp:
    """The ``origin/main`` commit, the sha256 of every segment a reconciliation applied and the rendering
    version. ``digest`` names it; the bank holds the same digest in its ``MARKER`` document, so a restored
    or foreign-written bank (whose marker differs) is never trusted."""

    origin_main: str | None
    segments: tuple[str, ...]
    rendering: int = RENDERING

    @property
    def digest(self) -> str:
        return sha256_hex(canonical_json({"origin_main": self.origin_main, "rendering": self.rendering, "segments": list(self.segments)}))

    def json(self) -> bytes:
        return canonical_json({"digest": self.digest, "origin_main": self.origin_main, "rendering": self.rendering, "segments": list(self.segments)})


def marker_item(stamp: Stamp) -> Item:
    return Item(MARKER, f"memory stamp {stamp.digest} rendering {stamp.rendering}", ("marker:stamp",), {"stamp": stamp.digest}, "unset", ())


def marker_rendering(bank: Bank) -> int | None:
    """The rendering version of the bank's marker: ``None`` without a marker, 1 for one written before versions."""
    held = bank.document(MARKER)
    if held is None:
        return None
    found = MARKER_RENDERING.search(held.text or "")
    return int(found[1]) if found else 1


def mark(bank: Bank, stamp: Stamp) -> None:
    """After a full reconciliation: the marker document in the bank, then the stamp file under the host lock."""
    marker = marker_item(stamp)
    held = bank.document(MARKER)
    if held is None or held.text != marker.content or held.metadata != dict(marker.metadata):
        bank.retain([marker])
    write_private(home().state / STAMP_FILE, stamp.json())


def unchanged(bank: Bank, universe: Universe) -> bool:
    """Whether a read may skip the bank diff: the stamp file names this ``origin/main`` commit, holds every
    gathered segment, and the bank's marker carries its digest. Anything else, unreadable included, is no."""
    try:
        raw = as_mapping(load_json((home().state / STAMP_FILE).read_bytes()), STAMP_FILE)
    except (OSError, RecordError):
        return False
    segments, origin = raw.get("segments"), raw.get("origin_main")
    if not is_array(segments) or not all(isinstance(item, str) for item in segments) or not (origin is None or isinstance(origin, str)):
        return False
    stamp = Stamp(origin, tuple(str(item) for item in segments))
    if raw.get("digest") != stamp.digest or origin != universe.main_commit or not set(universe.stamp.segments) <= set(stamp.segments):
        return False
    held = bank.document(MARKER)
    return held is not None and held.metadata.get("stamp") == stamp.digest


def read_back(bank: Bank, wanted: Wanted, revisions: Sequence[Revision]) -> list[str]:
    """Exact GET of each published revision's document; problems if its text, sha256 or metadata differ."""
    problems: list[str] = []
    for revision in revisions:
        target = next((item for item in wanted.of(revision.record.id) if item.revision == revision), None)
        if target is None:
            continue
        document = bank.document(target.document_id)
        if document is None:
            problems.append(f"{target.document_id}: missing after publication")
            continue
        if document.text != target.item.content or document.content_hash != sha256_hex(target.item.content.encode()):
            problems.append(f"{target.document_id}: text differs after publication")
        if document.metadata != dict(target.item.metadata):
            problems.append(f"{target.document_id}: metadata (rev {revision.rev[:12]}) differs after publication")
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
        # git grep exits 1 when no tree holds a matching ID; anything above 1 is a real failure.
        _, output = run_git(root, "-c", "grep.threads=4", "grep", "-h", "-o", "-I", "-E", r"(F|U|N|SL|RP|CP|REF|M)-[0-9]+", *trees, ok=(0, 1))
        take(output.decode("utf-8", "replace"))
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
