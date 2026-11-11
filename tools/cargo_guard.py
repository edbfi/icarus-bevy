#!/usr/bin/env python3
"""Cooperative, bounded Cargo builds; see docs/runbooks/cargo-builds.md.

Linux only, single root package, checkout-local targets, standard profiles.
This is an operational guard, not a sandbox against privileged or same-user code.

Contracts and limitations:
* Permanent flock in the private Git common state directory serializes builds.
  Per-target runtime leases are shared; cleanup is exclusive. Descriptors pass
  to the direct child, which must retain them. SIGKILL of the supervisor then
  does not drop exclusion while that child survives. Catchable HUP/INT/TERM stop
  the owned group. Escaped descendants or children closing descriptors need a
  cgroup supervisor. Never unlink lock/state files in use. Working Linux local
  filesystem flock is required; no NFS safety claim.
* Only fresh private 0700 targets are auto-owned, by device/inode. Even an empty
  legacy target needs reviewed exact-path adoption. Adoption deletes nothing;
  the next build fully retires it with Cargo. Other checkout caches are only
  canonicalized/deduplicated reports, including aliases. Own symlinks fail.
* Cleanup is based on ownership + exclusive lease + private target + mount and
  evidence checks + Cargo lock quiescence. /proc is only a best-effort veto for
  readable cwd/exe/fd/maps/compiler matches; denied, vanished or hidden processes
  cannot establish absence of use. Count observable limitations. This protocol
  assumes cooperation and cannot stop privileged/noncooperating same-user races.
* Cargo layout.rs uses profile/.cargo-lock, with .cargo-build-lock and
  .cargo-artifact-lock in newer versions. Probe existing locks, then CLOSE them
  before Cargo starts: holding Cargo's own lock would deadlock it. Source:
  https://github.com/rust-lang/cargo/blob/master/src/compiler/layout.rs
  Full Cargo clean does not itself lock against concurrent builds. Recheck Cargo
  lock locations when updating Cargo. There is no filename-based deletion.
* Cleanup-only metadata checks reject mounts, foreign entries, symlinks, and
  regular-file hardlinks outside target. Ordinary allocated-byte snapshots
  deduplicate inodes and permit links. Active evidence is protected lexically
  and after resolution across existing worktrees. Read bounded sha256 manifest
  hash/path metadata when present (tools/sha256_manifest.py decides which rows
  are manifests, from the name or at most the first 4 KiB), never other artifact
  contents; absent worktree/CI manifests retain lexical protection.
* PREPARED identifies reusable partial artifacts before invoking Cargo; validation
  is a separate success flag. Failed commands/timeouts retain this identity.
  Persist needs_clean before CLEAN; retry retires it. Recover stale owned regular
  pending state under the repository lock, retaining committed state. Unsafe
  aliases require review. Ordinary module changes do not alter layout identity.
  Cargo.lock/toolchain/profile/config/target layout changes fully retire
  the cache, including dependency artifacts, instead of accumulating generations.
* Provisional own cap 10 GiB; others are report-only. Admission reserves
  min(8 GiB, remaining cap), requiring >=2 GiB remaining. Cold=8 GiB reserve;
  warm 4 GiB=6 GiB reserve; warm 8 GiB=2 GiB reserve. Local floor 30 GiB;
  explicit CI floor 4 GiB. Floor/RAM/time poll every 250 ms; allocated bytes
  snapshot before/after only. Free-space deltas do not identify this cache's
  writes. This is no kernel quota: bursts and cap overshoot remain possible.
  Budgets need cold/warm calibration; no measured savings claim is made.
* Two jobs, >=3 GiB available RAM, <=10 GiB owned group resident RAM (shared
  pages conservatively summed); command deadline 30 minutes, lock wait the same.
  Cleanup/version probes bypass disk admission with deadlines 5 min/30 sec.
  Long-running servers need their own bounded operating procedure.
* Harmless inherited Cargo net/registry/http config is read-only. Compiler,
  wrapper, custom-profile and output redirection overrides are refused. Accepted
  feature flags are refused until this package defines a reviewed feature policy. RUSTUP_TOOLCHAIN must match the
  pinned channel and active host. CI explicitly selects its policy by environment;
  CI=true is no exception and no generated Cargo config is needed.
"""
from __future__ import annotations

import argparse
from collections.abc import Generator, Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
import fcntl
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
import tomllib
from typing import TypeIs, cast

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from tools import docs_lint, sha256_manifest, work_gc

GIB = 1024**3
RAM_FLOOR = 3 * GIB
OWNED_RAM_LIMIT = 10 * GIB
DISK_FLOOR = 30 * GIB
RESERVE = 8 * GIB
MIN_HEADROOM = 2 * GIB
TARGET_BUDGET = 10 * GIB
TIMEOUT = 1800.0
POLL = 0.25
SCHEMA = 2
# Existing evidence has about 1.5 MiB / 15,000 entries; keep bounded growth room.
FIXITY_METADATA_BYTES = 8 * 1024**2
FIXITY_METADATA_ENTRIES = 100_000
PROC = Path("/proc")
MOUNTINFO = PROC / "self/mountinfo"
TERMINATION = {signal.SIGHUP, signal.SIGINT, signal.SIGTERM}


@dataclass(frozen=True)
class Policy:
    disk_floor: int = DISK_FLOOR


LOCAL = Policy()
CI = Policy(disk_floor=4 * GIB)


class GuardError(RuntimeError):
    """A refused or interrupted operation."""


def selected_policy(env: Mapping[str, str], name: str | None) -> Policy:
    selected = name or env.get("ICARUS_CARGO_GUARD_POLICY", "local")
    if selected not in {"local", "ci"}:
        raise GuardError(f"unknown guard policy: {selected}")
    return CI if selected == "ci" else LOCAL


def digest(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def object_dict(value: object) -> TypeIs[dict[object, object]]:
    return isinstance(value, dict)


def table(value: object) -> dict[str, object]:
    if not object_dict(value):
        raise GuardError("expected a TOML table")
    return {str(key): item for key, item in value.items()}


def toml_object(path: Path) -> dict[str, object]:
    return table(cast(object, tomllib.loads(path.read_text(encoding="utf-8"))))


def git_text(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(root), *args], capture_output=True, text=True,
        timeout=10, check=True,
    ).stdout.strip()


@dataclass(frozen=True)
class Identity:
    root: Path
    common: Path

    @property
    def target(self) -> Path:
        return self.root / "target"

    @property
    def state_dir(self) -> Path:
        return self.common / "cargo-guard"

    @property
    def state(self) -> Path:
        return self.state_dir / f"{digest(str(self.root))}.json"

    @property
    def lease(self) -> Path:
        return self.state.with_suffix(".lease")


def identity(root: Path) -> Identity:
    root = root.resolve(strict=True)
    if Path(git_text(root, "rev-parse", "--show-toplevel")).resolve() != root:
        raise GuardError("run from the checkout root")
    common = Path(git_text(root, "rev-parse", "--path-format=absolute", "--git-common-dir")).resolve(strict=True)
    if work_gc.overlaps(root / "target", [common]):
        raise GuardError("Git common directory overlaps target")
    return Identity(root, common)


def state_directory(info: Identity) -> None:
    info.state_dir.mkdir(mode=0o700, exist_ok=True)
    entry = info.state_dir.lstat()
    if not stat.S_ISDIR(entry.st_mode) or entry.st_uid != os.getuid():
        raise GuardError("guard state directory must be owned and not a symlink")
    if stat.S_IMODE(entry.st_mode) != 0o700:
        raise GuardError("guard state directory must be private (0700)")


def acquire(fd: int, mode: int, timeout: float, label: str) -> None:
    deadline = time.monotonic() + timeout
    announced = False
    while True:
        try:
            fcntl.flock(fd, mode | fcntl.LOCK_NB)
            return
        except BlockingIOError:
            if not announced:
                print(f"cargo_guard: waiting for {label} (up to {timeout:g}s)", file=sys.stderr)
                announced = True
            if time.monotonic() >= deadline:
                raise GuardError(f"{label} timed out") from None
            time.sleep(POLL)


@contextmanager
def file_lock(info: Identity, path: Path, mode: int, timeout: float) -> Generator[int]:
    state_directory(info)
    fd = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        entry = os.fstat(fd)
        if not stat.S_ISREG(entry.st_mode) or entry.st_uid != os.getuid() or entry.st_nlink != 1:
            raise GuardError("guard lock must be an owned regular file without aliases")
        label = f"target lease ({info.target})" if path == info.lease else "build lock"
        acquire(fd, mode, timeout, label)
        yield fd
    finally:
        # Do NOT LOCK_UN: an inherited descriptor shares this open description.
        # Close only, so a surviving cooperating child retains the exclusion.
        os.close(fd)


@contextmanager
def build_lock(info: Identity, timeout: float = TIMEOUT) -> Generator[int]:
    with file_lock(info, info.state_dir / "build.lock", fcntl.LOCK_EX, timeout) as fd:
        yield fd


@contextmanager
def target_lease(info: Identity, *, exclusive: bool, timeout: float = TIMEOUT) -> Generator[int]:
    with file_lock(info, info.lease, fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH, timeout) as fd:
        yield fd


def check_configuration(info: Identity, env: Mapping[str, str]) -> str:
    target = info.target
    if target.is_symlink() or target.resolve() != target:
        raise GuardError("target must be the canonical checkout-local target directory")
    if target.exists() and (not target.is_dir() or target.stat().st_uid != os.getuid()):
        raise GuardError("target must be a directory owned by this user")
    mount_veto(target)
    if env.get("CARGO_TARGET_DIR", str(target)) != str(target):
        raise GuardError("unexpected CARGO_TARGET_DIR; unset it")
    channel = table(toml_object(info.root / "rust-toolchain.toml").get("toolchain")).get("channel")
    toolchain = env.get("RUSTUP_TOOLCHAIN")
    if not isinstance(channel, str) or (toolchain is not None and toolchain != channel and not toolchain.startswith(channel + "-")):
        raise GuardError("RUSTUP_TOOLCHAIN must match the pinned toolchain")
    forbidden = {"RUSTFLAGS", "RUSTDOCFLAGS", "RUSTC", "RUSTDOC", "RUSTC_WRAPPER",
                 "RUSTC_WORKSPACE_WRAPPER", "CARGO_INCREMENTAL", "CARGO_MAKEFLAGS", "MAKEFLAGS",
                 "CARGO_ENCODED_RUSTFLAGS", "CARGO_ENCODED_RUSTDOCFLAGS", "RUSTC_BOOTSTRAP",
                 "CC", "CXX", "AR", "CFLAGS", "CXXFLAGS", "LDFLAGS"}
    prefixes = ("CARGO_BUILD_", "CARGO_PROFILE_", "CARGO_TARGET_", "CARGO_ALIAS_", "CARGO_UNSTABLE_", "CC_", "CXX_", "AR_")
    for key, value in env.items():
        if key == "CARGO_TARGET_DIR" or (key == "CARGO_BUILD_JOBS" and value == "2"):
            continue
        if key in forbidden or key.startswith(prefixes):
            raise GuardError(f"unsupported compiler/profile/layout override: {key}; unset it")
    cargo_home = Path(env.get("CARGO_HOME", str(Path.home() / ".cargo")))
    if not cargo_home.is_absolute():
        raise GuardError("CARGO_HOME must be absolute")
    configs = {parent / ".cargo" / name for parent in (info.root, *info.root.parents) for name in ("config", "config.toml")}
    configs.update(cargo_home / name for name in ("config", "config.toml"))
    records: list[str] = []
    for config in sorted(configs):
        if not config.exists():
            continue
        settings = toml_object(config)
        if set(settings) - {"build", "net", "registry", "registries", "http"}:
            raise GuardError(f"unsupported Cargo configuration: {config}")
        if "build" in settings and table(settings["build"]) != {"jobs": 2}:
            raise GuardError(f"only build.jobs = 2 is supported in {config}")
        if "build" in settings:
            records.append(json.dumps(settings["build"], sort_keys=True))
    return digest("\n".join(records))


def mount_veto(target: Path, metadata: Path = MOUNTINFO) -> None:
    for line in metadata.read_text().splitlines():
        fields = line.split()
        if len(fields) < 7:
            raise GuardError("unreadable filesystem mount metadata")
        point = fields[4]
        for escaped, literal in (("\\040", " "), ("\\011", "\t"), ("\\012", "\n"), ("\\134", "\\")):
            point = point.replace(escaped, literal)
        if work_gc.is_within(Path(point), target):
            raise GuardError("target contains a mount/bind mount; manual review required")


def check_arguments(args: Sequence[str], package: str) -> None:
    """Accept normal selections, reject redirection and unsupported feature variants."""
    if not args or args[0] not in {"check", "build", "test", "clippy"}:
        raise GuardError("supported commands: check, build, test, clippy")
    if "--locked" not in args and "--frozen" not in args:
        raise GuardError("--locked is required")
    if any(any(token in arg for token in (";", ">", "<", "|", "`", "$(", "\n")) for arg in args):
        raise GuardError("redirection/shell syntax is unsupported")
    flags = {"--workspace", "--all-targets", "--locked", "--frozen", "--offline", "--release", "--lib", "--bins", "--tests", "--examples", "--benches", "--no-run", "--no-fail-fast", "-q", "--quiet", "-v", "-vv", "--verbose", "--all-features", "--no-default-features"}
    values = {"--bin", "--test", "--example", "--bench", "--package", "-p", "--message-format", "--color", "--features", "-F", "--profile", "--config"}
    index = 1
    while index < len(args):
        arg = args[index]
        if arg == "--":
            trailer = list(args[index + 1:])
            if args[0] == "clippy" and trailer == ["-D", "warnings"]:
                break
            if args[0] != "test":
                raise GuardError("unsupported compiler trailer")
            check_test_trailer(trailer)
            break
        key, equal, value = arg.partition("=")
        if key in {"--features", "-F", "--all-features", "--no-default-features"}:
            raise GuardError("feature flags require a reviewed feature policy; this package defines none")
        if key in values:
            if not equal:
                index += 1
                if index >= len(args):
                    raise GuardError(f"missing value: {key}")
                value = args[index]
            if not value or value.startswith("-"):
                raise GuardError(f"invalid argument value: {key}")
            if key in {"--package", "-p"} and value != package:
                raise GuardError("only the root package is supported")
            if key == "--profile" and value not in {"dev", "test", "release", "bench"}:
                raise GuardError("only standard profiles are supported")
            if key == "--config":
                setting = table(cast(object, tomllib.loads(value)))
                if not setting or set(setting) - {"net", "registry", "registries", "http"}:
                    raise GuardError("only net/registry/http --config is supported")
            if key == "--message-format" and value not in {"human", "short", "json", "json-diagnostic-short", "json-diagnostic-rendered-ansi", "json-render-diagnostics"}:
                raise GuardError("unsupported message format")
        elif arg in flags:
            pass
        elif args[0] == "test" and not arg.startswith("-"):
            pass  # Cargo's TESTNAME filter.
        else:
            raise GuardError(f"unsupported Cargo argument: {arg}")
        index += 1


def check_test_trailer(args: Sequence[str]) -> None:
    flags = {"--list", "--ignored", "--include-ignored", "--exact", "--nocapture", "--no-capture", "--show-output", "-q", "--quiet"}
    values = {"--test-threads", "--format", "--skip", "--color"}
    index = 0
    while index < len(args):
        key, equal, value = args[index].partition("=")
        if key in values:
            if not equal:
                index += 1
                if index >= len(args):
                    raise GuardError(f"missing test trailer value: {key}")
                value = args[index]
            if not value or value.startswith("-") or (key == "--test-threads" and (not value.isdigit() or int(value) < 1)):
                raise GuardError("invalid test trailer value")
        elif key not in flags and key.startswith("-"):
            raise GuardError(f"unsupported test trailer: {key}")
        index += 1


def package_name(root: Path) -> str:
    manifest = toml_object(root / "Cargo.toml")
    if "workspace" in manifest:
        raise GuardError("workspace layout requires a reviewed guard policy update")
    name = table(manifest.get("package")).get("name")
    if not isinstance(name, str) or not name:
        raise GuardError("expected one named root package")
    return name


def generation(info: Identity, config: str, toolchain: str) -> dict[str, str]:
    manifest = toml_object(info.root / "Cargo.toml")
    package = table(manifest.get("package"))
    roots: list[str] = []
    for name, enabled in (("src/lib.rs", "autolib"), ("src/main.rs", "autobins")):
        if package.get(enabled, True) and (info.root / name).is_file():
            roots.append(name)
    for directory, enabled in (("src/bin", "autobins"), ("tests", "autotests"), ("examples", "autoexamples"), ("benches", "autobenches")):
        source = info.root / directory
        if package.get(enabled, True) and source.is_dir():
            for entry in source.iterdir():
                if entry.is_file() and entry.suffix == ".rs":
                    roots.append(str(entry.relative_to(info.root)))
                elif entry.is_dir() and (entry / "main.rs").is_file():
                    roots.append(str((entry / "main.rs").relative_to(info.root)))
    layout = {key: value for key, value in manifest.items() if key in {"lib", "bin", "test", "example", "bench"}}
    return {
        "layout": digest(json.dumps([layout, sorted(roots)], sort_keys=True)),
        "dependencies": digest((info.root / "Cargo.lock").read_text() + json.dumps(manifest, sort_keys=True)),
        "toolchain": digest(toolchain + (info.root / "rust-toolchain.toml").read_text()),
        "config": digest(config),
    }


def target_inventory(target: Path, *, strict: bool = False) -> tuple[int, int]:
    """Deduplicated allocated bytes. Strict checks are for quiescent cleanup only."""
    inodes: set[tuple[int, int]] = set()
    regular_links: dict[tuple[int, int], tuple[int, int]] = {}
    total = count = 0
    if not target.exists():
        return total, count
    device = target.stat().st_dev
    deadline = time.monotonic() + 30
    for parent, dirs, files in os.walk(target, followlinks=False, onerror=raise_walk_error if strict else None):
        for name in (*dirs, *files):
            if time.monotonic() > deadline:
                raise GuardError("target metadata inventory exceeded 30 seconds")
            try:
                entry = (Path(parent) / name).lstat()
            except FileNotFoundError:
                if strict:
                    raise
                continue
            if strict and (entry.st_uid != os.getuid() or entry.st_dev != device or not (stat.S_ISREG(entry.st_mode) or stat.S_ISDIR(entry.st_mode))):
                raise GuardError(f"non-regular/foreign target entry requires review: {Path(parent) / name}")
            key = (entry.st_dev, entry.st_ino)
            if key not in inodes:
                total += entry.st_blocks * 512
                inodes.add(key)
            if strict and stat.S_ISREG(entry.st_mode):
                seen, _ = regular_links.get(key, (0, entry.st_nlink))
                regular_links[key] = (seen + 1, entry.st_nlink)
            count += 1
    if strict and any(seen != links for seen, links in regular_links.values()):
        raise GuardError("cleanup target has hardlinks outside its inventory; review required")
    return total, count


def raise_walk_error(error: OSError) -> None:
    raise error


def checkout_roots(info: Identity) -> list[Path]:
    roots = {info.root}
    for record in git_text(info.root, "worktree", "list", "--porcelain", "-z").split("\0"):
        if record.startswith("worktree "):
            try:
                root = Path(record.removeprefix("worktree ")).resolve()
                if root.is_dir():
                    roots.add(root)
            except (OSError, RuntimeError):
                continue  # Missing/prunable/invalid unrelated aliases grant no authority.
    return sorted(roots)


def report_caches(info: Identity) -> None:
    seen: set[Path] = set()
    for root in checkout_roots(info):
        try:
            target = (root / "target").resolve()
            if target in seen:
                continue
            seen.add(target)
            size, _ = target_inventory(target)
            print(f"cargo_guard: cache={target} allocated_bytes={size} (report only)", file=sys.stderr)
        except (OSError, RuntimeError) as error:
            print(f"cargo_guard: cache report unavailable: {root / 'target'}: {error}", file=sys.stderr)


def paths_overlap(left: Path | str, right: Path | str) -> bool:
    """Component comparison as text avoids allocating every ancestor for each evidence row."""
    shorter, longer = sorted((str(left), str(right)), key=len)
    return longer == shorter or longer.startswith(shorter if shorter.endswith("/") else shorter + "/")


class Resolver:
    """``os.path.realpath`` for many absolute paths without ``.``/``..`` parts sharing directories.

    A path resolves as its resolved parent joined with its name, each parent once; a
    component that is a symlink resolves in full, so the result equals ``os.path.realpath``.
    """

    def __init__(self) -> None:
        self.known: dict[str, str] = {}

    def resolve(self, path: str) -> str:
        known = self.known.get(path)
        if known is None:
            directory, name = os.path.split(path)
            if not name or directory == path:
                known = os.path.realpath(path)
            elif os.path.islink(path):
                known = os.path.realpath(path)
            else:
                known = os.path.join(self.resolve(directory), name)
            self.known[path] = known
        return known


def evidence_veto(info: Identity) -> None:
    deadline = time.monotonic() + 30
    remaining_bytes = FIXITY_METADATA_BYTES
    remaining_entries = FIXITY_METADATA_ENTRIES
    parsed_manifests: dict[Path, tuple[tuple[Path, Path], list[sha256_manifest.Entry]]] = {}
    checked_contexts: set[tuple[Path, Path]] = set()
    checked_paths: set[Path] = set()
    resolver = Resolver()
    target = str(info.target)
    for root in checkout_roots(info):
        for shard in evidence_shards(root / "evidence"):
            with shard.open(encoding="utf-8") as handle:
                for number, line in enumerate(handle, start=1):
                    if time.monotonic() > deadline:
                        raise GuardError("evidence metadata inventory exceeded 30 seconds")
                    row = docs_lint.ManifestRow.from_record(str(shard), number, docs_lint.parse_json_object(line))
                    if row.status not in docs_lint.RECORD_STATUSES or row.klass not in docs_lint.RECORD_CLASSES:
                        raise GuardError(f"unknown evidence lifecycle/class in {shard}:{number}")
                    if row.status != "active" or row.klass != "local-private":
                        continue
                    path = root / row.path
                    if path in checked_paths:
                        continue
                    checked_paths.add(path)
                    protected = [path]
                    if path.name == work_gc.FIXITY_NAME:
                        protected.append(path.parent)
                    resolved = [os.path.realpath(item) if ".." in item.parts else resolver.resolve(str(item)) for item in protected]
                    if any(paths_overlap(target, entry) for entry in [*protected, *resolved]):
                        raise GuardError(f"active evidence overlaps target: {row.identifier}")
                    resolved_manifest = Path(resolved[0])
                    context = (resolved_manifest, path.parent)
                    if context in checked_contexts:
                        continue
                    checked_contexts.add(context)
                    parsed = parsed_manifests.get(resolved_manifest)
                    if parsed is None:
                        size, entries = read_fixity_metadata(root, path, remaining_bytes, remaining_entries)
                        parsed = parsed_manifests[resolved_manifest] = ((root, path), entries)
                        remaining_bytes -= size
                        remaining_entries -= len(entries)
                    # Candidates depend on the root and lexical path the manifest is reached through.
                    parsed_in, entries = parsed
                    top, directory = os.path.normpath(root), os.path.dirname(os.path.normpath(path))
                    # Parsing is shared, but each lexical parent is a distinct dependency context.
                    for entry in entries:
                        if time.monotonic() > deadline:
                            raise GuardError("evidence metadata inventory exceeded 30 seconds")
                        candidates = (tuple(map(str, entry.candidates)) if parsed_in == (root, path)
                                      else sha256_manifest.candidate_texts(top, directory, entry.path))
                        if not candidates:
                            raise GuardError(f"unreadable fixity metadata: {path}: {entry.path} leaves the repository")
                        if any(paths_overlap(target, item) or paths_overlap(target, resolver.resolve(item)) for item in candidates):
                            raise GuardError(f"active fixity entry overlaps target: {path}: {entry.path}")


def read_fixity_metadata(root: Path, path: Path, byte_budget: int, entry_budget: int) -> tuple[int, list[sha256_manifest.Entry]]:
    """Protect listed paths and aliases; unrelated case symlinks grant no veto."""
    try:
        try:
            _ = path.lstat()
        except FileNotFoundError:
            return 0, []  # Private manifests may be absent in secondary worktrees/CI.
        # Parse only hash/path metadata with the shared reader, never hash artifacts.
        entries = sha256_manifest.read_manifest(root, path, max_bytes=byte_budget)
        if entries is None:
            return 0, []
        if len(entries) > entry_budget:
            raise GuardError("fixity metadata entry budget exceeded")
        return path.stat().st_size, entries
    except sha256_manifest.ManifestTooLarge as error:
        raise GuardError(f"invalid or oversized fixity metadata: {path}") from error
    except (OSError, ValueError, RuntimeError) as error:
        if isinstance(error, GuardError):
            raise
        raise GuardError(f"unreadable fixity metadata: {path}: {error}") from error


def evidence_shards(directory: Path) -> Iterator[Path]:
    if not directory.exists():
        return
    if not directory.is_dir() or directory.is_symlink():
        raise GuardError(f"invalid evidence metadata directory: {directory}")
    for parent, dirs, files in os.walk(directory, followlinks=False, onerror=raise_walk_error):
        for name in (*dirs, *files):
            entry = Path(parent) / name
            if entry.is_symlink():
                raise GuardError(f"evidence index symlink requires review: {entry}")
        for name in files:
            if name.endswith(".jsonl"):
                shard = Path(parent) / name
                if not shard.is_file() or shard.stat().st_size > docs_lint.SHARD_CAP:
                    raise GuardError(f"invalid evidence shard metadata: {shard}")
                yield shard


def process_veto(info: Identity, proc: Path = PROC) -> int:
    """Readable matches veto; unavailable metadata is counted, never a proof of absence."""
    limited = 0
    deadline = time.monotonic() + 15
    try:
        entries = list(proc.iterdir())
    except OSError:
        print("cargo_guard: /proc unavailable; process-use veto is limited", file=sys.stderr)
        return 1
    for entry in entries:
        if not entry.name.isdigit() or int(entry.name) == os.getpid():
            continue
        if time.monotonic() > deadline:
            limited += 1
            break
        links = [entry / "cwd", entry / "exe"]
        try:
            links.extend((entry / "fd").iterdir())
        except OSError:
            limited += 1
        for link in links:
            try:
                path = Path(os.readlink(link).removesuffix(" (deleted)"))
                if work_gc.is_within(path.resolve(), info.target):
                    raise GuardError(f"process {entry.name} uses target ({link.name})")
            except OSError:
                limited += 1
        try:
            for line in (entry / "maps").read_text(errors="surrogateescape").splitlines():
                fields = line.split(maxsplit=5)
                if len(fields) == 6 and fields[5].startswith("/") and work_gc.is_within(Path(fields[5].removesuffix(" (deleted)")).resolve(), info.target):
                    raise GuardError(f"process {entry.name} maps target")
        except OSError:
            limited += 1
        try:
            words = (entry / "cmdline").read_bytes().split(b"\0")
            if words and Path(os.fsdecode(words[0])).name in {"cargo", "rustc", "rustdoc", "clippy-driver"}:
                if Path(os.readlink(entry / "cwd")).resolve() == info.root:
                    raise GuardError(f"unguarded compiler process {entry.name} in checkout")
        except OSError:
            limited += 1
    print(f"cargo_guard: best-effort /proc scan: {limited} unavailable metadata observations; hidden processes may be unobservable", file=sys.stderr)
    return limited


def cargo_lock_probe(info: Identity) -> None:
    """Probe existing Cargo layout locks, then release BEFORE spawning Cargo.

    Cargo src/compiler/layout.rs: profile/.cargo-lock, .cargo-build-lock,
    .cargo-artifact-lock. Legacy layouts use .cargo-lock alone. No creation.
    """
    for parent, _dirs, files in os.walk(info.target, followlinks=False, onerror=raise_walk_error):
        for name in files:
            if name not in {".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock"}:
                continue
            fd = os.open(Path(parent) / name, os.O_RDONLY | os.O_NOFOLLOW)
            try:
                try:
                    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    raise GuardError(f"Cargo lock is busy: {Path(parent) / name}") from None
            finally:
                os.close(fd)


def cleanup_veto(info: Identity, env: Mapping[str, str]) -> None:
    _ = check_configuration(info, env)
    _ = target_inventory(info.target, strict=True)
    evidence_veto(info)
    _ = process_veto(info)
    cargo_lock_probe(info)


def available_ram() -> int:
    for line in Path("/proc/meminfo").read_text().splitlines():
        if line.startswith("MemAvailable:"):
            return int(line.split()[1]) * 1024
    raise GuardError("MemAvailable is unavailable")


def admission_reserve(allocated: int) -> int:
    remaining = TARGET_BUDGET - allocated
    if remaining < MIN_HEADROOM:
        raise GuardError("own target needs at least 2 GiB transient headroom below provisional 10 GiB cap; review retirement")
    return min(RESERVE, remaining)


def check_space(root: Path, reserve: int = 0, *, policy: Policy = LOCAL) -> None:
    if available_ram() < RAM_FLOOR:
        raise GuardError("below 3 GiB available RAM")
    if shutil.disk_usage(root).free < policy.disk_floor + reserve:
        raise GuardError(f"need {policy.disk_floor / GIB:g} GiB disk floor plus {reserve / GIB:g} GiB reserved growth")


@contextmanager
def defer_termination() -> Generator[None]:
    pending: list[int] = []
    handlers = {sig: signal.getsignal(sig) for sig in TERMINATION}

    def remember(signum: int, _frame: object) -> None:
        pending.append(signum)

    try:
        for sig in TERMINATION:
            _ = signal.signal(sig, remember)
        yield
    finally:
        for sig, handler in handlers.items():
            _ = signal.signal(sig, handler)
        if pending:
            interrupted(pending[0], None)


def stop_group(process: subprocess.Popen[str]) -> None:
    previous = signal.pthread_sigmask(signal.SIG_BLOCK, TERMINATION)
    try:
        for sig in (signal.SIGTERM, signal.SIGKILL):
            try:
                os.killpg(process.pid, sig)
            except ProcessLookupError:
                break
            if sig == signal.SIGTERM:
                time.sleep(POLL)
        _ = process.wait(timeout=5)
    finally:
        _ = signal.pthread_sigmask(signal.SIG_SETMASK, previous)


def child_status(pid: int) -> int | None:
    # Reserve the leader PID until stop_group, avoiding process-group PID reuse.
    result = os.waitid(os.P_PID, pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
    return None if result is None else result.si_status if result.si_code == os.CLD_EXITED else -result.si_status


def group_memory(group: int) -> int:
    pages = 0
    deadline = time.monotonic() + 2
    for entry in PROC.iterdir():
        if time.monotonic() > deadline:
            raise GuardError("owned process memory inventory exceeded two seconds")
        if not entry.name.isdigit():
            continue
        try:
            fields = (entry / "stat").read_text().rpartition(") ")[2].split()
            if len(fields) >= 3 and int(fields[2]) == group:
                pages += int((entry / "statm").read_text().split()[1])
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            # procfs may return ESRCH after opening a process file when its
            # task exits before the read; it is the same race as ENOENT.
            continue
    return pages * int(os.sysconf("SC_PAGE_SIZE"))


def run_owned(command: Sequence[str], info: Identity, env: Mapping[str, str], *,
              pass_fds: tuple[int, ...], timeout: float = TIMEOUT, monitor_space: bool = True,
              capture: bool = False, policy: Policy = LOCAL) -> str:
    with tempfile.TemporaryFile(mode="w+t", dir=info.state_dir) as output:
        process: subprocess.Popen[str] | None = None
        try:
            with defer_termination():
                process = subprocess.Popen(list(command), cwd=info.root, env=dict(env), start_new_session=True,
                                           pass_fds=pass_fds, text=True, stdout=output if capture else None)
            deadline = time.monotonic() + timeout
            status = child_status(process.pid)
            while status is None:
                if time.monotonic() >= deadline:
                    raise GuardError("command timed out")
                if capture and os.fstat(output.fileno()).st_size > 65536:
                    raise GuardError("toolchain version output exceeds 64 KiB")
                if group_memory(process.pid) > OWNED_RAM_LIMIT:
                    raise GuardError("owned command group exceeds 10 GiB resident RAM")
                if monitor_space:
                    check_space(info.root, policy=policy)
                time.sleep(POLL)
                status = child_status(process.pid)
            if status != 0:
                raise GuardError(f"command failed ({status}): {command[0]}")
        finally:
            with defer_termination():
                if process is not None:
                    stop_group(process)
        if not capture:
            return ""
        _ = output.seek(0)
        result = output.read(65537)
        if len(result) > 65536:
            raise GuardError("toolchain version output exceeds 64 KiB")
        return result


def read_state(info: Identity) -> dict[str, object] | None:
    if info.state.is_symlink():
        raise GuardError("guard state must not be a symlink")
    if not info.state.exists():
        return None
    state = docs_lint.parse_json_object(info.state.read_text())
    if state.get("schema") != SCHEMA or state.get("root") != str(info.root) or state.get("target") != str(info.target):
        raise GuardError("guard ownership identity differs; explicit review/adoption required")
    if info.target.exists():
        entry = info.target.stat()
        if (state.get("device"), state.get("inode")) != (entry.st_dev, entry.st_ino):
            raise GuardError("target was replaced outside the guard; explicit review/adoption required")
        if stat.S_IMODE(entry.st_mode) != 0o700:
            raise GuardError("owned target must be private (0700)")
    return state


def write_state(info: Identity, current: dict[str, str] | None, *, needs_clean: bool = False, validated: bool = False) -> None:
    entry = info.target.stat()
    state = {"schema": SCHEMA, "root": str(info.root), "target": str(info.target),
             "device": entry.st_dev, "inode": entry.st_ino, "generation": current,
             "needs_clean": needs_clean, "validated": validated}
    temporary = info.state.with_suffix(".pending")
    # The repository lock serializes state writers; target leases exclude cleanup.
    # The committed state remains authoritative; interrupted CLEAN was stamped first.
    if temporary.exists() or temporary.is_symlink():
        prior = temporary.lstat()
        if not stat.S_ISREG(prior.st_mode) or prior.st_uid != os.getuid() or prior.st_nlink != 1:
            raise GuardError("unsafe pending state requires review")
        temporary.unlink()
    fd = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as handle:
        _ = handle.write(json.dumps(state, sort_keys=True) + "\n")
        handle.flush()
        os.fsync(handle.fileno())
    os.replace(temporary, info.state)
    directory = os.open(info.state_dir, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def clean(info: Identity, env: Mapping[str, str], *, pass_fds: tuple[int, ...]) -> None:
    """Caller holds build lock + exclusive target lease; Cargo owns deletion."""
    if read_state(info) is None:
        raise GuardError("cleanup requires guard ownership")
    cleanup_veto(info, env)
    write_state(info, None, needs_clean=True)
    _ = run_owned(["cargo", "clean", "--locked"], info, env, pass_fds=pass_fds,
                  timeout=300, monitor_space=False)
    with defer_termination():
        info.target.mkdir(mode=0o700, exist_ok=True)
        write_state(info, None)
    if target_inventory(info.target)[0] > TARGET_BUDGET:
        raise GuardError("residual target exceeds provisional 10 GiB after full Cargo cleanup")


def execute(info: Identity, args: Sequence[str], env: Mapping[str, str], *, build_fd: int,
            adopt: str | None = None, retire: bool = False, inventory: bool = False,
            policy: Policy = LOCAL) -> None:
    config = check_configuration(info, env)
    if inventory:
        report_caches(info)
        return
    if adopt is None and not retire:
        check_arguments(args, package_name(info.root))
    owned_env = {**env, "CARGO_TARGET_DIR": str(info.target), "CARGO_BUILD_JOBS": "2"}
    with target_lease(info, exclusive=True) as lease:
        descriptors = (build_fd, lease)
        if adopt is not None:
            if adopt != str(info.target) or not info.target.is_dir():
                raise GuardError("adoption must name the existing canonical absolute target path")
            cleanup_veto(info, env)
            info.target.chmod(0o700)
            write_state(info, None, needs_clean=True)
            print(f"cargo_guard: adopted {info.target}; next build fully retires it", file=sys.stderr)
            return
        state = read_state(info)
        if state is None and info.target.exists():
            raise GuardError(f"unowned target: {info.target}; inventory and explicit --adopt-target required; nothing cleaned")
        if not info.target.exists():
            with defer_termination():
                info.target.mkdir(mode=0o700)
                write_state(info, None)
            state = None
        if retire:
            clean(info, owned_env, pass_fds=descriptors)
            return
        toolchain = run_owned(["cargo", "--version"], info, owned_env, pass_fds=descriptors,
                              timeout=30, monitor_space=False, capture=True)
        rustc = run_owned(["rustc", "-vV"], info, owned_env, pass_fds=descriptors,
                          timeout=30, monitor_space=False, capture=True)
        pinned = table(toml_object(info.root / "rust-toolchain.toml").get("toolchain")).get("channel")
        requested = env.get("RUSTUP_TOOLCHAIN")
        host = next((line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: ")), "")
        if requested is not None and requested not in {pinned, f"{pinned}-{host}"}:
            raise GuardError("RUSTUP_TOOLCHAIN must match the active pinned toolchain/host")
        current = generation(info, config, toolchain + rustc)
        if state is not None and (state.get("needs_clean") or state.get("generation") != current):
            clean(info, owned_env, pass_fds=descriptors)
        # PREPARED identifies partial artifacts too. Validation is a separate field.
        write_state(info, current)
        before, _ = target_inventory(info.target)
        check_space(info.root, admission_reserve(before), policy=policy)
        report_caches(info)
        fcntl.flock(lease, fcntl.LOCK_SH)
        try:
            _ = run_owned(["cargo", *args], info, owned_env, pass_fds=descriptors, policy=policy)
        finally:
            after, _ = target_inventory(info.target)
            print(f"cargo_guard: own allocated bytes before={before} after={after}; cap={TARGET_BUDGET}", file=sys.stderr)
        _ = read_state(info)
        check_space(info.root, policy=policy)
        if after > TARGET_BUDGET:
            raise GuardError("own target exceeds provisional 10 GiB cap; review retirement")
        if generation(info, check_configuration(info, env), toolchain + rustc) != current:
            write_state(info, current, needs_clean=True)
            raise GuardError("build inputs changed during command; next build needs cleanup")
        write_state(info, current, validated=True)


def interrupted(signum: int, _frame: object) -> None:
    raise GuardError(f"interrupted by signal {signum}")


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    _ = modes.add_argument("--inventory", action="store_true")
    _ = modes.add_argument("--adopt-target")
    _ = modes.add_argument("--retire-target", action="store_true")
    _ = modes.add_argument("--hold-target", action="store_true")
    _ = parser.add_argument("--policy", choices=("local", "ci"))
    _ = parser.add_argument("cargo", nargs=argparse.REMAINDER)
    options = parser.parse_args(argv)
    args = cast(list[str], options.cargo)
    if args[:1] == ["--"]:
        args = args[1:]
    for sig in TERMINATION:
        _ = signal.signal(sig, interrupted)
    try:
        info = identity(Path.cwd())
        policy = selected_policy(os.environ, cast(str | None, options.policy))
        if cast(bool, options.hold_target):
            if not args:
                raise GuardError("--hold-target requires -- command [args...]")
            with target_lease(info, exclusive=False) as lease:
                _ = check_configuration(info, os.environ)
                if read_state(info) is None or not info.target.is_dir():
                    raise GuardError("runtime lease requires an owned target")
                _ = run_owned(args, info, os.environ, pass_fds=(lease,), policy=policy)
        else:
            with build_lock(info) as fd:
                execute(info, args, os.environ, build_fd=fd,
                        adopt=cast(str | None, options.adopt_target),
                        retire=cast(bool, options.retire_target),
                        inventory=cast(bool, options.inventory), policy=policy)
    except (GuardError, OSError, ValueError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        print(f"cargo_guard: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
