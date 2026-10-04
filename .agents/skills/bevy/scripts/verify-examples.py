#!/usr/bin/env python3
"""Compile the actual documented Rust blocks; execute the core behavior tests.

Requires Python 3 and Cargo. Downloads crates on the first run. Rust build
artifacts are cached outside the skill; generated projects use a temporary folder.
Presentation examples are type-checked without native window/audio dependencies.
"""

import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bevy", default="0.19.1", help="Exact stable version to check")
    parser.add_argument("--core-only", action="store_true", help="Run only headless examples")
    parser.add_argument("--target-dir", type=Path, help="Reusable Cargo build cache")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", args.bevy):
        parser.error("--bevy must be an exact stable version, such as 0.19.1")

    root = Path(__file__).resolve().parents[1]
    blocks = {"core": [], "presentation": []}
    pattern = r"<!-- verify: (core|presentation) -->\s*```rust\n(.*?)\n```"
    for path in [root / "SKILL.md", *sorted((root / "references").glob("*.md"))]:
        content = path.read_text(encoding="utf-8")
        matches = list(re.finditer(pattern, content, re.S))
        if len(matches) != len(re.findall(r"^[ \t]*```(?:rust|rs)\b[^\n]*$", content, re.M)):
            raise SystemExit(f"Unmarked or malformed Rust block in {path}")
        for match in matches:
            line = content[:match.start()].count("\n") + 1
            blocks[match[1]].append((f"{path.relative_to(root)}:{line}", match[2]))

    cache = args.target_dir or Path(os.environ.get(
        "CARGO_TARGET_DIR", str(Path.home() / ".cache" / "bevy-skill-verification")
    ))
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(cache.resolve())
    features = {
        "core": ["std", "async_executor", "multi_threaded", "bevy_state"],
        "presentation": ["std", "async_executor", "multi_threaded", "bevy_state",
                         "2d_api", "3d_api", "ui_api", "bevy_pbr",
                         "bevy_world_serialization"],
    }
    groups = ["core"] if args.core_only else list(blocks)
    with tempfile.TemporaryDirectory(prefix="bevy-skill-check-") as temporary:
        for group in groups:
            if not blocks[group]:
                raise SystemExit(f"No {group} examples found")
            project = Path(temporary) / group
            (project / "src").mkdir(parents=True)
            manifest = (
                f'[package]\nname = "bevy-skill-{group}"\nversion = "0.0.0"\n'
                'edition = "2024"\npublish = false\n\n[dependencies]\n'
                f'bevy = {{ version = "={args.bevy}", default-features = false, '
                f'features = {features[group]!r} }}\n\n'
                '[profile.dev]\ndebug = 0\n[profile.test]\ndebug = 0\n'
            )
            (project / "Cargo.toml").write_text(manifest, encoding="utf-8")
            modules = ["#![allow(dead_code)]\n"]
            for index, (source, code) in enumerate(blocks[group]):
                print(f"{group}: {source}", flush=True)
                modules.append(f"// {source}\nmod example_{index} {{\n{code}\n}}\n")
            (project / "src/lib.rs").write_text("\n".join(modules), encoding="utf-8")
            command = ["cargo", "test"] if group == "core" else ["cargo", "check", "--tests"]
            subprocess.run(command + ["--manifest-path", str(project / "Cargo.toml")],
                           env=env, check=True)
    print(f"Verified Bevy {args.bevy}: " + ", ".join(
        f"{len(blocks[group])} {group} blocks" for group in groups
    ))


if __name__ == "__main__":
    main()
