# SPDX-License-Identifier: AGPL-3.0-or-later
"""The migration report: what an export moved, what it could not state, and what stays behind.

``render(result, root)`` returns Markdown with, in order: counts per type; every record with gaps,
grouped by type and gap; every ``legacy:<verbatim>`` value with its count and IDs; links taken from
ledger prose; rows whose several commits leave the scope without one; the judgments the rows force
(standing, partitions against the current release); release records with what each lacks and its
notes; citations kept as text; the residue; inbound references to spec and digest paths.

- **Residue** (``residue``): every concrete value in the research digests (``docs/research/<area>/``,
  not the archive), in ledger lines outside table rows and in ``docs/implementation-status.md`` that
  appears in no migrated record. A value (``VALUE``) is a hex literal, an ISO date, a number of two or
  more digits (thousands separators dropped), a code span, or a name (CamelCase, snake_case or letters
  glued to digits). Record citations and link targets are not values. A value counts as migrated when
  the same token occurs in any record field, note, legacy cell or spec body.
- **Inbound references** (``inbound``): every line of every text file at the revision that names a spec
  or digest path, literally (``docs/specs/...``) or as a relative Markdown link, with file and line.
"""

from __future__ import annotations

import posixpath
import re
from collections import Counter, defaultdict
from collections.abc import Iterable, Iterator
from dataclasses import dataclass
from pathlib import Path

from tools.evidence_catalog_model import ID_IN_TEXT
from tools.memory.check import CITATION
from tools.memory.gitio import git, read_blobs
from tools.memory.journal import JOURNAL, Revision, Segment
from tools.memory.legacy import Export
from tools.memory.model import LEGACY, Record, id_key
from tools.memory.state import partition, replay

VALUE = re.compile(
    r"`([^`\n]+)`|(0x[0-9A-Fa-f]+)|([0-9]{4}-[0-9]{2}-[0-9]{2})|([0-9]{1,3}(?:,[0-9]{3})+(?:\.[0-9]+)?|[0-9]+(?:\.[0-9]+)+|[0-9]{2,})"
    + r"|((?:[A-Za-z][A-Za-z0-9]*_[A-Za-z0-9_]*|[a-z]+[A-Z][A-Za-z0-9]*|[A-Z][a-z0-9]+[A-Z][A-Za-z0-9]*|[A-Z]+[0-9][A-Za-z0-9]*))"
)
LINK_TARGET = re.compile(r"\]\([^)]*\)|https?://\S+")
DIGESTS = "docs/research/"
ARCHIVE = "docs/research/archive/"
STATUS = "docs/implementation-status.md"
FROZEN = ("docs/history/", ARCHIVE)
PATH_MENTION = re.compile(r"(?:\.\./|\./)*(?:docs/)?(?:specs|research)/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*\.md")
MARKDOWN_LINK = re.compile(r"\]\(([^)\s#]+\.md)(?:#[^)\s]*)?\)")


@dataclass(frozen=True)
class Residue:
    path: str
    line: int
    value: str
    text: str


@dataclass(frozen=True)
class Inbound:
    path: str
    line: int
    target: str
    text: str


def values(text: str) -> Iterator[str]:
    """Concrete values in ``text`` (see ``VALUE``), normalised: code spans as written, numbers without separators."""
    text = LINK_TARGET.sub(" ", CITATION.sub(" ", text))
    for match in VALUE.finditer(text):
        value = next(group for group in match.groups() if group is not None)
        yield value.replace(",", "") if match[4] else value


def record_strings(record: Record) -> Iterator[str]:
    for value in record.fields.values():
        if isinstance(value, str):
            yield value
        elif isinstance(value, tuple):
            yield from value
    yield from record.notes
    if record.body is not None:
        yield record.body
    if record.legacy is not None:
        yield from record.legacy.cells.values()


def corpus(records: Iterable[Record]) -> set[str]:
    found: set[str] = set()
    for record in records:
        for text in record_strings(record):
            found.update(values(text))
            # A code span's content also counts through its parts, as prose may quote one part.
            found.update(value for span in re.finditer(r"`([^`\n]+)`", text) for value in values(span[1]))
    return found


def git_texts(root: Path, revision: str, *prefixes: str) -> dict[str, str]:
    """UTF-8 text of the regular files under ``prefixes`` at ``revision``; symlinks and binaries are skipped."""
    files: dict[str, str] = {}
    for item in git(root, "ls-tree", "-r", "-z", "--end-of-options", revision, "--", *prefixes).decode().split("\0"):
        metadata, _, path = item.partition("\t")
        if item and metadata.split()[0] in ("100644", "100755"):
            files[path] = metadata.split()[2]
    paths = sorted(files)
    texts: dict[str, str] = {}
    for path, data in zip(paths, read_blobs(root, [files[path] for path in paths]), strict=True):
        if data is None or b"\0" in data:
            continue
        try:
            texts[path] = data.decode("utf-8")
        except UnicodeDecodeError:
            continue
    return texts


def residue(result: Export, root: Path) -> list[Residue]:
    known = corpus(item.record for item in result.migrated)
    sources: list[tuple[str, int, str]] = []
    digests = git_texts(root, result.revision, DIGESTS)
    for path, text in sorted(digests.items()):
        if path.endswith(".md") and not path.startswith(ARCHIVE):
            sources.extend((path, number, line) for number, line in enumerate(text.split("\n"), 1))
    catalog = result.catalog
    if catalog is not None:
        for path, document in sorted(catalog.documents.items()):
            if path.startswith("docs/ledgers/"):
                sources.extend((path, number, line) for number, line in enumerate(document.lines, 1) if isinstance(line, str) and not line.startswith("|"))
    for path, text in git_texts(root, result.revision, STATUS).items():
        sources.extend((path, number, line) for number, line in enumerate(text.split("\n"), 1))
    found: list[Residue] = []
    for path, number, line in sources:
        for value in dict.fromkeys(values(line)):
            if value not in known:
                found.append(Residue(path, number, value, line.strip()))
    return found


def inbound(result: Export, root: Path) -> list[Inbound]:
    texts = git_texts(root, result.revision, ".")
    targets = {path for path in texts if path.endswith(".md") and (path.startswith("docs/specs/") or (path.startswith(DIGESTS) and not path.startswith(ARCHIVE)))}
    found: list[Inbound] = []
    for path, text in sorted(texts.items()):
        directory = posixpath.dirname(path)
        for number, line in enumerate(text.split("\n"), 1):
            hits: set[str] = set()
            for candidate in [*(match[0] for match in PATH_MENTION.finditer(line)), *(match[1] for match in MARKDOWN_LINK.finditer(line))]:
                for resolved in (candidate, posixpath.normpath(posixpath.join(directory, candidate)), f"docs/{candidate}"):
                    if resolved in targets and resolved != path:
                        hits.add(resolved)
            found.extend(Inbound(path, number, target, line.strip()) for target in sorted(hits))
    return found


def text_citations(result: Export) -> dict[str, int]:
    """ID mentions kept as text: in CP, SL and RP cells, in compatibility/bug ledger prose, in evidence labels and notes."""
    counts: Counter[str] = Counter()
    for item in result.migrated:
        record = item.record
        if record.legacy is None:
            continue
        if record.type in ("compatibility", "lead", "report"):
            counts[f"{record.type} cells"] += sum(len(ID_IN_TEXT.findall(cell)) for name, cell in record.legacy.cells.items() if name != "original")
        if record.type == "evidence":
            counts["evidence label and note"] += sum(len(ID_IN_TEXT.findall(text)) for text in (record.text("label") or "", *record.notes))
    catalog = result.catalog
    if catalog is not None:
        for path in ("docs/ledgers/compatibility.md", "docs/ledgers/bugs.md"):
            document = catalog.documents.get(path)
            if document is not None:
                counts[f"{path} prose"] += sum(len(ID_IN_TEXT.findall(line)) for line in document.lines if isinstance(line, str) and not line.startswith("|"))
    return dict(counts)


def _ids(identifiers: Iterable[str]) -> str:
    return ", ".join(sorted(identifiers, key=id_key))


def render(result: Export, root: Path) -> str:
    records = [item.record for item in result.migrated]
    lines = [f"# Migration report for {result.revision}", "", f"Segment time (commit time): {result.created.isoformat()}", "", "## Records", ""]
    lines.extend(f"- {kind}: {count}" for kind, count in sorted(Counter(record.type for record in records).items()))
    lines.extend(["", "## Gaps", ""])
    gaps: dict[tuple[str, str], list[str]] = defaultdict(list)
    for record in records:
        for gap in record.gaps:
            name, _, detail = gap.partition(": ")
            gaps[(record.type, name)].append(f"{record.id} ({detail})" if detail else record.id)
    kinds = Counter[str]()
    for (_kind, name), identifiers in gaps.items():
        kinds[name] += len(identifiers)
    lines.append("Records per gap kind (all types): " + ", ".join(f"`{name}` {count}" for name, count in sorted(kinds.items())) + ".")
    lines.append(f"Records with at least one gap: {sum(1 for record in records if record.gaps)} of {len(records)}.")
    lines.append("")
    for (kind, name), identifiers in sorted(gaps.items()):
        lines.append(f"- {kind} `{name}`: {len(identifiers)}: {', '.join(sorted(identifiers, key=lambda item: id_key(item.split(' ', 1)[0])))}")
    lines.extend(["", "## Values outside the vocabulary (`legacy:`)", ""])
    legacy_values: dict[tuple[str, str, str], list[str]] = defaultdict(list)
    for record in records:
        for name, value in (("status", record.status), *record.fields.items()):
            if isinstance(value, str) and value.startswith(LEGACY):
                legacy_values[(record.type, name, value)].append(record.id)
    for (kind, name, value), identifiers in sorted(legacy_values.items(), key=lambda item: (item[0][0], item[0][1], -len(item[1]), item[0][2])):
        lines.append(f"- {kind} `{name}` `{value}`: {len(identifiers)}: {_ids(identifiers)}")
    lines.extend(["", "## Links taken from ledger prose", ""])
    lines.extend(f"- {link.old} superseded by {link.new}: {link.path}:{link.line}: {link.text}" for link in result.prose_links)
    lines.extend(["", "## Rows with several commits (scope keeps none)", ""])
    lines.extend(f"- {identifier}: {', '.join(commits)}" for identifier, commits in sorted(result.commits.items(), key=lambda item: id_key(item[0])))
    lines.extend(["", "## Judgments the rows force", ""])
    by_type: dict[str, list[str]] = defaultdict(list)
    for record in records:
        by_type[record.type].append(record.id)
    lines.append(f"- bug status `fixed` (no status column; the table states the former behavior and the change): {_ids(by_type['bug'])}")
    lines.append(f"- lead status `open` (the table calls the leads hypotheses to test): {_ids(by_type['lead'])}")
    lines.append(f"- reference status `current` (documents on the current surface): {_ids(by_type['reference'])}")
    standing = [record.id for record in records if record.type == "finding" and "standing" in record.fields]
    findings = len(by_type["finding"])
    if standing:
        lines.append(f"- finding `standing` set from the row's own words on {len(standing)} of {findings}: {_ids(standing)}")
    else:
        lines.append(f"- `standing` is unset on every one of the {findings} migrated findings: no legacy row states confirmed, candidate"
                     + " or refuted in so many words (as a `;`-separated part of a cell), so it is a gap on each")
    lines.extend(current_lines(records))
    lines.append("- `subjects` set: 0 (never: no legacy table states a subject)")
    scoped = Counter(record.type for record in records if record.scope is not None)
    lines.append("- records with a legacy-text scope: " + ", ".join(f"{kind} {count}" for kind, count in sorted(scoped.items())))
    lines.extend(["", "## Releases", ""])
    for record in records:
        if record.type == "release":
            stated = ", ".join(f"{name} {record.fields[name]}" for name in ("version_string", "server_build", "client_build") if name in record.fields)
            lines.append(f"- {record.id} ({record.status}): states {stated or 'no field'}; lacks {', '.join(record.gaps) or 'nothing'}")
            lines.extend(f"  - note: {note}" for note in record.notes)
    lines.extend(["", "## Citations kept as text (not edges)", ""])
    lines.extend(f"- {name}: {count}" for name, count in sorted(text_citations(result).items()))
    found = residue(result, root)
    lines.extend(["", f"## Residue: values in no migrated record ({len(found)})", ""])
    for path, items in _grouped(found):
        lines.append(f"### {path} ({len(items)})")
        lines.append("")
        lines.extend(f"- {item.line}: `{item.value}`" for item in items)
        lines.append("")
    references = inbound(result, root)
    lines.extend([f"## Inbound references to spec and digest paths ({len(references)})", ""])
    lines.extend(
        f"- {item.path}:{item.line} -> {item.target}" + (" (frozen)" if item.path.startswith(FROZEN) else "") for item in references
    )
    return "\n".join(lines).rstrip("\n") + "\n"


def current_lines(records: list[Record]) -> list[str]:
    """Partitions of the active findings against the current REL record, and why none can be ``current``."""
    state = replay([Segment(f"{JOURNAL}/report", "", tuple(Revision.of(record) for record in records))])
    release = next((record for record in records if record.type == "release" and record.status == "current"), None)
    if release is None:
        return ["- no REL record is current, so no partition is computed"]
    placed = Counter[str]()
    pins: list[str] = []
    for record in records:
        if record.type == "finding" and state.status(record.id) == "active":
            where = partition(record, release, lambda _commit: None)
            placed[where.name] += 1
            if where.names_pin:
                pins.append(record.id)
    lines = [f"- active findings against {release.id}: " + ", ".join(f"{name} {count}" for name, count in sorted(placed.items()))
             + f"; {len(pins)} legacy-scope rows name the pin ({_ids(pins) or 'none'})"]
    if not placed["current"]:
        lines.append(f"- no active finding is `current` for {release.id}: every migrated scope is `legacy-text`, which the partition"
                     + " rule places in `legacy-scope` (never verified, even when a token names the pin) or, without tokens, in"
                     + " `unscoped`; only an explicit scope, a `verified_on` for the release or a build-independent decision is"
                     + " `current`, and migration writes none of these")
    return lines


def _grouped(found: list[Residue]) -> list[tuple[str, list[Residue]]]:
    groups: dict[str, list[Residue]] = defaultdict(list)
    for item in found:
        groups[item.path].append(item)
    return sorted(groups.items())
