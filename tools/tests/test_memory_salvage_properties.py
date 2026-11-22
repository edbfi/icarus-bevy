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
from tools.memory.model import JSON, Record
from tools.memory.salvage import DOCUMENT_NOTE, SALVAGED, Accepted, Salvage, SalvageError, classify, document_path, identity, pending_documents, renumber
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


def document(identifier: str, path: str, body: str | None = None, builds: tuple[str, ...] = ()) -> Record:
    text = f"# {path}\n" if body is None else body
    scope: dict[str, JSON] = {"basis": "legacy-text", "builds": list[JSON](builds)} if builds else {"basis": "explicit", "subjects": ["tooling"]}
    return Record.from_json({"id": identifier, "type": "reference", "status": "current", "title": path, "scope": scope,
                             "body_sha256": hashlib.sha256(text.encode()).hexdigest(), "legacy": {"path": path, "cells": {"document": path}}}, text)


def surface(identifier: str, name: str) -> Record:
    return Record.from_json({"id": identifier, "type": "compatibility", "status": "unknown", "name": name, "required_proof": "Invented proof",
                             "assigned_at_migration": True, "gaps": ["qualifier", "scope"],
                             "legacy": {"path": "docs/ledgers/compatibility.md", "cells": {"surface": name, "status": "Unknown", "required_proof": "Invented proof"}}})


def tracked(identifier: str, path: str, ref: str) -> Record:
    data = evidence(identifier, path, refs=[ref], notes=[TRACKED_NOTE.format(ref)], legacy={"path": "evidence/x.jsonl", "cells": {"row": "{}"}})
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
    return any(record.body == "# e.md, re-observed\n" for record in after.values())


def problems(state: State) -> set[str]:
    return {*state.problems, *revision_problems(state), *state.link_problems(), *state.supersession_cycles()}


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
    main_paths, main_names = ["docs/specs/c.md", "docs/specs/e.md"], ["Alpha", "Gamma", "Gamma"] if twins else ["Alpha", "Gamma"]
    branch_paths = ["docs/specs/a.md", *main_paths] if rng.random() < 0.5 else main_paths
    retoken, rewrite_c, two_holders = rng.random() < 0.3, rng.random() < 0.3, rng.random() < 0.2
    renewed = not retoken and rng.random() < 0.3  # e.md's body newly names a build outside its (unchanged) scope sections
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
            export[refs["docs/specs/e.md"]] = document(refs["docs/specs/e.md"], "docs/specs/e.md", "# e.md, re-observed\n", ("server:29990001",))
        if export is after and renewed:
            export[refs["docs/specs/e.md"]] = document(refs["docs/specs/e.md"], "docs/specs/e.md", "# e.md\nSeen on build 29990005.\n")
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
    if "Beta" in branch_names and rng.random() < 0.5:
        after["F-299"] = Record.from_json(row("F-299", "active").to_json() | {"refs": [numbered(branch_paths, branch_names)[1]["Beta"]]})
    revisions = {identifier: Revision.of(record) for identifier, record in before.items()}
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
    segments = (Segment("memory/journal/generated.jsonl", "", (*revisions.values(), *later)),)
    state = replay(segments)
    accepted = Accepted({identifier: entry.record for identifier, entry in state.entries.items()},
                        {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal", segments,
                        frozenset(identifier for identifier in state.entries if state.retired(identifier)))
    return after, before, accepted


class GeneratedSalvageTests(unittest.TestCase):
    def test_every_generated_case_keeps_the_invariants_and_replays_cleanly(self) -> None:
        for seed in SEEDS:
            raw_after, raw_before, accepted = case(seed)
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
            changed = {name for name, record in after.items() if record.type in SALVAGED and before.get(name) != record}
            self.assertEqual(set[str](), changed - set(result.kinds), f"seed {seed}: changed rows not reported")
            for name in shared:
                if name in changed:
                    self.assertEqual("undecided", result.kinds[name], f"seed {seed}: a row sharing an identity")
            self.assertNotIn("M-500", result.kinds, f"seed {seed}: an unchanged tracked row renumbered by the branch")
            # (Invariant (f) checks no proposal cites a REF, CP or RP whose identity differs from the branch record it stood for.)
            # Documents: a body revision only under equal tokens; no command for a path main holds twice.
            for pending in pending_documents(after, before, accepted):
                if pending.kind == "revise":
                    self.assertEqual(sorted(pending.names), sorted(pending.scope), f"seed {seed}: {pending.path}")
                if pending.kind == "ambiguous":
                    # No command, or (one REF holds this body) only the supersession of the others by it.
                    expected = [f"memory supersede {other} --by {pending.target}" for other in pending.holders if other != pending.target]
                    self.assertEqual(expected if pending.target is not None else [], pending.commands("FILE"), f"seed {seed}: {pending.path}")
                if pending.path == "docs/specs/e.md" and any(record.body == "# e.md\nSeen on build 29990005.\n" for record in raw_after.values()):
                    self.assertEqual(("decide", ("any:29990005",)), (pending.kind, pending.newly), f"seed {seed}: e.md newly names a build")
                if pending.path == "docs/specs/e.md" and retoken_case(raw_after):
                    self.assertEqual("supersede", pending.kind, f"seed {seed}: e.md names a build its REF's scope lacks")
            # Ambiguity before done: c.md changed on the branch with two REFs in force is listed, never done.
            in_force = [name for name, record in accepted.records.items()
                        if record.type == "reference" and name not in accepted.retired and document_path(record) == "docs/specs/c.md"]
            c_listed = [pending for pending in pending_documents(after, before, accepted) if pending.path == "docs/specs/c.md"]
            if len(in_force) > 1 and rewrite_case(raw_after):
                self.assertEqual(["ambiguous"], [pending.kind for pending in c_listed], f"seed {seed}: c.md with {in_force} in force")
            if len(in_force) == 1 and rewrite_case(raw_after) and accepted.records[in_force[0]].body == "# c.md, rewritten\n":
                self.assertEqual([], c_listed, f"seed {seed}: c.md done once the superseding REF alone is in force")
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
            if "F-299" in after:
                self.assertEqual("undecided", result.kinds["F-299"], f"seed {seed}: a row citing a new compatibility row")
            other = Salvage("spelled-otherwise", "d" * 40, "b" * 40, "c" * 40)
            _ = classify(other, after, before, accepted, unmapped, shared)
            self.assertEqual(([revision.line() for revision in result.proposals], result.kinds),
                             ([revision.line() for revision in other.proposals], other.kinds), f"seed {seed}")
            # Once the first output is accepted, nothing is proposed again and nothing held changes class.
            merged = (*accepted.segments, Segment("memory/journal/accepted-proposals.jsonl", "", tuple(result.proposals)))
            state = replay(merged)
            now = Accepted({identifier: entry.record for identifier, entry in state.entries.items()},
                           {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal", merged)
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
            if "M-501" in raw_after:
                # The new document proposed by hand (no legacy; the note names its path) and accepted: the row citing it now translates.
                body = next(record.body for record in raw_after.values() if record.legacy is not None and record.legacy.path == "docs/specs/a.md")
                assert body is not None
                hand = Revision.of(Record.from_json({"id": "REF-900", "type": "reference", "status": "current", "title": "docs/specs/a.md",
                                                     "scope": {"basis": "explicit"}, "body_sha256": hashlib.sha256(body.encode()).hexdigest(),
                                                     "notes": [DOCUMENT_NOTE.format("docs/specs/a.md")]}, body))
                with_hand = (*merged, Segment("memory/journal/hand.jsonl", "", (hand,)))
                state = replay(with_hand)
                later = Accepted({identifier: entry.record for identifier, entry in state.entries.items()},
                                 {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal", with_hand)
                (after_hand, before_hand), unmapped_hand, shared_hand = renumber((raw_after, raw_before), later.records, later.retired)
                third = Salvage("generated", "f" * 40, "b" * 40, "c" * 40)
                _ = classify(third, after_hand, before_hand, later, unmapped_hand, shared_hand)
                self.assertEqual("new", third.kinds["M-501"], f"seed {seed}: run after the document is accepted")
                self.assertEqual(("REF-900",), next(item.record for item in third.proposals if item.record.id == "M-501").links.refs, f"seed {seed}")


if __name__ == "__main__":
    _ = unittest.main()
