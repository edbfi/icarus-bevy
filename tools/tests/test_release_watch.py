"""Release-pin tracking and weekly diff regressions; never runs SteamCMD."""

import json
from pathlib import Path
import struct
import tempfile
import unittest

from tools.release_watch import (
    Arguments,
    diff_installs,
    manifest_build,
    network_checksum,
    parse_ini_tags,
    pe_section_raw_sizes,
    pin,
    pinned_build,
    project_version,
    public_build,
    render_pin,
)

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

    def test_pin_rewrites_the_file_from_a_fetched_install(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "steamapps").mkdir()
            _ = (root / "steamapps/appmanifest_2089300.acf").write_text('"AppState"\n{\n\t"buildid"\t\t"25793607"\n}\n')
            (root / "Icarus/Config").mkdir(parents=True)
            _ = (root / "Icarus/Config/version.json").write_text(json.dumps(VERSION))
            args = Arguments()
            args.install, args.pin = str(root), str(root / "release.toml")
            self.assertEqual(pin(args), 0)
            text = (root / "release.toml").read_text()
            self.assertEqual(text, render_pin(25793607, "3.0.31.158478-rel-DangerousHorizons"))
            self.assertEqual(pinned_build(root / "release.toml"), 25793607)
            self.assertEqual(manifest_build('"buildid" "7"'), 7)

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

    def test_tracked_pin_matches_the_renderer(self) -> None:
        tracked = Path(__file__).resolve().parents[2] / "config/release.toml"
        self.assertEqual(tracked.read_text(), render_pin(pinned_build(tracked), "3.0.32.158743-rel-DangerousHorizons"))


if __name__ == "__main__":
    _ = unittest.main()
