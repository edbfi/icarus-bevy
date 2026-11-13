"""Release-pin tracking and weekly diff regressions; never runs SteamCMD."""

import json
import hashlib
from pathlib import Path
import struct
import subprocess
import tempfile
import tomllib
import unittest
from unittest.mock import patch

from tools.release_watch import (
    Arguments,
    diff_installs,
    install_tag_counts,
    manifest_build,
    network_checksum,
    parse_ini_tags,
    pe_section_raw_sizes,
    pin,
    pinned_build,
    project_version,
    public_build,
    release_table,
    render_release,
    repin,
)

from tools.runtime_types import JsonObject, integer_field, json_object, parse_json, string_field

TRACKED = Path(__file__).resolve().parents[2] / "config/release.toml"


def _toml(text: str) -> JsonObject:
    return json_object(parse_json(json.dumps(tomllib.loads(text))))


APP_INFO = '''"2089300"
{
\t"depots"
\t{
\t\t"branches"
\t\t{
\t\t\t"public"
\t\t\t{
\t\t\t\t"buildid"\t\t"25793607"
\t\t\t\t"timeupdated"\t\t"1791499164"
\t\t\t}
\t\t\t"2.2.57"
\t\t\t{
\t\t\t\t"buildid"\t\t"18602851"
\t\t\t}
\t\t}
\t}
}
'''
FIELDS: dict[str, int | str] = {
    "Major": 3,
    "Minor": 0,
    "Patch": 31,
    "Changelist": 158478,
    "BuildType": "Shipping",
    "FeatureLevel": "DangerousHorizons",
}
VERSION: dict[str, object] = {"Name": "Icarus", "Version": FIELDS}


def _synthetic_pe_header(sections: list[tuple[bytes, int]]) -> bytes:
    e_lfanew = 0x80
    header = bytearray(e_lfanew + 24 + len(sections) * 40)
    header[0:2] = b"MZ"
    struct.pack_into("<I", header, 0x3C, e_lfanew)
    header[e_lfanew : e_lfanew + 4] = b"PE\x00\x00"
    struct.pack_into("<H", header, e_lfanew + 6, len(sections))
    struct.pack_into("<H", header, e_lfanew + 20, 0)
    table = e_lfanew + 24
    for idx, (name, raw_size) in enumerate(sections):
        entry = table + idx * 40
        header[entry : entry + len(name)] = name[:8]
        struct.pack_into("<I", header, entry + 16, raw_size)
    return bytes(header)


class ReleaseWatchTests(unittest.TestCase):
    def test_packed_tags_and_loose_ini_precedence(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pak = root / "Icarus/Content/Paks/pakchunk0-WindowsServer.pak"
            pak.parent.mkdir(parents=True)
            _ = pak.write_bytes(b"authored stand-in for the mocked pak reader")
            digest = hashlib.sha256(pak.read_bytes()).hexdigest()
            report = {"source_sha256": digest, "hashes_verified": True,
                      "declared_configuration": {"tag_names": ["Authored.One", "authored.Two"]}}
            result = subprocess.CompletedProcess(["node"], 0, json.dumps(report), "")
            with patch("tools.release_watch.subprocess.run", return_value=result) as reader:
                self.assertEqual(install_tag_counts(root), (2, 3))
                reader.assert_called_once_with(
                    ["node", str(Path(__file__).resolve().parents[1] / "pak_protocol_metadata.cjs"),
                     "--path", str(pak), "--sha256", digest],
                    capture_output=True, text=True, timeout=180, check=True,
                )
            ini = root / "Icarus/Config/DefaultGameplayTags.ini"
            ini.parent.mkdir(parents=True)
            _ = ini.write_text('+GameplayTagList=(Tag="Fixture.Only")\n')
            with patch("tools.release_watch.subprocess.run", side_effect=AssertionError("unexpected pak read")):
                self.assertEqual(install_tag_counts(root), (1, 2))

    def test_packed_tags_fail_closed_on_reader_failure_or_invalid_output(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pak = root / "Icarus/Content/Paks/pakchunk0-WindowsServer.pak"
            pak.parent.mkdir(parents=True)
            _ = pak.write_bytes(b"authored fixture")
            digest = hashlib.sha256(pak.read_bytes()).hexdigest()
            valid = {"source_sha256": digest, "hashes_verified": True,
                     "declared_configuration": {"tag_names": ["Authored.One"]}}
            bad_reports: list[object] = [
                {**valid, "source_sha256": "0" * 64},
                {**valid, "hashes_verified": False},
                {**valid, "declared_configuration": {"tag_names": "not-a-list"}},
                {**valid, "declared_configuration": {"tag_names": [3]}},
                {**valid, "declared_configuration": {"tag_names": ["Bad..Name"]}},
            ]
            for report in bad_reports:
                result = subprocess.CompletedProcess(["node"], 0, json.dumps(report), "")
                with self.subTest(report=report), patch("tools.release_watch.subprocess.run", return_value=result):
                    with self.assertRaises(ValueError):
                        _ = install_tag_counts(root)
            for error in [subprocess.CalledProcessError(1, "node"), subprocess.TimeoutExpired("node", 180)]:
                with self.subTest(error=error), patch("tools.release_watch.subprocess.run", side_effect=error):
                    with self.assertRaises(type(error)):
                        _ = install_tag_counts(root)

    def test_checksums_agree_with_the_observed_and_derived_values(self) -> None:
        self.assertEqual(network_checksum("1.0.0"), 1447163286)
        self.assertEqual(network_checksum("3.0.30.158174-rel-DangerousHorizons"), 1379612455)
        self.assertEqual(network_checksum("3.0.31.158478-rel-DangerousHorizons"), 2406501229)
        self.assertEqual(network_checksum("3.0.32.158743-rel-DangerousHorizons"), 243431684)
        with self.assertRaises(ValueError):
            _ = network_checksum("3.0.31-\u00fc")

    def test_public_branch_build_is_read_not_another_branch(self) -> None:
        self.assertEqual(public_build(APP_INFO), 25793607)
        with self.assertRaises(ValueError):
            _ = public_build(APP_INFO.replace('"public"', '"beta"'))

    def test_version_json_composes_the_logged_project_version(self) -> None:
        self.assertEqual(project_version(json.dumps(VERSION)), "3.0.31.158478-rel-DangerousHorizons")
        bads: list[object] = [
            {"Version": {**FIELDS, "Patch": True}},
            {"Version": {**FIELDS, "FeatureLevel": "a b"}},
            {"Name": "Icarus"},
            [1],
        ]
        for bad in bads:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                _ = project_version(json.dumps(bad))

    def test_pin_rewrites_only_the_release_table_and_marks_the_rest_carried_over(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "steamapps").mkdir()
            _ = (root / "steamapps/appmanifest_2089300.acf").write_text('"AppState"\n{\n\t"buildid"\t\t"25793607"\n}\n')
            (root / "Icarus/Config").mkdir(parents=True)
            _ = (root / "Icarus/Config/version.json").write_text(json.dumps(VERSION))
            profile = root / "release.toml"
            before = TRACKED.read_text()
            _ = profile.write_text(before)
            args = Arguments()
            args.install, args.pin = str(root), str(profile)
            with patch("sys.stdout"), patch("sys.stderr"):
                self.assertEqual(pin(args), 0)
            text = profile.read_text()
            self.assertEqual(pinned_build(profile), 25793607)
            start, end = release_table(text)
            self.assertEqual("".join(text.splitlines(keepends=True)[start:end]),
                             render_release(25793607, "3.0.31.158478-rel-DangerousHorizons"))
            old, new = _toml(before), _toml(text)
            self.assertEqual(sorted(new), sorted(old))
            for section in old:
                if section != "release":
                    self.assertEqual(new[section], old[section], section)
            old_build = integer_field(json_object(old["release"]), "build_id")
            self.assertIn(f"# CARRIED OVER from build {old_build} (", text)
            self.assertIn("UNQUALIFIED", text)

            # A second unqualified bump keeps naming the last qualified build.
            again, carried, qualified = repin(text, 7, "9.9.9.1-rel-Test")
            self.assertEqual(qualified, old_build)
            self.assertEqual(carried, [name for name in old if name != "release"])
            self.assertEqual(again.count("# CARRIED OVER"), 1)
            self.assertEqual(integer_field(json_object(_toml(again)["release"]), "build_id"), 7)
            # Re-pinning the same build changes nothing.
            self.assertEqual(repin(text, 25793607, "3.0.31.158478-rel-DangerousHorizons"), (text, [], None))
            self.assertEqual(manifest_build('"buildid" "7"'), 7)

    def test_pin_refuses_a_file_the_server_could_not_load(self) -> None:
        bare = render_release(25793607, "3.0.31.158478-rel-DangerousHorizons")
        with self.assertRaises(ValueError):
            _ = repin(bare, 7, "9.9.9.1-rel-Test")
        with self.assertRaises(ValueError):
            _ = repin(bare + "[tags]\nx = 1\n" + bare, 7, "9.9.9.1-rel-Test")
        with tempfile.TemporaryDirectory() as directory, self.assertRaises(FileNotFoundError):
            args = Arguments()
            args.install, args.pin = directory, str(Path(directory) / "missing.toml")
            (Path(directory) / "steamapps").mkdir()
            _ = (Path(directory) / "steamapps/appmanifest_2089300.acf").write_text('"buildid" "7"')
            (Path(directory) / "Icarus/Config").mkdir(parents=True)
            _ = (Path(directory) / "Icarus/Config/version.json").write_text(json.dumps(VERSION))
            _ = pin(args)

    def test_pe_section_and_ini_tag_diff_between_installs(self) -> None:
        pe_old = _synthetic_pe_header([(b".text", 100_000), (b".pdata", 10_000)])
        pe_new = _synthetic_pe_header([(b".text", 51_872), (b".pdata", 8_080)])
        self.assertEqual(pe_section_raw_sizes(pe_old), {".text": 100_000, ".pdata": 10_000})
        with self.assertRaises(ValueError):
            _ = pe_section_raw_sizes(b"not-a-pe")

        ini_old = '+GameplayTagList=(Tag="AI.Boss.Mammoth",DevComment="")\n'
        ini_new = ini_old + '+GameplayTagList=(Tag="Affliction.Frostbite",DevComment="")\n'
        self.assertEqual(parse_ini_tags(ini_old), (1, 3))
        self.assertEqual(parse_ini_tags(ini_new), (2, 5))

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            older = root / "older"
            newer = root / "newer"
            for target, build_id, patch, cl, pe_bytes, ini_text in [
                (older, 25793607, 31, 158478, pe_old, ini_old),
                (newer, 25956137, 32, 158743, pe_new, ini_new),
            ]:
                (target / "steamapps").mkdir(parents=True)
                _ = (target / "steamapps/appmanifest_2089300.acf").write_text(
                    f'"AppState"\n{{\n\t"buildid"\t\t"{build_id}"\n}}\n'
                )
                (target / "Icarus/Config").mkdir(parents=True)
                ver = {"Name": "Icarus", "Version": {**FIELDS, "Patch": patch, "Changelist": cl}}
                _ = (target / "Icarus/Config/version.json").write_text(json.dumps(ver))
                _ = (target / "Icarus/Config/DefaultGameplayTags.ini").write_text(ini_text)
                (target / "Icarus/Binaries/Win64").mkdir(parents=True)
                _ = (target / "Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe").write_bytes(pe_bytes)

            report = diff_installs(older, newer)
            self.assertEqual(report["build_changed"], True)
            self.assertEqual(report["checksum_changed"], True)
            self.assertEqual(report["ini_tag_delta"], 1)
            self.assertEqual(report["ini_expanded_node_delta"], 2)
            self.assertEqual(report["pe_section_raw_size_deltas"], {".pdata": -1920, ".text": -48128})

            (older / "Icarus/Config/DefaultGameplayTags.ini").unlink()
            with self.assertRaises(FileNotFoundError):
                _ = diff_installs(older, newer)

            (older / "Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe").unlink()
            with self.assertRaises(FileNotFoundError):
                _ = diff_installs(older, newer)

    def test_tracked_pin_matches_the_renderer_and_is_qualified(self) -> None:
        text = TRACKED.read_text()
        release = json_object(_toml(text)["release"])
        version = string_field(release, "project_version")
        self.assertEqual(release["network_checksum"], network_checksum(version))
        start, end = release_table(text)
        self.assertEqual("".join(text.splitlines(keepends=True)[start:end]),
                         render_release(pinned_build(TRACKED), version))
        # The committed pin is qualified: no section is carried over.
        self.assertNotIn("# CARRIED OVER", text)


if __name__ == "__main__":
    _ = unittest.main()
