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
    QUALIFIED_KEY,
    release_table,
    release_values,
    render_release,
    repin,
    rewrite_release,
    verify_rewrite,
)

from tools.runtime_types import JsonObject, integer_field, json_object, parse_json, string_field

TRACKED = Path(__file__).resolve().parents[2] / "config/release.toml"


def _toml(text: str) -> JsonObject:
    return json_object(parse_json(json.dumps(tomllib.loads(text))))


def _without_table(text: str, name: str) -> str:
    """`text` without the top-level table `name` (header to the next header)."""
    start = text.index(f"\n[{name}]\n")
    following = text.find("\n[", start + 1)
    return text[:start] + (text[following:] if following >= 0 else "\n")


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
VERSION_31 = "3.0.31.158478-rel-DangerousHorizons"


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

    def test_pin_rewrites_only_the_release_table_and_records_the_qualified_build(self) -> None:
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
            old, new = _toml(before), _toml(text)
            old_build = integer_field(json_object(old["release"]), "build_id")
            old_version = string_field(json_object(old["release"]), "project_version")
            self.assertEqual(pinned_build(profile), 25793607)
            self.assertEqual(json_object(new["release"]), release_values(25793607, VERSION_31, old_build))
            self.assertIn(f"{QUALIFIED_KEY} = {old_build}\n", text)
            self.assertEqual(sorted(new), sorted(old))
            for section in old:
                if section != "release":
                    self.assertEqual(new[section], old[section], section)
            # Only the [release] key lines changed; comments and spacing stay.
            start, end = release_table(before)
            lines = before.splitlines(keepends=True)
            self.assertEqual(text, "".join(lines[:start]) + render_release(25793607, VERSION_31, old_build)
                             + "".join(lines[start + 5 : end]) + "".join(lines[end:]))

            # A second unqualified bump keeps naming the last qualified build.
            again, carried, qualified = repin(text, 7, "9.9.9.1-rel-Test")
            self.assertEqual(qualified, old_build)
            self.assertEqual(carried, [name for name in old if name != "release"])
            self.assertEqual(again.count(f"\n{QUALIFIED_KEY} = "), 1)
            self.assertEqual(json_object(_toml(again)["release"]), release_values(7, "9.9.9.1-rel-Test", old_build))
            # Re-pinning the same unqualified build changes nothing and still reports the sections.
            self.assertEqual(repin(text, 25793607, VERSION_31), (text, carried, old_build))
            # Re-pinning the build the sections are qualified for clears the key: the
            # file is byte-identical to the qualified original again.
            self.assertEqual(repin(again, old_build, old_version), (before, [], None))
            # Re-pinning a qualified build changes nothing.
            self.assertEqual(repin(before, old_build, old_version), (before, [], None))
            # One build cannot be pinned under another project version.
            with self.assertRaises(ValueError):
                _ = repin(before, old_build, "9.9.9.1-rel-Test")
            self.assertEqual(manifest_build('"buildid" "7"'), 7)

    def test_pin_keeps_comments_inside_the_release_table(self) -> None:
        tracked = TRACKED.read_text()
        start, _ = release_table(tracked)
        lines = tracked.splitlines(keepends=True)
        commented = "".join(lines[: start + 2] + ["# a note about the build\n", "\n"] + lines[start + 2 :])
        text, _, qualified = repin(commented, 7, "9.9.9.1-rel-Test")
        self.assertEqual(json_object(_toml(text)["release"]), release_values(7, "9.9.9.1-rel-Test", qualified))
        self.assertEqual(text.count("\nbuild_id = "), 1)
        self.assertEqual(text.count("\nproject_version = "), 1)
        self.assertIn("# a note about the build\n", text)
        # The table ends at the next header, not at the first comment or blank line.
        self.assertLess(text.index(f"\n{QUALIFIED_KEY} = "), text.index("\n[tags]\n"))
        back, carried, cleared = repin(text, pinned_build(TRACKED), string_field(
            json_object(_toml(tracked)["release"]), "project_version"))
        self.assertEqual((back, carried, cleared), (commented, [], None))

    def test_pin_refuses_a_file_the_server_could_not_load(self) -> None:
        bare = render_release(25793607, VERSION_31)
        tracked = TRACKED.read_text()
        cases: dict[str, tuple[str, str]] = {
            "pin only": (bare, "lacks tags, class_net_cache, row_handles"),
            "one other section": (bare + "[pawn]\nx = 1\n", "lacks tags, class_net_cache, row_handles"),
            "one required section": (bare + "[tags]\nx = 1\n", "lacks class_net_cache, row_handles"),
            "two release tables": (bare + "[tags]\nx = 1\n" + bare, "not valid TOML"),
            "unknown release key": (tracked.replace("[release]\n", "[release]\nextra = 1\n"), "unknown keys extra"),
            "malformed qualified key": (tracked.replace("[release]\n", f'[release]\n{QUALIFIED_KEY} = "1"\n'),
                                        "must be a build id"),
            "invalid TOML": (tracked + "[tags]\n", "not valid TOML"),
        }
        for name in ("tags", "class_net_cache", "row_handles"):
            cases[f"no {name}"] = (_without_table(tracked, name), f"lacks {name};")
        for label, (text, reason) in cases.items():
            with self.subTest(label), self.assertRaisesRegex(ValueError, reason):
                _ = repin(text, 7, "9.9.9.1-rel-Test")

    def test_pin_refuses_a_rewrite_it_cannot_verify(self) -> None:
        tracked = TRACKED.read_text()
        before = _toml(tracked)
        values = release_values(7, "9.9.9.1-rel-Test", 26613688)
        verify_rewrite(before, rewrite_release(tracked, values), values)
        changed = tracked.replace("ini_tag_count = 1929", "ini_tag_count = 1930")
        partial = _without_table(tracked, "row_handles")
        cases: dict[str, tuple[JsonObject, str, JsonObject, str]] = {
            "other table changed": (before, rewrite_release(changed, values), values, "other than"),
            "release not as intended": (before, rewrite_release(tracked, values),
                                        release_values(7, "9.9.9.1-rel-Test"), "intended keys"),
            "required section absent": (_toml(partial), rewrite_release(partial, values), values,
                                        "lacks row_handles;"),
            "invalid result": (before, rewrite_release(tracked, values) + "[tags]\n", values, "not valid TOML"),
        }
        for label, (old, text, intended, reason) in cases.items():
            with self.subTest(label), self.assertRaisesRegex(ValueError, reason):
                verify_rewrite(old, text, intended)
        # A key line the rewrite does not recognise yields a duplicate key: refused,
        # and the file is left untouched.
        quoted = tracked.replace("build_id = ", '"build_id" = ', 1)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "steamapps").mkdir()
            _ = (root / "steamapps/appmanifest_2089300.acf").write_text('"buildid" "7"')
            (root / "Icarus/Config").mkdir(parents=True)
            _ = (root / "Icarus/Config/version.json").write_text(json.dumps(VERSION))
            profile = root / "release.toml"
            _ = profile.write_text(quoted)
            args = Arguments()
            args.install, args.pin = str(root), str(profile)
            with patch("sys.stdout"), patch("sys.stderr"), self.assertRaises(ValueError):
                _ = pin(args)
            self.assertEqual(profile.read_text(), quoted)

    def test_pin_requires_an_existing_profile(self) -> None:
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
        self.assertEqual(release, release_values(pinned_build(TRACKED), version))
        self.assertTrue("".join(text.splitlines(keepends=True)[start:end])
                        .startswith(render_release(pinned_build(TRACKED), version)))
        # The committed pin is qualified: no section is carried over.
        self.assertNotIn(QUALIFIED_KEY, release)
        self.assertNotRegex(text, rf"(?m)^\s*{QUALIFIED_KEY}\s*=")


if __name__ == "__main__":
    _ = unittest.main()
