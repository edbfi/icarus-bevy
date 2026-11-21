# SPDX-License-Identifier: AGPL-3.0-or-later
"""The memory service and its durability without a running service: homes, the connection file, unit
generation, configuration assertions, the floor, watermarks, rotation, backup with a stand-in
``pg_dump``, and the drill's comparison. Every key, path and document is invented."""

from __future__ import annotations

import io
import json
import hashlib
import os
import shutil
import socket
import subprocess
import tempfile
import unittest
from contextlib import redirect_stderr
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import NamedTuple, cast, override
from unittest.mock import patch

from tools.memory import durability, service
from tools.memory.bank import BankError
from tools.memory.cli import main
from tools.memory.gitio import GitError
from tools.memory.hindsight import HindsightError
from tools.memory.journal import JournalError
from tools.memory.durability import NO_JOURNAL, Backup, Watermark, backup, compare, restore, retained, rotate, verify
from tools.memory.service import (
    API_PORT, BACKUP_SCRIPT, BACKUP_TIMER, BACKUP_UNIT, Api, Connection, FLOOR, PG_PORT, SETTINGS, UNIT, Assets, FloorError, Home, ServiceError, Settings, bank_config_problems,
    backup_interpreter, connection, ensure_settings, feature_problems, file_variables, home, install, installed_interpreter, parse_settings, read_settings, render_settings,
    require_floor, reranker_cap, reranker_check, set_reranker, settings_check, unit_files, unit_word,
)

from tools.tests.memory_fixtures import clean_git_environment, commit_all, new_repository, run_git

KEY, PASSWORD = "1" * 64, "2" * 64


class Usage(NamedTuple):
    total: int
    used: int
    free: int


class Scratch(unittest.TestCase):
    root: Path = Path()
    where: Home = Home(Path(), Path(), Path())

    @override
    def setUp(self) -> None:
        self.root = Path(self.enterContext(tempfile.TemporaryDirectory()))
        self.where = Home.under(self.root / "memory")


class Homes(unittest.TestCase):
    def test_xdg_defaults_and_overrides(self) -> None:
        self.assertEqual(home({"HOME": "/h"}), Home(Path("/h/.config/icarus-memory"), Path("/h/.local/share/icarus-memory"), Path("/h/.local/state/icarus-memory")))
        chosen = home({"HOME": "/h", "XDG_CONFIG_HOME": "/c", "XDG_DATA_HOME": "/d", "XDG_STATE_HOME": "relative"})
        self.assertEqual((chosen.config, chosen.data, chosen.state), (Path("/c/icarus-memory"), Path("/d/icarus-memory"), Path("/h/.local/state/icarus-memory")))
        self.assertFalse(chosen.scratch)

    def test_icarus_memory_home_is_one_scratch_root(self) -> None:
        chosen = home({"HOME": "/h", "XDG_CONFIG_HOME": "/c", "ICARUS_MEMORY_HOME": "/s"})
        self.assertEqual((chosen.config, chosen.data, chosen.state, chosen.scratch), (Path("/s/config"), Path("/s/data"), Path("/s/state"), True))
        self.assertEqual((chosen.connection_file, chosen.pg0_home, chosen.backups), (Path("/s/config/connection.env"), Path("/s/data/pg0"), Path("/s/state/backups")))


class ConnectionFile(Scratch):
    def test_created_owner_only_and_read_back(self) -> None:
        settings = ensure_settings(self.where)
        for directory in (self.where.config, self.where.data, self.where.state, self.where.backups):
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
        self.assertEqual(self.where.connection_file.stat().st_mode & 0o777, 0o600)
        found = connection(self.where)
        self.assertEqual((found.url, found.bank, found.key), (f"http://127.0.0.1:{API_PORT}", "icarus", settings.key))
        self.assertNotIn(settings.key, repr(found) + repr(settings))
        self.assertEqual(ensure_settings(self.where), settings, "an existing file is kept")
        self.assertIn(f"@hindsight:{PG_PORT}?listen_addresses=127.0.0.1&fsync=on&synchronous_commit=on", self.where.connection_file.read_text())

    def test_group_or_world_access_is_refused(self) -> None:
        _ = ensure_settings(self.where)
        for mode in (0o640, 0o604, 0o620, 0o602):
            os.chmod(self.where.connection_file, mode)
            with self.assertRaisesRegex(ServiceError, "0600"):
                _ = connection(self.where)
        os.chmod(self.where.connection_file, 0o400)
        self.assertEqual(connection(self.where).bank, "icarus")

    def test_symlink_and_missing_file_are_refused(self) -> None:
        _ = ensure_settings(self.where)
        target = self.root / "elsewhere.env"
        _ = self.where.connection_file.rename(target)
        with self.assertRaisesRegex(ServiceError, "install"):
            _ = connection(self.where)
        self.where.connection_file.symlink_to(target)
        with self.assertRaises(ServiceError):
            _ = connection(self.where)

    def test_only_the_generated_shape_parses(self) -> None:
        good = render_settings(Settings(API_PORT, KEY, PG_PORT, PASSWORD, "icarus"))
        self.assertEqual(parse_settings(good).pg_password, PASSWORD)
        for broken in (
            good + "EXTRA=1\n",
            good.replace("127.0.0.1\n", "0.0.0.0\n", 1),
            good.replace("&fsync=on", ""),
            good.replace(KEY, "short"),
            good.replace("ICARUS_MEMORY_BANK=icarus", "ICARUS_MEMORY_BANK=other"),
            good.replace(f"@hindsight:{PG_PORT}", f"@hindsight:{API_PORT}"),
            good + f"HINDSIGHT_API_TENANT_API_KEY={KEY}\n",
        ):
            with self.assertRaises(ServiceError):
                _ = parse_settings(broken)


    def test_the_reranker_setting_is_off_by_default_or_a_cap_with_four_threads(self) -> None:
        default = render_settings(Settings(API_PORT, KEY, PG_PORT, PASSWORD, "icarus"))
        self.assertIn("ICARUS_MEMORY_RERANKER=off\nHINDSIGHT_API_ENABLE_RERANKING=false\n", default)
        self.assertNotIn("MAX_CANDIDATES", default)
        self.assertNotIn("ONNX_THREADS", default)
        older = "".join(line + "\n" for line in default.splitlines() if "RERANK" not in line)
        self.assertEqual((None, None), (parse_settings(default).reranker_cap, parse_settings(older).reranker_cap))
        on = render_settings(Settings(API_PORT, KEY, PG_PORT, PASSWORD, "icarus", "10"))
        self.assertIn("ICARUS_MEMORY_RERANKER=10\nHINDSIGHT_API_ENABLE_RERANKING=true\nHINDSIGHT_API_RERANKER_MAX_CANDIDATES=10\n"
                      + "HINDSIGHT_API_RERANKER_ONNX_THREADS=4\n", on)
        self.assertEqual(10, parse_settings(on).reranker_cap)
        for broken in (on.replace("ICARUS_MEMORY_RERANKER=10", "ICARUS_MEMORY_RERANKER=0"),
                       on.replace("MAX_CANDIDATES=10", "MAX_CANDIDATES=20"),
                       on.replace("ONNX_THREADS=4", "ONNX_THREADS=2"),
                       default.replace("ENABLE_RERANKING=false", "ENABLE_RERANKING=true")):
            with self.assertRaises(ServiceError):
                _ = parse_settings(broken)
        settings = ensure_settings(self.where)
        changed, written = set_reranker(self.where, "20")
        self.assertTrue(written)
        self.assertEqual((changed.key, changed.reranker_cap), (settings.key, 20))
        self.assertFalse(set_reranker(self.where, "20")[1], "an unchanged setting is not rewritten")
        self.assertEqual(20, reranker_cap(self.where))
        with self.assertRaises(ServiceError):
            _ = set_reranker(self.where, "many")

    def test_a_cap_takes_ascii_digits_only(self) -> None:
        _ = ensure_settings(self.where)
        for value in ("\u00b2", "\u0665", "\uff11\uff10", "1\u00b2"):
            with self.assertRaisesRegex(ServiceError, "candidate cap from 1 to 500"):
                _ = set_reranker(self.where, value)
        self.assertIsNone(reranker_cap(self.where), "a refused value is never written")
        stderr = io.StringIO()
        with (patch("tools.memory.service.home", return_value=self.where), patch("tools.memory.service.repository_root", return_value=self.root),
              patch("tools.memory.service.checked_assets", return_value=Assets(self.root / "runtime")), redirect_stderr(stderr)):
            self.assertEqual(1, main(["service", "install", "--reranker", "\u00b2"]))
        self.assertIn("memory service install: ICARUS_MEMORY_RERANKER must be off or a candidate cap from 1 to 500", stderr.getvalue())


    def test_a_missing_postgresql_build_names_the_install_command(self) -> None:
        requirements = self.root / "requirements.txt"
        _ = requirements.write_text("invented==1\n", encoding="utf-8")
        runtime = Assets(self.root / "runtime")
        runtime.root.mkdir()
        _ = (runtime.root / "wheels.sha256").write_text(hashlib.sha256(requirements.read_bytes()).hexdigest() + "\n", encoding="utf-8")
        problems = runtime.problems(requirements)
        self.assertIn(f"missing {runtime.pg_bin / 'postgres'}; run tools/hindsight_runtime install (it provisions the Python environment, the models and PostgreSQL)",
                      problems)
        self.assertFalse(any("does not match" in problem for problem in problems))


class Units(Scratch):
    def test_service_unit_carries_the_instance_settings_and_no_secret(self) -> None:
        settings = ensure_settings(self.where)
        files = unit_files(self.where, Assets(self.root / "runtime"), self.root / "primary", "/usr/bin/python3")
        self.assertEqual(sorted(files), sorted([UNIT, BACKUP_UNIT, BACKUP_TIMER]))
        service = files[UNIT]
        for name, value in SETTINGS.items():
            self.assertIn(f"Environment={name}={value}\n", service)
        for line in ("StartLimitBurst=10", "RestartSec=5", f"EnvironmentFile={self.where.connection_file}", "KillMode=control-group", "Restart=on-failure", "CPUQuota=400%",
                     "MemoryHigh=4608M", "MemoryMax=6144M", "StandardOutput=null", "StandardError=null", "RuntimeDirectory=icarus-memory", "Environment=TMPDIR=%t/icarus-memory", f"Environment=PG0_HOME={self.where.pg0_home}",
                     f"ExecStartPre=-{self.root}/runtime/venv/lib/python3.14/site-packages/pg0/bin/pg0 stop --name hindsight --timeout 30",
                     f"ExecStart=/usr/bin/flock --shared --nonblock {self.root}/runtime/assets.lock {self.root}/runtime/venv/bin/python -m hindsight_api.main"):
            self.assertIn(line + "\n", service)
        self.assertIn(f"WorkingDirectory={self.root}/primary\n", files[BACKUP_UNIT])
        self.assertIn("Environment=ICARUS_MEMORY_PYTHON=/usr/bin/python3\n", files[BACKUP_UNIT])
        self.assertIn("ExecStart=/bin/sh -c ", files[BACKUP_UNIT])
        self.assertIn('exec \\"$$ICARUS_MEMORY_PYTHON\\" -m tools.memory backup', files[BACKUP_UNIT])
        self.assertEqual("/usr/bin/python3", installed_interpreter(files[BACKUP_UNIT]))
        self.assertIn("OnCalendar=daily\n", files[BACKUP_TIMER])
        text = "".join(files.values())
        self.assertNotIn(settings.key, text)
        self.assertNotIn(settings.pg_password, text)

    def test_the_backup_explains_a_primary_checkout_whose_tool_predates_backup(self) -> None:
        primary = self.root / "primary"
        primary.mkdir()
        older = self.root / "older-python"
        _ = older.write_text('#!/bin/sh\necho "memory: error: argument COMMAND: invalid choice: \'backup\'" >&2\nexit 2\n', encoding="utf-8")
        current = self.root / "current-python"
        _ = current.write_text('#!/bin/sh\necho "$@"\n', encoding="utf-8")
        for fake in (older, current):
            fake.chmod(0o700)
        environment = {"PATH": "/usr/bin:/bin", "ICARUS_MEMORY_PYTHON": str(older)}
        result = subprocess.run(["/bin/sh", "-c", BACKUP_SCRIPT], cwd=primary, env=environment, capture_output=True, text=True, check=False)
        self.assertEqual(1, result.returncode)
        self.assertEqual(f"memory backup: could not run the backup command of the memory tool in the primary checkout {primary}; " +
                         "it may predate the command (pull main there) or fail to start. No backup was made.\n", result.stderr)
        environment["ICARUS_MEMORY_PYTHON"] = str(current)
        result = subprocess.run(["/bin/sh", "-c", BACKUP_SCRIPT], cwd=primary, env=environment, capture_output=True, text=True, check=False)
        self.assertEqual((0, "-m tools.memory backup\n"), (result.returncode, result.stdout))

    def test_the_interpreter_skips_virtual_environments(self) -> None:
        venv, system = self.root / "checkout/.venv", self.root / "system"
        for directory in (venv / "bin", system / "bin"):
            directory.mkdir(parents=True)
            (directory / "python3").touch(mode=0o755)
        (venv / "pyvenv.cfg").touch()
        chosen = backup_interpreter({"PATH": f"{venv / 'bin'}:relative:{system / 'bin'}:/usr/bin"})
        self.assertEqual(str(system / "bin/python3"), chosen)
        self.assertIsNone(installed_interpreter(f"Environment=ICARUS_MEMORY_PYTHON={self.root / 'gone/python3'}\n"))

    def test_words_are_escaped_for_systemd(self) -> None:
        self.assertEqual(unit_word("/plain/path-1.2"), "/plain/path-1.2")
        self.assertEqual(unit_word("/with space/100%/$HOME"), '"/with space/100%%/$$HOME"')
        self.assertEqual(unit_word('a"b\\c'), '"a\\"b\\\\c"')

    def service_environment(self, where: Home, unit: str) -> dict[str, str]:
        """What systemd gives the API process: the unit's ``Environment=`` lines, then the connection file, which
        overrides them (systemd.exec(5): ``EnvironmentFile=`` settings override ``Environment=``)."""
        variables: dict[str, str] = {}
        for line in unit.splitlines():
            if line.startswith("Environment="):
                name, _, value = line.removeprefix("Environment=").strip('"').partition("=")
                variables[name] = value
        return variables | file_variables(where.connection_file.read_text(encoding="utf-8"))

    def install_with(self, where: Home, reranker: str | None) -> tuple[dict[str, str], bool]:
        """``install`` against a stand-in systemd; the service's environment afterwards, and whether it restarted."""
        units = self.root / "units"
        started: list[Settings] = []

        def start(settings: Settings) -> float:
            started.append(settings)
            return 1.0

        runtime = Assets(self.root / "runtime")
        with (patch("tools.memory.service.systemctl"), patch("tools.memory.service.unit_directory", return_value=units),
              patch("tools.memory.service.unit_active", return_value=True), patch("tools.memory.service.start_unit", side_effect=start),
              patch("tools.memory.service.ensure_bank"), patch("tools.memory.service.primary_checkout_of", return_value=self.root),
              patch("tools.memory.service.backup_interpreter", return_value="/usr/bin/python3")):
            _ = install(where, runtime, self.root, reranker)
        return self.service_environment(where, (units / UNIT).read_text(encoding="utf-8")), bool(started)

    def test_install_sets_reranking_explicitly_in_the_service_environment(self) -> None:
        where = Home(self.root / "config", self.root / "data", self.root / "state")
        _ = ensure_settings(where)
        written = where.connection_file.read_text(encoding="utf-8")
        older = "".join(line + "\n" for line in written.splitlines() if "RERANK" not in line)
        _ = where.connection_file.write_text(older, encoding="utf-8")
        self.assertEqual(5, sum(1 for line in older.splitlines() if not line.startswith("#")))
        self.assertIn("predates the reranker setting", settings_check(where).detail)
        self.assertFalse(settings_check(where).ok)
        # (a) A pre-change five-line file: a plain install writes the setting out and restarts.
        environment, restarted = self.install_with(where, None)
        self.assertEqual(("off", "false"), (environment["ICARUS_MEMORY_RERANKER"], environment["HINDSIGHT_API_ENABLE_RERANKING"]))
        self.assertNotIn("HINDSIGHT_API_RERANKER_MAX_CANDIDATES", environment)
        self.assertTrue(restarted)
        self.assertEqual(written, where.connection_file.read_text(encoding="utf-8"), "the generated shape, key and ports kept")
        self.assertTrue(settings_check(where).ok)
        self.assertFalse(self.install_with(where, None)[1], "an install with nothing to change leaves the running service alone")
        # Even the old file under the new unit runs with reranking off: the unit carries the floor.
        _ = where.connection_file.write_text(older, encoding="utf-8")
        self.assertEqual("false", self.service_environment(where, unit_files(where, Assets(self.root / "runtime"), self.root, "/usr/bin/python3")[UNIT])
                         ["HINDSIGHT_API_ENABLE_RERANKING"])
        # (c) A cap turns it on with the cap and four threads; a plain install keeps it.
        environment, restarted = self.install_with(where, "10")
        self.assertEqual(("10", "true", "10", "4"), (environment["ICARUS_MEMORY_RERANKER"], environment["HINDSIGHT_API_ENABLE_RERANKING"],
                                                     environment["HINDSIGHT_API_RERANKER_MAX_CANDIDATES"], environment["HINDSIGHT_API_RERANKER_ONNX_THREADS"]))
        self.assertTrue(restarted)
        environment, restarted = self.install_with(where, None)
        self.assertEqual(("10", "true"), (environment["ICARUS_MEMORY_RERANKER"], environment["HINDSIGHT_API_ENABLE_RERANKING"]))
        self.assertFalse(restarted)
        # (b) Off again: explicitly false, and the cap and threads are gone.
        environment, restarted = self.install_with(where, "off")
        self.assertEqual(("off", "false"), (environment["ICARUS_MEMORY_RERANKER"], environment["HINDSIGHT_API_ENABLE_RERANKING"]))
        self.assertFalse({"HINDSIGHT_API_RERANKER_MAX_CANDIDATES", "HINDSIGHT_API_RERANKER_ONNX_THREADS"} & set(environment))
        self.assertTrue(restarted)

    def test_status_names_reranking_that_differs_from_the_settings(self) -> None:
        off = Settings(API_PORT, KEY, PG_PORT, PASSWORD, "icarus")
        mismatch = reranker_check(off, True)
        self.assertFalse(mismatch.ok)
        self.assertTrue(mismatch.detail.startswith("reranking is on in the service but the settings say off; run memory service install"))
        self.assertTrue(reranker_check(off, False).ok)
        capped = reranker_check(Settings(API_PORT, KEY, PG_PORT, PASSWORD, "icarus", "10"), False)
        self.assertTrue(capped.detail.startswith("reranking is off in the service but the settings say on"))
        self.assertIn("unreported", reranker_check(off, None).detail)

    def test_install_never_manages_a_scratch_home(self) -> None:
        with self.assertRaisesRegex(ServiceError, "ICARUS_MEMORY_HOME"):
            _ = install(self.where, Assets(self.root / "runtime"), self.root)
        self.assertFalse(self.where.config.exists())


class Configuration(unittest.TestCase):
    def test_bank_config_requires_no_overrides_and_the_values(self) -> None:
        config: dict[str, object] = {"retain_extraction_mode": "chunks", "retain_chunk_size": 1800, "enable_observations": False,
                                     "enable_auto_consolidation": False, "enable_reranking": True}
        self.assertEqual(bank_config_problems({"config": config, "overrides": {}}), [])
        problems = bank_config_problems({"config": config | {"retain_chunk_size": 3000, "enable_observations": 0}, "overrides": {"retain_chunk_size": 3000}})
        self.assertEqual(len(problems), 3)
        self.assertTrue(problems[0].startswith("bank overrides present"))

    def test_features_must_be_off_on_1_2_3(self) -> None:
        flags: dict[str, object] = {"mcp": False, "bank_config_api": False, "file_upload_api": False, "document_import_api": False,
                                    "observations": False, "llm_trace": False, "worker": True}
        self.assertEqual(feature_problems({"api_version": "1.2.3", "features": flags}), [])
        self.assertEqual(len(feature_problems({"api_version": "1.2.4", "features": flags | {"mcp": True}})), 2)
        self.assertEqual(len(feature_problems({})), 7)


class Floor(Scratch):
    def test_floor_is_30_gib(self) -> None:
        require_floor(self.root, free=lambda _path: FLOOR)
        with self.assertRaisesRegex(FloorError, "30 GiB floor"):
            require_floor(self.root, free=lambda _path: FLOOR - 1)

    def test_backup_and_restore_refuse_below_the_floor(self) -> None:
        _ = ensure_settings(self.where)
        with patch("tools.memory.service.shutil.disk_usage", return_value=Usage(FLOOR * 2, FLOOR * 2 - 1024, 1024)):
            with self.assertRaises(FloorError):
                _ = backup(self.where, Assets(self.root / "runtime"))
        self.assertEqual(list(self.where.backups.iterdir()), [])


class Watermarks(unittest.TestCase):
    def test_canonical_round_trip(self) -> None:
        mark = Watermark("a" * 40, ("c" * 64, "b" * 64, "c" * 64))
        data = mark.encode()
        self.assertEqual(data, b'{"origin_main":"' + b"a" * 40 + b'","segments":["' + b"b" * 64 + b'","' + b"c" * 64 + b'"]}\n')
        self.assertEqual(Watermark.decode(data), Watermark("a" * 40, ("b" * 64, "c" * 64)))
        self.assertEqual(NO_JOURNAL.encode(), b'{"origin_main":null,"segments":[]}\n')

    def test_invalid_watermarks_are_refused(self) -> None:
        with self.assertRaises(ServiceError):
            _ = Watermark("main", ()).encode()
        with self.assertRaises(ServiceError):
            _ = Watermark(None, ("x",)).encode()
        for data in (b'{"segments":[],"origin_main":null}\n', b'{"origin_main":null}\n', b'{"origin_main":null,"segments":[1]}\n', b"[]\n", b"{"):
            with self.assertRaises(ServiceError):
                _ = Watermark.decode(data)


class GitCalls(Scratch):
    def test_git_failures_degrade_cleanly(self) -> None:
        outside = self.root / "not-a-checkout"
        outside.mkdir()
        with clean_git_environment():
            self.assertIsNone(durability.descends_from(outside, "a" * 40, "b" * 40))
            repository = self.root / "repository"
            repository.mkdir()
            new_repository(repository)
            _ = (repository / "file").write_text("one\n", encoding="utf-8")
            commit_all(repository)
            first = run_git(repository, "rev-parse", "HEAD").strip()
            _ = (repository / "file").write_text("two\n", encoding="utf-8")
            commit_all(repository)
            second = run_git(repository, "rev-parse", "HEAD").strip()
            self.assertEqual((True, False, None), (durability.descends_from(repository, first, second), durability.descends_from(repository, second, first),
                                                   durability.descends_from(repository, "f" * 40, second)))
            with patch("tools.memory.service.Path.cwd", return_value=outside), self.assertRaisesRegex(ServiceError, "inside a checkout"):
                _ = service.repository_root()


def stamp(day: int, hour: int = 3) -> datetime:
    return datetime(2026, 11, 1, hour, tzinfo=UTC) + timedelta(days=day)


class Rotation(Scratch):
    def test_seven_daily_and_four_weekly(self) -> None:
        stamps = [stamp(day, hour) for day in range(40) for hour in (3, 15)]
        keep = retained(stamps)
        # Days 33-39 are 4-10 December 2026 (ISO weeks 49 and 50); weeks 47 and 48 end on days 21 and 28.
        self.assertEqual(keep, {stamp(day, 15) for day in (21, 28, *range(33, 40))})
        self.assertEqual(retained([]), set())

    def test_rotate_removes_complete_sets_only(self) -> None:
        self.where.backups.mkdir(parents=True)
        for day in range(12):
            name = f"icarus-memory-{stamp(day).strftime('%Y%m%dT%H%M%SZ')}"
            for suffix in (".dump", ".watermark.json", ".sha256"):
                _ = (self.where.backups / (name + suffix)).write_bytes(b"x")
        incomplete = self.where.backups / "icarus-memory-20261001T000000Z.dump"
        _ = incomplete.write_bytes(b"x")
        removed = rotate(self.where.backups)
        kept = sorted(path.name for path in self.where.backups.glob("*.sha256"))
        self.assertEqual(len(kept), len(retained(stamp(day) for day in range(12))))
        self.assertEqual(len(removed), 3 * (12 - len(kept)))
        self.assertTrue(incomplete.exists())


FAKE_PG_DUMP = """#!/bin/sh
printf '%s\\n' "$@" > "{log}"
printf 'PGPASSWORD=%s\\n' "$PGPASSWORD" >> "{log}"
while [ "$#" -gt 0 ]; do if [ "$1" = -f ]; then printf 'PGDMP invented dump' > "$2"; fi; shift; done
"""


class Backups(Scratch):
    def runtime(self) -> Assets:
        runtime = Assets(self.root / "runtime")
        runtime.pg_bin.mkdir(parents=True)
        tool = runtime.pg_bin / "pg_dump"
        _ = tool.write_text(FAKE_PG_DUMP.format(log=self.root / "pg_dump.log"), encoding="utf-8")
        tool.chmod(0o700)
        return runtime

    def test_backup_writes_dump_watermark_and_sidecar(self) -> None:
        settings = ensure_settings(self.where)
        mark = Watermark("d" * 40, ("e" * 64,))
        with patch("tools.memory.service.shutil.disk_usage", return_value=Usage(FLOOR * 4, FLOOR, FLOOR * 3)):
            made = backup(self.where, self.runtime(), watermark=lambda: mark, now=stamp(0))
        self.assertEqual(made.backup.dump.name, "icarus-memory-20261101T030000Z.dump")
        self.assertEqual(verify(made.backup), mark)
        for path in made.backup.members():
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        arguments = (self.root / "pg_dump.log").read_text(encoding="utf-8").splitlines()
        self.assertEqual(arguments[0], "-Fc")
        self.assertIn(str(settings.pg_port), arguments)
        self.assertNotIn(settings.pg_password, arguments[:-1], "the password goes by environment only")
        self.assertEqual(arguments[-1], f"PGPASSWORD={settings.pg_password}")
        self.assertEqual(json.loads(made.backup.watermark.read_bytes()), {"origin_main": "d" * 40, "segments": ["e" * 64]})

    def test_verify_detects_tampering_and_incomplete_sets(self) -> None:
        _ = ensure_settings(self.where)
        with patch("tools.memory.service.shutil.disk_usage", return_value=Usage(FLOOR * 4, FLOOR, FLOOR * 3)):
            made = backup(self.where, self.runtime(), now=stamp(1))
        self.assertEqual(verify(made.backup), NO_JOURNAL)
        _ = made.backup.watermark.write_bytes(b'{"origin_main":null,"segments":["' + b"f" * 64 + b'"]}\n')
        with self.assertRaisesRegex(ServiceError, "sha256 does not match"):
            _ = verify(made.backup)
        made.backup.sidecar.unlink()
        with self.assertRaisesRegex(ServiceError, "incomplete"):
            _ = verify(made.backup)
        with self.assertRaisesRegex(ServiceError, "not a memory backup name"):
            _ = verify(Backup(self.root / "other.dump"))

    def test_restore_never_manages_a_scratch_home(self) -> None:
        with self.assertRaisesRegex(ServiceError, "--drill"):
            _ = restore(self.where, Assets(self.root / "runtime"), self.root / "x.dump", replace=True)


class RestoreRollback(Backups):
    """A failure after the data directories are swapped puts the previous one back, whatever its type."""

    def test_any_replay_failure_restores_the_previous_data_directory(self) -> None:
        installed = Home(self.root / "live/config", self.root / "live/data", self.root / "live/state")
        units = self.root / "units"
        units.mkdir()
        _ = (units / UNIT).write_text("[Unit]\n", encoding="utf-8")
        settings = ensure_settings(installed)
        floor = patch("tools.memory.service.shutil.disk_usage", return_value=Usage(FLOOR * 4, FLOOR, FLOOR * 3))
        with floor:
            made = backup(installed, self.runtime(), now=stamp(2))
        calls: list[tuple[str, ...]] = []

        def systemctl(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
            del check
            calls.append(args)
            return subprocess.CompletedProcess(args, 0, "", "")

        def restore_cluster(pg0_home: Path, _dump: Path, _settings: Settings, _runtime: Assets) -> None:
            pg0_home.mkdir()
            _ = (pg0_home / "restored").write_text("the dump", encoding="utf-8")

        def noop(*_args: object, **_kwargs: object) -> None:
            return None

        def no_checks(*_args: object) -> list[object]:
            return []

        def started(_settings: Settings) -> float:
            return 0.0

        def not_ready(_api: Api) -> bool:
            return False

        def unit_files_here() -> Path:
            return units

        for failure in (HindsightError("POST /memories: HTTP 500"), BankError("bank still differs"), GitError("git rev-parse: bad"),
                        JournalError("memory/journal/x.jsonl: malformed"), KeyboardInterrupt()):
            with self.subTest(type(failure).__name__):
                for leftover in ("pg0", "pg0.previous", "pg0.restoring"):
                    shutil.rmtree(installed.data / leftover, ignore_errors=True)
                installed.pg0_home.mkdir()
                _ = (installed.pg0_home / "previous").write_text("live data", encoding="utf-8")
                calls.clear()

                def replay(_target: Connection, _mark: Watermark, failure: BaseException = failure) -> None:
                    raise failure

                with (floor, patch.object(durability, "unit_directory", unit_files_here), patch.object(durability, "systemctl", systemctl),
                      patch.object(durability, "stop_postgres", noop), patch.object(durability, "restore_cluster", restore_cluster),
                      patch.object(durability, "start_unit", started), patch.object(durability, "health", no_checks),
                      patch.object(Api, "ready", not_ready)):
                    expected = KeyboardInterrupt if isinstance(failure, KeyboardInterrupt) else ServiceError
                    with self.assertRaises(expected):
                        _ = restore(installed, Assets(self.root / "runtime"), made.backup.dump, replace=True, replay=replay)
                self.assertTrue((installed.pg0_home / "previous").is_file(), "the previous data directory is back")
                self.assertFalse((installed.data / "pg0.previous").exists())
                self.assertTrue((installed.data / "pg0.restoring/restored").is_file(), "the restored one is kept in staging")
                self.assertEqual(("start", UNIT), calls[-1], "the old unit is started again")
        self.assertEqual(settings.bank, "icarus")


class Serve(Scratch):
    def test_a_scratch_instance_refuses_a_port_in_use(self) -> None:
        with socket.socket() as taken:
            taken.bind(("127.0.0.1", 0))
            taken.listen()
            port = cast(tuple[str, int], taken.getsockname())[1]
            settings = ensure_settings(self.where, port=port, pg_port=PG_PORT + 1000 if port != PG_PORT + 1000 else PG_PORT + 1001)
            with self.assertRaisesRegex(ServiceError, f"port {port} is in use"):
                _ = durability.serve(self.where, settings, Assets(self.root / "runtime"))


class FakeBank:
    """A read-only stand-in for the REST API holding one bank."""

    def __init__(self, documents: dict[str, tuple[str, list[str]]]) -> None:
        self.documents: dict[str, tuple[str, list[str]]] = documents

    def get(self, path: str) -> dict[str, object]:
        if path.startswith("/v1/default/banks?"):
            return {"banks": [{"bank_id": "icarus"}]}
        if path.startswith("/v1/default/banks/icarus/documents?"):
            items: list[object] = [{"id": key, "content_hash": str(hash(text)), "tags": tags} for key, (text, tags) in self.documents.items()]
            return {"items": items, "total": len(items)}
        text, _tags = self.documents[path.rsplit("/", 1)[1]]
        return {"original_text": text, "document_metadata": {"segment": "zz"}}


class Drill(unittest.TestCase):
    def test_compare_finds_every_difference(self) -> None:
        live = FakeBank({"ZZ-TEST-1": ("one", ["a", "b"]), "ZZ-TEST-2": ("two", ["c"])})
        self.assertEqual(compare(live, FakeBank(dict(live.documents))), (2, []))
        _, problems = compare(live, FakeBank({"ZZ-TEST-1": ("one", ["b", "a"]), "ZZ-TEST-3": ("two", ["c"])}))
        self.assertEqual(problems, ["icarus/ZZ-TEST-2: missing from restored", "icarus/ZZ-TEST-3: missing from live"])
        _, problems = compare(live, FakeBank({"ZZ-TEST-1": ("one", ["a"]), "ZZ-TEST-2": ("TWO", ["c"])}))
        self.assertEqual(problems, ["icarus/ZZ-TEST-1: differs", "icarus/ZZ-TEST-2: differs"])


class CommandLine(unittest.TestCase):
    def test_restore_needs_a_file_or_drill(self) -> None:
        for argv in (["restore"], ["restore", "x.dump", "--drill"], ["restore", "--drill", "--replace"]):
            with redirect_stderr(io.StringIO()) as error:
                self.assertEqual(main(argv), 2)
            self.assertIn("FILE [--replace] or --drill", error.getvalue())

    def test_settings_survive_a_scratch_round_trip(self) -> None:
        with tempfile.TemporaryDirectory() as root, patch.dict(os.environ, {"ICARUS_MEMORY_HOME": root}):
            created = ensure_settings(home())
            self.assertEqual(read_settings(home()), created)
            self.assertTrue(Path(root, "config/connection.env").is_file())


if __name__ == "__main__":
    _ = unittest.main()
