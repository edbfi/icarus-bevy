#!/usr/bin/env python3
"""Track, inspect and diff the official ICARUS release that config/release.toml pins.

`check` asks Steam (anonymous SteamCMD app info, nothing downloaded) for the
dedicated server's public build and compares it with the pin; exit status 3
means the pin is behind. `pin --install DIR` rewrites only the `[release]`
table of the complete profile from a fetched install's app manifest and
Icarus/Config/version.json; every other section is carried over, marked in the
file and in the output as unqualified for the new build. `diff --older DIR
--newer DIR` compares two fetched server installs (manifest build ID,
project_version, network_checksum, PE section sizes and DefaultGameplayTags.ini
tag counts) so weekly updates start with a mechanical intake and diff report.
The project version is composed as both observed server builds log it (F-683):
`<major>.<minor>.<patch>.<changelist>-rel-<feature level>`.
"""

import argparse
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
import tomllib
import zlib

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from tools.runtime_types import JsonObject, JsonValue, json_object, parse_json

APP_ID = 2089300
PIN = Path(__file__).resolve().parents[1] / "config/release.toml"
EXIT_BEHIND = 3
SHIPPING_EXE_REL = Path("Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe")
DEFAULT_TAGS_INI_REL = Path("Icarus/Config/DefaultGameplayTags.ini")
DEFAULT_TAGS_PAK_REL = Path("Icarus/Content/Paks/pakchunk0-WindowsServer.pak")
PAK_METADATA_READER = Path(__file__).resolve().with_name("pak_protocol_metadata.cjs")


class Arguments(argparse.Namespace):
    command: str = ""
    steamcmd: str = "steamcmd"
    install: str = ""
    older: str = ""
    newer: str = ""
    pin: str = str(PIN)


@dataclass(frozen=True)
class InstallProfile:
    build_id: int
    project_version: str
    network_checksum: int
    pe_sections: dict[str, int]
    ini_tag_count: int
    ini_expanded_node_count: int


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


def pe_section_raw_sizes(pe_header: bytes) -> dict[str, int]:
    """Parse section headers from a PE/COFF image prefix and return each
    section's SizeOfRawData keyed by section name."""
    if len(pe_header) < 0x40 or pe_header[:2] != b"MZ":
        raise ValueError("not a valid MZ PE header")
    e_lfanew = int.from_bytes(pe_header[0x3C:0x40], "little")
    if e_lfanew < 0x40 or e_lfanew + 24 > len(pe_header):
        raise ValueError("PE signature offset out of bounds")
    if pe_header[e_lfanew : e_lfanew + 4] != b"PE\x00\x00":
        raise ValueError("missing PE\\0\\0 signature")
    num_sections = int.from_bytes(pe_header[e_lfanew + 6 : e_lfanew + 8], "little")
    opt_header_size = int.from_bytes(pe_header[e_lfanew + 20 : e_lfanew + 22], "little")
    section_table = e_lfanew + 24 + opt_header_size
    if section_table + num_sections * 40 > len(pe_header):
        raise ValueError("PE section table truncated")
    sections: dict[str, int] = {}
    for idx in range(num_sections):
        entry = section_table + idx * 40
        raw_name = pe_header[entry : entry + 8].split(b"\x00", 1)[0]
        name = raw_name.decode("ascii", errors="replace")
        raw_size = int.from_bytes(pe_header[entry + 16 : entry + 20], "little")
        sections[name] = raw_size
    return sections


def parse_ini_tags(ini_text: str) -> tuple[int, int]:
    """Count declared GameplayTagList tags in DefaultGameplayTags.ini and the
    number of unique ASCII-case-folded nodes after dot-prefix expansion."""
    declared = [match[1] for match in re.finditer(r'Tag="([^"]+)"', ini_text)]
    return count_tag_nodes(declared)


def count_tag_nodes(declared: list[str]) -> tuple[int, int]:
    expanded: set[str] = set()
    for tag in declared:
        parts = tag.split(".")
        for end in range(1, len(parts) + 1):
            expanded.add(".".join(parts[:end]).lower())
    return len(declared), len(expanded)


def install_tag_counts(install: Path) -> tuple[int, int]:
    """Use a loose INI when present, otherwise inspect its packed member.

    The existing pak reader verifies the sampled digest before and after the
    read and only exposes protocol configuration, never gameplay table values.
    """
    ini_path = install / DEFAULT_TAGS_INI_REL
    if ini_path.is_file():
        return parse_ini_tags(ini_path.read_text(encoding="utf-8", errors="replace"))
    pak_path = install / DEFAULT_TAGS_PAK_REL
    with pak_path.open("rb") as handle:
        digest = hashlib.file_digest(handle, "sha256").hexdigest()
    result = subprocess.run(
        ["node", str(PAK_METADATA_READER), "--path", str(pak_path), "--sha256", digest],
        capture_output=True, text=True, timeout=180, check=True,
    )
    report = json_object(parse_json(result.stdout))
    if report.get("source_sha256") != digest or report.get("hashes_verified") is not True:
        raise ValueError("packed gameplay tags source verification failed")
    configuration = json_object(report.get("declared_configuration"))
    values = configuration.get("tag_names")
    if not isinstance(values, list):
        raise ValueError("packed gameplay tags must be a list")
    tags: list[str] = []
    for value in values:
        if not isinstance(value, str) or not value.isascii() or not all(value.split(".")):
            raise ValueError("packed gameplay tag must be a nonempty ASCII dotted name")
        tags.append(value)
    return count_tag_nodes(tags)


def inspect_install(install: Path) -> InstallProfile:
    build = manifest_build((install / f"steamapps/appmanifest_{APP_ID}.acf").read_text())
    version = project_version((install / "Icarus/Config/version.json").read_text())
    exe_path = install / SHIPPING_EXE_REL
    if not exe_path.is_file():
        raise FileNotFoundError(f"missing shipping executable: {exe_path}")
    with exe_path.open("rb") as handle:
        sections = pe_section_raw_sizes(handle.read(4096))
    ini_tags, ini_nodes = install_tag_counts(install)
    return InstallProfile(
        build_id=build,
        project_version=version,
        network_checksum=network_checksum(version),
        pe_sections=sections,
        ini_tag_count=ini_tags,
        ini_expanded_node_count=ini_nodes,
    )


def diff_installs(older: Path, newer: Path) -> JsonObject:
    old_prof = inspect_install(older)
    new_prof = inspect_install(newer)
    all_sections = sorted(set(old_prof.pe_sections) | set(new_prof.pe_sections))
    section_deltas: JsonObject = {}
    for name in all_sections:
        old_size = old_prof.pe_sections.get(name, 0)
        new_size = new_prof.pe_sections.get(name, 0)
        section_deltas[name] = new_size - old_size
    return {
        "older": {
            "build_id": old_prof.build_id,
            "project_version": old_prof.project_version,
            "network_checksum": old_prof.network_checksum,
            "ini_tag_count": old_prof.ini_tag_count,
            "ini_expanded_node_count": old_prof.ini_expanded_node_count,
        },
        "newer": {
            "build_id": new_prof.build_id,
            "project_version": new_prof.project_version,
            "network_checksum": new_prof.network_checksum,
            "ini_tag_count": new_prof.ini_tag_count,
            "ini_expanded_node_count": new_prof.ini_expanded_node_count,
        },
        "build_changed": old_prof.build_id != new_prof.build_id,
        "checksum_changed": old_prof.network_checksum != new_prof.network_checksum,
        "ini_tag_delta": new_prof.ini_tag_count - old_prof.ini_tag_count,
        "ini_expanded_node_delta": new_prof.ini_expanded_node_count - old_prof.ini_expanded_node_count,
        "pe_section_raw_size_deltas": section_deltas,
    }


CARRIED_OVER_PREFIX = "# CARRIED OVER from build "


def render_release(build_id: int, version: str) -> str:
    """The `[release]` table for one build; the checksum is derived."""
    return (f"[release]\napp_id = {APP_ID}\nbuild_id = {build_id}\n"
            + f'project_version = "{version}"\nnetwork_checksum = {network_checksum(version)}\n')


def release_table(text: str) -> tuple[int, int]:
    """Line span `[start, end)` of the `[release]` header and its key lines."""
    lines = text.splitlines(keepends=True)
    headers = [index for index, line in enumerate(lines) if line.strip() == "[release]"]
    if len(headers) != 1:
        raise ValueError("release profile needs exactly one [release] table")
    end = headers[0] + 1
    while end < len(lines) and lines[end].strip() and not lines[end].lstrip().startswith(("#", "[")):
        end += 1
    return headers[0], end


def carried_over_marker(build_id: int, version: str) -> str:
    return (f"{CARRIED_OVER_PREFIX}{build_id} ({version}): every section below\n"
            + "# was qualified for that build and is UNQUALIFIED for this [release].\n"
            + "# Qualify each section for this build, then delete this comment.\n")


def carried_over_from(text: str) -> int | None:
    """The last qualified build a carried-over marker names, if any."""
    match = re.search(r"^" + re.escape(CARRIED_OVER_PREFIX) + r"([0-9]+) ", text, re.MULTILINE)
    return int(match[1]) if match is not None else None


def repin(text: str, build_id: int, version: str) -> tuple[str, list[str], int | None]:
    """Replace the `[release]` table of a complete profile in place.

    Returns the new text, the carried-over section names and the build they
    were last qualified for (`None` when the build is unchanged). Sections are
    never dropped, so the server can still load the file; a marker comment
    after `[release]` names the build they came from. An existing marker keeps
    naming the last qualified build across repeated unqualified bumps.
    """
    parsed = json_object(parse_json(json.dumps(tomllib.loads(text))))
    previous = json_object(parsed.get("release", None))
    sections = [name for name in parsed if name != "release"]
    if not sections:
        raise ValueError("release profile has only [release]; the server cannot load it")
    old_build, old_version = previous.get("build_id"), previous.get("project_version")
    if not isinstance(old_build, int) or isinstance(old_build, bool) or not isinstance(old_version, str):
        raise ValueError("pinned [release] needs build_id and project_version")
    lines = text.splitlines(keepends=True)
    start, end = release_table(text)
    head, rest = "".join(lines[:start]), "".join(lines[end:])
    if old_build == build_id and old_version == version:
        return text, [], None
    qualified = carried_over_from(text)
    if qualified is None:
        rest = "\n" + carried_over_marker(old_build, old_version) + rest
        qualified = old_build
    return head + render_release(build_id, version) + rest, sections, qualified


def pinned_build(pin_path: Path) -> int:
    release = json_object(json_object(parse_json(json.dumps(tomllib.loads(pin_path.read_text())))).get("release", None))
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
    path = Path(args.pin)
    text, carried, qualified = repin(path.read_text(), build, version)
    _ = path.write_text(text)
    report: JsonObject = {"build_id": build, "project_version": version, "network_checksum": network_checksum(version),
                          "carried_over_sections": list[JsonValue](carried), "qualified_for_build": qualified}
    print(json.dumps(report))
    if carried:
        print(f"release_watch: only [release] now names build {build}; sections {', '.join(carried)} are carried "
              + f"over from build {qualified} and are UNQUALIFIED for build {build}. Qualify them before relying "
              + "on this pin.", file=sys.stderr)
    return 0


def diff_cmd(args: Arguments) -> int:
    report = diff_installs(Path(args.older), Path(args.newer))
    print(json.dumps(report))
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
    diffing = sub.add_parser("diff")
    _ = diffing.add_argument("--older", required=True)
    _ = diffing.add_argument("--newer", required=True)
    args = parser.parse_args(namespace=Arguments())
    if args.command == "check":
        return check(args)
    if args.command == "diff":
        return diff_cmd(args)
    return pin(args)


if __name__ == "__main__":
    sys.exit(main())
