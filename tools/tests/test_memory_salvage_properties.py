# SPDX-License-Identifier: AGPL-3.0-or-later
"""Salvage's classification (``salvage.classify``) on generated histories: fixed seeds, a few hundred small cases.

Each case has 3 to 6 migrated rows at the merge base, a branch that edits their status cells (active, retired,
``retired: ...``, ``superseded-by`` one or two successors, a cell outside the vocabulary, a claim change), new
rows (some superseding each other, so cycles occur), a successor that exists nowhere, a row ``main`` changed
independently and a row ``main`` lacks. Every case must satisfy ``salvage.invariants`` (it raises otherwise),
and the proposals must pass the replay checks ``check --ci`` applies."""

from __future__ import annotations

import hashlib
import random
import unittest

from tools.memory.check import revision_problems
from tools.memory.journal import Revision, Segment
from tools.memory.legacy import TRACKED_NOTE
from tools.memory.legacy_prose import section
from tools.memory.legacy_rules import extract
from tools.memory.model import JSON, Record, id_key
from tools.memory.salvage import (DOCUMENT_NOTE, SALVAGED, Accepted, Pending, Salvage, SalvageError, body_tokens, classify, document_path, identity,
                                  pending_documents, relink, renumber)
from tools.memory.state import State, replay
from tools.tests.memory_fixtures import evidence, finding

SEEDS = range(300)


def row(identifier: str, cell: str, claim: str = "Invented claim", successors: tuple[str, ...] = (), original: str | None = None) -> Record:
    """A migrated finding as the exporter writes one: stored status from the cell, implied status and successors."""
    lowered = cell.lower()
    implied = "superseded" if "superseded" in lowered else "retired" if lowered.startswith("retired") and lowered != "retired" else None
    cells: dict[str, JSON] = {"claim": f"{claim} {identifier}", "status": cell}
    if original is not None:
        cells["original"] = original  # the raw line: a change to it alone is cosmetic
    legacy: dict[str, JSON] = {"path": "docs/ledgers/findings/generated.md", "cells": cells}
    if implied is not None:
        legacy["implied_status"] = implied
    if successors:
        legacy["superseded_by"] = list(successors)
    data = finding(identifier, f"{claim} {identifier}", status=cell if cell in ("active", "retired") else f"legacy:{cell}", legacy=legacy)
    del data["cites"]
    return Record.from_json(data)


SAME_BODY = "# One body for several documents\n"


def exporter_scope(path: str, body: str) -> tuple[bool, dict[str, JSON] | None]:
    """The exporter's rule for a document's scope (``legacy_prose.references``): a spec's Status and Scope sections when
    it has them, else its whole text; and whether the sections were read."""
    sections = {name.lower(): section(body, name) for name in ("Status", "Scope")} if path.startswith("docs/specs/") else {}
    return any(sections.values()), extract(sections if any(sections.values()) else {"body": body}).scope


def scope_tokens(scope: dict[str, JSON] | None) -> set[str]:
    return {item for key in ("builds", "releases") if scope is not None and isinstance(items := scope.get(key), list)
            for item in items if isinstance(item, str)}


def document(identifier: str, path: str, body: str | None = None) -> Record:
    text = f"# {path}\n" if body is None else body
    scope: dict[str, JSON] = exporter_scope(path, text)[1] or {"basis": "explicit", "subjects": ["tooling"]}
    return Record.from_json({"id": identifier, "type": "reference", "status": "current", "title": path, "scope": scope,
                             "body_sha256": hashlib.sha256(text.encode()).hexdigest(), "legacy": {"path": path, "cells": {"document": path}}}, text)


def surface(identifier: str, name: str) -> Record:
    return Record.from_json({"id": identifier, "type": "compatibility", "status": "unknown", "name": name, "required_proof": "Invented proof",
                             "assigned_at_migration": True, "gaps": ["qualifier", "scope"],
                             "legacy": {"path": "docs/ledgers/compatibility.md", "cells": {"surface": name, "status": "Unknown", "required_proof": "Invented proof"}}})


def tracked(identifier: str, path: str, ref: str, status: str | None = None) -> Record:
    line = "{}" if status is None else f'{{"status":"{status}"}}'
    data = evidence(identifier, path, refs=[ref], notes=[TRACKED_NOTE.format(ref)], legacy={"path": "evidence/x.jsonl", "cells": {"row": line}})
    if status is not None:
        data["status"] = status
    data["class"] = "tracked"
    return Record.from_json(data)


def numbered(paths: list[str], names: list[str]) -> tuple[dict[str, str], dict[str, str]]:
    """An export's positional numbers: REF-N by path order, CP-N by table order (a repeated name: its first row)."""
    cps: dict[str, str] = {}
    for number, name in enumerate(names, 1):
        _ = cps.setdefault(name, f"CP-{number:03d}")
    return {path: f"REF-{number:03d}" for number, path in enumerate(sorted(paths), 1)}, cps


def base_ids(before: dict[str, Record]) -> list[str]:
    return sorted(name for name, record in before.items() if record.type == "finding")


def rewrite_case(after: dict[str, Record]) -> bool:
    return any(record.body == "# c.md, rewritten\n" and record.legacy is not None for record in after.values())


def retoken_case(after: dict[str, Record]) -> bool:
    return any((record.body or "").startswith("# e.md, re-observed\n") for record in after.values())


def problems(state: State) -> set[str]:
    return {*state.problems, *revision_problems(state), *state.link_problems(), *state.supersession_cycles()}


def superseding_document(identifier: str, path: str, old: str | None, body: str) -> Record:
    """A REF proposed by hand for ``path`` (its note names it), superseding ``old`` (if any)."""
    data: dict[str, JSON] = {"id": identifier, "type": "reference", "status": "current", "title": path, "scope": {"basis": "explicit"},
                             "body_sha256": hashlib.sha256(body.encode()).hexdigest(), "notes": [DOCUMENT_NOTE.format(path)]}
    if old is not None:
        data["supersedes"] = [old]
    return Record.from_json(data, body)


def accepted_of(segments: tuple[Segment, ...]) -> Accepted:
    """What ``accepted_on`` reads from a main holding ``segments``."""
    state = replay(segments)
    return Accepted({identifier: entry.record for identifier, entry in state.entries.items()},
                    {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal", segments,
                    frozenset(identifier for identifier in state.entries if state.retired(identifier)))


def case(seed: int) -> tuple[dict[str, Record], dict[str, Record], Accepted]:
    rng = random.Random(seed)
    base_ids = [f"F-{100 + number}" for number in range(rng.randint(3, 6))]
    fresh_ids = [f"F-{200 + number}" for number in range(rng.randint(0, 2))]
    before = {identifier: row(identifier, rng.choice(["active", "active", "retired", "retired: old"])) for identifier in base_ids}
    after = dict(before)
    candidates = [*base_ids, *fresh_ids, "F-999"]
    for identifier in base_ids:
        choice = rng.random()
        if choice < 0.2:
            continue
        if choice < 0.45:
            count = rng.choice([1, 1, 2])
            # New rows are named often, so a new row gets two incoming links and, naming one back, a cycle.
            pool = [item for item in candidates if item != identifier] + fresh_ids * 2
            named = tuple(dict.fromkeys(rng.sample(pool, min(count, len(pool)))))
            after[identifier] = row(identifier, f"superseded-by {named[0]}", successors=named)
        elif choice < 0.75:
            after[identifier] = row(identifier, rng.choice(["active", "retired", "retired: replaced", "parked"]))
        else:
            after[identifier] = row(identifier, "active", claim="Changed claim")
    if rng.random() < 0.25 and len(base_ids) >= 2:
        # A cosmetic-only successor of a superseded row: the raw line changed, nothing else.
        successor, old = base_ids[-1], base_ids[-2]
        held = before[successor].legacy
        before_cell = held.cells["status"] if held is not None else "active"
        after[successor] = row(successor, str(before_cell), original=f"|  {successor}  | spaced |")
        after[old] = row(old, f"superseded-by {successor}", successors=(successor,))
    for identifier in fresh_ids:
        if rng.random() < 0.4:
            other = rng.choice([item for item in candidates if item != identifier])
            after[identifier] = row(identifier, f"superseded-by {other}", successors=(other,))
        else:
            after[identifier] = row(identifier, "active")
    # Migration-assigned records: the branch may insert a document sorting first and a compatibility row in the middle,
    # renumbering the rest in its export; an unchanged tracked row and a finding cite later ones by those numbers.
    # Main may itself hold one name twice (two accepted rows of one identity).
    twins = rng.random() < 0.2
    main_paths, main_names = ["docs/research/net/d.md", "docs/specs/c.md", "docs/specs/e.md"], ["Alpha", "Gamma", "Gamma"] if twins else ["Alpha", "Gamma"]
    branch_paths = ["docs/specs/a.md", *main_paths] if rng.random() < 0.5 else main_paths
    retoken, rewrite_c, two_holders = rng.random() < 0.3, rng.random() < 0.3, rng.random() < 0.2
    renewed = not retoken and rng.random() < 0.3  # e.md's body newly names a build outside its (unchanged) scope sections
    main_e = rng.random() < 0.3  # main changes e.md since the merge base
    digest = rng.random() < 0.3  # the branch changes the digest d.md
    # Documents sharing one body: two new ones on the branch; an unrelated REF of that body on main (no identity).
    same_body = rng.random() < 0.3
    if same_body and "docs/specs/a.md" in branch_paths:
        branch_paths = [*branch_paths, "docs/specs/b.md"]
    branch_names = ["Alpha", "Beta", *main_names[1:]] if rng.random() < 0.5 else list(main_names)
    if twins and rng.random() < 0.4:
        branch_names.remove("Gamma")  # the branch drops one of main's twins: the other is still ambiguous on main
    omega = rng.random() < 0.15
    if omega:
        branch_names[0] = "Omega"  # a rename to a name main added after the merge base
    elif twins and rng.random() < 0.3 and "Gamma" in branch_names:
        branch_names[branch_names.index("Gamma")] = "Gamma renamed"  # a rename of one of main's twins
    elif rng.random() < 0.25:
        branch_names[0] = "Alpha renamed"  # a rename: an accepted name lost, a new one gained
    if rng.random() < 0.25:
        branch_names.append("Delta")  # an unrelated new row
    # A repeated name: two new rows of one name, or a second row of an accepted name.
    repeated = rng.choice([[], [], ["Dup", "Dup"], ["Gamma"]])
    branch_names = [*branch_names, *repeated]
    for paths, names, export in ((main_paths, main_names, before), (branch_paths, branch_names, after)):
        refs, cps = numbered(paths, names)
        export.update({refs[path]: document(refs[path], path, SAME_BODY if same_body and path in ("docs/specs/a.md", "docs/specs/b.md") else None)
                       for path in paths})
        if export is after and retoken:
            # The branch's e.md text changes and names a build the accepted REF's scope does not hold.
            export[refs["docs/specs/e.md"]] = document(refs["docs/specs/e.md"], "docs/specs/e.md", "# e.md, re-observed\n\n## Status\n\nObserved with server build 29990001.\n")
        if export is after and renewed:
            export[refs["docs/specs/e.md"]] = document(refs["docs/specs/e.md"], "docs/specs/e.md", "# e.md\nSeen on build 29990005.\n")
        if export is after and digest:
            # A digest: its scope is read from its whole text.
            export[refs["docs/research/net/d.md"]] = document(refs["docs/research/net/d.md"], "docs/research/net/d.md", "# d.md\nSeen on build 29990007.\n")
        if export is after and rewrite_c:
            export[refs["docs/specs/c.md"]] = document(refs["docs/specs/c.md"], "docs/specs/c.md", "# c.md, rewritten\n")
        export.update({f"CP-{number:03d}": surface(f"CP-{number:03d}", name) for number, name in enumerate(names, 1)})
        if twins and export is after and "Gamma" in names and rng.random() < 0.5:
            # The branch changes one of main's twin rows.
            last = max(f"CP-{number:03d}" for number, name in enumerate(names, 1) if name == "Gamma")
            export[last] = Record.from_json(export[last].to_json() | {"status": "observed"})
        export["M-500"] = tracked("M-500", "docs/specs/e.md", refs["docs/specs/e.md"])
        cited = export[base_ids[0]].to_json() | {"refs": [cps["Gamma" if not twins else names[1] if names[1] != "Gamma" else names[0]]]}
        export[base_ids[0]] = Record.from_json(cited)
    if "docs/specs/a.md" in branch_paths and rng.random() < 0.5:
        after["M-501"] = tracked("M-501", "docs/specs/a.md", numbered(branch_paths, branch_names)[0]["docs/specs/a.md"])
    if rng.random() < 0.3:
        # The branch retires the tracked row M-500, whose accepted record cites e.md's REF as the cut-over numbered it.
        after["M-500"] = tracked("M-500", "docs/specs/e.md", numbered(branch_paths, branch_names)[0]["docs/specs/e.md"], "retired")
    if rng.random() < 0.5:
        # A new tracked row citing e.md as the branch has it (main's text may differ).
        after["M-502"] = tracked("M-502", "docs/specs/e.md", numbered(branch_paths, branch_names)[0]["docs/specs/e.md"])
    if "Beta" in branch_names and rng.random() < 0.5:
        after["F-299"] = Record.from_json(row("F-299", "active").to_json() | {"refs": [numbered(branch_paths, branch_names)[1]["Beta"]]})
    revisions = {identifier: Revision.of(record) for identifier, record in before.items()}
    main_edits: list[Revision] = []
    if rewrite_c and not two_holders and rng.random() < 0.4:
        # Main changed c.md too since the merge base: a body revision would drop its edit.
        main_c = numbered(main_paths, main_names)[0]["docs/specs/c.md"]
        main_edits.append(Revision.of(document(main_c, "docs/specs/c.md", "# c.md, main's edit\n"), [revisions[main_c].rev]))
    if main_e:
        main_e_ref = numbered(main_paths, main_names)[0]["docs/specs/e.md"]
        main_edits.append(Revision.of(document(main_e_ref, "docs/specs/e.md", "# e.md, main's edit\n"), [revisions[main_e_ref].rev]))
    if rng.random() < 0.25:
        # Main superseded e.md's REF before this run by a REF naming e.md by note (the branch's text, or other text), or
        # (the supersession forgotten) holds both in force.
        main_e_ref = numbered(main_paths, main_names)[0]["docs/specs/e.md"]
        branch_e = next(record.body or "" for record in after.values() if document_path(record) == "docs/specs/e.md")
        # Or main re-homed e.md: a new REF with the text the merge base had, superseding the migrated one.
        body = rng.choice([branch_e, "# e.md, on main\n", revisions[main_e_ref].record.body or ""])
        revisions["REF-960"] = Revision.of(superseding_document("REF-960", "docs/specs/e.md", main_e_ref if rng.random() < 0.7 else None, body))
        if rng.random() < 0.3:
            # Main's M-500 then cites both e.md's old REF and REF-960: equating the two would collapse them into one citation.
            held = revisions["M-500"]
            main_edits.append(Revision.of(Record.from_json({**held.record.to_json(), "refs": [main_e_ref, "REF-960"]}), [held.rev]))
    if "REF-960" not in revisions and not main_e and rng.random() < 0.15:
        # Main retired e.md's REF: no accepted REF stands for e.md, which the branch's rows may still cite.
        e_ref = numbered(main_paths, main_names)[0]["docs/specs/e.md"]
        held = revisions[e_ref]
        main_edits.append(Revision.of(Record.from_json({**held.record.to_json(), "status": "retired"}, held.record.body), [held.rev]))
    elif "REF-960" not in revisions and rng.random() < 0.15:
        # Main superseded e.md's REF by a reference of another path: none stands for e.md any more.
        revisions["REF-985"] = Revision.of(superseding_document("REF-985", "docs/specs/z.md", numbered(main_paths, main_names)[0]["docs/specs/e.md"], "# z.md\n"))
    if "docs/specs/a.md" in branch_paths and rng.random() < 0.25:
        # Main added a.md itself since the merge base, by hand, with other text than the branch's.
        revisions["REF-970"] = Revision.of(superseding_document("REF-970", "docs/specs/a.md", None, "# a.md, on main\n"))
    if two_holders:
        # A second accepted REF naming c.md by note, maybe with the branch's new body (a superseding REF accepted), and
        # maybe superseding the first (the supersession accepted too: then it alone is in force).
        body = "# c.md, rewritten\n" if rng.random() < 0.5 else "# c.md, by hand\n"
        hand: dict[str, JSON] = {"id": "REF-951", "type": "reference", "status": "current", "title": "c.md", "scope": {"basis": "explicit"},
                                 "body_sha256": hashlib.sha256(body.encode()).hexdigest(), "notes": [DOCUMENT_NOTE.format("docs/specs/c.md")]}
        if rng.random() < 0.5:
            hand["supersedes"] = [numbered(main_paths, main_names)[0]["docs/specs/c.md"]]
        revisions["REF-951"] = Revision.of(Record.from_json(hand, body))
    if omega:
        revisions["CP-900"] = Revision.of(Record.from_json({"id": "CP-900", "type": "compatibility", "status": "unknown", "name": "Omega",
                                                            "qualifier": "Generated", "required_proof": "Generated proof",
                                                            "scope": {"basis": "explicit", "subjects": ["original-server"]}}))
    if same_body:
        # An unrelated accepted REF with the same body and no document identity: it must never stand for a.md or b.md.
        unrelated = Record.from_json({"id": "REF-950", "type": "reference", "status": "current", "title": "Unrelated", "scope": {"basis": "explicit"},
                                      "body_sha256": hashlib.sha256(SAME_BODY.encode()).hexdigest()}, SAME_BODY)
        revisions["REF-950"] = Revision.of(unrelated)
    later: list[Revision] = []
    if rng.random() < 0.5:
        changed = rng.choice(base_ids)
        record = before[changed]
        later.append(Revision.of(Record.from_json({**record.to_json(), "status": "retired", "notes": ["Generated: main retired it"]}),
                                 [revisions[changed].rev]))
    if rng.random() < 0.3:
        _ = revisions.pop(rng.choice(base_ids[1:]))
    segments = (Segment("memory/journal/generated.jsonl", "", (*revisions.values(), *later, *main_edits)),)
    state = replay(segments)
    accepted = Accepted({identifier: entry.record for identifier, entry in state.entries.items()},
                        {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal", segments,
                        frozenset(identifier for identifier in state.entries if state.retired(identifier)))
    return after, before, accepted


def standing_refs(on_accepted: Accepted) -> dict[str, list[Record]]:
    """The accepted REFs still standing for each document path (not superseded or retired)."""
    found: dict[str, list[Record]] = {}
    for name, record in on_accepted.records.items():
        if (path := document_path(record)) is not None and name not in on_accepted.retired:
            found.setdefault(path, []).append(record)
    return found


def settle_documents(on_accepted: Accepted, listed: list[Pending]) -> tuple[Accepted, set[str]]:
    """``on_accepted`` once each listed document is settled as the runbook's facts allow: a REF with the note holding the
    branch's body, superseding every REF in force for its path (none of which then stands for it)."""
    top = max((int(name.split("-", 1)[1]) for name in on_accepted.records if name.startswith("REF-")), default=0)
    revisions: list[Revision] = []
    for pending in listed:
        top += 1
        data: dict[str, JSON] = {"id": f"REF-{top:03d}", "type": "reference", "status": "current", "title": pending.path, "scope": {"basis": "explicit"},
                                 "body_sha256": hashlib.sha256(pending.body.encode()).hexdigest(), "notes": [DOCUMENT_NOTE.format(pending.path)]}
        if pending.in_force:
            data["supersedes"] = [held.id for held in pending.in_force]
        revisions.append(Revision.of(Record.from_json(data, pending.body)))
    return accepted_of((*on_accepted.segments, Segment("memory/journal/settled-documents.jsonl", "", tuple(revisions)))), {pending.path for pending in listed}


class GeneratedSalvageTests(unittest.TestCase):
    def assert_documents(self, where: str, raw_after: dict[str, Record], raw_before: dict[str, Record], on_accepted: Accepted) -> None:
        """The document facts of one run against ``on_accepted``: every document that is not done is listed and no done
        one; each printed fact is true of the accepted state and the branch; nothing printed is a command; and once a
        listed document is settled, the next run no longer lists it."""
        (after, before), unmapped, shared = renumber((raw_after, raw_before), on_accepted.records, on_accepted.retired)
        result = Salvage("generated", "7" * 40, "b" * 40, "c" * 40)
        _ = classify(result, after, before, on_accepted, unmapped, shared)
        listed = pending_documents(after, before, on_accepted, result.cited)
        self.assertEqual(len(listed), len({pending.path for pending in listed}), f"{where}: a document listed twice")
        by_path = {pending.path: pending for pending in listed}
        branch = {path: record for record in raw_after.values() if (path := document_path(record)) is not None}
        based = {path: record for record in before.values() if (path := document_path(record)) is not None}
        standing = standing_refs(on_accepted)
        self.assertEqual(set[str](), set(by_path) - set(branch), f"{where}: a document the branch does not hold is listed")
        # Every row held because it cites a document's provisional number is named beside that document.
        numbered_as = {path: name for name, record in after.items() if (path := document_path(record)) is not None}
        waiting = {path: {held for held, reason in result.undecided.items() if f"({name}, a provisional number)" in reason}
                   for path, name in numbered_as.items() if name in unmapped}
        for path, record in branch.items():
            holding = sorted(standing.get(path, []), key=lambda other: id_key(other.id))
            done = len(holding) == 1 and holding[0].body == record.body
            changed = path not in based or record.body != based[path].body
            # The branch's own documents, and those held rows cite as the branch has them; never main's later edits alone.
            if done or not (changed or path in result.cited):
                self.assertNotIn(path, by_path, f"{where}: {path} listed though {'it is done' if done else 'the branch did not change it'}")
                continue
            self.assertIn(path, by_path, f"{where}: {path} is not done ({[other.id for other in holding]} in force) and is not listed")
            pending = by_path[path]
            names = record.scope.tokens if record.scope is not None else ()
            text = body_tokens(record.body or "")
            self.assertEqual([(other.id, other.scope.tokens if other.scope is not None else (), other.body == record.body,
                               set() if other.body == record.body else text - body_tokens(other.body or "")) for other in holding],
                             [(held.id, held.scope, held.holds, set(held.newly)) for held in pending.in_force], f"{where}: {path}, the REFs in force")
            # The scope as the exporter reads it for this kind of document, and the label saying where it was read.
            from_sections, scope = exporter_scope(path, record.body or "")
            self.assertEqual((from_sections, scope_tokens(scope), set(body_tokens(record.body or "")) - set(names) if from_sections else set()),
                             (pending.from_sections, set(pending.names), set(pending.more)), f"{where}: {path}, tokens")
            label = "from its Status and Scope sections" if from_sections else "from its whole text"
            self.assertIn(f"the exporter reads as its scope, {label}: ", pending.line(), f"{where}: {path}")
            self.assertEqual(from_sections, "its body also names" in pending.line(), f"{where}: {path}")
            self.assertEqual(path not in based and bool(holding) or path in based and [based[path]] != holding, pending.main_changed,
                             f"{where}: {path}, main changed it")
            self.assertEqual(sorted(waiting.get(path, set())), sorted(pending.citing), f"{where}: {path}, the rows citing it")
            self.assertEqual(changed, pending.branch_changed, f"{where}: {path}, the branch changed it")
            # The base's reference: the REF the cut-over migrated from this path, and what became of it.
            migrated = sorted((name for name, other in on_accepted.records.items() if other.type == "reference" and other.legacy is not None
                               and other.legacy.path == path), key=id_key)
            if path in based and not migrated and based[path].id in on_accepted.records:
                migrated = [based[path].id]

            def fate(name: str) -> str:
                superseders = sorted((other.id for other in on_accepted.records.values() if name in other.links.supersedes), key=id_key)
                return ("now in force" if name not in on_accepted.retired else f"superseded by {', '.join(superseders)}" if superseders else "retired")

            self.assertEqual(None if path not in based else tuple((name, fate(name)) for name in migrated), pending.based_on, f"{where}: {path}, its base REF")
            for fact in [*pending.facts("FILE"), pending.line()]:
                self.assertFalse(fact.startswith("memory "), f"{where}: {path}: {fact}")
        settled_state, settled = settle_documents(on_accepted, listed)
        self.assertEqual(set[str](), problems(replay(settled_state.segments)) - problems(replay(on_accepted.segments)), f"{where}: settling")
        (after_settled, before_settled), unmapped_settled, shared_settled = renumber((raw_after, raw_before), settled_state.records, settled_state.retired)
        next_run = Salvage("generated", "8" * 40, "b" * 40, "c" * 40)
        _ = classify(next_run, after_settled, before_settled, settled_state, unmapped_settled, shared_settled)
        again = [pending.path for pending in pending_documents(after_settled, before_settled, settled_state, next_run.cited) if pending.path in settled]
        self.assertEqual([], again, f"{where}: listed again once settled")

    def test_every_generated_case_keeps_the_invariants_and_replays_cleanly(self) -> None:
        for seed in SEEDS:
            raw_after, raw_before, accepted = case(seed)
            self.assert_documents(f"seed {seed}, run 1", raw_after, raw_before, accepted)
            after, before = raw_after, raw_before
            (after, before), unmapped, shared = renumber((after, before), accepted.records, accepted.retired)
            result = Salvage("generated", "a" * 40, "b" * 40, "c" * 40)
            try:
                _ = classify(result, after, before, accepted, unmapped, shared)
            except SalvageError as error:
                self.fail(f"seed {seed}: {error}")
            base = replay(accepted.segments)
            proposed = replay([*accepted.segments, Segment("memory/journal/proposals.jsonl", "", tuple(result.proposals))])
            self.assertEqual(set[str](), problems(proposed) - problems(base), f"seed {seed}")
            # Another spelling of the ref and another tip (an unrelated commit): byte-identical proposals, same classes.
            # No record is lost: the translated export holds every record, and every changed one is reported.
            self.assertEqual(len(raw_after), len(after), f"seed {seed}: renumbering lost a record")
            # A branch REF whose text differs from main's is numbered provisionally; for an unchanged row it is main's REF.
            standins = {name: other for name, reason in unmapped.items() if "as this branch has it" in reason
                        for other, base in before.items() if identity(base) == identity(after[name])}
            changed = {name for name, record in after.items() if record.type in SALVAGED and before.get(name) != relink(record, standins)}
            self.assertEqual(set[str](), changed - set(result.kinds), f"seed {seed}: changed rows not reported")
            for name in shared:
                if name in changed:
                    self.assertEqual("undecided", result.kinds[name], f"seed {seed}: a row sharing an identity")
            if raw_after["M-500"].status == raw_before["M-500"].status:
                self.assertNotIn("M-500", result.kinds, f"seed {seed}: an unchanged tracked row renumbered by the branch")
            # (Invariant (f) checks no proposal cites a REF, CP or RP whose identity differs from the branch record it stood for.)
            e_alone = len(standing_refs(accepted).get("docs/specs/e.md", [])) == 1
            # Specific documents: e.md's extra build, c.md's two REFs in force, main's edit, the superseding REF alone.
            listed = {pending.path: pending for pending in pending_documents(after, before, accepted, result.cited)}
            if e_alone and "docs/specs/e.md" in listed and any(record.body == "# e.md\nSeen on build 29990005.\n" for record in raw_after.values()):
                self.assertIn(listed["docs/specs/e.md"].in_force[0].newly, [("any:29990005",), ()], f"seed {seed}: e.md's text names a build")
            in_force = [name for name, record in accepted.records.items()
                        if record.type == "reference" and name not in accepted.retired and document_path(record) == "docs/specs/c.md"]
            c_listed = listed.get("docs/specs/c.md")
            if len(in_force) > 1 and rewrite_case(raw_after):
                self.assertEqual(len(in_force), len(c_listed.in_force) if c_listed else 0, f"seed {seed}: c.md with {in_force} in force")
            if len(in_force) == 1 and rewrite_case(raw_after) and accepted.records[in_force[0]].body == "# c.md, main's edit\n":
                self.assertTrue(c_listed and c_listed.main_changed, f"seed {seed}: main changed c.md too")
            if len(in_force) == 1 and rewrite_case(raw_after) and accepted.records[in_force[0]].body == "# c.md, rewritten\n":
                self.assertIsNone(c_listed, f"seed {seed}: c.md done once the superseding REF alone is in force")
            self.assertEqual([], [name for name, reason in result.undecided.items() if "cannot carry the link (cosmetic)" in reason],
                             f"seed {seed}: a cosmetic row refused as a successor")
            def counted(export: dict[str, Record]) -> dict[str | None, int]:
                found: dict[str | None, int] = {}
                for record in export.values():
                    if record.type in ("compatibility", "report"):
                        found[identity(record)] = found.get(identity(record), 0) + 1
                return found

            accepted_keys = {identity(record) for record in accepted.records.values() if record.type in ("compatibility", "report")}
            base_counts, branch_counts = counted(raw_before), counted(raw_after)
            lost = {key for key, count in base_counts.items() if count > branch_counts.get(key, 0)} & accepted_keys
            if lost:
                self.assertEqual([], [item.record.id for item in result.proposals if item.record.type in ("compatibility", "report")],
                                 f"seed {seed}: an accepted identity disappeared while a new row of its table is proposed")
            held_identities = {identity(record) for record in accepted.records.values()} - {None}
            self.assertEqual([], [item.record.id for item in result.proposals
                                  if result.kinds.get(item.record.id) == "new" and identity(item.record) in held_identities],
                             f"seed {seed}: a new record whose identity the accepted journal already has")
            if sum(1 for record in raw_after.values() if record.text("name") == "Gamma") > 1 == sum(1 for record in raw_before.values() if record.text("name") == "Gamma"):
                self.assertEqual("undecided", result.kinds[base_ids(raw_before)[0]], f"seed {seed}: a row citing a shared name")
            if "M-501" in after:
                self.assertEqual("undecided", result.kinds["M-501"], f"seed {seed}: a row citing a new document")
            if "M-502" in raw_after and e_alone:
                # A citation of e.md is of e.md as the branch has it: proposed citing main's REF only when main holds that text.
                branch_e = next(record.body for record in raw_after.values() if record.legacy is not None and record.legacy.path == "docs/specs/e.md")
                main_e = next(record.body for name, record in accepted.records.items() if document_path(record) == "docs/specs/e.md" and name not in accepted.retired)
                e_listed = [pending.citing for pending in pending_documents(after, before, accepted, result.cited) if pending.path == "docs/specs/e.md"]
                if branch_e == main_e:
                    self.assertEqual("new", result.kinds["M-502"], f"seed {seed}: a row citing e.md as main holds it")
                else:
                    self.assertEqual("undecided", result.kinds["M-502"], f"seed {seed}: a row citing e.md as main does not hold it")
                    self.assertIn("docs/specs/e.md as this branch has it", result.undecided["M-502"], f"seed {seed}")
                    branch_changed_e = branch_e != next(record.body for record in raw_before.values() if document_path(record) == "docs/specs/e.md")
                    if not branch_changed_e:
                        waiting = tuple(name for name in ("M-500", "M-502") if "docs/specs/e.md as this branch has it" in result.undecided.get(name, ""))
                        self.assertEqual([waiting], e_listed, f"seed {seed}: e.md, unchanged on the branch, listed for the rows it holds")
            if len(accepted.records["M-500"].links.refs) == 1:
                self.assertNotEqual("conflict", result.kinds.get("M-500"), f"seed {seed}: the tracked row conflicts only by REF number")
            elif raw_after["M-500"].status != raw_before["M-500"].status:
                # Main and the branch both changed M-500: a conflict, or held first for citing e.md as the branch has it.
                self.assertIn(result.kinds["M-500"], ("conflict", "undecided"), f"seed {seed}: main and the branch both changed M-500")
            if "F-299" in after:
                self.assertEqual("undecided", result.kinds["F-299"], f"seed {seed}: a row citing a new compatibility row")
            other = Salvage("spelled-otherwise", "d" * 40, "b" * 40, "c" * 40)
            _ = classify(other, after, before, accepted, unmapped, shared)
            self.assertEqual(([revision.line() for revision in result.proposals], result.kinds),
                             ([revision.line() for revision in other.proposals], other.kinds), f"seed {seed}")
            # Once the first output is accepted, nothing is proposed again and nothing held changes class.
            merged = (*accepted.segments, Segment("memory/journal/accepted-proposals.jsonl", "", tuple(result.proposals)))
            now = accepted_of(merged)
            self.assert_documents(f"seed {seed}, run 2", raw_after, raw_before, now)
            again = Salvage("generated", "e" * 40, "b" * 40, "c" * 40)
            (after_again, before_again), unmapped_again, shared_again = renumber((raw_after, raw_before), now.records, now.retired)
            try:
                _ = classify(again, after_again, before_again, now, unmapped_again, shared_again)
            except SalvageError as error:
                self.fail(f"seed {seed}, second run: {error}")
            self.assertEqual([], [revision.record.id for revision in again.proposals], f"seed {seed}: second run proposes again")
            # Held rows keep their class; a provisional number may move once other records are accepted, so those compare by identity.
            held = {name: kind for name, kind in result.kinds.items() if kind in ("supersede", "undecided", "conflict") and name not in unmapped}
            self.assertEqual(held, {name: kind for name, kind in again.kinds.items() if name in held}, f"seed {seed}: second run")
            held_new = sorted(str(identity(after[name])) for name, kind in result.kinds.items() if kind == "undecided" and name in unmapped and name in after)
            again_new = sorted(str(identity(after_again[name])) for name, kind in again.kinds.items() if kind == "undecided" and name in unmapped_again and name in after_again)
            self.assertEqual(held_new, again_new, f"seed {seed}: second run, provisional rows")
            changed = {name for name, record in after.items() if before.get(name) != record}
            proposed = {revision.record.id for revision in result.proposals} & changed
            # Accepted, or (a cosmetic row that carried a link in run 1, now accepted) cosmetic: nothing left to propose either way.
            self.assertEqual(set[str](), {name for name in proposed if again.kinds.get(name) not in ("accepted", "cosmetic")},
                             f"seed {seed}: what run 1 proposed is accepted in run 2")
            self.assertEqual(set[str](), {name for name, kind in again.kinds.items() if kind == "conflict"} - set(held), f"seed {seed}: new conflicts")
            # Run 1 accepted, then main supersedes e.md's REF by one holding the branch's e.md text: what run 1 proposed is
            # still accepted (a citation of a REF and of its same-document successor are one), and nothing new conflicts.
            e_in_force = [name for name, record in now.records.items() if document_path(record) == "docs/specs/e.md" and name not in now.retired]
            if len(e_in_force) == 1:
                branch_e = next(record.body or "" for record in raw_after.values() if document_path(record) == "docs/specs/e.md")
                succeeded = (*merged, Segment("memory/journal/succeeded.jsonl", "", (Revision.of(superseding_document("REF-961", "docs/specs/e.md", e_in_force[0], branch_e)),)))
                successor = accepted_of(succeeded)
                self.assert_documents(f"seed {seed}, after e.md was superseded", raw_after, raw_before, successor)
                (after_next, before_next), unmapped_next, shared_next = renumber((raw_after, raw_before), successor.records, successor.retired)
                following = Salvage("generated", "9" * 40, "b" * 40, "c" * 40)
                try:
                    _ = classify(following, after_next, before_next, successor, unmapped_next, shared_next)
                except SalvageError as error:
                    self.fail(f"seed {seed}, after e.md was superseded: {error}")
                self.assertEqual(set[str](), {name for name in proposed if following.kinds.get(name) not in ("accepted", "cosmetic")},
                                 f"seed {seed}: what run 1 proposed is accepted after e.md's REF was superseded")
                self.assertEqual(set[str](), {name for name, kind in following.kinds.items() if kind == "conflict"} - set(held),
                                 f"seed {seed}: new conflicts after e.md's REF was superseded")
                if "M-502" in raw_after:
                    # Main now holds the branch's e.md text: a row held in run 1 for it is proposed citing REF-961.
                    self.assertIn(following.kinds["M-502"], ("accepted", "new"), f"seed {seed}: M-502 after e.md was superseded")
            if "M-501" in raw_after and "REF-970" not in accepted.records:
                # The new document proposed by hand (no legacy; the note names its path) and accepted: the row citing it now translates.
                body = next(record.body for record in raw_after.values() if record.legacy is not None and record.legacy.path == "docs/specs/a.md")
                assert body is not None
                hand = Revision.of(Record.from_json({"id": "REF-900", "type": "reference", "status": "current", "title": "docs/specs/a.md",
                                                     "scope": {"basis": "explicit"}, "body_sha256": hashlib.sha256(body.encode()).hexdigest(),
                                                     "notes": [DOCUMENT_NOTE.format("docs/specs/a.md")]}, body))
                with_hand = (*merged, Segment("memory/journal/hand.jsonl", "", (hand,)))
                later = accepted_of(with_hand)
                self.assert_documents(f"seed {seed}, run 3", raw_after, raw_before, later)
                (after_hand, before_hand), unmapped_hand, shared_hand = renumber((raw_after, raw_before), later.records, later.retired)
                third = Salvage("generated", "f" * 40, "b" * 40, "c" * 40)
                _ = classify(third, after_hand, before_hand, later, unmapped_hand, shared_hand)
                self.assertEqual("new", third.kinds["M-501"], f"seed {seed}: run after the document is accepted")
                self.assertEqual(("REF-900",), next(item.record for item in third.proposals if item.record.id == "M-501").links.refs, f"seed {seed}")


if __name__ == "__main__":
    _ = unittest.main()
