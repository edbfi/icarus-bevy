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
    glued to a word, ``-`` or ``.`` (``COMMIT``), found only in the cells the legacy schema reserves for
    source revisions and limits (a finding's Evidence and Scope and limits cells, the Evidence cell of a
    compatibility row, a lead's Static evidence). Elsewhere such words are pawn
    IDs, tag hashes or GUID parts. A row with several distinct commits keeps none and names them in a gap,
    ``scope.commit: several commits in row (a, b)``.
    A finding's claim is not such a cell: 8 findings name a commit-like word only there (F-497's feature
    commit, but also F-702's tag hashes), and nothing mechanical tells which is a commit; the migration
    report lists them as a known limit (``claim_commits``).

  Builds named only in a cited evidence label are never copied. A row without tokens has no scope;
  ``scope`` is then a gap where the type requires it.
- **Links** are kept where the row stated them. From the catalog ``Index``: a status cell's
  ``superseded-by F-NNN`` is ``legacy.superseded_by``; a resolved unknown's Findings cell names the
  findings that resolve it (``legacy.resolved_by``); every selector and every evidence ID in the row's
  evidence cell (a finding's Evidence, an unknown's REA ID) is ``cites``; every other ID is ``refs``. A
  bare evidence ID that occurs only as the base of a selector in that cell is not repeated. Ranges are
  expanded by the catalog over defined IDs only (a range may end in a selector: ``M-1..M-4/ev_x`` cites
  M-1 to M-3 and the selector); the text stays verbatim in ``legacy``. An evidence row's
  ``superseded_by`` is ``legacy.superseded_by``. Ledger prose outside the tables that states a
  supersession (``PROSE_SUPERSESSION``: ``F-A superseded-by F-B``, ``F-A is retired in favor of F-B``)
  becomes ``legacy.superseded_by`` on the old record, with the prose line kept as a
  ``prose <path>:<line>`` cell. Citations in compatibility and bug-table cells and ID mentions in
  evidence labels and notes stay text.
- **IDs.** F, U, N, M and SL keep theirs. Compatibility rows become ``CP-001...`` and campaign rows
  ``RP-001...`` in table order, specs ``REF-001...`` in path order, releases ``REL-x.y.z``.
- **Areas.** A finding's or evidence row's area is the catalog topic of its file (the ledger or shard
  folder named after the area); a spec's is its folder under ``docs/specs``.

Judgments the rows force, all listed in the migration report: native defects (no status column; the
table states each row's "former behavior" and its "change") are stored ``fixed``; static leads (the
table calls them "hypotheses to test") are ``open``; specs on the current surface are ``current``;
release profiles are ``past`` except the pin in ``config/release.toml``.

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
# The cells, per legacy ID prefix, that hold the row's evidence (cites) and its source revision (commits).
EVIDENCE_CELL: dict[str, str] = {"F": "evidence", "U": "rea_id"}
COMMIT_CELLS: dict[str, tuple[str, ...]] = {"F": ("evidence", "scope_and_limits"), "U": ("rea_id",)}


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
    """The scope a row's cells yield, and every token behind it."""

    scope: dict[str, JSON] | None
    tokens: tuple[Token, ...]
    commits: tuple[str, ...]


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


def extract(token_cells: Mapping[str, str], commit_cells: Iterable[str] = ()) -> Extraction:
    """Scope tokens of a row: builds and releases from every cell, commits from ``commit_cells`` only."""
    tokens: list[Token] = []
    for name, text in token_cells.items():
        for match in BUILD.finditer(text):
            role = ROLE.search(text[:match.start()])
            tokens.append(Token(match[1], name, text[max(0, match.start() - 30):match.end()], role[1].lower() if role else None))
        tokens.extend(Token(match[1], name, text[max(0, match.start() - 30):match.end()]) for match in RELEASE.finditer(text))
    commits: list[str] = []
    for name in commit_cells:
        for match in COMMIT.finditer(token_cells.get(name, "")):
            word = match[1]
            if re.search(r"[0-9]", word) and re.search(r"[a-f]", word):
                tokens.append(Token(word, name, token_cells[name][max(0, match.start() - 30):match.end()], "commit"))
                commits.append(word)
    distinct = tuple(dict.fromkeys(commits))
    builds = {f"{token.role or 'any'}:{token.value}" for token in tokens if BUILD.fullmatch(token.value) and token.role != "commit"}
    builds -= {f"any:{token.value}" for token in tokens if token.role in ("server", "client")}
    releases = {token.value for token in tokens if RELEASE.fullmatch(token.value)}
    scope: dict[str, JSON] = {"basis": "legacy-text"}
    if builds:
        scope["builds"] = list[JSON](sorted(builds, key=id_key))
    if releases:
        scope["releases"] = list[JSON](sorted(releases))
    if len(distinct) == 1:
        scope["commit"] = distinct[0]
    return Extraction(scope if len(scope) > 1 else None, tuple(tokens), distinct)


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
    """Add ``legacy`` and the gaps, then validate. Gaps: required fields the row does not state, and
    ``scope.commit: several commits in row (a, b)`` when the row names more than one commit."""
    data["legacy"] = legacy
    kind = str(data["type"])
    gaps = [name for name in TYPES[kind].required if name not in data and not (name == "body" and body is not None)]
    if extraction is not None and len(extraction.commits) > 1:
        gaps.append(several_commits(extraction.commits))
    if gaps:
        data["gaps"] = list[JSON](sorted(gaps, key=id_key))
    return Record.from_json(data, body)


def several_commits(commits: Sequence[str]) -> str:
    return f"scope.commit: several commits in row ({', '.join(commits)})"
