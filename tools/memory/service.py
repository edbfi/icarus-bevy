# SPDX-License-Identifier: AGPL-3.0-or-later
"""The always-on memory service: one loopback Hindsight 1.2.3 instance with bank ``icarus``, run by a
systemd user unit (``.agents/rules/memory.md``, "Instance" and "Agents and service").

Every other module finds the service with ``connection()`` and its directories with ``home()``; nothing
else reads the key or knows where state lives.

State lives outside every checkout. ``home()`` gives three directories, each owner-only (0700):

- ``config``: ``$XDG_CONFIG_HOME/icarus-memory`` with ``connection-v2.env``, the ONE file holding the
  service's API key, its loopback port and the pg0 database URL (with its password). It is 0600, in
  systemd ``EnvironmentFile`` syntax, read by the unit and by ``connection()``; any group or world
  permission makes it unreadable to the tool.

  The name carries the bank rendering guard (``bank.OlderTool``): a tool released before the guard reads
  ``connection.env`` and would re-render every document in its older rendering. ``install`` therefore
  moves an old ``connection.env`` to the new name with a NEW API key (``rotate_connection``; ports,
  database password and reranker setting kept) and restarts the unit, and leaves a tombstone named
  ``connection.env`` (on a fresh host too) that such a tool cannot parse: its every command, ``service
  install`` included, stops at "connection.env:1: malformed or repeated line" before writing anything, and
  a process still holding the old key is refused by the service. A second ``install`` rotates nothing.
- ``data``: ``$XDG_DATA_HOME/icarus-memory`` with ``pg0/`` (``PG0_HOME``) and the service's private
  ``home/`` and ``tmp/``.
- ``state``: ``$XDG_STATE_HOME/icarus-memory`` with ``backups/`` and ``memory.lock``, the host lock every
  bank write, ID allocation and sync holds (``host_lock``).

``ICARUS_MEMORY_HOME`` replaces all three with ``<root>/config``, ``<root>/data`` and
``<root>/state``, for tests and scratch instances; such a home is never managed by systemd.

The Python environment, the pg0 PostgreSQL 18.1.0 build and the pinned ONNX models are the ones
``tools/hindsight_runtime install`` provisions in the primary checkout's ``work/hindsight-runtime/``
(found from the Git common directory). They are only read and executed here. The unit holds a shared
``flock`` on that directory's ``assets.lock`` for its whole life, so ``install`` (exclusive) refuses
while the service runs.

The instance is configured only through environment variables (``SETTINGS``); the bank is created
empty by ``PUT /v1/default/banks/icarus`` at first start, so its effective configuration comes from
the global level with no bank overrides. ``status`` asserts both (``bank_config_problems``,
``feature_problems``), since bank ``PUT``/``PATCH`` and template import can still add overrides.

Commands: ``memory service install|start|stop|status|uninstall`` (``run_service``); they are registered
with ``memory backup`` and ``memory restore`` by ``tools.memory.durability.register``.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import http.client
import json
import os
import re
import secrets
import shutil
import stat
import subprocess
import sys
import time
from collections.abc import Callable, Generator, Mapping
from contextlib import contextmanager
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Protocol, cast

from tools.memory.gitio import GitError, git
from tools.memory.paths import ProtectedPathsError, primary_checkout

BANK = "icarus"
API_PORT, PG_PORT = 18681, 18682
FLOOR = 30 * 1024**3
UNIT = "icarus-memory.service"
BACKUP_UNIT = "icarus-memory-backup.service"
BACKUP_TIMER = "icarus-memory-backup.timer"
CONNECTION_FILE = "connection-v2.env"
# The connection file of tools without the bank rendering guard (released before it); never read, only retired to
# ``TOMBSTONE``: a first line without ``=`` such a tool refuses to parse, saying what to do.
LEGACY_CONNECTION_FILE = "connection.env"
TOMBSTONE = (b"retired: this checkout's memory tool is older than the bank. Merge main, then run"
             + b" python3 -m tools.memory service install from it.\n")
HOME_VARIABLE = "ICARUS_MEMORY_HOME"
READY_SECONDS = 300.0
LOCK_FILE = "memory.lock"
LOCK_SECONDS = 300.0
MAX_RESPONSE = 256 * 1024 * 1024

EMBEDDING = "models--intfloat--multilingual-e5-small/snapshots/614241f622f53c4eeff9890bdc4f31cfecc418b3"
RERANKER = "models--hindsight-ai--hs-rerank-1/snapshots/927f5be31280298a51cf29af8a29cb111ed812c5"
PG_VERSION = "18.1.0"

# The ``ICARUS_MEMORY_RERANKER`` setting: ``off`` (the default: fused ranking only) or a cap on the candidates the
# cross-encoder scores per recall. Hindsight 1.2.3's recall request has no per-request cap (its ``rerank`` options
# are prune and dedup only), so it is instance-wide: the connection file carries the setting and the Hindsight
# variables it implies. On the real corpus (3,093 records, 4 cores) reranking cost recall p50 12 s at cap 50 and
# 1.7 s at cap 10 with 4 threads against 0.37 s off, and answered the held-out questions no better.
RERANKER_DEFAULT = "off"
RERANKER_THREADS = "4"
RERANKER_VARIABLE = "ICARUS_MEMORY_RERANKER"
# Hindsight's own reranking switch. Its 1.2.3 default is on (``DEFAULT_ENABLE_RERANKING = True``), so "off" is
# always written, never assumed.
RERANKING_VARIABLE = "HINDSIGHT_API_ENABLE_RERANKING"
# Every Hindsight setting the instance depends on; values are checked against config.py of 1.2.3.
SETTINGS: dict[str, str] = {
    "HINDSIGHT_API_LLM_PROVIDER": "none",
    "HINDSIGHT_API_SKIP_LLM_VERIFICATION": "true",
    "HINDSIGHT_API_RETAIN_EXTRACTION_MODE": "chunks",
    "HINDSIGHT_API_RETAIN_CHUNK_SIZE": "1800",
    "HINDSIGHT_API_ENABLE_OBSERVATIONS": "false",
    "HINDSIGHT_API_ENABLE_AUTO_CONSOLIDATION": "false",
    "HINDSIGHT_API_ENABLE_FILE_UPLOAD_API": "false",
    "HINDSIGHT_API_ENABLE_DOCUMENT_IMPORT_API": "false",
    "HINDSIGHT_API_ENABLE_BANK_CONFIG_API": "false",
    "HINDSIGHT_API_MCP_ENABLED": "false",
    "HINDSIGHT_API_LLM_TRACE_ENABLED": "false",
    "HINDSIGHT_API_WORKER_ID": "icarus-memory",
    "HINDSIGHT_API_TENANT_EXTENSION": "hindsight_api.extensions.builtin.tenant:ApiKeyTenantExtension",
    "HINDSIGHT_API_EMBEDDINGS_PROVIDER": "onnx",
    "HINDSIGHT_API_EMBEDDINGS_ONNX_DEVICE": "cpu",
    "HINDSIGHT_API_RERANKER_PROVIDER": "onnx",
    # The floor: the connection file overrides it (``EnvironmentFile=`` wins over ``Environment=``, systemd.exec(5)),
    # so only a file that turns reranking on turns it on.
    RERANKING_VARIABLE: "false",
    "HINDSIGHT_API_LOG_LEVEL": "WARNING",
    "HF_HUB_OFFLINE": "1",
    "HF_HUB_DISABLE_TELEMETRY": "1",
    "HF_HUB_DISABLE_IMPLICIT_TOKEN": "1",
    "LANG": "C.UTF-8",
    "PATH": "/usr/bin:/bin",
    "PYTHONDONTWRITEBYTECODE": "1",
    "PYTHONNOUSERSITE": "1",
    "PYTHON_DOTENV_DISABLED": "1",
    "OMP_NUM_THREADS": "2",
    "OPENBLAS_NUM_THREADS": "1",
    "TOKENIZERS_PARALLELISM": "false",
}
# GET /version feature flags and GET .../banks/icarus/config values the instance must report.
EXPECTED_FEATURES: dict[str, bool] = {
    "mcp": False, "bank_config_api": False, "file_upload_api": False, "document_import_api": False,
    "observations": False, "llm_trace": False,
}
EXPECTED_BANK_CONFIG: dict[str, str | int | bool] = {
    "retain_extraction_mode": "chunks", "retain_chunk_size": 1800,
    "enable_observations": False, "enable_auto_consolidation": False,
}
API_VERSION = "1.2.3"
DATABASE_QUERY = "listen_addresses=127.0.0.1&fsync=on&synchronous_commit=on"
_KEY = re.compile(r"[0-9a-f]{64}")
_DATABASE_URL = re.compile(r"pg0://hindsight:([0-9a-f]{64})@hindsight:([0-9]{4,5})\?" + re.escape(DATABASE_QUERY))


class ServiceError(RuntimeError):
    """A precondition failed or the service did not behave; the message never contains a secret."""


class FloorError(ServiceError):
    """Free disk is below the 30 GiB floor; backups, restores, drills and rebuilds refuse."""


@dataclass(frozen=True)
class Home:
    config: Path
    data: Path
    state: Path
    scratch: bool = False

    @property
    def connection_file(self) -> Path:
        return self.config / CONNECTION_FILE

    @property
    def pg0_home(self) -> Path:
        return self.data / "pg0"

    @property
    def backups(self) -> Path:
        return self.state / "backups"

    @classmethod
    def under(cls, root: Path) -> Home:
        return cls(root / "config", root / "data", root / "state", scratch=True)


def home(environ: Mapping[str, str] | None = None) -> Home:
    """The service's directories; ``ICARUS_MEMORY_HOME`` selects a scratch root."""
    env = os.environ if environ is None else environ
    if root := env.get(HOME_VARIABLE):
        return Home.under(Path(root))
    user = Path(env.get("HOME") or Path.home())

    def xdg(name: str, default: str) -> Path:
        value = env.get(name, "")
        return (Path(value) if value.startswith("/") else user / default) / "icarus-memory"

    return Home(xdg("XDG_CONFIG_HOME", ".config"), xdg("XDG_DATA_HOME", ".local/share"), xdg("XDG_STATE_HOME", ".local/state"))


@contextmanager
def host_lock(timeout: float = LOCK_SECONDS) -> Generator[None]:
    """The host-wide lock every bank write, ID allocation, sync and ``check --bank`` holds: ``<state>/memory.lock``."""
    directory = home().state
    make_directory(directory)
    path = directory / LOCK_FILE
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600)
    try:
        stop = time.monotonic() + timeout
        while True:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() > stop:
                    raise ServiceError(f"the memory host lock {path} was held for over {timeout:.0f} s") from None
                time.sleep(0.1)
        yield
    finally:
        os.close(descriptor)


@dataclass(frozen=True)
class Connection:
    """Where the service is: ``url`` (loopback HTTP), ``key`` (bearer; never shown) and ``bank``."""

    url: str
    key: str = field(repr=False)
    bank: str


@dataclass(frozen=True)
class Settings:
    """The parsed connection file."""

    port: int
    key: str = field(repr=False)
    pg_port: int
    pg_password: str = field(repr=False)
    bank: str
    reranker: str = RERANKER_DEFAULT

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    def connection(self) -> Connection:
        return Connection(self.url, self.key, self.bank)

    def environment(self) -> dict[str, str]:
        """The file's variables, exactly as the unit's ``EnvironmentFile`` passes them."""
        return {
            "HINDSIGHT_API_HOST": "127.0.0.1",
            "HINDSIGHT_API_PORT": str(self.port),
            "HINDSIGHT_API_TENANT_API_KEY": self.key,
            "HINDSIGHT_API_DATABASE_URL": f"pg0://hindsight:{self.pg_password}@hindsight:{self.pg_port}?{DATABASE_QUERY}",
            "ICARUS_MEMORY_BANK": self.bank,
            **reranker_environment(self.reranker),
        }

    @property
    def reranker_cap(self) -> int | None:
        """Candidates the reranker scores per recall; ``None`` when reranking is off."""
        return None if self.reranker == "off" else int(self.reranker)


def reranker_setting(value: str) -> str:
    """``off`` or a cap from 1 to 500, normalised; anything else is a ``ServiceError``."""
    if value == "off":
        return value
    if not (value.isascii() and value.isdigit()) or not 1 <= int(value) <= 500:
        raise ServiceError(f"{RERANKER_VARIABLE} must be off or a candidate cap from 1 to 500, not {value!r}")
    return str(int(value))


def reranker_environment(setting: str) -> dict[str, str]:
    """The setting and the Hindsight variables it implies (the unit's EnvironmentFile passes them): reranking on
    also takes the cap and 4 ONNX threads (cap 10: p50 2.5 s on 2 threads, 1.7 s on 4)."""
    if setting == "off":
        return {RERANKER_VARIABLE: setting, RERANKING_VARIABLE: "false"}
    return {RERANKER_VARIABLE: setting, RERANKING_VARIABLE: "true", "HINDSIGHT_API_RERANKER_MAX_CANDIDATES": setting,
            "HINDSIGHT_API_RERANKER_ONNX_THREADS": RERANKER_THREADS}


def render_settings(settings: Settings) -> str:
    lines = ["# icarus memory service: owner-only; read by the systemd unit and python3 -m tools.memory"]
    lines.extend(f"{name}={value}" for name, value in settings.environment().items())
    return "\n".join(lines) + "\n"


def file_variables(text: str) -> dict[str, str]:
    """The variables of a connection file as written (``NAME=value`` lines; comments and blank lines skipped)."""
    values: dict[str, str] = {}
    for number, line in enumerate(text.splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        name, separator, value = line.partition("=")
        if not separator or name in values:
            raise ServiceError(f"{CONNECTION_FILE}:{number}: malformed or repeated line")
        values[name] = value
    return values


def parse_settings(text: str) -> Settings:
    """Strict parse of the connection file; only the exact generated shape is accepted."""
    values = file_variables(text)
    expected = ("HINDSIGHT_API_HOST", "HINDSIGHT_API_PORT", "HINDSIGHT_API_TENANT_API_KEY", "HINDSIGHT_API_DATABASE_URL", "ICARUS_MEMORY_BANK")
    # A file written before the reranker setting existed holds only the five; it means the default, which
    # ``install`` then writes out (Hindsight's own default is on).
    reranker = reranker_setting(values.get(RERANKER_VARIABLE, RERANKER_DEFAULT))
    implied = reranker_environment(reranker) if RERANKER_VARIABLE in values else {}
    if sorted(values) != sorted((*expected, *implied)) or any(values[name] != value for name, value in implied.items()):
        raise ServiceError(f"{CONNECTION_FILE}: expected exactly {', '.join(expected)} and, optionally, {RERANKER_VARIABLE} with the variables it implies")
    database = _DATABASE_URL.fullmatch(values["HINDSIGHT_API_DATABASE_URL"])
    port = values["HINDSIGHT_API_PORT"]
    if values["HINDSIGHT_API_HOST"] != "127.0.0.1" or not port.isdigit() or not 1024 <= int(port) <= 65535:
        raise ServiceError(f"{CONNECTION_FILE}: the service must listen on a loopback port between 1024 and 65535")
    if not _KEY.fullmatch(values["HINDSIGHT_API_TENANT_API_KEY"]):
        raise ServiceError(f"{CONNECTION_FILE}: the API key must be 64 lowercase hex digits")
    if database is None:
        raise ServiceError(f"{CONNECTION_FILE}: the database URL must be pg0://hindsight:<64 hex>@hindsight:<port>?{DATABASE_QUERY}")
    pg_port = int(database.group(2))
    if not 1024 <= pg_port <= 65535 or pg_port == int(port):
        raise ServiceError(f"{CONNECTION_FILE}: the database port must be distinct and between 1024 and 65535")
    if values["ICARUS_MEMORY_BANK"] != BANK:
        raise ServiceError(f"{CONNECTION_FILE}: the bank must be {BANK}")
    return Settings(int(port), values["HINDSIGHT_API_TENANT_API_KEY"], pg_port, database.group(1), BANK, reranker)


def read_settings(where: Home, path: Path | None = None) -> Settings:
    """The connection file (or ``path``), refused unless it is a regular file owned by this user with no group or world bits."""
    path = where.connection_file if path is None else path
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError as error:
        raise ServiceError(f"{path}: cannot open ({error.strerror}); run memory service install") from error
    with os.fdopen(descriptor, "rb") as handle:
        info = os.fstat(handle.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid():
            raise ServiceError(f"{path}: must be a regular file owned by this user")
        if info.st_mode & 0o077:
            raise ServiceError(f"{path}: mode {stat.S_IMODE(info.st_mode):04o} gives group or world access; it must be 0600")
        data = handle.read(65536)
    try:
        return parse_settings(data.decode("utf-8"))
    except UnicodeDecodeError as error:
        raise ServiceError(f"{path}: not UTF-8") from error


def connection(where: Home | None = None) -> Connection:
    """The service's ``Connection(url, key, bank)``, from the 0600 connection file. The interface for other slices."""
    return read_settings(home() if where is None else where).connection()


def make_directory(path: Path) -> None:
    """Create ``path`` (and parents) owner-only; refuse an existing directory that is not 0700 and ours."""
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) != 0o700:
        raise ServiceError(f"{path}: must be a directory owned by this user with mode 0700")


def write_private(path: Path, data: bytes) -> None:
    """Atomically replace ``path`` with ``data`` at mode 0600, fsynced with its directory."""
    temporary = path.with_name(f".{path.name}.{secrets.token_hex(4)}.tmp")
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600)
    try:
        with os.fdopen(descriptor, "wb") as handle:
            _ = handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)
    directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)


def tombstoned(path: Path) -> bool:
    """Whether ``path`` is exactly ``TOMBSTONE``: a regular file of this user's, read without following a link."""
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    except OSError:
        return False
    with os.fdopen(descriptor, "rb") as handle:
        info = os.fstat(handle.fileno())
        return stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and handle.read(len(TOMBSTONE) + 1) == TOMBSTONE


def rotate_connection(where: Home) -> bool:
    """Retire ``connection.env`` (tools without the rendering guard read it) for ``CONNECTION_FILE`` with a new API
    key, keeping ports, database password and reranker setting, and leave ``TOMBSTONE`` in its place; True when it
    rotated. The tombstone is not a settings file, so such a tool's every command, its ``service install``
    included, stops at "connection.env:1: malformed or repeated line" before writing anything, and the line says
    why. Already tombstoned: nothing to do. No old file (a fresh host): the tombstone alone, so an old checkout is
    never the one to create ``connection.env``. An old file beside the new one is tombstoned once it reads as
    settings; a file that is neither is refused (``read_settings``), never overwritten."""
    legacy = where.config / LEGACY_CONNECTION_FILE
    if tombstoned(legacy):
        return False
    rotated = False
    if legacy.exists() or legacy.is_symlink():
        old = read_settings(where, legacy)
        if not where.connection_file.exists():
            write_private(where.connection_file, render_settings(replace(old, key=secrets.token_hex(32))).encode())
            rotated = True
    write_private(legacy, TOMBSTONE)
    return rotated


def ensure_settings(where: Home, *, port: int = API_PORT, pg_port: int = PG_PORT, reranker: str = RERANKER_DEFAULT) -> Settings:
    """The existing connection file, or a new one with a fresh key and database password and the ``reranker``
    setting (the drill's scratch instance passes the live one, which its comparison checks against)."""
    for directory in (where.config, where.data, where.state, where.backups):
        make_directory(directory)
    if where.connection_file.exists():
        return read_settings(where)
    settings = Settings(port, secrets.token_hex(32), pg_port, secrets.token_hex(32), BANK, reranker_setting(reranker))
    write_private(where.connection_file, render_settings(settings).encode())
    return read_settings(where)


def set_reranker(where: Home, value: str) -> tuple[Settings, bool]:
    """Write ``value`` as the reranker setting (key, ports and password kept) in the generated shape, so a file
    that predates the setting gains its explicit variables; True when the file changed."""
    current = read_settings(where)
    wanted = replace(current, reranker=reranker_setting(value))
    text = render_settings(wanted)
    if where.connection_file.read_text(encoding="utf-8") == text:
        return current, False
    write_private(where.connection_file, text.encode())
    return read_settings(where), True


def reranker_cap(where: Home | None = None) -> int | None:
    """The configured candidate cap (``None``: reranking off); the default when the file is unreadable."""
    try:
        return read_settings(home() if where is None else where).reranker_cap
    except ServiceError:
        return None


def free_bytes(path: Path) -> int:
    probe = path
    while not probe.exists():
        probe = probe.parent
    return shutil.disk_usage(probe).free


def require_floor(path: Path, *, free: Callable[[Path], int] = free_bytes) -> None:
    """Raise ``FloorError`` unless the filesystem holding ``path`` has at least ``FLOOR`` bytes free."""
    available = free(path)
    if available < FLOOR:
        raise FloorError(f"{available / 1024**3:.1f} GiB free is below the 30 GiB floor; refusing")


@dataclass(frozen=True)
class Assets:
    """The provisioned runtime in the primary checkout's ``work/hindsight-runtime/`` (read and execute only)."""

    root: Path

    @property
    def python(self) -> Path:
        return self.root / "venv/bin/python"

    @property
    def pg0(self) -> Path:
        return self.root / "venv/lib/python3.14/site-packages/pg0/bin/pg0"

    @property
    def pg_install(self) -> Path:
        return self.root / "pg0-install"

    @property
    def pg_bin(self) -> Path:
        return self.pg_install / PG_VERSION / "bin"

    @property
    def lock(self) -> Path:
        return self.root / "assets.lock"

    @property
    def models(self) -> Path:
        return self.root / ".cache/hf"

    @property
    def embedding(self) -> Path:
        return self.models / "hub" / EMBEDDING

    @property
    def reranker(self) -> Path:
        return self.models / "hub" / RERANKER

    def problems(self, requirements: Path) -> list[str]:
        """Why the assets cannot run the service; empty when ``install`` verified them for ``requirements``."""
        found: list[str] = []
        stamp = self.root / "wheels.sha256"
        try:
            verified = stamp.read_text(encoding="utf-8").strip() == hashlib.sha256(requirements.read_bytes()).hexdigest()
        except OSError:
            verified = False
        if not verified:
            found.append(f"{stamp} does not match {requirements.name}; run tools/hindsight_runtime install")
        required = [self.python, self.pg0, self.lock, self.pg_bin / "postgres", self.pg_bin / "pg_dump", self.pg_bin / "pg_restore",
                    self.pg_bin / "psql", self.embedding / "onnx/model.onnx", self.reranker / "onnx/model.onnx"]
        found.extend(f"missing {path}" for path in required if not path.exists())
        return found


def assets(start: Path) -> Assets:
    return Assets(primary_checkout_of(start) / "work/hindsight-runtime")


def service_environment(where: Home, runtime: Assets) -> dict[str, str]:
    """The non-secret environment of the API process; the connection file adds the rest."""
    private = where.data / "home"
    return SETTINGS | {
        "HOME": str(private), "TMPDIR": str(where.data / "tmp"),
        "XDG_CONFIG_HOME": str(private / "config"), "XDG_DATA_HOME": str(private / "data"),
        "XDG_STATE_HOME": str(private / "state"), "XDG_CACHE_HOME": str(private / "cache"),
        "HF_HOME": str(runtime.models),
        "PG0_HOME": str(where.pg0_home), "PG0_INSTALLATION_DIR": str(runtime.pg_install), "PG0_EXECUTABLE": str(runtime.pg0),
        "HINDSIGHT_API_EMBEDDINGS_ONNX_MODEL_PATH": str(runtime.embedding / "onnx/model.onnx"),
        "HINDSIGHT_API_EMBEDDINGS_ONNX_TOKENIZER_NAME_OR_PATH": str(runtime.embedding),
        "HINDSIGHT_API_RERANKER_ONNX_MODEL_PATH": str(runtime.reranker / "onnx/model.onnx"),
        "HINDSIGHT_API_RERANKER_ONNX_TOKENIZER_NAME_OR_PATH": str(runtime.reranker),
    }


def api_command(runtime: Assets) -> list[str]:
    """The API under a shared lock on the assets, so ``tools/hindsight_runtime install`` waits for it."""
    return ["/usr/bin/flock", "--shared", "--nonblock", str(runtime.lock), str(runtime.python), "-m", "hindsight_api.main"]


def prepare_runtime_directories(where: Home) -> None:
    for directory in (where.data / "home", where.data / "tmp"):
        make_directory(directory)


_PLAIN = re.compile(r"[A-Za-z0-9_@+=:,./-]+")


def unit_word(value: str) -> str:
    """One systemd word: ``%`` and ``$`` escaped, quoted unless plain."""
    escaped = value.replace("%", "%%").replace("$", "$$")
    if _PLAIN.fullmatch(value):
        return escaped
    return '"' + escaped.replace("\\", "\\\\").replace('"', '\\"') + '"'


PYTHON_VARIABLE = "ICARUS_MEMORY_PYTHON"
# The backup runs the tool from the primary checkout; when that checkout's tool predates ``backup``, say so.
BACKUP_SCRIPT = (
    'if "$ICARUS_MEMORY_PYTHON" -m tools.memory backup --help >/dev/null 2>&1; then exec "$ICARUS_MEMORY_PYTHON" -m tools.memory backup; fi; '
    'echo "memory backup: could not run the backup command of the memory tool in the primary checkout $PWD; it may predate the command (pull main there) or fail to start. No backup was made." >&2; '
    "exit 1"
)


def unit_files(where: Home, runtime: Assets, checkout: Path, interpreter: str) -> dict[str, str]:
    """The service unit, and the daily backup service and timer, for this host. Never committed: they hold host paths.

    ``checkout`` is the primary checkout (the Git common directory's), whatever checkout installs or checks
    them, so the backup always runs the tool from there."""
    # pg0 leaves a .pgpass in TMPDIR at every start; the runtime directory is removed at every stop.
    variables = service_environment(where, runtime) | {"TMPDIR": "%t/icarus-memory"}
    environment = "\n".join(f"Environment={unit_word(f'{name}={value}')}" if name != "TMPDIR" else f"Environment=TMPDIR={value}"
                             for name, value in variables.items())
    pre_start = " ".join(unit_word(item) for item in [str(runtime.pg0), "stop", "--name", "hindsight", "--timeout", "30"])
    service = f"""# Generated by python3 -m tools.memory service install; do not edit.
[Unit]
Description=icarus memory service (Hindsight {API_VERSION}, loopback)
# A crash loop gives up after 10 starts in 10 minutes; memory service start resets the counter.
StartLimitIntervalSec=600
StartLimitBurst=10

[Service]
Type=simple
EnvironmentFile={unit_word(str(where.connection_file))}
{environment}
WorkingDirectory={unit_word(str(where.data))}
RuntimeDirectory=icarus-memory
RuntimeDirectoryMode=0700
UMask=0077
# A PostgreSQL left by anything outside this unit's cgroup is stopped first.
ExecStartPre=-{pre_start}
ExecStart={" ".join(unit_word(item) for item in api_command(runtime))}
# The whole cgroup, PostgreSQL included, is stopped with the API and after any API exit.
KillMode=control-group
TimeoutStopSec=60
Restart=on-failure
RestartSec=5
CPUQuota=400%
# Measured on the real corpus (3,093 records): a full rebuild peaks at API 2,966 + PostgreSQL 525 MiB (3,491 MiB);
# recall with reranking off at 1,776 MiB; a session with a rebuild and cap-50 reranking at 3,515 + 531 MiB (4,046 MiB).
# Reclaim only above the rebuild peak plus a third; never more than 6 GiB.
MemoryHigh=4608M
MemoryMax=6144M
Nice=10
LimitCORE=0
# Raw output can carry the database URI; it is discarded.
StandardOutput=null
StandardError=null

[Install]
WantedBy=default.target
"""
    backup = f"""# Generated by python3 -m tools.memory service install; do not edit.
[Unit]
Description=icarus memory daily backup
After={UNIT}

[Service]
Type=oneshot
WorkingDirectory={unit_word(str(checkout))}
Environment=PATH=/usr/bin:/bin
Environment={unit_word(f"{PYTHON_VARIABLE}={interpreter}")}
ExecStart=/bin/sh -c {unit_word(BACKUP_SCRIPT)}
CPUQuota=400%
Nice=10
"""
    timer = """# Generated by python3 -m tools.memory service install; do not edit.
[Unit]
Description=icarus memory daily backup

[Timer]
OnCalendar=daily
RandomizedDelaySec=900
Persistent=true

[Install]
WantedBy=timers.target
"""
    return {UNIT: service, BACKUP_UNIT: backup, BACKUP_TIMER: timer}


def unit_directory(environ: Mapping[str, str] | None = None) -> Path:
    env = os.environ if environ is None else environ
    value = env.get("XDG_CONFIG_HOME", "")
    base = Path(value) if value.startswith("/") else Path(env.get("HOME") or Path.home()) / ".config"
    return base / "systemd/user"


def systemctl(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(["systemctl", "--user", *args], capture_output=True, text=True, timeout=180, check=False)
    if check and result.returncode != 0:
        raise ServiceError(f"systemctl --user {' '.join(args)} failed: {result.stderr.strip() or result.returncode}")
    return result


# REST


class Api:
    """A loopback HTTP client; the key goes only into the Authorization header."""

    def __init__(self, target: Connection, *, timeout: float = 30.0) -> None:
        match = re.fullmatch(r"http://127\.0\.0\.1:([0-9]{4,5})", target.url)
        if match is None:
            raise ServiceError("the memory service URL must be http://127.0.0.1:<port>")
        self.port: int = int(match.group(1))
        self.target: Connection = target
        self.timeout: float = timeout

    def call(self, method: str, path: str, body: object = None, *, auth: bool = True) -> tuple[int, object]:
        """Status and decoded JSON body (``None`` when the body is not JSON)."""
        headers = {"Accept": "application/json"}
        if auth:
            headers["Authorization"] = f"Bearer {self.target.key}"
        payload = None if body is None else json.dumps(body).encode()
        if payload is not None:
            headers["Content-Type"] = "application/json"
        client = http.client.HTTPConnection("127.0.0.1", self.port, timeout=self.timeout)
        try:
            client.request(method, path, body=payload, headers=headers)
            response = client.getresponse()
            data = response.read(MAX_RESPONSE + 1)
            status = response.status
        except OSError as error:
            raise ServiceError(f"{method} {path}: {error.strerror or type(error).__name__}") from error
        finally:
            client.close()
        if len(data) > MAX_RESPONSE:
            raise ServiceError(f"{method} {path}: response larger than {MAX_RESPONSE} bytes")
        try:
            # json.loads is untyped; callers narrow the object.
            return status, cast(object, json.loads(data)) if data else None
        except ValueError:
            return status, None

    def get(self, path: str) -> dict[str, object]:
        status, body = self.call("GET", path)
        if status != 200 or not isinstance(body, dict):
            raise ServiceError(f"GET {path}: HTTP {status}")
        return {str(key): value for key, value in cast(dict[object, object], body).items()}

    def ready(self) -> bool:
        try:
            return self.call("GET", "/health/ready", auth=False)[0] == 200
        except ServiceError:
            return False


def objects(value: object, label: str) -> list[dict[str, object]]:
    """A JSON array of objects, narrowed."""
    if not isinstance(value, list):
        raise ServiceError(f"{label}: expected a JSON array")
    found: list[dict[str, object]] = []
    for item in cast(list[object], value):
        if not isinstance(item, dict):
            raise ServiceError(f"{label}: expected JSON objects")
        found.append({str(key): entry for key, entry in cast(dict[object, object], item).items()})
    return found


def feature_problems(version: Mapping[str, object]) -> list[str]:
    """Differences between ``GET /version`` and the required API version and feature flags."""
    found = [] if version.get("api_version") == API_VERSION else [f"api_version is {version.get('api_version')!r}, not {API_VERSION}"]
    features = version.get("features")
    flags = {str(key): value for key, value in cast(dict[object, object], features).items()} if isinstance(features, dict) else {}
    found.extend(f"feature {name} is {flags.get(name)!r}, not {value}" for name, value in EXPECTED_FEATURES.items() if flags.get(name) is not value)
    return found


def bank_config_problems(body: Mapping[str, object]) -> list[str]:
    """Differences between ``GET .../banks/icarus/config`` and the instance rules: no overrides, and the expected values."""
    found: list[str] = []
    overrides = body.get("overrides")
    if overrides not in ({}, None):
        found.append(f"bank overrides present: {json.dumps(overrides, sort_keys=True)}")
    config = body.get("config")
    values = {str(key): value for key, value in cast(dict[object, object], config).items()} if isinstance(config, dict) else {}
    for name, expected in EXPECTED_BANK_CONFIG.items():
        actual = values.get(name)
        if actual != expected or type(actual) is not type(expected):
            found.append(f"{name} is {actual!r}, not {expected!r}")
    return found


def ensure_bank(api: Api, bank: str = BANK) -> None:
    """Create the bank with no overrides when absent (first start); idempotent."""
    status, _ = api.call("GET", f"/v1/default/banks/{bank}/config")
    if status == 404:
        status, _ = api.call("PUT", f"/v1/default/banks/{bank}", {})
    if status != 200:
        raise ServiceError(f"bank {bank}: HTTP {status}")


def document_total(api: Api) -> int:
    """Documents in every bank of the instance."""
    total = 0
    for bank in objects(api.get("/v1/default/banks?limit=1000").get("banks"), "banks"):
        listed = api.get(f"/v1/default/banks/{bank.get('bank_id')}/documents?limit=1")
        count = listed.get("total")
        if not isinstance(count, int):
            raise ServiceError("documents: missing total")
        total += count
    return total


# Health


@dataclass(frozen=True)
class Check:
    name: str
    ok: bool
    detail: str


def database_settings(settings: Settings, runtime: Assets) -> dict[str, str]:
    """``SHOW`` of fsync, synchronous_commit and listen_addresses through psql; the password goes by environment."""
    query = "SELECT name || '=' || setting FROM pg_settings WHERE name IN ('fsync','synchronous_commit','listen_addresses') ORDER BY name"
    result = subprocess.run(
        [str(runtime.pg_bin / "psql"), "-X", "-A", "-t", "-q", "-h", "127.0.0.1", "-p", str(settings.pg_port), "-U", "hindsight", "-d", "hindsight", "-c", query],
        capture_output=True, text=True, timeout=30, check=False,
        env={"PATH": "/usr/bin:/bin", "PGPASSWORD": settings.pg_password, "PGCONNECT_TIMEOUT": "5"},
    )
    if result.returncode != 0:
        raise ServiceError(f"psql failed with exit {result.returncode}")
    return dict(line.split("=", 1) for line in result.stdout.split() if "=" in line)


def health(settings: Settings, runtime: Assets) -> list[Check]:
    """Readiness (database reachable), PostgreSQL durability settings, anonymous denial, authenticated access,
    feature flags and bank configuration."""
    api = Api(settings.connection())
    checks = [Check("ready", api.ready(), "GET /health/ready (database reachable)")]
    if not checks[0].ok:
        return checks
    try:
        shown = database_settings(settings, runtime)
        expected = {"fsync": "on", "synchronous_commit": "on", "listen_addresses": "127.0.0.1"}
        checks.append(Check("database", shown == expected, ", ".join(f"{name}={value}" for name, value in sorted(shown.items()))))
    except (ServiceError, OSError, subprocess.TimeoutExpired) as error:
        checks.append(Check("database", False, str(error)))
    try:
        anonymous = api.call("GET", "/v1/default/banks", auth=False)[0]
        checks.append(Check("anonymous denied", anonymous == 401, f"HTTP {anonymous} without a key"))
        banks = objects(api.get("/v1/default/banks?limit=1000").get("banks"), "banks")
        checks.append(Check("authenticated", True, f"banks: {', '.join(str(bank.get('bank_id')) for bank in banks) or 'none'}"))
        problems = feature_problems(api.get("/version"))
        checks.append(Check("features", not problems, "; ".join(problems) or f"{API_VERSION}; " + ", ".join(f"{name}={value}".lower() for name, value in EXPECTED_FEATURES.items())))
        config = api.get(f"/v1/default/banks/{settings.bank}/config")
        effective = config.get("config")
        reranking = cast(dict[object, object], effective).get("enable_reranking") if isinstance(effective, dict) else None
        checks.append(reranker_check(settings, reranking))
        problems = bank_config_problems(config)
        checks.append(Check("bank config", not problems, "; ".join(problems) or f"{settings.bank}: no overrides; " + ", ".join(f"{name}={value}" for name, value in EXPECTED_BANK_CONFIG.items())))
    except ServiceError as error:
        checks.append(Check("authenticated", False, str(error)))
    return checks


def reranker_check(settings: Settings, reranking: object) -> Check:
    """Whether the service's effective reranking (the bank config's ``enable_reranking``) is what the settings say."""
    wanted = settings.reranker_cap is not None
    detail = f"{RERANKER_VARIABLE}={settings.reranker}; bank enable_reranking={reranking}" + (f", at most {settings.reranker_cap} candidates" if wanted else "")
    if reranking is not wanted:
        effective = "on" if reranking is True else "off" if reranking is False else f"unreported ({reranking!r})"
        detail = f"reranking is {effective} in the service but the settings say {'on' if wanted else 'off'}; run memory service install ({detail})"
    return Check("reranker", reranking is wanted, detail)


def settings_check(where: Home) -> Check:
    """Whether the connection file is in the generated shape; a file written before the reranker setting existed
    leaves Hindsight's own default (reranking on) to the service until ``install`` rewrites it."""
    try:
        text = where.connection_file.read_text(encoding="utf-8")
        settings = parse_settings(text)
    except (OSError, UnicodeDecodeError, ServiceError) as error:
        return Check("settings", False, str(error))
    if text != render_settings(settings):
        return Check("settings", False, f"{CONNECTION_FILE} predates the reranker setting (no {RERANKING_VARIABLE}); run memory service install")
    return Check("settings", True, f"{RERANKER_VARIABLE}={settings.reranker}, {RERANKING_VARIABLE}={settings.environment()[RERANKING_VARIABLE]}")


def wait_ready(api: Api, *, alive: Callable[[], bool], seconds: float = READY_SECONDS) -> float:
    """Seconds until ``/health/ready`` answers 200; raises if ``alive`` turns false or time runs out."""
    started = time.monotonic()
    while time.monotonic() - started < seconds:
        if api.ready():
            return time.monotonic() - started
        if not alive():
            raise ServiceError("the service exited before it became ready")
        time.sleep(1)
    raise ServiceError(f"the service was not ready after {seconds:.0f} s")


def unit_active() -> bool:
    return systemctl("is-active", "--quiet", UNIT, check=False).returncode == 0


def unit_property(name: str) -> str:
    return systemctl("show", "--value", "-p", name, UNIT, check=False).stdout.strip()


def start_unit(settings: Settings) -> float:
    """Start (or restart) the unit, wait for readiness and create the bank if absent; returns seconds to ready."""
    began = time.monotonic()
    _ = systemctl("reset-failed", UNIT, check=False)
    _ = systemctl("restart" if unit_active() else "start", UNIT)
    api = Api(settings.connection())
    _ = wait_ready(api, alive=lambda: unit_property("ActiveState") in {"active", "activating", "reloading"})
    ensure_bank(api, settings.bank)
    return time.monotonic() - began


def installed_interpreter(text: str) -> str | None:
    """The interpreter an installed backup unit names, when it still exists and is executable."""
    prefix = f"Environment={PYTHON_VARIABLE}="
    for line in text.splitlines():
        if line.startswith(prefix):
            value = line.removeprefix(prefix)
            return value if _PLAIN.fullmatch(value) and os.access(value, os.X_OK) else None
    return None


def status_lines(where: Home, settings: Settings, runtime: Assets, checkout: Path) -> tuple[bool, list[str]]:
    checks: list[Check] = []
    installed = {name: (unit_directory() / name) for name in (UNIT, BACKUP_UNIT, BACKUP_TIMER)}
    backup_unit = installed[BACKUP_UNIT].read_text(encoding="utf-8") if installed[BACKUP_UNIT].is_file() else ""
    # The units are compared from any checkout: the primary checkout, and the interpreter they were installed with.
    expected = unit_files(where, runtime, primary_checkout_of(checkout), installed_interpreter(backup_unit) or backup_interpreter())
    stale = [name for name, path in installed.items() if not path.is_file() or path.read_text(encoding="utf-8") != expected[name]]
    checks.append(Check("unit files", not stale, f"stale or missing: {', '.join(stale)}" if stale else "current"))
    checks.append(settings_check(where))
    active = unit_property("ActiveState")
    checks.append(Check("unit", active == "active", f"{UNIT} {active}, restarts {unit_property('NRestarts') or '?'}"))
    timer = systemctl("is-active", BACKUP_TIMER, check=False).stdout.strip()
    checks.append(Check("backup timer", timer == "active", f"{BACKUP_TIMER} {timer}"))
    checks.extend(health(settings, runtime))
    lines = [f"{'ok  ' if check.ok else 'FAIL'} {check.name}: {check.detail}" for check in checks]
    memory = [unit_property(name) for name in ("MemoryCurrent", "MemoryPeak")]
    if all(value.isdigit() for value in memory):
        lines.append(f"info memory: current {int(memory[0]) / 1024**2:.0f} MiB, peak {int(memory[1]) / 1024**2:.0f} MiB (unit cgroup)")
    available = free_bytes(where.state)
    floor_note = "" if available >= FLOOR else "; below the floor: backup, restore and rebuild refuse"
    lines.append(f"info disk: {available / 1024**3:.1f} GiB free, floor 30 GiB{floor_note}")
    return all(check.ok for check in checks), lines


def primary_checkout_of(start: Path) -> Path:
    try:
        return primary_checkout(start)
    except ProtectedPathsError as error:
        raise ServiceError(str(error)) from error


def backup_interpreter(environ: Mapping[str, str] | None = None) -> str:
    """The Python 3 the backup timer runs: the first ``python3`` on PATH (as the hooks run it) that is not
    in a virtual environment, else this one. A virtual environment's ``bin`` (``pyvenv.cfg`` beside it),
    such as the checkout ``.venv`` that an interactive ``uv run python3`` alias puts first, is skipped:
    it belongs to one checkout."""
    env = os.environ if environ is None else environ
    for directory in env.get("PATH", "").split(os.pathsep):
        if not directory.startswith("/") or (Path(directory).parent / "pyvenv.cfg").exists():
            continue
        candidate = Path(directory) / "python3"
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return str(candidate)
    return sys.executable


def install(where: Home, runtime: Assets, checkout: Path, reranker: str | None = None) -> float:
    """Write the connection file (key, ports and password once; an old ``connection.env`` is rotated to it with a
    new key, ``rotate_connection``) and the units, enable them, start the service; returns seconds to ready. The connection file is rewritten in the generated shape every time, with
    ``reranker`` (``off`` or a cap) or else the setting it holds (``off`` when it predates the setting), so
    the service's reranking is always set explicitly; any change restarts the service."""
    wanted = None if reranker is None else reranker_setting(reranker)
    if where.scratch:
        raise ServiceError(f"{HOME_VARIABLE} is set: a scratch home is never managed by systemd")
    for directory in (where.config, where.data, where.state, where.backups):
        make_directory(directory)
    rotated = rotate_connection(where)
    settings, changed = set_reranker(where, ensure_settings(where).reranker if wanted is None else wanted)
    changed = changed or rotated
    prepare_runtime_directories(where)
    make_directory(where.pg0_home)
    directory = unit_directory()
    directory.mkdir(parents=True, exist_ok=True)
    for name, text in unit_files(where, runtime, primary_checkout_of(checkout), backup_interpreter()).items():
        path = directory / name
        if not path.is_file() or path.read_text(encoding="utf-8") != text:
            write_private(path, text.encode())
            changed = True
    _ = systemctl("daemon-reload")
    _ = systemctl("enable", UNIT, BACKUP_TIMER)
    _ = systemctl("start", BACKUP_TIMER)
    if changed or not unit_active():
        return start_unit(settings)
    ensure_bank(Api(settings.connection()), settings.bank)
    return 0.0


def uninstall() -> list[Path]:
    """Disable and remove the units; data, connection file and backups stay. Returns the removed unit files."""
    _ = systemctl("disable", "--now", UNIT, BACKUP_TIMER, check=False)
    removed: list[Path] = []
    for name in (UNIT, BACKUP_UNIT, BACKUP_TIMER):
        path = unit_directory() / name
        if path.is_file():
            path.unlink()
            removed.append(path)
    _ = systemctl("daemon-reload")
    return removed


# Command line


def repository_root() -> Path:
    try:
        return Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())
    except GitError as error:
        raise ServiceError("run the memory tool inside a checkout") from error


def checked_assets(root: Path) -> Assets:
    runtime = assets(root)
    if problems := runtime.problems(root / "tools/hindsight_runtime.requirements.txt"):
        raise ServiceError("; ".join(problems))
    return runtime


def argument(options: argparse.Namespace, name: str) -> object:
    value: object = getattr(options, name, None)
    return value


def run_service(options: argparse.Namespace) -> int:
    action = str(argument(options, "action"))
    where = home()
    try:
        if action == "stop":
            _ = systemctl("stop", UNIT)
            print(f"memory service: {UNIT} stopped")
            return 0
        if action == "uninstall":
            for path in uninstall():
                print(f"memory service: removed {path}")
            print(f"memory service: kept {where.config}, {where.data} and {where.state}")
            return 0
        root = repository_root()
        runtime = checked_assets(root)
        if action == "install":
            setting = argument(options, "reranker")
            seconds = install(where, runtime, root, setting if isinstance(setting, str) else None)
            print(f"memory service: installed; ready after {seconds:.1f} s" if seconds else "memory service: installed; already running")
        elif action == "start":
            settings = read_settings(where)
            prepare_runtime_directories(where)
            print(f"memory service: ready after {start_unit(settings):.1f} s")
        healthy, lines = status_lines(where, read_settings(where), runtime, root)
        print("\n".join(lines))
        return 0 if healthy else 1
    except (ServiceError, OSError, subprocess.TimeoutExpired) as error:
        print(f"memory service {action}: {error}", file=sys.stderr)
        return 1


class Subcommands(Protocol):
    """What ``register`` needs of ``ArgumentParser.add_subparsers()``."""

    def add_parser(self, name: str, *, help: str | None = ...) -> argparse.ArgumentParser: ...
