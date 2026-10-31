"""Connected launcher binding and launch-shape regressions; never invokes Docker."""

import datetime
import json
from pathlib import Path
import stat
import tempfile
import unittest

from unittest.mock import patch

from tools.run_reference_connected import (CAPTURE_FILTER, DIRECT_ENGINE_INI, ENGINE_INI, GAME_PORT, QUERY_PORT,
    ApprovedRun, Arguments, capture_launch, prepare_launch, returned_to_lobby, server_settings, validate_session)
from tools.run_reference_offline import IMAGE, REFERENCE_EXE, digest
from tools.runtime_types import JsonObject, json_object, parse_json

ROOT = Path(__file__).resolve().parents[2]


class Fixture:
    def __init__(self, directory: str) -> None:
        self.root: Path = Path(directory).resolve()
        self.case: Path = self.root / "work/connected-case"
        self.case.mkdir(parents=True)
        self.scope: Path = self.case / "scope.md"
        _ = self.scope.write_text("## auth\n- status: granted\n## network_profile\n- mode: connected\n"
                                 + "## signoff\n- ready_for_act: true\n")
        self.reference: Path = self.root / "icarus-server-123" / REFERENCE_EXE
        self.reference.parent.mkdir(parents=True)
        _ = self.reference.write_bytes(b"authored non-executable reference fixture")
        self.save: Path = self.case / "inputs" / "authored_prospect.json"
        self.save.parent.mkdir()
        _ = self.save.write_bytes(b'{"authored":true}')
        self.now: datetime.datetime = datetime.datetime.now(datetime.UTC)
        self.session: JsonObject = {
            "scope": "reference-connected", "authorization_source": "authored test authorization",
            "case_root": str(self.case), "scope_sha256": digest(self.scope), "image": IMAGE,
            "reference_root": str(self.root / "icarus-server-123"), "reference_executable": str(self.reference),
            "reference_sha256": digest(self.reference), "save_input": str(self.save),
            "save_input_sha256": digest(self.save), "scenario_id": "s1",
            "shutdown_if_not_joined_seconds": 900, "shutdown_if_empty_seconds": 60, "max_players": 1,
            "observation_seconds": 1800, "run_timeout_seconds": 1920, "max_runs": 2,
            "start_utc": (self.now - datetime.timedelta(seconds=1)).isoformat(),
            "end_utc": (self.now + datetime.timedelta(hours=2)).isoformat(),
        }
        self.path: Path = self.case / "session.json"
        self.args: Arguments = Arguments()
        self.args.case_root = str(self.case)
        self.args.session = str(self.path)
        self.args.run_name = "authored"
        self.write()

    def write(self, update: JsonObject | None = None) -> None:
        _ = self.path.write_text(json.dumps({**self.session, **(update or {})}))

    def approve(self) -> ApprovedRun:
        return validate_session(self.root, self.args, self.now)


class ConnectedTests(unittest.TestCase):
    def test_session_binds_a_fetched_install_and_connected_scope(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            approved = fixture.approve()
            self.assertEqual((approved.scenario, approved.max_players), ("s1", 1))
            other = fixture.root / "icarus-orig" / REFERENCE_EXE
            other.parent.mkdir(parents=True)
            _ = other.write_bytes(b"authored")
            updates: list[JsonObject] = [
                {"scope": "reference-offline"}, {"authorization_source": " "}, {"image": "other"},
                {"reference_root": str(fixture.root / "icarus-orig"), "reference_executable": str(other),
                 "reference_sha256": digest(other)},
                {"reference_sha256": "0" * 64}, {"save_input_sha256": "0" * 64}, {"scenario_id": "S 1"},
                {"observation_seconds": 59}, {"observation_seconds": 2701}, {"run_timeout_seconds": 1919},
                {"run_timeout_seconds": 2821}, {"shutdown_if_not_joined_seconds": 29},
                {"shutdown_if_empty_seconds": 601}, {"max_players": 9}, {"max_runs": 5},
                {"end_utc": (fixture.now + datetime.timedelta(seconds=1900)).isoformat()},
            ]
            for update in updates:
                with self.subTest(update=update):
                    fixture.write(update)
                    with self.assertRaises(ValueError):
                        _ = fixture.approve()
            fixture.write()
            _ = fixture.scope.write_text(fixture.scope.read_text().replace("connected", "offline"))
            fixture.write({"scope_sha256": digest(fixture.scope)})
            with self.assertRaisesRegex(ValueError, "network_profile"):
                _ = fixture.approve()

    def test_launch_publishes_only_the_game_and_query_ports(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            steam = fixture.root / "steamcmd"
            for bits in ("32", "64"):
                (steam / f"linux{bits}").mkdir(parents=True)
                _ = (steam / f"linux{bits}/steamclient.so").write_bytes(f"authored {bits}".encode())
            with patch("tools.run_reference_connected.STEAMCLIENT_ROOT", new=steam):
                argv = prepare_launch(ROOT, approved, run, "icarus-connected-authored", "token", "abcd1234")
            self.assertEqual((run / "home/.steam/sdk64/steamclient.so").read_bytes(), b"authored 64")
            self.assertEqual(argv[argv.index("--network") + 1], "bridge")
            published = [argv[i + 1] for i, value in enumerate(argv) if value == "--publish"]
            self.assertEqual(published, [f"{GAME_PORT}:{GAME_PORT}/udp", f"{QUERY_PORT}:{QUERY_PORT}/udp"])
            self.assertIn(f"type=bind,src={approved.reference},dst=/reference,readonly", argv)
            self.assertIn("-LoadProspect=authored_prospect", argv[-1])
            self.assertIn('-SteamServerName="icarus-research-s1"', argv[-1])
            settings = (run / "home/Icarus/Saved/Config/WindowsServer/ServerSettings.ini").read_text()
            self.assertEqual(settings, server_settings(approved, "abcd1234"))
            self.assertIn("JoinPassword=abcd1234\nMaxPlayers=1\nResumeProspect=False\n", settings)
            secret = run / "join-password.txt"
            self.assertEqual(stat.S_IMODE(secret.stat().st_mode), 0o600)
            record = json_object(parse_json((run / "invocation.json").read_text()))
            self.assertNotIn("abcd1234", json.dumps(record))
            self.assertEqual(record["save_copy_sha256"], digest(fixture.save))

    def test_capture_binding_needs_a_local_image_id_and_a_bounded_cap(self) -> None:
        image = "sha256:" + "a" * 64
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            self.assertIsNone(fixture.approve().capture_image)
            fixture.write({"capture_image": image, "capture_max_bytes": 64 * 1024**2})
            approved = fixture.approve()
            self.assertEqual((approved.capture_image, approved.capture_max_bytes), (image, 64 * 1024**2))
            updates: list[JsonObject] = [
                {"capture_image": image}, {"capture_max_bytes": 64 * 1024**2},
                {"capture_image": "alpine:latest", "capture_max_bytes": 64 * 1024**2},
                {"capture_image": "sha256:" + "A" * 64, "capture_max_bytes": 64 * 1024**2},
                {"capture_image": image, "capture_max_bytes": 1024**2 - 1},
                {"capture_image": image, "capture_max_bytes": 2 * 1024**3 + 1},
            ]
            for update in updates:
                with self.subTest(update=update):
                    fixture.write(update)
                    with self.assertRaises(ValueError):
                        _ = fixture.approve()

    def test_capture_sidecar_shares_only_the_server_namespace_unprivileged(self) -> None:
        image = "sha256:" + "b" * 64
        server = "c" * 64
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            fixture.write({"capture_image": image, "capture_max_bytes": 64 * 1024**2})
            approved = fixture.approve()
            run = fixture.case / "run"
            run.mkdir()
            argv = capture_launch(approved, run, "icarus-connected-authored-capture", "token", server)
            self.assertTrue((run / "capture").is_dir())
            self.assertEqual(stat.S_IMODE((run / "capture").stat().st_mode), 0o777)
            self.assertEqual(argv[argv.index("--network") + 1], "container:" + server)
            self.assertNotIn("--publish", argv)
            self.assertNotIn("--privileged", argv)
            caps = [argv[i + 1] for i, value in enumerate(argv) if value == "--cap-add"]
            self.assertEqual(sorted(caps), ["NET_RAW", "SETGID", "SETUID"])
            self.assertEqual(argv[argv.index("--cap-drop") + 1], "ALL")
            self.assertIn("--read-only", argv)
            self.assertIn(f"type=bind,src={run / 'capture'},dst=/capture", argv)
            self.assertEqual(argv[argv.index("--label") + 1], "icarus.research.run=icarus-connected-authored-capture")
            command = argv[argv.index(image):]
            self.assertEqual(command[1], "tcpdump")
            self.assertEqual(command[command.index("-Z") + 1], "capture")
            self.assertEqual(command[-1], CAPTURE_FILTER)
            self.assertEqual(CAPTURE_FILTER, f"udp port {GAME_PORT} or udp port {QUERY_PORT}")
            with self.assertRaises(ValueError):
                _ = capture_launch(approved, fixture.case / "other", "name", "token", "not-an-id")
            fixture.write()
            with self.assertRaises(ValueError):
                _ = capture_launch(fixture.approve(), fixture.case / "other", "name", "token", server)

    def test_direct_connection_method_writes_only_the_steam_networking_switches(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(directory)
            steam = fixture.root / "steamcmd"
            for bits in ("32", "64"):
                (steam / f"linux{bits}").mkdir(parents=True)
                _ = (steam / f"linux{bits}/steamclient.so").write_bytes(b"authored")
            for method, written in (("default", False), ("direct", True)):
                with self.subTest(method=method):
                    fixture.write({"connection_method": method})
                    approved = fixture.approve()
                    run = fixture.case / ("run-" + method)
                    run.mkdir()
                    with patch("tools.run_reference_connected.STEAMCLIENT_ROOT", new=steam):
                        _ = prepare_launch(ROOT, approved, run, "icarus-connected-authored", "token", "abcd1234")
                    engine = run / "home" / ENGINE_INI
                    self.assertEqual(engine.is_file(), written)
                    if written:
                        self.assertEqual(engine.read_text(), DIRECT_ENGINE_INI)
                    record = json_object(parse_json((run / "invocation.json").read_text()))
                    self.assertEqual(record["engine_ini_sha256"] is not None, written)
            fixture.write({"connection_method": "p2p"})
            with self.assertRaises(ValueError):
                _ = fixture.approve()

    def test_lobby_return_needs_load_then_save_then_lobby(self) -> None:
        load, save, lobby = ("Prospect file loaded. Loading map...", "-------  Saving and shutting down  -------",
                             "LogLoad: Took 6.4 seconds to LoadMap(/Game/Maps/DedicatedServerEntry)")
        startup_lobby = "LogLoad: Took 1.0 seconds to LoadMap(/Game/Maps/DedicatedServerEntry)"
        self.assertTrue(returned_to_lobby("\n".join([startup_lobby, load, save, lobby])))
        self.assertFalse(returned_to_lobby("\n".join([startup_lobby, load])))
        self.assertFalse(returned_to_lobby("\n".join([startup_lobby, load, save])))
        self.assertFalse(returned_to_lobby("\n".join([lobby, save, load])))


if __name__ == "__main__":
    _ = unittest.main()
