# SPDX-License-Identifier: AGPL-3.0-or-later
"""Command line of ``python3 -m tools.memory``.

One dispatch mechanism: every subcommand sets ``handler`` (``set_defaults``), a function from the parsed
options to the exit status. This module defines ``get``, ``check`` and ``protected-paths``; each other
module adds its commands with one ``register(commands)`` call below (``online``: the bank commands;
``durability``: ``service``, ``backup``, ``restore``; ``mcp``: the stdio MCP server; ``salvage``: legacy rows
of branches older than the cut-over).

Exit status: 0 success; 1 problems, an unknown ID or unreadable input; 2 usage;
``EXIT_NO_JOURNAL`` (3) when ``check`` finds no segment in the working tree or the reference tree.
Exit 3 is never a pass: the hook and the CI step that run ``check --ci`` fail on every non-zero status.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Callable, Sequence
from pathlib import Path

from tools.memory.bank import Universe
from tools.memory.check import run_check
from tools.memory import durability, mcp, online, salvage
from tools.memory.gitio import GitError, git
from tools.memory.journal import JournalError, read_git, read_tree
from tools.memory.paths import ProtectedPathsError, protected_paths

EXIT_NO_JOURNAL = 3
ACCEPTANCE_REF = "origin/main"


class Options(argparse.Namespace):
    command: str = ""
    id: str = ""
    offline: bool = False
    history: bool = False
    all_proposals: bool = False
    full: bool = False
    json: bool = False
    ci: bool = False
    bank: bool = False
    pin: bool = False
    base: str = ACCEPTANCE_REF
    handler: Callable[[Options], int] | None = None


def parser() -> argparse.ArgumentParser:
    """The whole command line; ``--help`` prints the command list."""
    top = argparse.ArgumentParser(prog="memory", description="The project's memory tool (.agents/rules/memory.md).")
    commands = top.add_subparsers(dest="command", required=True, metavar="COMMAND")
    get = commands.add_parser("get", help="one record by exact GET, verified against the journal")
    _ = get.add_argument("id", help="ID or evidence selector (M-N/ev_<12 hex>)")
    _ = get.add_argument("--offline", action="store_true", help="read the journal in this worktree; no service")
    _ = get.add_argument("--history", action="store_true", help="also list every revision")
    _ = get.add_argument("--all-proposals", action="store_true", help="also show other worktrees' proposed superseders and resolvers")
    _ = get.add_argument("--full", action="store_true", help="with --history: each revision's whole text")
    _ = get.add_argument("--json", action="store_true", help="print JSON")
    get.set_defaults(handler=run_get)
    check = commands.add_parser("check", help="journal, citations, forks, notices; --bank and --pin need the bank or the pin")
    _ = check.add_argument("--ci", action="store_true", help="the checks that need no service")
    _ = check.add_argument("--bank", action="store_true", help="assert the instance and compare every document with the journal")
    _ = check.add_argument("--pin", action="store_true", help="list cited records not current")
    _ = check.add_argument("--base", default=ACCEPTANCE_REF, help=f"accepted tree-ish (default {ACCEPTANCE_REF})")
    _ = check.add_argument("--json", action="store_true", help="print JSON of --bank and --pin")
    check.set_defaults(handler=run_check_command)
    paths = commands.add_parser("protected-paths", help="paths scratch cleanup must keep; fails closed")
    paths.set_defaults(handler=run_protected_paths)
    online.register(commands)
    durability.register(commands)
    mcp.register(commands)
    salvage.register(commands)
    return top


def parse_options(argv: Sequence[str] | None) -> Options:
    options = Options()
    _ = parser().parse_args(argv, namespace=options)
    return options


def repository_root() -> Path:
    return Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())


def offline_universe(root: Path) -> Universe:
    """``origin/main`` and this worktree's journal as a ``Universe``, read without the service, other worktrees
    or other refs: what ``get --offline`` formats with ``online.describe``. Without ``origin/main`` nothing
    counts as accepted (a notice says so)."""
    local = read_tree(root)
    try:
        main, known, commit = read_git(root, ACCEPTANCE_REF), True, git(root, "rev-parse", "--verify", "--end-of-options", f"{ACCEPTANCE_REF}^{{commit}}").decode().strip()
    except GitError as error:
        main, known, commit, notices = (), False, None, (f"{ACCEPTANCE_REF} unreadable ({error}); nothing counts as accepted",)
    else:
        notices = ()
    own = tuple(sorted({segment.name: segment for segment in (*main, *local)}.values(), key=lambda segment: segment.path))
    accepted = frozenset(revision.rev for segment in main for revision in segment.revisions)
    return Universe(root, own, main, local, own, accepted, known, commit, notices)


def main(argv: Sequence[str] | None = None) -> int:
    options = parse_options(argv)
    if options.handler is None:
        return 2
    return options.handler(options)


def run_protected_paths(_options: Options) -> int:
    try:
        result = protected_paths(Path.cwd())
    except ProtectedPathsError as error:
        print(f"memory protected-paths: {error}; treat every path as protected", file=sys.stderr)
        return 1
    if result.paths:
        print("\n".join(result.paths))
    for notice in result.notices:
        print(f"memory protected-paths: notice: {notice}", file=sys.stderr)
    return 0


def run_get(options: Options) -> int:
    """``get`` reads the bank (``online.get``); ``get --offline`` prints the same formatter (``online.describe``) over
    ``origin/main`` and this worktree's journal, without the lines only the bank can answer (its documents verified)."""
    if not options.offline:
        return online.run(options)
    try:
        root = repository_root()
        universe = offline_universe(root)
        described = online.describe(universe, root, options.id, history=options.history, all_proposals=options.all_proposals, full=options.full)
    except (GitError, JournalError, LookupError, ProtectedPathsError) as error:
        print(f"memory get: {error}", file=sys.stderr)
        return 1
    lines = [*described.lines, *(f"notice: {notice}" for notice in universe.notices)]
    print(json.dumps(described.data, ensure_ascii=False, indent=2, sort_keys=True) if options.json else "\n".join(lines))
    return 0


def run_check_command(options: Options) -> int:
    """``--ci`` (the core), ``--bank`` and ``--pin`` (``online``); no flag runs ``--ci`` and ``--bank``."""
    plain = not (options.ci or options.bank or options.pin)
    try:
        root = repository_root()
    except GitError as error:
        print(f"memory check: {error}", file=sys.stderr)
        return 1
    core = report_check(root, options.base) if options.ci or plain else 0
    if not (options.bank or options.pin or plain):
        return core
    return online.run(options) or core


def report_check(root: Path, base: str) -> int:
    report = run_check(root, base)
    if report.no_journal:
        print("memory check: no journal (memory/journal/ holds no segment); nothing checked")
        return EXIT_NO_JOURNAL
    for problem in report.problems:
        print(problem)
    for notice in report.notices:
        print(f"notice: {notice}")
    print(f"memory check: {len(report.problems)} problem(s), {len(report.notices)} notice(s)")
    return 1 if report.problems else 0
