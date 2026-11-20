# SPDX-License-Identifier: AGPL-3.0-or-later
"""Command line of ``python3 -m tools.memory``: the offline commands ``get --offline``, ``check --ci``
and ``protected-paths``. Commands that need the service are added on top of this module.

Exit status: 0 success; 1 problems, an unknown ID or unreadable input; 2 usage;
``EXIT_NO_JOURNAL`` (3) when ``check`` finds no journal at all.
"""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from collections.abc import Sequence
from pathlib import Path

from tools.memory.canonical import render_text
from tools.memory.check import run_check
from tools.memory.gitio import GitError, descends, git
from tools.memory.journal import JournalError, read_git, read_tree
from tools.memory.model import JSON, RecordError
from tools.memory.paths import ProtectedPathsError, protected_paths
from tools.memory.state import RELEASE_CONFIG, Entry, State, partition, pinned_build, replay

EXIT_NO_JOURNAL = 3
ACCEPTANCE_REF = "origin/main"


class Options(argparse.Namespace):
    command: str = ""
    id: str = ""
    offline: bool = False
    history: bool = False
    json: bool = False
    ci: bool = False
    bank: bool = False
    pin: bool = False
    base: str = ACCEPTANCE_REF


def parse_options(argv: Sequence[str] | None) -> Options:
    parser = argparse.ArgumentParser(prog="memory", description="The project's memory tool (.agents/rules/memory.md).")
    commands = parser.add_subparsers(dest="command", required=True)
    get = commands.add_parser("get", help="show one record from the journal")
    _ = get.add_argument("id", help="ID or evidence selector (M-N/ev_<12 hex>)")
    _ = get.add_argument("--offline", action="store_true", help="read the journal in this worktree")
    _ = get.add_argument("--history", action="store_true", help="also list every revision")
    _ = get.add_argument("--json", action="store_true", help="print JSON")
    check = commands.add_parser("check", help="check the journal and citations")
    _ = check.add_argument("--ci", action="store_true", help="the checks that need no service")
    _ = check.add_argument("--base", default=ACCEPTANCE_REF, help=f"accepted tree-ish (default {ACCEPTANCE_REF})")
    _ = commands.add_parser("protected-paths", help="paths scratch cleanup must keep; fails closed")
    from tools.memory import online  # imported here: it reads this module's parsed options

    online.register(commands)
    options = Options()
    _ = parser.parse_args(argv, namespace=options)
    return options


def repository_root() -> Path:
    return Path(git(Path.cwd(), "rev-parse", "--show-toplevel").decode().strip())


def document_state(entry: Entry, accepted: set[str] | None) -> str:
    """accepted, pending (an accepted record with a newer revision), proposed, conflict, or unknown without the acceptance ref."""
    if entry.conflict:
        return "conflict"
    if accepted is None:
        return "unknown"
    if entry.heads[0] in accepted:
        return "accepted"
    return "pending" if any(sha in accepted for sha in entry.revisions) else "proposed"


def describe(root: Path, state: State, citation: str, *, history: bool) -> tuple[dict[str, JSON], list[str]]:
    """``get --offline`` as JSON and as text lines. Raises ``LookupError`` for an unknown ID or selector."""
    resolution = state.resolve(citation)
    if resolution.problem is not None:
        raise LookupError(resolution.problem)
    identifier = resolution.target
    entry = state.entries[identifier]
    record = entry.record
    try:
        accepted: set[str] | None = {revision.sha256 for segment in read_git(root, ACCEPTANCE_REF) for revision in segment.revisions}
    except (GitError, JournalError):
        accepted = None
    try:
        build = pinned_build(root)
        release = state.release(build)
        reason = f"no REL record for server build {build}" if release is None else ""
    except (OSError, RecordError, tomllib.TOMLDecodeError) as error:
        release, reason = None, f"{RELEASE_CONFIG}: {error}"
    status = state.status(identifier)
    superseded_by, successors, resolved_by = state.superseded_by.get(identifier, ()), state.successors(identifier), state.resolved_by.get(identifier, ())
    heads: list[JSON] = [
        {"sha256": sha, "segment": entry.revisions[sha].segment, "prev": list(entry.revisions[sha].revision.prev),
         "text": render_text(entry.revisions[sha].revision.record)}
        for sha in entry.heads
    ]
    result: dict[str, JSON] = {
        "id": identifier, "state": document_state(entry, accepted), "status": status, "stored_status": record.status,
        "superseded_by": list(superseded_by), "successors": list(successors), "resolved_by": list(resolved_by),
        "heads": heads, "record": record.to_json(), "text": render_text(record),
    }
    lines = [render_text(record), "", f"state: {result['state']}", f"status: {status}" + (f" (stored {record.status})" if status != record.status else "")]
    if "/" in citation:
        result["selector"] = citation
        result["member"] = resolution.member
        lines.insert(0, f"{citation}: " + (f"listed member {resolution.member}" if resolution.member else "legacy case note") + f" of {identifier}\n")
    if release is None:
        result["partition"] = {"name": "unknown", "reason": reason}
        lines.append(f"partition: unknown ({reason})")
    else:
        placed = partition(record, release, lambda commit: descends(root, commit))
        result["partition"] = {"name": placed.name, "release": release.id, "tokens": list(placed.tokens), "names_pin": placed.names_pin,
                               "commit": placed.commit, "descends": placed.descends}
        detail = [f"tokens {', '.join(placed.tokens)}"] if placed.tokens else []
        if placed.name == "legacy-scope":
            detail.append(f"names the pin: {'yes' if placed.names_pin else 'no'}")
        if placed.name == "native":
            answer = "unknown" if placed.descends is None else "yes" if placed.descends else "no"
            detail.append(f"HEAD descends from {placed.commit}: {answer}")
        lines.append(f"partition: {placed.name} against {release.id}" + "".join(f"; {item}" for item in detail))
    if superseded_by:
        lines.append(f"superseded by: {', '.join(superseded_by)} (terminal: {', '.join(successors)})")
    if resolved_by:
        lines.append(f"resolved by: {', '.join(resolved_by)}")
    for sha in entry.heads:
        located = entry.revisions[sha]
        prev = ", ".join(item[:12] for item in located.revision.prev) or "none"
        lines.append(f"{'conflict head' if entry.conflict else 'revision'}: {sha} in {located.segment} (prev {prev})")
        if entry.conflict and sha != entry.heads[0]:
            lines.extend(["", render_text(located.revision.record), ""])
    if history:
        result["history"] = [
            {"sha256": item.revision.sha256, "segment": item.segment, "prev": list(item.revision.prev), "record": item.revision.record.to_json()}
            for item in entry.history()
        ]
        lines.append("history:")
        for item in entry.history():
            lines.extend(["", f"{item.revision.sha256} in {item.segment}", render_text(item.revision.record)])
    return result, lines


def main(argv: Sequence[str] | None = None) -> int:
    options = parse_options(argv)
    if options.command == "protected-paths":
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
    if options.command not in ("get", "check") or not (options.offline or options.ci) or options.bank or options.pin:
        from tools.memory import online

        return online.run(options, report_check)
    try:
        root = repository_root()
        if options.command == "check":
            return report_check(root, options.base)
        state = replay(read_tree(root))
        result, lines = describe(root, state, options.id, history=options.history)
    except (GitError, JournalError, LookupError) as error:
        print(f"memory {options.command}: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True) if options.json else "\n".join(lines))
    return 0


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
