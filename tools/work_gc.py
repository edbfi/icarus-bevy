#!/usr/bin/env python3
"""Scratch lint and cleanup for the ignored ``work/`` directory.

``work/`` holds one directory per case (rules in ``.agents/rules/work.md``).
Cases keep their scope, notes and receipts; what they may shed is
*reproducible scratch*, which the original inputs and the case's own scripts
recreate:

* Ghidra projects: ``<name>.gpr`` with its ``<name>.rep`` and lock files;
* Wine/Proton prefixes: a directory holding ``version`` and ``pfx/system.reg``;
* caches: a directory tagged with a valid ``CACHEDIR.TAG`` or named ``.cache``;
* byte-identical copies (at least 1 MiB) of files under the original input roots.

A candidate is never removed when it equals, contains or lies inside the path
of an active ``local-private`` evidence row, when its case is declared open in
``case.json``, when a running process uses the case, or when the case changed
within the settle window. Age is only a guard, never a reason to delete.

The default run reports and deletes nothing. ``--prune`` removes eligible
candidates and appends one line per removal to ``work/.gc/log.jsonl``.
``--check`` exits 1 while eligible scratch remains, or when ``/proc`` is unavailable
and liveness is unknown, for the milestone checklist.

Usage::

    python3 tools/work_gc.py              # report per case and review signals
    python3 tools/work_gc.py --prune      # remove eligible reproducible scratch
    python3 tools/work_gc.py --check      # exit 1 if eligible scratch remains
    python3 tools/work_gc.py --json       # machine-readable report
"""

from __future__ import annotations

import argparse
from collections.abc import Callable, Iterable, Iterator, Sequence
from dataclasses import dataclass, field
from datetime import UTC, datetime
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import sys
import time
from typing import TypeIs, cast

MIB = 1024 * 1024

# ---------------------------------------------------------------------------
# Policy constants. Changing these is a deliberate, reviewed act.
# ---------------------------------------------------------------------------

WORK = "work"
LOG_DIR = ".gc"
CASE_FILE = "case.json"
CASE_STATES: frozenset[str] = frozenset({"open", "closed"})
ORIGINAL_ROOTS: tuple[str, ...] = ("icarus-orig", "icarus-client-orig", "reference-data")
COPY_MIN_BYTES = MIB
SETTLE_HOURS = 6.0
CACHEDIR_SIGNATURE = b"Signature: 8a477f597d28d172789f06886806bc55"


def _object_dict(value: object) -> TypeIs[dict[object, object]]:
    return isinstance(value, dict)


def load_object(text: str) -> dict[object, object] | None:
    """One parsed JSON value if it is an object; raises ValueError on invalid JSON."""
    # json.loads is untyped; the value is an object until narrowed.
    value = cast(object, json.loads(text))
    return value if _object_dict(value) else None


@dataclass(frozen=True)
class Candidate:
    path: Path
    kind: str
    size: int


@dataclass
class Case:
    name: str
    path: Path
    size: int = 0
    newest: float = 0.0
    state: str | None = None
    state_error: str | None = None
    indexed: bool = False
    live: bool = False
    candidates: list[Candidate] = field(default_factory=list)
    held: list[Candidate] = field(default_factory=list)
    deferred: list[Candidate] = field(default_factory=list)

    @property
    def eligible_bytes(self) -> int:
        return sum(candidate.size for candidate in self.candidates)

    @property
    def held_bytes(self) -> int:
        """Scratch overlapping active evidence: narrow or retire the row before it can go."""
        return sum(candidate.size for candidate in self.held)

    @property
    def deferred_bytes(self) -> int:
        """Scratch in a case that is open, live, settling or has an unusable case.json."""
        return sum(candidate.size for candidate in self.deferred)


def tree_size(path: Path) -> tuple[int, float]:
    """Apparent bytes and newest mtime under ``path``, never following symlinks."""
    try:
        info = path.lstat()
    except OSError:
        return 0, 0.0
    total, newest = info.st_size, info.st_mtime
    if not stat.S_ISDIR(info.st_mode):
        return total, newest
    for root, dirs, files in os.walk(path, followlinks=False):
        for name in (*dirs, *files):
            try:
                entry = os.lstat(os.path.join(root, name))
            except OSError:
                continue
            if not stat.S_ISDIR(entry.st_mode):
                total += entry.st_size
            newest = max(newest, entry.st_mtime)
    return total, newest


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(MIB), b""):
            digest.update(block)
    return digest.hexdigest()


def protected_paths(root: Path) -> set[Path]:
    """Absolute paths of active ``local-private`` evidence rows under ``work/``."""
    protected: set[Path] = set()
    for shard in sorted((root / "evidence").rglob("*.jsonl")):
        for line in shard.read_text(encoding="utf-8").splitlines():
            if not line.strip():
                continue
            record = load_object(line)
            if record is None:
                raise SystemExit(f"work_gc: {shard}: evidence line is not an object")
            path = record.get("path")
            if record.get("status") != "active" or record.get("class") != "local-private" or not isinstance(path, str):
                continue
            if path.startswith(f"{WORK}/"):
                protected.add(root / path.rstrip("/"))
    return protected


def read_state(case: Path) -> tuple[str | None, str | None]:
    """The declared ``case.json`` state, or an error describing why it is unusable."""
    manifest = case / CASE_FILE
    if not manifest.is_file():
        return None, None
    try:
        record = load_object(manifest.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        return None, f"unreadable {CASE_FILE}: {error}"
    state = record.get("state") if record is not None else None
    if not isinstance(state, str) or state not in CASE_STATES:
        return None, f"{CASE_FILE} state must be one of {sorted(CASE_STATES)}"
    return state, None


def live_paths() -> set[Path] | None:
    """Working directories, executables and open files of running processes; None without /proc."""
    proc = Path("/proc")
    if not (proc / "self").exists():
        return None
    seen: set[Path] = set()
    for entry in proc.iterdir():
        if not entry.name.isdigit():
            continue
        links = [entry / "cwd", entry / "exe"]
        try:
            links.extend((entry / "fd").iterdir())
        except OSError:
            pass
        for link in links:
            try:
                seen.add(Path(os.readlink(link)))
            except OSError:
                continue
        try:
            words = (entry / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        try:
            cwd = Path(os.readlink(entry / "cwd"))
        except OSError:
            # Another user's process: absolute arguments still count.
            cwd = Path("/")
        seen.update(argument_paths(cwd, [os.fsdecode(word) for word in words if word]))
    return seen


def argument_paths(cwd: Path, words: Sequence[str]) -> set[Path]:
    """Every command-line word, and every ``--option=value`` value, as a path resolved against ``cwd``."""
    found: set[Path] = set()
    for word in words:
        for text in (word, word.partition("=")[2]):
            if text:
                found.add(Path(os.path.normpath(cwd / text)))
    return found


def is_within(path: Path, parent: Path) -> bool:
    return path == parent or parent in path.parents


def overlaps(path: Path, others: Iterable[Path]) -> bool:
    return any(is_within(path, other) or is_within(other, path) for other in others)


class OriginalIndex:
    """Lazily hashed files of at least ``COPY_MIN_BYTES`` under the original input roots."""

    def __init__(self, roots: Sequence[Path]) -> None:
        self.by_size: dict[int, list[Path]] = {}
        self.digests: dict[Path, str] = {}
        for root in roots:
            for path in self._files(root):
                try:
                    size = path.lstat().st_size
                except OSError:
                    continue
                if size >= COPY_MIN_BYTES:
                    self.by_size.setdefault(size, []).append(path)

    @staticmethod
    def _files(root: Path) -> Iterator[Path]:
        if not root.is_dir():
            return
        for parent, _dirs, files in os.walk(root, followlinks=False):
            for name in files:
                path = Path(parent) / name
                if path.is_file() and not path.is_symlink():
                    yield path

    def is_copy(self, path: Path, size: int) -> bool:
        originals = self.by_size.get(size)
        if not originals:
            return False
        digest = sha256_file(path)
        for original in originals:
            if original not in self.digests:
                self.digests[original] = sha256_file(original)
            if self.digests[original] == digest:
                return True
        return False


def has_cachedir_tag(directory: Path) -> bool:
    tag = directory / "CACHEDIR.TAG"
    try:
        with tag.open("rb") as handle:
            return handle.read(len(CACHEDIR_SIGNATURE)) == CACHEDIR_SIGNATURE
    except OSError:
        return False


def find_candidates(case: Path, originals: OriginalIndex) -> list[Candidate]:
    """Reproducible scratch under ``case``; directories are reported whole and not descended."""
    found: list[Candidate] = []

    def add(path: Path, kind: str) -> None:
        found.append(Candidate(path, kind, tree_size(path)[0]))

    for parent, dirs, files in os.walk(case, followlinks=False):
        here = Path(parent)
        if here != case and (here.name == ".cache" or has_cachedir_tag(here)):
            add(here, "cache")
            dirs.clear()
            continue
        if here != case and (here / "version").is_file() and (here / "pfx" / "system.reg").is_file():
            add(here, "wine-prefix")
            dirs.clear()
            continue
        projects = {name[: -len(".gpr")] for name in files if name.endswith(".gpr")}
        for project in sorted(projects):
            add(here / f"{project}.gpr", "ghidra-project")
            if f"{project}.rep" in dirs:
                add(here / f"{project}.rep", "ghidra-project")
            for name in files:
                if name.startswith(f"{project}.lock"):
                    add(here / name, "ghidra-project")
        dirs[:] = [name for name in dirs if not any(name == f"{project}.rep" for project in projects)]
        claimed = {candidate.path for candidate in found}
        for name in files:
            path = here / name
            if path in claimed or path.is_symlink():
                continue
            try:
                size = path.lstat().st_size
                if size >= COPY_MIN_BYTES and originals.is_copy(path, size):
                    found.append(Candidate(path, "original-copy", size))
            except OSError:
                continue
    return found


@dataclass
class Report:
    root: Path
    cases: list[Case]
    loose: list[tuple[Path, int]]
    live_known: bool

    @property
    def total(self) -> int:
        return sum(case.size for case in self.cases) + sum(size for _path, size in self.loose)


def is_eligible(case: Case, now: float, settle_hours: float) -> bool:
    return case.state != "open" and case.state_error is None and not case.live and now - case.newest >= settle_hours * 3600


def scan(
    root: Path,
    *,
    now: float,
    settle_hours: float = SETTLE_HOURS,
    live: Callable[[], set[Path] | None] = live_paths,
) -> Report:
    work = root / WORK
    protected = protected_paths(root)
    running = live()
    originals = OriginalIndex([root / name for name in ORIGINAL_ROOTS])
    cases: list[Case] = []
    loose: list[tuple[Path, int]] = []
    for entry in sorted(work.iterdir()) if work.is_dir() else []:
        if entry.name == LOG_DIR:
            continue
        if not entry.is_dir() or entry.is_symlink():
            loose.append((entry, tree_size(entry)[0]))
            continue
        case = Case(entry.name, entry)
        case.size, case.newest = tree_size(entry)
        case.state, case.state_error = read_state(entry)
        case.indexed = any(is_within(path, entry) for path in protected)
        case.live = running is None or any(is_within(path, entry) for path in running)
        for candidate in find_candidates(entry, originals):
            (case.held if overlaps(candidate.path, protected) else case.candidates).append(candidate)
        if not is_eligible(case, now, settle_hours):
            case.deferred, case.candidates = case.candidates, []
        cases.append(case)
    return Report(root, cases, loose, running is not None)


def remove(path: Path, work: Path) -> None:
    """Delete one candidate after checking that it still resolves inside ``work/``."""
    resolved_parent = path.parent.resolve()
    if not is_within(resolved_parent, work.resolve()):
        raise SystemExit(f"work_gc: refusing {path}: parent resolves outside {work}")
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def prune(report: Report, now: float) -> list[Candidate]:
    work = report.root / WORK
    log = work / LOG_DIR / "log.jsonl"
    log.parent.mkdir(exist_ok=True)
    removed: list[Candidate] = []
    with log.open("a", encoding="utf-8") as handle:
        for case in report.cases:
            for candidate in case.candidates:
                entry = {
                    "removed_at": datetime.fromtimestamp(now, UTC).isoformat(timespec="seconds"),
                    "case": case.name,
                    "path": candidate.path.relative_to(report.root).as_posix(),
                    "kind": candidate.kind,
                    "bytes": candidate.size,
                }
                # The entry is durable before the removal, so the log never misses a deleted path.
                _ = handle.write(json.dumps(entry, sort_keys=True) + "\n")
                handle.flush()
                os.fsync(handle.fileno())
                remove(candidate.path, work)
                removed.append(candidate)
            case.candidates = []
    return removed


def mib(size: int) -> str:
    return f"{size / MIB:,.1f} MiB"


def render(report: Report) -> list[str]:
    lines: list[str] = []
    for case in sorted(report.cases, key=lambda item: item.eligible_bytes + item.held_bytes + item.deferred_bytes, reverse=True):
        if not case.candidates and not case.held and not case.deferred:
            continue
        flags = [
            "indexed" if case.indexed else "unindexed",
            case.state or ("bad-case-json" if case.state_error else "no-case-json"),
            *(["live"] if case.live else []),
        ]
        lines.append(
            f"{case.name}: {mib(case.size)}; eligible {mib(case.eligible_bytes)}; held {mib(case.held_bytes)}; "
            + f"deferred {mib(case.deferred_bytes)} [{', '.join(flags)}]"
        )
    eligible = sum(case.eligible_bytes for case in report.cases)
    held = [case for case in report.cases if case.held]
    deferred = sum(case.deferred_bytes for case in report.cases)
    unindexed = [case.name for case in report.cases if not case.indexed]
    missing_state = [case.name for case in report.cases if case.state is None]
    lines.append("review signals:")
    lines.append(f"  {len(unindexed)} case(s) without an active evidence row (keep notes, or retire the case deliberately)")
    lines.append(f"  {len(missing_state)} case(s) without a valid {CASE_FILE}")
    lines.append(
        f"  {len(held)} case(s) hold {mib(sum(case.held_bytes for case in held))} of scratch under active evidence rows; "
        + "narrow a directory row to its digested files, or retire it, before pruning"
    )
    lines.append(f"  {len(report.loose)} loose top-level file(s), {mib(sum(size for _path, size in report.loose))}; new scratch belongs in a case")
    for case in report.cases:
        if case.state_error:
            lines.append(f"  {case.name}: {case.state_error}")
    if not report.live_known:
        lines.append("  /proc is unavailable: every case is treated as live")
    lines.append(
        f"work_gc: {len(report.cases)} case(s), {mib(report.total)}; reproducible scratch eligible {mib(eligible)}, "
        + f"held by evidence {mib(sum(case.held_bytes for case in held))}, deferred {mib(deferred)}"
    )
    return lines


def as_json(report: Report) -> dict[str, object]:
    def candidates(items: Sequence[Candidate]) -> list[dict[str, object]]:
        return [{"path": item.path.relative_to(report.root).as_posix(), "kind": item.kind, "bytes": item.size} for item in items]

    return {
        "total_bytes": report.total,
        "live_known": report.live_known,
        "loose": [{"path": path.relative_to(report.root).as_posix(), "bytes": size} for path, size in report.loose],
        "cases": [
            {
                "name": case.name,
                "bytes": case.size,
                "state": case.state,
                "state_error": case.state_error,
                "indexed": case.indexed,
                "live": case.live,
                "eligible": candidates(case.candidates),
                "held": candidates(case.held),
                "deferred": candidates(case.deferred),
            }
            for case in report.cases
        ],
    }


def find_root(start: Path) -> Path:
    for candidate in (start, *start.parents):
        if (candidate / ".git").exists() and (candidate / "evidence").is_dir():
            return candidate
    raise SystemExit("work_gc: repository root with evidence/ not found")


class Options(argparse.Namespace):
    root: Path | None = None
    prune: bool = False
    check: bool = False
    json: bool = False
    settle_hours: float = SETTLE_HOURS


def parse_options(argv: Sequence[str] | None) -> Options:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    _ = parser.add_argument("--root", type=Path, default=None)
    mode = parser.add_mutually_exclusive_group()
    _ = mode.add_argument("--prune", action="store_true", help="remove eligible reproducible scratch")
    _ = mode.add_argument("--check", action="store_true", help="exit 1 while eligible scratch remains")
    _ = parser.add_argument("--json", action="store_true", help="print the report as JSON")
    _ = parser.add_argument("--settle-hours", type=float, default=SETTLE_HOURS, help="skip cases changed more recently")
    options = Options()
    _ = parser.parse_args(argv, namespace=options)
    return options


def main(argv: Sequence[str] | None = None) -> int:
    options = parse_options(argv)
    root = options.root.resolve() if options.root else find_root(Path.cwd().resolve())
    now = time.time()
    report = scan(root, now=now, settle_hours=options.settle_hours)
    if options.prune:
        removed = prune(report, now)
        summary = f"work_gc: removed {len(removed)} candidate(s), {mib(sum(item.size for item in removed))}"
        print(summary, file=sys.stderr if options.json else sys.stdout)
        report = scan(root, now=now, settle_hours=options.settle_hours)
    if options.json:
        print(json.dumps(as_json(report), indent=1, sort_keys=True))
    else:
        print("\n".join(render(report)))
    if options.check and not report.live_known:
        print("work_gc: --check needs /proc to tell live cases apart", file=sys.stderr)
        return 1
    if options.check and any(case.candidates for case in report.cases):
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
