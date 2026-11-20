# SPDX-License-Identifier: AGPL-3.0-or-later
"""Canonical JSON, digests, text, tags and entities of a record. Every function here is pure.

- ``canonical_json``: keys sorted, no insignificant whitespace, UTF-8. Strings must already be NFC and
  free of control characters other than TAB and LF; they are refused, never normalised, so a digest
  always covers exactly the bytes that are stored.
- ``record_sha256``: the sha256 of a record's canonical JSON (a REF's JSON holds ``body_sha256``).
- ``render_text``: the text of a record's bank document. A REF's text is its body. Any other record
  renders as ``<ID> <type> <status>`` on the first line, its headline field (claim, question, ...) on
  the second, then its other fields in schema order; bundle members and legacy cells are omitted.
  The result is LF-only and at most ``TEXT_LIMIT`` characters, or ``RenderError``.
- ``tags`` and ``entities``: the tag set and entity list of a bank document.
"""

from __future__ import annotations

import json
import re

from tools.memory.model import JSON, RUN, Record, RecordError, Value, sha256_hex, string_problem

TEXT_LIMIT = 1800
STATES = ("accepted", "proposed", "pending", "orphaned", "conflict")
# A Markdown code span on one line, and the identifiers entity extraction takes from it.
CODE_SPAN = re.compile(r"(`+)([^`\n](?:[^\n]*?[^`\n])?)\1(?!`)")
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:(?:::|\.|/)[A-Za-z0-9_]+)*")


class RenderError(RecordError):
    """A record has no admissible canonical form or text."""


def canonical_json(value: JSON) -> bytes:
    _admissible(value)
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def _admissible(value: JSON) -> None:
    if isinstance(value, str):
        if problem := string_problem(value):
            raise RenderError(f"string {value[:40]!r}: {problem}")
    elif isinstance(value, list):
        for item in value:
            _admissible(item)
    elif isinstance(value, dict):
        for key, item in value.items():
            _admissible(key)
            _admissible(item)


def record_sha256(record: Record) -> str:
    return sha256_hex(canonical_json(record.to_json()))


def _show(value: Value) -> str:
    if isinstance(value, tuple):
        return ", ".join(value)
    if isinstance(value, dict):
        return "; ".join(f"{key} {item}" for key, item in value.items())
    if isinstance(value, bool):
        return "true"
    return str(value)


def render_text(record: Record) -> str:
    if record.body is not None:
        return record.body
    spec = record.spec
    lines = [f"{record.id} {record.type} {record.status}"]
    if (headline := record.text(spec.headline)) is not None:
        lines.append(headline)
    lines.extend(
        f"{name}: {_show(record.fields[name])}"
        for name in spec.fields
        if name in record.fields and name not in (spec.headline, "members")
    )
    if (scope := record.scope) is not None:
        parts = [f"basis {scope.basis}"]
        parts.extend(f"{name} {', '.join(items)}" for name, items in (
            ("subjects", scope.subjects), ("builds", scope.builds), ("releases", scope.releases)) if items)
        if scope.commit:
            parts.append(f"commit {scope.commit}")
        lines.append("scope: " + "; ".join(parts))
    lines.extend(f"{name}: {', '.join(items)}" for name in ("supersedes", "resolves", "cites", "refs") if (items := record.links.named(name)))
    if record.links.verified_on:
        lines.append("verified_on: " + "; ".join(f"{item.build} ({', '.join(item.cites)})" for item in record.links.verified_on))
    if record.run is not None:
        lines.extend(f"run.{name}: {_show(record.run[name])}" for name in RUN if name in record.run)
    lines.extend(f"note: {note}" for note in record.notes)
    if record.gaps:
        lines.append("gaps: " + ", ".join(record.gaps))
    if record.legacy is not None:
        lines.append(f"migrated from {record.legacy.path}" + ("; ID assigned at migration" if record.assigned_at_migration else ""))
    text = "\n".join(lines)
    if len(text) > TEXT_LIMIT:
        raise RenderError(f"{record.id}: text is {len(text)} characters; the limit is {TEXT_LIMIT}")
    if problem := string_problem(text):
        raise RenderError(f"{record.id}: text: {problem}")
    return text


def tags(record: Record, *, state: str, status: str, segment: str) -> tuple[str, ...]:
    """Sorted tags of a bank document: ``state`` is one of ``STATES``, ``status`` the derived status."""
    if state not in STATES:
        raise RenderError(f"{record.id}: unknown document state {state!r}")
    found = {f"type:{record.type}", f"id:{record.id}", f"state:{state}", f"status:{status}", f"seg:{segment}"}
    if (area := record.text("area")) is not None:
        found.add(f"area:{area}")
    if (scope := record.scope) is not None:
        found.add(f"basis:{scope.basis}")
        found.update(f"subject:{subject}" for subject in scope.subjects)
        found.update(f"build:{build}" for build in scope.builds)
        found.update(f"release:{release}" for release in scope.releases)
    return tuple(sorted(found))


def entities(record: Record) -> tuple[str, ...]:
    """Sorted entities: linked IDs and selectors, builds, and identifiers in code spans of the text."""
    found = {*record.links.targets(), *(cite for cite in record.links.citations() if "/" in cite)}
    if record.legacy is not None:
        found.update(record.legacy.superseded_by)
    if record.scope is not None:
        found.update(record.scope.builds)
    found.update(item.build for item in record.links.verified_on)
    for span in CODE_SPAN.finditer(render_text(record)):
        found.update(token[0] for token in IDENTIFIER.finditer(span[2]) if len(token[0]) >= 3)
    return tuple(sorted(found))
