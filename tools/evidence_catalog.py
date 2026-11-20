# SPDX-License-Identifier: AGPL-3.0-or-later
"""Migrate and query the evidence index, or prepare a reviewed topic publication."""

from __future__ import annotations

import argparse
import sys
from collections import Counter
from collections.abc import Sequence
from pathlib import Path

from tools.docs_lint import parse_json_object
from tools.evidence_catalog_model import CatalogError, Index, json_line, topic
from tools.evidence_catalog_publish import (
    Request,
    check_checkout,
    patch,
    prepare,
    require_clean_sources,
    resolved_catalog,
    view_files,
)
from tools.evidence_catalog_store import Catalog, commit_id, legacy_files, load_directory, new_output


class Options(argparse.Namespace):
    repo: Path = Path(__file__).resolve().parent.parent
    command: str = ""
    rev: str | None = None
    out: Path = Path()
    catalog: Path | None = None
    topic: list[str] = []
    identifier: str = ""
    request: Path = Path()
    approved_sha256: str = ""
    staged: bool = False


def options(argv: Sequence[str] | None) -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--repo", type=Path)
    commands = parser.add_subparsers(dest="command", required=True)
    migrate = commands.add_parser("migrate", help="export the complete catalog from a Git commit")
    _ = migrate.add_argument("--rev", required=True)
    _ = migrate.add_argument("--out", type=Path, required=True)
    verify = commands.add_parser("verify", help="verify full byte parity against the source Git commit")
    _ = verify.add_argument("--catalog", type=Path, required=True)
    render = commands.add_parser("render", help="write generated views into a fresh directory")
    _ = render.add_argument("--catalog", type=Path, required=True)
    _ = render.add_argument("--out", type=Path, required=True)
    _ = render.add_argument("--topic", action="append", default=[])
    lookup = commands.add_parser("lookup", help="lookup an exact record ID or a qualified evidence member")
    source = lookup.add_mutually_exclusive_group(required=True)
    _ = source.add_argument("--catalog", type=Path)
    _ = source.add_argument("--rev")
    _ = lookup.add_argument("identifier")
    request_id = commands.add_parser("request-id", help="print the SHA-256 of a typed publication request")
    _ = request_id.add_argument("request", type=Path)
    publish = commands.add_parser("publish", help="prepare the reviewed change as a Git patch")
    _ = publish.add_argument("request", type=Path)
    _ = publish.add_argument("--approved-sha256", required=True)
    _ = publish.add_argument("--out", type=Path, required=True)
    check = commands.add_parser("check", help="enforce generated views for opted-in checkout topics")
    _ = check.add_argument("--staged", action="store_true", help="read candidate files from the Git index")
    return parser.parse_args(argv, namespace=Options())


def report(catalog: Catalog, index: Index) -> str:
    return json_line({
        "revision": catalog.revision,
        "sources": len(catalog.documents),
        "records": dict(sorted(Counter(entry.record.kind for entry in index.entries.values()).items())),
        "listed_members": sum(
            len(entry.record.evidence(entry.path, entry.line).ev) + len(entry.record.evidence(entry.path, entry.line).unk)
            for entry in index.entries.values() if entry.record.kind == "M"
        ),
    })


def require_directory(path: Path | None) -> Catalog:
    if path is None:
        raise CatalogError("catalog directory required")
    return load_directory(path)


def run(args: Options) -> None:
    root = args.repo.resolve()
    if args.command == "check":
        check_checkout(root, staged=args.staged)
    elif args.command == "migrate":
        revision = commit_id(root, args.rev or "")
        catalog = resolved_catalog(root, revision)
        index = Index(catalog.documents)
        new_output(root, args.out, catalog.files())
        print(report(catalog, index))
    elif args.command == "verify":
        catalog = require_directory(args.catalog)
        if catalog.authority is not None:
            raise CatalogError("verify requires a full migration snapshot")
        rendered = view_files(catalog)
        original = legacy_files(root, catalog.revision)
        if rendered != original:
            paths = sorted(path for path in rendered.keys() | original.keys() if rendered.get(path) != original.get(path))
            raise CatalogError(f"Git source parity mismatch: {', '.join(paths)}")
        index = Index(catalog.documents)
        for target in index.reverse:
            if "/" in target:
                _ = index.selector(target)
        print(report(catalog, index))
    elif args.command == "render":
        catalog = require_directory(args.catalog)
        files = view_files(catalog)
        if args.topic:
            available = {topic(path) for path in files}
            if not set(args.topic) <= available:
                raise CatalogError("unknown render topic")
            files = {path: content for path, content in files.items() if topic(path) in args.topic}
        new_output(root, args.out, files)
    elif args.command == "lookup":
        if args.catalog is not None:
            catalog = load_directory(args.catalog)
        else:
            catalog = resolved_catalog(root, commit_id(root, args.rev or ""))
        print(json_line(Index(catalog.documents).lookup(args.identifier)))
    else:
        request = Request.parse(parse_json_object(args.request.read_bytes()))
        if args.command == "request-id":
            print(request.approval())
            return
        if request.approval() != args.approved_sha256:
            raise CatalogError("approval SHA-256 does not match this publication request")
        require_clean_sources(root, request.base)
        publication = prepare(root, request)
        output = patch(publication)
        require_clean_sources(root, request.base)
        new_output(root, args.out, {"change.patch": output})
        print(json_line({
            "base": request.base, "approved_sha256": request.approval(), "patch": str(args.out / "change.patch"),
        }))


def main(argv: Sequence[str] | None = None) -> int:
    try:
        run(options(argv))
    except (OSError, ValueError) as error:
        print(f"evidence catalog: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
