# SPDX-License-Identifier: AGPL-3.0-or-later
"""Read Git snapshots and write portable evidence catalogs."""

from __future__ import annotations

import re
import subprocess
from collections.abc import Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

from tools.docs_lint import parse_json_object
from tools.evidence_catalog_model import (
    CatalogError,
    Document,
    as_list,
    as_object,
    as_text,
    digest,
    expect_keys,
    json_line,
    source_path,
    topic,
)

AUTHORITY = "evidence-catalog"
MANIFEST = "manifest.json"
SCHEMA = 1
COMMIT = re.compile(r"(?:[0-9a-f]{40}|[0-9a-f]{64})")
SHA256 = re.compile(r"[0-9a-f]{64}")
LEGACY_ROOTS = ("docs/ledgers", "evidence")


def git(root: Path, *args: str) -> bytes:
    result = subprocess.run(
        ["git", "--no-replace-objects", "-C", str(root), *args],
        capture_output=True, check=False,
    )
    if result.returncode:
        raise CatalogError(f"git {args[0]}: {result.stderr.decode(errors='replace').strip()}")
    return result.stdout


def commit_id(root: Path, rev: str) -> str:
    return git(root, "rev-parse", "--verify", "--end-of-options", f"{rev}^{{commit}}").decode().strip()


def tree_files(root: Path, rev: str, paths: Iterable[str]) -> dict[str, str]:
    files: dict[str, str] = {}
    for item in git(root, "ls-tree", "-r", "-z", rev, "--", *paths).decode().split("\0"):
        if item:
            metadata, path = item.split("\t", 1)
            mode, _kind, _oid = metadata.split()
            files[path] = mode
    return files


def read_blobs(root: Path, rev: str, paths: Mapping[str, str]) -> dict[str, bytes]:
    files: dict[str, bytes] = {}
    for path, mode in sorted(paths.items()):
        if mode != "100644":
            raise CatalogError(f"{path}: expected a regular, non-executable Git blob")
        files[path] = git(root, "show", f"{rev}:{path}")
    return files


def legacy_files(root: Path, rev: str) -> dict[str, bytes]:
    paths = tree_files(root, rev, LEGACY_ROOTS)
    for path in paths:
        if topic(path) is None and path != "evidence/README.md":
            raise CatalogError(f"{path}: unsupported ledger or evidence index layout at {rev}")
    return read_blobs(root, rev, {path: mode for path, mode in paths.items() if topic(path) is not None})


def document_file(path: str) -> str:
    return f"sources/{source_path(path)}.json"


@dataclass(frozen=True)
class Catalog:
    revision: str
    documents: dict[str, Document]
    authority: dict[str, object] | None = None

    @classmethod
    def from_legacy(cls, root: Path, rev: str) -> Catalog:
        files = legacy_files(root, rev)
        if not files:
            raise CatalogError(f"no supported ledger or evidence index files at {rev}")
        return cls(rev, {path: Document.parse(path, source) for path, source in files.items()})

    def files(self) -> dict[str, bytes]:
        documents = dict(sorted(self.documents.items()))
        files = {document_file(path): document.serialized() for path, document in documents.items()}
        manifest: dict[str, object] = {
            "schema": SCHEMA,
            "revision": self.revision,
            "sources": [
                {
                    "path": path,
                    "sha256": digest(document.render()),
                    "catalog_sha256": digest(files[document_file(path)]),
                }
                for path, document in documents.items()
            ],
        }
        if self.authority is not None:
            manifest["authority"] = self.authority
        files[MANIFEST] = (json_line(manifest) + "\n").encode()
        return files

    @classmethod
    def from_files(cls, files: Mapping[str, bytes]) -> Catalog:
        if MANIFEST not in files:
            raise CatalogError("missing catalog manifest")
        manifest = parse_json_object(files[MANIFEST])
        expect_keys(manifest, {"schema", "revision", "sources"}, {"authority"})
        revision = as_text(manifest["revision"], "revision")
        if type(manifest["schema"]) is not int or manifest["schema"] != SCHEMA or not COMMIT.fullmatch(revision):
            raise CatalogError("invalid catalog schema or revision")
        documents: dict[str, Document] = {}
        for item in as_list(manifest["sources"], "sources"):
            source = as_object(item, "source")
            expect_keys(source, {"path", "sha256", "catalog_sha256"})
            path = as_text(source["path"], "path")
            serialized = files.get(document_file(path))
            if path in documents or serialized is None or digest(serialized) != source["catalog_sha256"]:
                raise CatalogError(f"{path}: missing, duplicate or modified catalog source")
            document = Document.from_bytes(path, serialized)
            if digest(document.render()) != source["sha256"]:
                raise CatalogError(f"{path}: rendered source digest mismatch")
            documents[path] = document
        authority = as_object(manifest["authority"], "authority") if "authority" in manifest else None
        catalog = cls(revision, documents, authority)
        if catalog.files() != files:
            raise CatalogError("catalog files and manifest do not match the canonical layout")
        return catalog


def read_regular(root: Path, name: str) -> bytes:
    path = root / name
    if path.resolve() != path or not path.is_file():
        raise CatalogError(f"{name}: expected a regular file without symlinks")
    return path.read_bytes()


def load_directory(directory: Path) -> Catalog:
    directory = directory.resolve()
    if not directory.is_dir():
        raise CatalogError(f"catalog directory does not exist: {directory}")
    files: dict[str, bytes] = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise CatalogError(f"unexpected symlink in catalog: {path}")
        if not path.is_dir():
            rel = path.relative_to(directory).as_posix()
            files[rel] = read_regular(directory, rel)
    return Catalog.from_files(files)


def new_output(root: Path, out: Path, files: Mapping[str, bytes]) -> None:
    out = out.resolve()
    for protected in (*LEGACY_ROOTS, AUTHORITY, ".git"):
        directory = root / protected
        if out == directory or out.is_relative_to(directory) or out.is_relative_to(directory.resolve()):
            raise CatalogError(f"output directory is inside {protected}")
    # Reserving the directory is exclusive. Manifests and patches are written last.
    out.mkdir()
    for rel, content in files.items():
        name = PurePosixPath(rel)
        if name.is_absolute() or ".." in name.parts:
            raise CatalogError(f"invalid output path {rel}")
        destination = out / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        pending = destination.with_name(f".{destination.name}.part")
        with pending.open("xb") as stream:
            _ = stream.write(content)
        _ = pending.replace(destination)


def checkout_paths(root: Path, *prefixes: str) -> set[str]:
    candidates = git(root, "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", *prefixes)
    return {
        path for path in candidates.decode().split("\0")
        if path and ((root / path).exists() or (root / path).is_symlink())
    }
