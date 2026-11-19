"""Fixity and Linux bind-mount verification for the offline M-1404 intake."""

from collections.abc import Sequence
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
import shlex
import socket
import stat
import sys
import time

from tools.sha256_manifest import DIGEST_LINE

EVIDENCE = "M-1404"
INTAKE_CASE = Path("work/issue284-release-20261113")
MANIFEST = INTAKE_CASE / "intake/official-fixity.sha256"
INSTALL = INTAKE_CASE / "official"
MANIFEST_SHA256 = "6c2c8ab0246918bd2eb47195cdc74e06d1c8e124d27227f9313c20b65a9bc35e"
BUILD_ID = 26613688
RELEASE = "3.0.36.159996-rel-DangerousHorizons"
DOCKER_SOCKET = Path("/run/docker.sock")
MOUNT_RECEIPT = ".reference-mount-verified"
FIND_FORMAT = r"%y %D %i %s %n %T@ %C@ %P\0"
MAX_INSTALL_BYTES = 16 * 1024**3
MAX_VERIFY_SECONDS = 300
EMPTY_STEAM_DIRS = frozenset({Path("steamapps/downloading"), Path("steamapps/temp")})


@dataclass(frozen=True)
class Intake:
    root: Path
    files: dict[Path, str]


def read_intake(repo: Path, executable: Path) -> Intake:
    """Read the pinned manifest's repository-relative install file digests."""
    manifest = repo / MANIFEST
    if manifest.resolve(strict=True) != manifest:
        raise ValueError("redirected intake manifest")
    fd = os.open(manifest, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(fd, "rb") as stream:
        if not stat.S_ISREG(os.fstat(stream.fileno()).st_mode):
            raise ValueError("intake manifest must be a regular file")
        raw = stream.read(1024**2 + 1)
    if len(raw) > 1024**2 or hashlib.sha256(raw).hexdigest() != MANIFEST_SHA256:
        raise ValueError("intake manifest digest mismatch")

    files: dict[Path, str] = {}
    for line in raw.decode("utf-8").splitlines():
        entry = DIGEST_LINE.fullmatch(line)
        if entry is None:
            raise ValueError("invalid intake digest line")
        parts = entry[2].split("/")
        if any(part in {"", ".", ".."} or "\0" in part for part in parts):
            raise ValueError("noncanonical intake manifest entry")
        path = Path(*parts)
        if not path.is_relative_to(INSTALL) or path == INSTALL:
            raise ValueError("manifest entry is outside the install")
        relative = path.relative_to(INSTALL)
        if relative in files:
            raise ValueError("duplicate intake digest entry")
        files[relative] = entry[1].lower()
    if executable not in files:
        raise ValueError("intake manifest does not name the reference executable")
    return Intake(repo / INSTALL, files)


@dataclass(frozen=True)
class TreeIdentity:
    sha256: str
    length: int


def find_time(nanoseconds: int) -> str:
    seconds, fraction = divmod(nanoseconds, 10**9)
    if seconds < 0:
        raise ValueError("intake timestamp predates the Unix epoch")
    return f"{seconds}.{fraction:09d}0"


def identity_record(info: os.stat_result, relative: Path, kind: str) -> bytes:
    # Match GNU find's NUL-terminated output under LC_ALL=C, including the root.
    prefix = (f"{kind} {info.st_dev} {info.st_ino} {info.st_size} {info.st_nlink} "
              f"{find_time(info.st_mtime_ns)} {find_time(info.st_ctime_ns)} ")
    return prefix.encode() + (os.fsencode(relative) if relative.parts else b"") + b"\0"


def check_install_mounts(root: Path) -> None:
    for line in Path("/proc/self/mountinfo").read_bytes().splitlines():
        # Linux escapes space, tab, newline and backslash in mountinfo paths.
        unescaped = re.sub(rb"\\([0-7]{3})", lambda match: bytes([int(match[1], 8)]), line.split()[4])
        path = Path(os.fsdecode(unescaped))
        if path.is_relative_to(root):
            raise ValueError("unexpected mount within the reference install")


def walk_error(error: OSError) -> None:
    raise error


def verify_install(intake: Intake, deadline: float) -> TreeIdentity:
    """Hash once on the host and capture the identities of those same files."""
    if intake.root.resolve(strict=True) != intake.root:
        raise ValueError("redirected reference install")
    check_install_mounts(intake.root)
    pending_files = set(intake.files)
    pending_dirs = {parent for path in pending_files for parent in path.parents}
    allowed_dirs = pending_dirs | EMPTY_STEAM_DIRS
    records: list[bytes] = []
    remaining_bytes = MAX_INSTALL_BYTES

    def check_time() -> None:
        if time.monotonic() >= deadline:
            raise TimeoutError("reference fixity verification exceeded its preparation window")

    for directory, dirs, names, parent_fd in os.fwalk(intake.root, onerror=walk_error):
        check_time()
        relative = Path(directory).relative_to(intake.root)
        if relative not in allowed_dirs:
            raise ValueError("unlisted directory in reference install")
        if relative in EMPTY_STEAM_DIRS and (dirs or names):
            raise ValueError("expected an empty Steam directory in reference install")
        pending_dirs.discard(relative)
        records.append(identity_record(os.fstat(parent_fd), relative, "d"))
        for name in dirs:
            if relative / name not in allowed_dirs or not stat.S_ISDIR(
                os.stat(name, dir_fd=parent_fd, follow_symlinks=False).st_mode
            ):
                raise ValueError("unlisted or redirected directory in reference install")
        for name in names:
            path = relative / name
            if path not in pending_files:
                raise ValueError("unlisted file in reference install")
            pending_files.remove(path)
            if not stat.S_ISREG(os.stat(name, dir_fd=parent_fd, follow_symlinks=False).st_mode):
                raise ValueError("reference entry must be a regular file, without symbolic links")
            fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent_fd)
            with os.fdopen(fd, "rb") as stream:
                info = os.fstat(stream.fileno())
                if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
                    raise ValueError("reference entry must be a regular file with one link")
                if info.st_size > remaining_bytes:
                    raise ValueError("reference install byte budget exceeded")
                record = identity_record(info, path, "f")
                checksum = hashlib.sha256()
                unread = info.st_size
                while unread:
                    check_time()
                    chunk = stream.read(min(unread, 1024**2))
                    if not chunk:
                        raise ValueError("reference entry shortened during hashing")
                    checksum.update(chunk)
                    unread -= len(chunk)
                if stream.read(1) or identity_record(os.fstat(stream.fileno()), path, "f") != record:
                    raise ValueError("reference entry changed during hashing")
                if checksum.hexdigest() != intake.files[path]:
                    raise ValueError(f"reference file digest mismatch: {path}")
                remaining_bytes -= info.st_size
                records.append(record)
    if pending_files or pending_dirs:
        raise ValueError("manifest entry is missing from reference install")
    data = b"".join(sorted(records))
    return TreeIdentity(hashlib.sha256(data).hexdigest(), len(data))


def local_docker_argv(argv: Sequence[str]) -> list[str]:
    return [argv[0], "--host", f"unix://{DOCKER_SOCKET}", *argv[1:]]


def verify_local_dockerd() -> None:
    """Reject environment-selected endpoints, socket forwarding and VM daemons."""
    if any(os.environ.get(key) for key in ("DOCKER_HOST", "DOCKER_CONTEXT")):
        raise ValueError("intake requires the pinned local Docker socket without host/context overrides")
    if DOCKER_SOCKET.resolve(strict=True) != DOCKER_SOCKET:
        raise ValueError("redirected Docker socket")
    info = DOCKER_SOCKET.stat()
    if not stat.S_ISSOCK(info.st_mode) or info.st_uid != 0:
        raise ValueError("intake requires a root-owned local Docker socket")
    with socket.socket(socket.AF_UNIX) as peer:
        peer.settimeout(2)
        peer.setsockopt(socket.SOL_SOCKET, socket.SO_PASSCRED, 1)
        peer.connect(str(DOCKER_SOCKET))
        peer.sendall(b"GET /_ping HTTP/1.1\r\nHost: docker\r\nConnection: close\r\n\r\n")
        # Socket activation can make the listen-time peer systemd. Verify the
        # kernel credentials of the process that actually sends the response.
        # Linux struct ucred holds three native-endian 32-bit integers.
        credentials_size = 12
        response = peer.recvmsg(4096, socket.CMSG_SPACE(credentials_size))
        reply, ancillary, flags = response[:3]
        if flags & socket.MSG_CTRUNC or not reply.startswith(b"HTTP/1.1 200 OK\r\n") or len(ancillary) != 1:
            raise ValueError("could not authenticate the local Docker response")
        level, kind, data = ancillary[0]
        if (level, kind, len(data)) != (socket.SOL_SOCKET, socket.SCM_CREDENTIALS, credentials_size):
            raise ValueError("missing Docker sender credentials")
        pid = int.from_bytes(data[:4], sys.byteorder, signed=True)
        uid = int.from_bytes(data[4:8], sys.byteorder)
        if pid <= 0 or uid != 0 or Path(f"/proc/{pid}/comm").read_text().strip() != "dockerd":
            raise ValueError("Docker response sender is not a local root dockerd")
    if not os.path.samestat(info, DOCKER_SOCKET.stat()):
        raise ValueError("Docker socket changed during verification")


def mount_identity(path: Path) -> str:
    if path.resolve(strict=True) != path:
        raise ValueError("redirected mount source")
    info = path.stat()
    return f"{info.st_dev}:{info.st_ino}"


def guard_launch(tree: TreeIdentity, home: Path, probe: Path | None, token: str) -> str:
    """Run inside the actual bind-mount view before executing the original."""
    checks = ["/reference:ro", "/home/container:rw"]
    identity_checks = [f'[[ $(stat -c "%d:%i" /home/container) = {mount_identity(home)} ]]']
    if probe is not None:
        checks.append("/probes/handshake_probe:ro")
        identity_checks.append(f'[[ $(stat -c "%d:%i" /probes/handshake_probe) = {mount_identity(probe)} ]]')
    mount_checks = [
        f"awk '$5 == \"{target}\" {{ count++; good = ($6 ~ /(^|,){mode}(,|$)/) }} "
        + "END { exit count != 1 || !good }' /proc/self/mountinfo"
        for target, mode in (check.split(":") for check in checks)
    ]
    return "\n".join([
        "set -euo pipefail", """trap 'echo "reference guard failed at line $LINENO" >&2' ERR""",
        *mount_checks, *identity_checks,
        "awk 'index($5, \"/reference/\") == 1 || index($5, \"/home/container/\") == 1 { exit 1 }' /proc/self/mountinfo",
        f"actual=$(export LC_ALL=C; timeout 10s find -P /reference -xdev -printf {shlex.quote(FIND_FORMAT)} "
        + f"| head -c {tree.length + 1} | sort -z | sha256sum)",
        f'[[ $actual = "{tree.sha256}  -" ]]',
        f"(umask 077; set -C; printf '%s\\n' {shlex.quote(token)} > /home/container/{MOUNT_RECEIPT})",
        "",
    ])


def mount_verified(receipt: Path, token: str) -> bool:
    try:
        fd = os.open(receipt, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return False
    expected = (token + "\n").encode()
    with os.fdopen(fd, "rb") as stream:
        return (stat.S_ISREG(os.fstat(stream.fileno()).st_mode)
                and stream.read(len(expected) + 1) == expected)
