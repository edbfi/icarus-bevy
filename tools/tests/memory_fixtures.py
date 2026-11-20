# SPDX-License-Identifier: AGPL-3.0-or-later
"""Invented records and Git repositories for the memory tool tests; no text comes from the project's ledgers.

The designated chain is modelled with new (non-migrated) records: F-101 is superseded by F-102, which
is superseded by F-106; U-101 is resolved by F-103, a finding on a later release; M-101 is a bundle
with listed members that F-102 cites by selector; REL-9.0.1 and REL-9.0.2 are two releases.
"""

from __future__ import annotations

import os
import subprocess
from collections.abc import Generator
from contextlib import contextmanager
from datetime import UTC, datetime
from pathlib import Path
from unittest.mock import patch

from tools.memory.journal import Revision, write_change_set
from tools.memory.model import JSON, Record

OLD_SERVER, NEW_SERVER, NEW_CLIENT = "90000001", "90000002", "90000003"
MEMBER = "ev_a1b2c3d4e5f6" + "0" * 52
OTHER_MEMBER = "unk_0f0f0f0f0f0f" + "1" * 52
DIGEST = "ab" * 32
CREATED = datetime(2026, 11, 20, 12, 0, 0, tzinfo=UTC)


def explicit(*builds: str, commit: str | None = None, subjects: tuple[str, ...] = ("original-server",)) -> dict[str, JSON]:
    scope: dict[str, JSON] = {"basis": "explicit", "subjects": list(subjects)}
    if builds:
        scope["builds"] = list(builds)
    if commit is not None:
        scope["commit"] = commit
    return scope


def finding(identifier: str, claim: str, **extra: JSON) -> dict[str, JSON]:
    data: dict[str, JSON] = {
        "id": identifier, "type": "finding", "status": "active", "claim": claim, "kind": "observed",
        "confidence": "medium", "standing": "confirmed", "area": "listing", "scope": explicit(f"server:{OLD_SERVER}"),
        "cites": ["M-102"],
    }
    data.update(extra)
    return data


def unknown(identifier: str, question: str, **extra: JSON) -> dict[str, JSON]:
    data: dict[str, JSON] = {
        "id": identifier, "type": "unknown", "status": "open", "question": question, "severity": "medium",
        "next_probe": "Read the invented handler", "area": "listing",
    }
    data.update(extra)
    return data


def evidence(identifier: str, path: str, **extra: JSON) -> dict[str, JSON]:
    data: dict[str, JSON] = {
        "id": identifier, "type": "evidence", "status": "active", "path": path, "sha256": DIGEST, "size": 12,
        "class": "local-private", "kind": "receipt", "date": "2026-11-20", "label": "Invented receipt",
    }
    data.update(extra)
    return data


def release(identifier: str, server: str, client: str) -> dict[str, JSON]:
    return {
        "id": identifier, "type": "release", "status": "current", "version_string": f"{identifier[4:]}.1-test",
        "server_build": server, "client_build": client, "dates": {"released": "2026-11-01"}, "sources": ["invented"],
    }


def chain() -> list[dict[str, JSON]]:
    """The designated chain, invented: supersession, resolution on another release, a bundle and releases."""
    return [
        finding("F-101", "Invented: the prompt comes from rule `L_x` (`ListingRules`)", kind="derived", refs=["F-104"]),
        finding("F-102", "Invented: any `L_x` marks a listing locked", supersedes=["F-101"], refs=["U-101"],
                cites=["M-101/ev_a1b2c3d4e5f6", "M-102"]),
        finding("F-103", "Invented: a locked listing admits the right password", resolves=["U-101"],
                scope=explicit(f"server:{NEW_SERVER}", f"client:{NEW_CLIENT}")),
        finding("F-104", "Invented: native listing sets no password flag", scope=explicit(commit="c3d4e5f", subjects=("native",))),
        finding("F-106", "Invented: the lock follows the key, re-read", supersedes=["F-102"]),
        unknown("U-101", "Which invented key prompts for a password?"),
        evidence("M-101", "work/invented-case/rea-bundle.json", kind="rea-bundle", records=2, members=[MEMBER, OTHER_MEMBER]),
        evidence("M-102", "work/invented-case/report.md", kind="report"),
        release("REL-9.0.1", OLD_SERVER, "90000000"),
        release("REL-9.0.2", NEW_SERVER, NEW_CLIENT),
    ]


def revisions(items: list[dict[str, JSON]]) -> list[Revision]:
    return [Revision.of(Record.from_json(item)) for item in items]


@contextmanager
def clean_git_environment() -> Generator[None]:
    """Hooks export ``GIT_*`` variables (index, directory) that would redirect a fixture repository."""
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    with patch.dict(os.environ, environment, clear=True):
        yield


def run_git(root: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(root), "-c", "user.name=Test", "-c", "user.email=test@example.invalid", "-c", "commit.gpgsign=false", *args],
        check=True, capture_output=True, text=True,
    ).stdout


def new_repository(root: Path) -> None:
    _ = run_git(root, "init", "-q", "-b", "main")


def commit_all(root: Path, message: str = "fixture") -> None:
    """Commit everything and point ``origin/main`` at it, as a fetched acceptance ref."""
    _ = run_git(root, "add", "-A")
    _ = run_git(root, "commit", "-q", "--allow-empty", "-m", message)
    _ = run_git(root, "update-ref", "refs/remotes/origin/main", "HEAD")


def write(root: Path, items: list[dict[str, JSON]], *, second: int = 0) -> tuple[Path, ...]:
    return write_change_set(root, CREATED.replace(second=second), revisions(items))
