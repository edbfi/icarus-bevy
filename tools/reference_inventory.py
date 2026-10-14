#!/usr/bin/env python3
"""Hash an explicitly authorized local reference tree without changing it."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
from typing import TypedDict, final


class FileRecord(TypedDict):
    path: str
    bytes: int
    sha256: str


class Inventory(TypedDict):
    schema_version: int
    files: list[FileRecord]


@final
class Arguments(argparse.Namespace):
    def __init__(self) -> None:
        super().__init__()
        self.root: Path = Path()
        self.output: Path = Path()


def collect(root: Path) -> Inventory:
    root = root.resolve(strict=True)
    if not root.is_dir():
        raise ValueError("reference root must be a directory")
    files: list[FileRecord] = []
    for directory, directories, names in os.walk(root, followlinks=False):
        base = Path(directory)
        if any((base / name).is_symlink() for name in directories + names):
            raise ValueError(f"symlinks are not supported in reference tree: {base}")
        directories.sort()
        for name in sorted(names):
            path = base / name
            if not path.is_file():
                raise ValueError(f"not a regular file: {path}")
            before = path.stat()
            with path.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            after = path.stat()
            if (before.st_size, before.st_mtime_ns) != (
                after.st_size, after.st_mtime_ns
            ):
                raise ValueError(f"reference changed during hashing: {path}")
            files.append({
                "path": path.relative_to(root).as_posix(),
                "bytes": after.st_size,
                "sha256": digest,
            })
    return {"schema_version": 1, "files": sorted(files, key=lambda row: row["path"])}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--root", type=Path, required=True)
    _ = parser.add_argument("--output", type=Path, required=True)
    args = Arguments()
    _ = parser.parse_args(namespace=args)
    root = args.root.resolve(strict=True)
    output = args.output.resolve()
    if output.is_relative_to(root):
        parser.error("output must be outside the reference tree")
    result = collect(root)
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", encoding="utf-8", dir=output.parent, delete=False
        ) as stream:
            temporary = Path(stream.name)
            json.dump(result, stream, indent=2)
            _ = stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        _ = temporary.replace(output)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    print(f"Inventoried {len(result['files'])} files into {output}")


if __name__ == "__main__":
    main()
