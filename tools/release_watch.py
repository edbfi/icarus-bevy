#!/usr/bin/env python3
"""Track the official ICARUS release that config/release.toml pins.

`check` asks Steam (anonymous SteamCMD app info, nothing downloaded) for the
dedicated server's public build and compares it with the pin; exit status 3
means the pin is behind. `pin --install DIR` rewrites the pin from a fetched
install's app manifest and Icarus/Config/version.json. The project version is
composed as both observed server builds log it (F-683):
`<major>.<minor>.<patch>.<changelist>-rel-<feature level>`.
"""

import argparse
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import zlib

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.runtime_types import json_object, parse_json

APP_ID = 2089300
PIN = Path(__file__).resolve().parents[1] / "config/release.toml"
EXIT_BEHIND = 3


class Arguments(argparse.Namespace):
    command: str = ""
    steamcmd: str = "steamcmd"
    install: str = ""
    pin: str = str(PIN)


def network_checksum(project_version: str) -> int:
    """The engine's default Hello version: CRC-32 of the ASCII-lowercased
    version string, each UTF-16 unit as four little-endian bytes (F-541)."""
    if not project_version.isascii():
        raise ValueError("project_version must be ASCII")
    text = f"Icarus {project_version}, NetCL: 0, EngineNetVer: 17, GameNetVer: 0"
    lowered = "".join(chr(ord(c) + 32) if "A" <= c <= "Z" else c for c in text)
    return zlib.crc32(lowered.encode("utf-32-le"))


def public_build(app_info: str) -> int:
    """The public branch's build ID in SteamCMD app_info_print output."""
    branches = app_info.find('"branches"')
    match = re.search(r'"public"\s*\{\s*"buildid"\s+"([0-9]+)"', app_info[branches:]) if branches >= 0 else None
    if match is None:
        raise ValueError("no public branch build in app info")
    return int(match[1])


def manifest_build(manifest: str) -> int:
    match = re.search(r'"buildid"\s+"([0-9]+)"', manifest)
    if match is None:
        raise ValueError("no buildid in app manifest")
    return int(match[1])


def project_version(version_json: str) -> str:
    version = json_object(json_object(parse_json(version_json)).get("Version", None))
    parts: list[str] = []
    for key in ("Major", "Minor", "Patch", "Changelist"):
        value = version.get(key)
        if not isinstance(value, int) or isinstance(value, bool) or value < 0:
            raise ValueError(f"Version.{key} must be a nonnegative integer")
        parts.append(str(value))
    level = version.get("FeatureLevel")
    if not isinstance(level, str) or re.fullmatch(r"[A-Za-z0-9]+", level) is None:
        raise ValueError("Version.FeatureLevel must be alphanumeric")
    return ".".join(parts) + "-rel-" + level


def render_pin(build_id: int, version: str) -> str:
    return ("# The official release this repository targets. ICARUS ships weekly; move this\n"
            + "# pin with `python3 tools/release_watch.py pin --install <fetched install>`\n"
            + '# (docs/architecture.md, "Release tracking"). The checksum is derived and checked.\n'
            + f"[release]\napp_id = {APP_ID}\nbuild_id = {build_id}\n"
            + f'project_version = "{version}"\nnetwork_checksum = {network_checksum(version)}\n')


def pinned_build(pin: Path) -> int:
    release = json_object(json_object(parse_json(json.dumps(tomllib.loads(pin.read_text())))).get("release", None))
    build = release.get("build_id")
    if not isinstance(build, int) or isinstance(build, bool):
        raise ValueError("pin build_id must be an integer")
    return build


def check(args: Arguments) -> int:
    result = subprocess.run([args.steamcmd, "+login", "anonymous", "+app_info_update", "1",
                             "+app_info_print", str(APP_ID), "+quit"],
                            capture_output=True, text=True, timeout=300, check=True)
    live, pinned = public_build(result.stdout), pinned_build(Path(args.pin))
    print(json.dumps({"app_id": APP_ID, "pinned_build": pinned, "live_build": live, "behind": live != pinned}))
    return EXIT_BEHIND if live != pinned else 0


def pin(args: Arguments) -> int:
    install = Path(args.install)
    build = manifest_build((install / f"steamapps/appmanifest_{APP_ID}.acf").read_text())
    version = project_version((install / "Icarus/Config/version.json").read_text())
    _ = Path(args.pin).write_text(render_pin(build, version))
    print(json.dumps({"build_id": build, "project_version": version, "network_checksum": network_checksum(version)}))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    checking = sub.add_parser("check")
    _ = checking.add_argument("--steamcmd", default="steamcmd")
    _ = checking.add_argument("--pin", default=str(PIN))
    pinning = sub.add_parser("pin")
    _ = pinning.add_argument("--install", required=True)
    _ = pinning.add_argument("--pin", default=str(PIN))
    args = parser.parse_args(namespace=Arguments())
    return check(args) if args.command == "check" else pin(args)


if __name__ == "__main__":
    sys.exit(main())
