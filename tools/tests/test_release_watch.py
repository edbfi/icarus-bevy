"""Release-pin tracking regressions; never runs SteamCMD."""

import json
from pathlib import Path
import tempfile
import unittest

from tools.release_watch import (Arguments, manifest_build, network_checksum, pin, pinned_build, project_version,
    public_build, render_pin)

APP_INFO = '''"2089300"
{
	"depots"
	{
		"branches"
		{
			"public"
			{
				"buildid"		"25793607"
				"timeupdated"		"1791499164"
			}
			"2.2.57"
			{
				"buildid"		"18602851"
			}
		}
	}
}
'''
FIELDS: dict[str, int | str] = {"Major": 3, "Minor": 0, "Patch": 31, "Changelist": 158478,
                                "BuildType": "Shipping", "FeatureLevel": "DangerousHorizons"}
VERSION: dict[str, object] = {"Name": "Icarus", "Version": FIELDS}


class ReleaseWatchTests(unittest.TestCase):
    def test_checksums_agree_with_the_observed_and_derived_values(self) -> None:
        self.assertEqual(network_checksum("1.0.0"), 1447163286)
        self.assertEqual(network_checksum("3.0.30.158174-rel-DangerousHorizons"), 1379612455)
        self.assertEqual(network_checksum("3.0.31.158478-rel-DangerousHorizons"), 2406501229)
        with self.assertRaises(ValueError):
            _ = network_checksum("3.0.31-ü")

    def test_public_branch_build_is_read_not_another_branch(self) -> None:
        self.assertEqual(public_build(APP_INFO), 25793607)
        with self.assertRaises(ValueError):
            _ = public_build(APP_INFO.replace('"public"', '"beta"'))

    def test_version_json_composes_the_logged_project_version(self) -> None:
        self.assertEqual(project_version(json.dumps(VERSION)), "3.0.31.158478-rel-DangerousHorizons")
        bads: list[object] = [{"Version": {**FIELDS, "Patch": True}}, {"Version": {**FIELDS, "FeatureLevel": "a b"}},
                              {"Name": "Icarus"}, [1]]
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

    def test_tracked_pin_matches_the_renderer(self) -> None:
        tracked = Path(__file__).resolve().parents[2] / "config/release.toml"
        self.assertEqual(tracked.read_text(), render_pin(pinned_build(tracked), "3.0.31.158478-rel-DangerousHorizons"))


if __name__ == "__main__":
    _ = unittest.main()
