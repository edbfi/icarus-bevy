# SPDX-License-Identifier: AGPL-3.0-or-later
"""``memory backup`` and ``memory restore FILE [--replace] | --drill`` (``.agents/rules/memory.md``,
"Evidence and durability").

A backup is three files in ``$XDG_STATE_HOME/icarus-memory/backups/`` named by its UTC time:

- ``icarus-memory-<YYYYmmddTHHMMSSZ>.dump``: ``pg_dump -Fc`` of the whole database, made with the
  pg0 build's own binaries;
- ``<name>.watermark.json``: the journal watermark, canonical JSON
  ``{"origin_main": <commit or null>, "segments": [<sorted applied segment sha256>]}``;
- ``<name>.sha256``: ``sha256sum`` lines for the other two, written last, so a backup without it is
  incomplete.

Rotation keeps the newest complete backup of each of the 7 most recent days and of each of the 4
most recent ISO weeks that have one; everything else in the set is removed.

Seams for the journal (the defaults mean "no journal applied"; ``watermark``, ``replay_forward`` and
``journal_comparison`` below supply them for the commands):

- ``backup(..., watermark=)``: a ``Callable[[], Watermark]`` called *before* the dump starts, so the
  watermark never claims more than the dump holds (replay is idempotent).
- ``restore(..., replay=)``: ``Callable[[Connection, Watermark], None]``, called once the restored
  instance passes status; it checks the watermark and replays forward. The previous data directory
  is removed only after it returns.
- ``drill(..., journal_check=)``: ``Callable[[Connection, Watermark], list[str]]`` returning problems
  of the restored scratch instance against the journal.

The free-disk floor gates all three; it never stops the service.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from datetime import UTC, date, datetime
from pathlib import Path
from typing import Protocol, cast

from tools.memory.bank import BankError, gather, reconcile
from tools.memory.gitio import GitError, git
from tools.memory.hindsight import Client, HindsightError
from tools.memory.journal import JournalError, segment_bytes
from tools.memory.model import sha256_hex
from tools.memory.online import compare_bank
from tools.memory.service import (
    UNIT, Api, Assets, Connection, Home, ServiceError, Settings, Subcommands, api_command, argument, checked_assets, connection,
    document_total, ensure_bank, ensure_settings, health, home, host_lock, make_directory, objects, prepare_runtime_directories, read_settings, repository_root,
    require_floor, run_service, service_environment, start_unit, systemctl, unit_directory, wait_ready, write_private,
)

PREFIX = "icarus-memory-"
STAMP = "%Y%m%dT%H%M%SZ"
DAILY, WEEKLY = 7, 4
DRILL_PORT, DRILL_PG_PORT = 18683, 18684
_NAME = re.compile(re.escape(PREFIX) + r"([0-9]{8}T[0-9]{6}Z)")
_COMMIT = re.compile(r"[0-9a-f]{40}")
_SHA256 = re.compile(r"[0-9a-f]{64}")


@dataclass(frozen=True)
class Watermark:
    """The journal state a backup holds: the ``origin/main`` commit and the applied segment hashes."""

    origin_main: str | None
    segments: tuple[str, ...]

    def encode(self) -> bytes:
        if self.origin_main is not None and not _COMMIT.fullmatch(self.origin_main):
            raise ServiceError("watermark: origin_main must be a 40-hex commit or null")
        if any(not _SHA256.fullmatch(item) for item in self.segments):
            raise ServiceError("watermark: segments must be sha256 hex digests")
        body = {"origin_main": self.origin_main, "segments": sorted(set(self.segments))}
        return json.dumps(body, sort_keys=True, separators=(",", ":")).encode() + b"\n"

    @classmethod
    def decode(cls, data: bytes) -> Watermark:
        try:
            # json.loads is untyped; narrowed below.
            raw = cast(object, json.loads(data))
        except ValueError as error:
            raise ServiceError(f"watermark: invalid JSON: {error}") from error
        if not isinstance(raw, dict):
            raise ServiceError("watermark: expected a JSON object")
        body = {str(key): value for key, value in cast(dict[object, object], raw).items()}
        origin, segments = body.get("origin_main"), body.get("segments")
        if sorted(body) != ["origin_main", "segments"] or not isinstance(segments, list) or not (origin is None or isinstance(origin, str)):
            raise ServiceError("watermark: expected exactly origin_main (string or null) and segments (array)")
        items = tuple(item for item in cast(list[object], segments) if isinstance(item, str))
        result = cls(origin, items)
        if len(items) != len(cast(list[object], segments)) or result.encode() != data:
            raise ServiceError("watermark: not in canonical form")
        return result


NO_JOURNAL = Watermark(None, ())


def no_journal() -> Watermark:
    return NO_JOURNAL


def no_replay(_connection: Connection, _watermark: Watermark) -> None:
    return None


def no_journal_check(_connection: Connection, _watermark: Watermark) -> list[str]:
    return []


@dataclass(frozen=True)
class Backup:
    dump: Path

    @property
    def watermark(self) -> Path:
        return self.dump.with_name(self.dump.stem + ".watermark.json")

    @property
    def sidecar(self) -> Path:
        return self.dump.with_suffix(".sha256")

    @property
    def stamp(self) -> datetime:
        match = _NAME.fullmatch(self.dump.stem)
        if match is None or self.dump.suffix != ".dump":
            raise ServiceError(f"{self.dump.name}: not a memory backup name ({PREFIX}<UTC stamp>.dump)")
        return datetime.strptime(match.group(1), STAMP).replace(tzinfo=UTC)

    def members(self) -> tuple[Path, Path, Path]:
        return self.dump, self.watermark, self.sidecar


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


def verify(backup: Backup) -> Watermark:
    """Check the sidecar against the dump and the watermark; returns the watermark."""
    _ = backup.stamp
    try:
        lines = backup.sidecar.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        raise ServiceError(f"{backup.sidecar}: unreadable ({error.strerror}); the backup is incomplete") from error
    expected = {backup.dump.name, backup.watermark.name}
    listed: dict[str, str] = {}
    for line in lines:
        digest, separator, name = line.partition("  ")
        if not separator or not _SHA256.fullmatch(digest) or name not in expected or name in listed:
            raise ServiceError(f"{backup.sidecar}: malformed line {line!r}")
        listed[name] = digest
    if set(listed) != expected:
        raise ServiceError(f"{backup.sidecar}: must list {', '.join(sorted(expected))}")
    for name, digest in listed.items():
        if sha256_file(backup.dump.parent / name) != digest:
            raise ServiceError(f"{name}: sha256 does not match {backup.sidecar.name}")
    return Watermark.decode(backup.watermark.read_bytes())


def complete(directory: Path) -> list[Backup]:
    """Backups with a sidecar, oldest first."""
    found = [Backup(path.with_suffix(".dump")) for path in directory.glob(f"{PREFIX}*.sha256") if _NAME.fullmatch(path.stem)]
    return sorted(found, key=lambda item: item.stamp)


def retained(stamps: Iterable[datetime], *, daily: int = DAILY, weekly: int = WEEKLY) -> set[datetime]:
    """The newest stamp of each of the ``daily`` most recent days and of each of the ``weekly`` most recent ISO weeks."""
    keep: set[datetime] = set()
    days: list[date] = []
    weeks: list[tuple[int, int]] = []
    for stamp in sorted(set(stamps), reverse=True):
        day, (year, week, _) = stamp.date(), stamp.isocalendar()
        if day not in days and len(days) < daily:
            days.append(day)
            keep.add(stamp)
        if (year, week) not in weeks and len(weeks) < weekly:
            weeks.append((year, week))
            keep.add(stamp)
    return keep


def rotate(directory: Path) -> list[Path]:
    """Remove complete backups outside the retention; returns the removed files."""
    backups = complete(directory)
    keep = retained(item.stamp for item in backups)
    removed: list[Path] = []
    for item in backups:
        if item.stamp in keep:
            continue
        for path in (item.sidecar, item.dump, item.watermark):
            if path.exists():
                path.unlink()
                removed.append(path)
    return removed


def pg_environment(settings: Settings) -> dict[str, str]:
    return {"PATH": "/usr/bin:/bin", "PGPASSWORD": settings.pg_password, "PGCONNECT_TIMEOUT": "10"}


def pg_target(settings: Settings) -> list[str]:
    return ["-h", "127.0.0.1", "-p", str(settings.pg_port), "-U", "hindsight", "-d", "hindsight"]


def run_quietly(command: list[str], *, env: dict[str, str], timeout: float, label: str) -> None:
    """Run a PostgreSQL tool; on failure show only its last stderr line (raw output can carry data)."""
    result = subprocess.run(command, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True, timeout=timeout, check=False)
    if result.returncode != 0:
        last = (result.stderr.strip().splitlines() or [""])[-1][:300]
        raise ServiceError(f"{label} failed with exit {result.returncode}: {last}")


@dataclass(frozen=True)
class Made:
    backup: Backup
    size: int
    seconds: float
    removed: tuple[Path, ...]


class Lock:
    """An exclusive, non-blocking lock on ``<backups>/.lock`` for backup, restore and drill."""

    def __init__(self, directory: Path) -> None:
        make_directory(directory)
        self.descriptor: int = os.open(directory / ".lock", os.O_RDWR | os.O_CREAT | os.O_CLOEXEC, 0o600)
        try:
            fcntl.flock(self.descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            os.close(self.descriptor)
            raise ServiceError("another backup, restore or drill is running") from error

    def __enter__(self) -> Lock:
        return self

    def __exit__(self, *_: object) -> None:
        os.close(self.descriptor)


def backup(where: Home, runtime: Assets, *, watermark: Callable[[], Watermark] = no_journal, now: datetime | None = None) -> Made:
    """Dump the live database with its watermark and sidecar, then rotate."""
    require_floor(where.state)
    settings = read_settings(where)
    with Lock(where.backups):
        began = time.monotonic()
        moment = (now or datetime.now(UTC)).astimezone(UTC)
        made = Backup(where.backups / f"{PREFIX}{moment.strftime(STAMP)}.dump")
        if any(path.exists() for path in made.members()):
            raise ServiceError(f"{made.dump.name} already exists")
        mark = watermark().encode()
        partial = made.dump.with_name(made.dump.name + ".partial")
        try:
            run_quietly([str(runtime.pg_bin / "pg_dump"), "-Fc", "-f", str(partial), *pg_target(settings)],
                        env=pg_environment(settings), timeout=1800, label="pg_dump")
            with partial.open("rb") as handle:
                os.fsync(handle.fileno())
            os.chmod(partial, 0o600)
            os.replace(partial, made.dump)
        finally:
            partial.unlink(missing_ok=True)
        write_private(made.watermark, mark)
        lines = f"{sha256_file(made.dump)}  {made.dump.name}\n{hashlib.sha256(mark).hexdigest()}  {made.watermark.name}\n"
        write_private(made.sidecar, lines.encode())
        removed = rotate(where.backups)
        return Made(made, made.dump.stat().st_size, time.monotonic() - began, tuple(removed))


def pg0_environment(pg0_home: Path, runtime: Assets, scratch: Path) -> dict[str, str]:
    return {"PATH": "/usr/bin:/bin", "HOME": str(scratch), "TMPDIR": str(scratch), "PG0_HOME": str(pg0_home), "PG0_INSTALLATION_DIR": str(runtime.pg_install)}


def stop_postgres(pg0_home: Path, runtime: Assets) -> None:
    """``pg0 stop`` for one PG0_HOME; a PostgreSQL that is not running is not an error."""
    with tempfile.TemporaryDirectory(prefix="pg0-") as scratch:
        _ = subprocess.run([str(runtime.pg0), "stop", "--name", "hindsight", "--timeout", "60"], env=pg0_environment(pg0_home, runtime, Path(scratch)),
                           stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=90, check=False)


def restore_cluster(pg0_home: Path, dump: Path, settings: Settings, runtime: Assets) -> None:
    """Create a fresh pg0 instance at ``pg0_home`` (which must not exist) and ``pg_restore`` the dump into it.

    pg0 takes the database password only as an argument, as Hindsight itself passes it at every start."""
    if pg0_home.exists():
        raise ServiceError(f"{pg0_home} exists; a restore needs a fresh data directory")
    make_directory(pg0_home)
    with tempfile.TemporaryDirectory(prefix="pg0-") as scratch:
        env = pg0_environment(pg0_home, runtime, Path(scratch))
        try:
            run_quietly([str(runtime.pg0), "start", "--name", "hindsight", "--port", str(settings.pg_port), "--username", "hindsight",
                         "--password", settings.pg_password, "--database", "hindsight", "-c", "listen_addresses=127.0.0.1",
                         "-c", "fsync=on", "-c", "synchronous_commit=on"], env=env, timeout=180, label="pg0 start")
            run_quietly([str(runtime.pg_bin / "pg_restore"), "--no-owner", "--exit-on-error", "--single-transaction", *pg_target(settings), str(dump)],
                        env=pg_environment(settings), timeout=3600, label="pg_restore")
        finally:
            stop_postgres(pg0_home, runtime)


def port_open(port: int) -> bool:
    with socket.socket() as probe:
        probe.settimeout(1)
        return probe.connect_ex(("127.0.0.1", port)) == 0


@dataclass(frozen=True)
class Restored:
    watermark: Watermark
    seconds: float


def restore(where: Home, runtime: Assets, dump: Path, *, replace: bool, replay: Callable[[Connection, Watermark], None] = no_replay) -> Restored:
    """Restore ``dump`` into a fresh data directory of the installed service; the old one is kept until status passes."""
    if where.scratch or not (unit_directory() / UNIT).is_file():
        raise ServiceError("restore manages the installed service; run memory service install first (a scratch home uses --drill)")
    require_floor(where.data)
    began = time.monotonic()
    settings = read_settings(where)
    with Lock(where.backups):
        mark = verify(Backup(dump.resolve()))
        api = Api(settings.connection())
        documents = document_total(api) if api.ready() else None
        occupied = documents != 0 if documents is not None else where.pg0_home.is_dir() and any(where.pg0_home.iterdir())
        if occupied and not replace:
            what = f"holds {documents} document(s)" if documents is not None else "is not running, so its content is unknown"
            raise ServiceError(f"the instance {what}; pass --replace to overwrite it")
        previous, staging = where.data / "pg0.previous", where.data / "pg0.restoring"
        if previous.exists():
            raise ServiceError(f"{previous} remains from an earlier restore; inspect it and remove it first")
        if staging.exists():
            shutil.rmtree(staging)
        _ = systemctl("stop", UNIT)
        stop_postgres(where.pg0_home, runtime)
        restore_cluster(staging, dump, settings, runtime)
        if where.pg0_home.exists():
            _ = where.pg0_home.rename(previous)
        _ = staging.rename(where.pg0_home)
        try:
            prepare_runtime_directories(where)
            _ = start_unit(settings)
            failed = [f"{check.name}: {check.detail}" for check in health(settings, runtime) if not check.ok]
            if failed:
                raise ServiceError("; ".join(failed))
            replay(settings.connection(), mark)
        except BaseException as error:
            # Any failure after the swap (a bank, journal or Git error in replay, an interrupt) puts the previous
            # data directory back and restarts the old unit; the restored one is kept in staging for inspection.
            _ = systemctl("stop", UNIT, check=False)
            stop_postgres(where.pg0_home, runtime)
            _ = where.pg0_home.rename(staging)
            if previous.exists():
                _ = previous.rename(where.pg0_home)
                _ = systemctl("start", UNIT, check=False)
            if not isinstance(error, Exception):
                raise
            raise ServiceError(f"the restored instance failed ({error}); the previous data directory is back, the restored one is {staging}") from error
        if previous.exists():
            shutil.rmtree(previous)
        return Restored(mark, time.monotonic() - began)


class Reader(Protocol):
    """Read-only REST access, as ``Api.get``."""

    def get(self, path: str) -> dict[str, object]: ...


def inventory(api: Reader) -> dict[str, dict[str, tuple[str, tuple[str, ...], str, str]]]:
    """Per bank, per document: listed content hash, sorted tags, and the sha256 of the text and of the metadata from an exact GET."""
    found: dict[str, dict[str, tuple[str, tuple[str, ...], str, str]]] = {}
    for bank in objects(api.get("/v1/default/banks?limit=1000").get("banks"), "banks"):
        name = str(bank.get("bank_id"))
        documents: dict[str, tuple[str, tuple[str, ...], str, str]] = {}
        offset = 0
        while True:
            page = objects(api.get(f"/v1/default/banks/{name}/documents?limit=500&offset={offset}").get("items"), "documents")
            for item in page:
                identifier = str(item.get("id"))
                tags = item.get("tags")
                tag_list = tuple(sorted(str(tag) for tag in cast(list[object], tags))) if isinstance(tags, list) else ()
                exact = api.get(f"/v1/default/banks/{name}/documents/{identifier}")
                text = exact.get("original_text")
                metadata = json.dumps(exact.get("document_metadata"), sort_keys=True).encode()
                documents[identifier] = (str(item.get("content_hash")), tag_list,
                                         hashlib.sha256(text.encode()).hexdigest() if isinstance(text, str) else "",
                                         hashlib.sha256(metadata).hexdigest())
            if len(page) < 500:
                break
            offset += len(page)
        found[name] = documents
    return found


def compare(live: Reader, restored: Reader) -> tuple[int, list[str]]:
    """Document count and problems: banks, documents, content hashes, text, tags or metadata that differ."""
    before, after = inventory(live), inventory(restored)
    problems = [f"banks differ: live {sorted(before)}, restored {sorted(after)}"] if sorted(before) != sorted(after) else []
    for bank in sorted(set(before) & set(after)):
        left, right = before[bank], after[bank]
        problems.extend(f"{bank}/{identifier}: {'missing from restored' if identifier not in right else 'missing from live' if identifier not in left else 'differs'}"
                        for identifier in sorted(set(left) | set(right)) if left.get(identifier) != right.get(identifier))
    return sum(len(documents) for documents in before.values()), problems


@dataclass(frozen=True)
class Drilled:
    made: Made
    restore_seconds: float
    ready_seconds: float
    documents: int
    problems: tuple[str, ...]


def stop_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            _ = process.wait(60)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            _ = process.wait(10)


def serve(scratch: Home, settings: Settings, runtime: Assets) -> tuple[subprocess.Popen[bytes], float]:
    """Run the API of a scratch home (never the installed service) in its own session, held to 4 cores at
    nice 10, output discarded; returns the process and the seconds until it was ready. Stop it with ``stop_process``."""
    if not scratch.scratch:
        raise ServiceError("serve runs scratch homes only; the installed service runs under systemd")
    # Another instance on the port would answer the readiness probe in this one's place.
    for port in (settings.port, settings.pg_port):
        if port_open(port):
            raise ServiceError(f"port {port} is in use; a scratch instance needs its own ports")
    prepare_runtime_directories(scratch)
    env = service_environment(scratch, runtime) | settings.environment()
    process = subprocess.Popen(["/usr/bin/taskset", "-c", "0-3", "/usr/bin/nice", "-n", "10", *api_command(runtime)], env=env, cwd=scratch.data,
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    try:
        seconds = wait_ready(Api(settings.connection()), alive=lambda: process.poll() is None)
        ensure_bank(Api(settings.connection()), settings.bank)
    except BaseException:
        stop_process(process)
        stop_postgres(scratch.pg0_home, runtime)
        raise
    return process, seconds


def drill(where: Home, runtime: Assets, *, watermark: Callable[[], Watermark] = no_journal,
          journal_check: Callable[[Connection, Watermark], list[str]] = no_journal_check) -> Drilled:
    """Back up the live instance, restore that backup into a scratch home on other ports, compare it read-only with the live one, remove it."""
    require_floor(where.state)
    live = Api(read_settings(where).connection())
    if not live.ready():
        raise ServiceError("the live service is not ready; the drill compares against it")
    for port in (DRILL_PORT, DRILL_PG_PORT):
        if port_open(port):
            raise ServiceError(f"drill port {port} is in use")
    made = backup(where, runtime, watermark=watermark)
    mark = verify(made.backup)
    root = Path(tempfile.mkdtemp(prefix="drill-", dir=where.state))
    process: subprocess.Popen[bytes] | None = None
    scratch = Home.under(root)
    try:
        settings = ensure_settings(scratch, port=DRILL_PORT, pg_port=DRILL_PG_PORT)
        began = time.monotonic()
        with Lock(where.backups):
            restore_cluster(scratch.pg0_home, made.backup.dump, settings, runtime)
        restore_seconds = time.monotonic() - began
        prepare_runtime_directories(scratch)
        process, ready_seconds = serve(scratch, settings, runtime)
        restored = Api(settings.connection())
        problems = [f"{check.name}: {check.detail}" for check in health(settings, runtime) if not check.ok]
        documents, differences = compare(live, restored)
        problems.extend(differences)
        problems.extend(journal_check(settings.connection(), mark))
        return Drilled(made, restore_seconds, ready_seconds, documents, tuple(problems))
    finally:
        if process is not None:
            stop_process(process)
        stop_postgres(scratch.pg0_home, runtime)
        shutil.rmtree(root)


# The journal side of backup, restore and drill (the seams of ``backup``, ``restore`` and ``drill``).


def watermark(root: Path) -> Watermark:
    """Replay first, then the journal state the bank holds: the ``origin/main`` commit and the sha256 of
    every applied segment. Called before the dump starts, so it never claims more than the dump holds."""
    bank = Client(connection())
    with host_lock():
        universe = gather(root)
        _ = reconcile(bank, universe, bulk=True)
    return Watermark(origin_commit(root), tuple(sorted(sha256_hex(segment_bytes(segment)) for segment in universe.segments)))


def origin_commit(root: Path) -> str | None:
    try:
        return git(root, "rev-parse", "--verify", "--quiet", "origin/main^{commit}").decode().strip()
    except GitError:
        return None


def watermark_problems(root: Path, mark: Watermark) -> list[str]:
    """Why replaying this checkout's journal onto a dump with ``mark`` would move it backwards: its
    ``origin/main`` must be the watermark's or descend from it."""
    if mark.origin_main is None:
        return []
    current = origin_commit(root)
    if current is None:
        return [f"the backup was taken at origin/main {mark.origin_main[:12]}; this checkout has no origin/main (fetch it)"]
    if descends_from(root, mark.origin_main, current) is not True:
        return [f"this checkout's origin/main {current[:12]} does not contain the backup's {mark.origin_main[:12]}; fetch first"]
    return []


def descends_from(root: Path, ancestor: str, commit: str) -> bool | None:
    result = subprocess.run(["git", "-C", str(root), "merge-base", "--is-ancestor", "--end-of-options", ancestor, commit],
                            capture_output=True, check=False)
    return {0: True, 1: False}.get(result.returncode)


def replay_forward(root: Path) -> Callable[[Connection, Watermark], None]:
    """``restore``'s seam: check the watermark, then apply this checkout's journal to the restored instance."""

    def run_replay(target: Connection, mark: Watermark) -> None:
        if problems := watermark_problems(root, mark):
            raise ServiceError("; ".join(problems))
        with host_lock():
            _ = reconcile(Client(target), gather(root), bulk=True)

    return run_replay


def journal_comparison(root: Path) -> Callable[[Connection, Watermark], list[str]]:
    """The drill's seam: on the restored scratch instance, check the watermark, replay forward, then
    compare the bank with the journal document by document (``check --bank``)."""

    def compare(target: Connection, mark: Watermark) -> list[str]:
        if problems := watermark_problems(root, mark):
            return problems
        bank = Client(target)
        with host_lock():
            _ = reconcile(bank, gather(root), bulk=True)
            return [f"journal: {problem}" for problem in compare_bank(bank, root).data_problems()]

    return compare


# Command line


def run_backup(_options: argparse.Namespace) -> int:
    try:
        root = repository_root()
        made = backup(home(), checked_assets(root), watermark=lambda: watermark(root))
    except (ServiceError, OSError, subprocess.TimeoutExpired, HindsightError, BankError, GitError, JournalError) as error:
        print(f"memory backup: {error}", file=sys.stderr)
        return 1
    print(f"memory backup: {made.backup.dump} ({made.size} bytes, {made.seconds:.1f} s)")
    for path in made.removed:
        print(f"memory backup: rotated out {path.name}")
    return 0


def run_restore(options: argparse.Namespace) -> int:
    file, is_drill, replace = argument(options, "file"), argument(options, "drill") is True, argument(options, "replace") is True
    if is_drill == isinstance(file, str) or (is_drill and replace):
        print("memory restore: give FILE [--replace] or --drill", file=sys.stderr)
        return 2
    try:
        root = repository_root()
        runtime = checked_assets(root)
        if isinstance(file, str):
            done = restore(home(), runtime, Path(file), replace=replace, replay=replay_forward(root))
            origin = done.watermark.origin_main or "none"
            print(f"memory restore: restored {file} in {done.seconds:.1f} s; watermark origin/main {origin}, {len(done.watermark.segments)} segment(s)")
            return 0
        drilled = drill(home(), runtime, watermark=lambda: watermark(root), journal_check=journal_comparison(root))
    except (ServiceError, OSError, subprocess.TimeoutExpired, HindsightError, BankError, GitError, JournalError) as error:
        print(f"memory restore: {error}", file=sys.stderr)
        return 1
    made = drilled.made
    print(f"memory restore --drill: backup {made.backup.dump.name} ({made.size} bytes, {made.seconds:.1f} s)")
    print(f"memory restore --drill: restore {drilled.restore_seconds:.1f} s; scratch ready {drilled.ready_seconds:.1f} s; {drilled.documents} document(s) compared")
    for problem in drilled.problems:
        print(f"problem: {problem}")
    print(f"memory restore --drill: {'passed' if not drilled.problems else f'{len(drilled.problems)} problem(s)'}")
    return 1 if drilled.problems else 0


def register(commands: Subcommands) -> None:
    """``memory service ...``, ``memory backup`` and ``memory restore ...``."""
    service = commands.add_parser("service", help="install and run the always-on memory service")
    _ = service.add_argument("action", choices=("install", "start", "stop", "status", "uninstall"))
    _ = service.add_argument("--reranker", metavar="off|N", help="with install: turn reranking off or cap its candidates per recall (default: the current setting, off on a first install)")
    service.set_defaults(handler=run_service)
    made = commands.add_parser("backup", help="pg_dump the memory service with its watermark, then rotate")
    made.set_defaults(handler=run_backup)
    restored = commands.add_parser("restore", help="restore a backup into a fresh data directory, or --drill into a scratch instance")
    _ = restored.add_argument("file", nargs="?", help=f"a {PREFIX}<stamp>.dump with its .sha256 and .watermark.json")
    _ = restored.add_argument("--replace", action="store_true", help="overwrite an instance that holds documents")
    _ = restored.add_argument("--drill", action="store_true", help="restore a fresh backup into a scratch instance, compare, remove it")
    restored.set_defaults(handler=run_restore)
