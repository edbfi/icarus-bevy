# SPDX-License-Identifier: AGPL-3.0-or-later
"""The mechanical rules by which a legacy row becomes a record (shared by every legacy converter).

``export(root, rev)`` reads the revision through the typed evidence catalog (``resolved_catalog`` and
``Index``: one parse path for F, U, N and M rows of every topic), parses what the catalog leaves as
text (the compatibility tables, the campaign and static-lead tables of ``bugs.md``, the specs and the
release profiles) with ``legacy_tables``, and returns every record once, as a first revision. Nothing
is checked out and no artifact is read or hashed.

Migration moves records; it never upgrades, invents or fills. Every rule is mechanical:

- **Verbatim.** Each record keeps its row in ``legacy``: the source path and every cell exactly as the
  catalog decodes it (``original`` holds the raw line where the catalog needed one for byte parity; an
  evidence row is kept whole, as its shard line, under ``row``). Every field the schema requires that
  the row does not state is named in ``gaps``.
- **Vocabulary.** A cell equal to a vocabulary value, ignoring case, is that value (``Unknown`` is
  ``unknown``); any other non-empty cell is ``legacy:<verbatim cell>``. A finding's ``confidence`` is the
  first ``;``-part of its Scope and limits cell (the legacy schema's confidence slot), mapped the same way.
- **Status.** The stored status is the status cell mapped as above. ``implied_status`` is
  ``superseded`` when the cell contains the word ``superseded``, else ``retired`` when it starts with
  ``retired`` and is not exactly that, else ``resolved`` for an unknown whose cell is ``resolved``.
- **Standing** is set only when a ``;``-separated part of one of the row's cells is exactly
  ``confirmed``, ``candidate`` or ``refuted``. **Subjects** are never set: no legacy table has a subject
  column, and the vocabulary's plain words (native, decision, tooling) are ordinary prose in the rows.
- **Scope** (``basis: legacy-text``) holds only tokens literally in the row's own cells:

  - a build is an 8-digit number starting with 2 (``BUILD``), alone or glued to ``build``, not part of
    a longer number, word, hex string or selector. It is ``server:<id>`` or ``client:<id>`` only when
    the word ``server`` or ``client`` stands immediately before it, optionally followed by ``build``
    (``ROLE``); otherwise ``any:<id>``. A role-tagged occurrence drops the ``any:`` form of that build.
  - a release is ``3.0.N`` or ``3.0.N.M`` (``RELEASE``), the version family of every release profile;
    other dotted versions in the rows name tools, not releases.
  - a commit is a 7 to 40 character lowercase hex word with at least one digit and one letter, not
    glued to a word, ``-`` or ``.`` (``COMMIT``), found only in a finding's Scope and limits cell
    (``SCOPE_COMMIT_CELLS``). A row whose scope cell names exactly one such word and no unclassified token
    has it as ``scope.commit``; otherwise the scope has no commit and a note says why (``scope commit not
    set: ...``), with ``scope`` a gap when the row yields no other token.
  - a **source revision** is the same kind of word in an evidence cell (``REVISION_CELLS``: a finding's
    Evidence, an unknown's REA ID, a compatibility row's Evidence, a lead's Static evidence). The legacy
    Evidence column is "evidence class; IDs; source revision" (``.agents/rules/docs.md``): its hash names
    the revision of the sources read, not a commit a property was measured on. It is never scope; the
    record keeps it as the note ``source revision (<cell> cell): a, b``.
  - an **unclassified** token is an all-digit word of commit length (7 to 40 digits) in either kind of
    cell that is not a build (``BUILD``); the ``COMMIT`` boundaries already exclude ID parts and dates.
    It may be a commit or a number; nothing mechanical decides (a Git lookup would differ between
    clones), so it is named in a note (``unclassified (<cell> cell): ...``) and no commit of that cell is
    presented as the row's.

  Elsewhere commit-like words are pawn IDs, tag hashes or GUID parts. A finding's claim is not a commit
  cell: 8 findings name a commit-like word only there (F-497's feature commit, but also F-702's tag
  hashes); the migration report lists them as a known limit (``claim_commits``).

  Builds named only in a cited evidence label are never copied. A row without tokens has no scope;
  ``scope`` is then a gap where the type requires it.
- **Links** are kept where the row stated them. From the catalog ``Index``: a status cell's
  ``superseded-by F-NNN`` is ``legacy.superseded_by``; every selector and every evidence ID in the row's
  evidence cell (a finding's Evidence, an unknown's REA ID) is ``cites``; every other ID is ``refs``. An
  unknown's Findings column lists related findings; it never says which one resolved the question, so
  they are ``refs`` and a resolved unknown derives ``resolved`` from its legacy status alone. A
  bare evidence ID that occurs only as the base of a selector in that cell is not repeated. Ranges are
  expanded by the catalog over defined IDs only (a range may end in a selector: ``M-1..M-4/ev_x`` cites
  M-1 to M-3 and the selector); the text stays verbatim in ``legacy``. An evidence row's
  ``superseded_by`` is ``legacy.superseded_by``. Ledger prose outside the tables that states a
  supersession (``PROSE_SUPERSESSION``: ``F-A superseded-by F-B``, ``F-A is retired in favor of F-B``)
  becomes ``legacy.superseded_by`` on the old record, with the prose line kept as a
  ``prose <path>:<line>`` cell. Compatibility, campaign and lead rows link the same way
  (``table_links``): selectors, and evidence IDs in the row's evidence cell, are ``cites``; every other ID
  is ``refs``. ID mentions in evidence labels and notes and in ledger prose stay text.
- **IDs.** F, U, N, M and SL keep theirs. Compatibility rows become ``CP-001...`` and campaign rows
  ``RP-001...`` in table order, specs ``REF-001...`` in path order, releases ``REL-x.y.z``.
- **Areas.** A finding's or evidence row's area is the catalog topic of its file (the ledger or shard
  folder named after the area); a spec's is its folder under ``docs/specs``.

Judgments the rows force, all listed in the migration report: the native defect and static lead tables
have no status column and their heading and preamble state no status of their rows, so those records
are ``legacy:unstated`` (``UNSTATED``), keep the heading and preamble verbatim as their ``table`` cell
and say so in a note (the released model has no ``status`` gap); specs on the current surface are
``current``; release profiles are ``past`` except the pin in ``config/release.toml``.

"""

from __future__ import annotations

import re
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass

from tools.docs_lint import LEGACY_NOTE, LISTED_SELECTOR
from tools.memory.model import JSON, LEGACY, STANDINGS, TYPES, Record, id_key

BUILD = re.compile(r"(?<![\w.])(?<![0-9],)(?:[Bb]uild)?(2[0-9]{7})(?!\w|[.,][0-9])")
ROLE = re.compile(r"\b(server|client)\s+(?:build\s+)?$", re.IGNORECASE)
RELEASE = re.compile(r"(?<![\w.])(3\.0\.[0-9]+(?:\.[0-9]+)?)(?!\w|\.[0-9])")
COMMIT = re.compile(r"(?<![\w.-])([0-9a-f]{7,40})(?![\w-])")
PROSE_SUPERSESSION = re.compile(r"\b(F-[0-9]{3,})\s+(?:superseded-by|is retired in favou?r of)\s+(F-[0-9]{3,})\b")
SELECTORS = (LISTED_SELECTOR, LEGACY_NOTE)
SELECTOR_SUFFIX = re.compile(r"\b(M-[0-9]{3,})/(?:(?:ev|unk)_[0-9a-f]+|E-[0-9]{3,})\b")
# The cells, per legacy ID prefix, that hold the row's evidence (cites) and source revision, and its scope commit.
EVIDENCE_CELL: dict[str, str] = {"F": "evidence", "U": "rea_id", "CP": "evidence", "SL": "static_evidence"}
REVISION_CELLS: dict[str, tuple[str, ...]] = {prefix: (cell,) for prefix, cell in EVIDENCE_CELL.items()}
SCOPE_COMMIT_CELLS: dict[str, tuple[str, ...]] = {"F": ("scope_and_limits",)}
UNSTATED = LEGACY + "unstated"
UNSTATED_NOTE = "status unstated: the table has no status column, and its heading and preamble (the table cell) state none"


class MigrationError(ValueError):
    """The legacy data at a revision cannot be migrated as the rules require."""


@dataclass(frozen=True)
class Token:
    """One scope token as extracted, with the text before it (for the hand audit)."""

    value: str
    cell: str
    context: str
    role: str | None = None


@dataclass(frozen=True)
class Extraction:
    """The scope a row's cells yield, every token behind it, and the notes on commit-like tokens it did not
    place in scope (``revisions``: source revisions; ``unplaced``: scope commits left out, and why)."""

    scope: dict[str, JSON] | None
    tokens: tuple[Token, ...]
    notes: tuple[str, ...] = ()
    revisions: tuple[str, ...] = ()
    unplaced: tuple[str, ...] = ()


@dataclass(frozen=True)
class Migrated:
    """A converted record, where it came from (``path:line``) and its scope extraction."""

    record: Record
    source: str
    extraction: Extraction | None = None


def vocabulary(value: str, allowed: Sequence[str]) -> str:
    """``value`` as a vocabulary value (case-insensitively equal) or ``legacy:<verbatim>``."""
    for item in allowed:
        if value.lower() == item:
            return item
    return LEGACY + value


def commit_words(text: str) -> tuple[list[str], list[str]]:
    """The distinct candidate words of ``text`` in order (commit-like or unclassified all-digit), and the
    unclassified ones among them."""
    found: dict[str, None] = {}
    unclassified: dict[str, None] = {}
    for match in COMMIT.finditer(text):
        word = match[1]
        if re.search(r"[a-f]", word) and re.search(r"[0-9]", word):
            found[word] = None
        elif word.isdigit() and not BUILD.fullmatch(word):
            found[word] = unclassified[word] = None
    return list(found), list(unclassified)


def label(cell: str) -> str:
    return {"scope_and_limits": "limits", "rea_id": "REA ID", "static_evidence": "static evidence"}.get(cell, cell)


def undecided(digits: Sequence[str]) -> str:
    return f"; all-digit {', '.join(digits)} {'is' if len(digits) == 1 else 'are'} a commit or a number, not decided at migration"


def extract(token_cells: Mapping[str, str], scope_cells: Iterable[str] = (), revision_cells: Iterable[str] = ()) -> Extraction:
    """Scope tokens of a row: builds and releases from every cell, a commit from ``scope_cells`` only; source
    revisions from ``revision_cells`` and unplaced commit candidates become notes (module docstring)."""
    tokens: list[Token] = []
    for name, text in token_cells.items():
        for match in BUILD.finditer(text):
            role = ROLE.search(text[:match.start()])
            tokens.append(Token(match[1], name, text[max(0, match.start() - 30):match.end()], role[1].lower() if role else None))
        tokens.extend(Token(match[1], name, text[max(0, match.start() - 30):match.end()]) for match in RELEASE.finditer(text))
    notes: list[str] = []
    revisions: list[str] = []
    for name in revision_cells:
        found, digits = commit_words(token_cells.get(name, ""))
        revisions += found
        if found:
            notes.append(f"source revision{' candidates' if digits else ''} ({label(name)} cell): {', '.join(found)}" + (undecided(digits) if digits else ""))
    candidates: list[str] = []
    unclassified: list[str] = []
    for name in scope_cells:
        found, digits = commit_words(token_cells.get(name, ""))
        candidates += found
        unclassified += digits
        if len(found) > 1 or digits:
            notes.append(f"scope commit not set: the {label(name)} cell names {', '.join(found)}" + (undecided(digits) if digits else " (several commits)"))
    distinct = tuple(word for word in dict.fromkeys(candidates) if word not in unclassified)
    placed = len(distinct) == 1 and not unclassified
    unplaced = () if placed else tuple(dict.fromkeys(candidates))
    tokens.extend(Token(word, "scope", word, "commit") for word in distinct)
    builds = {f"{token.role or 'any'}:{token.value}" for token in tokens if BUILD.fullmatch(token.value) and token.role != "commit"}
    builds -= {f"any:{token.value}" for token in tokens if token.role in ("server", "client")}
    releases = {token.value for token in tokens if RELEASE.fullmatch(token.value)}
    scope: dict[str, JSON] = {"basis": "legacy-text"}
    if builds:
        scope["builds"] = list[JSON](sorted(builds, key=id_key))
    if releases:
        scope["releases"] = list[JSON](sorted(releases))
    if placed:
        scope["commit"] = distinct[0]
    return Extraction(scope if len(scope) > 1 else None, tuple(tokens), tuple(notes), tuple(dict.fromkeys(revisions)), unplaced)


def standing(cells: Iterable[str]) -> str | None:
    """The standing a row states in so many words: a ``;``-part of a cell that is exactly a standing."""
    found = {part.strip() for cell in cells for part in cell.split(";")} & set(STANDINGS)
    return found.pop() if len(found) == 1 else None


def implied_status(cell: str, kind: str) -> str | None:
    lowered = cell.lower()
    if re.search(r"\bsuperseded\b", lowered):
        return "superseded"
    if lowered.startswith("retired") and lowered != "retired":
        return "retired"
    if kind == "unknown" and lowered == "resolved":
        return "resolved"
    return None


def finish(data: dict[str, JSON], legacy: dict[str, JSON], extraction: Extraction | None = None, body: str | None = None) -> Record:
    """Add ``legacy``, the extraction's notes (after any the row itself holds) and the gaps (required fields
    the row does not state), then validate."""
    data["legacy"] = legacy
    kind = str(data["type"])
    if extraction is not None and extraction.notes:
        held = data.get("notes")
        data["notes"] = [*(held if isinstance(held, list) else []), *extraction.notes]
    gaps = [name for name in TYPES[kind].required if name not in data and not (name == "body" and body is not None)]
    if gaps:
        data["gaps"] = list[JSON](sorted(gaps, key=id_key))
    return Record.from_json(data, body)
