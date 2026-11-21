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
from tools.memory.model import JSON, Record
from tools.memory.legacy import TRACKED_NOTE
from tools.memory.salvage import SALVAGED, Accepted, Salvage, SalvageError, classify, renumber
from tools.memory.state import State, replay
from tools.tests.memory_fixtures import evidence, finding

SEEDS = range(300)


def row(identifier: str, cell: str, claim: str = "Invented claim", successors: tuple[str, ...] = ()) -> Record:
    """A migrated finding as the exporter writes one: stored status from the cell, implied status and successors."""
    lowered = cell.lower()
    implied = "superseded" if "superseded" in lowered else "retired" if lowered.startswith("retired") and lowered != "retired" else None
    legacy: dict[str, JSON] = {"path": "docs/ledgers/findings/generated.md", "cells": {"claim": f"{claim} {identifier}", "status": cell}}
    if implied is not None:
        legacy["implied_status"] = implied
    if successors:
        legacy["superseded_by"] = list(successors)
    data = finding(identifier, f"{claim} {identifier}", status=cell if cell in ("active", "retired") else f"legacy:{cell}", legacy=legacy)
    del data["cites"]
    return Record.from_json(data)


def document(identifier: str, path: str) -> Record:
    body = f"# {path}\n"
    return Record.from_json({"id": identifier, "type": "reference", "status": "current", "title": path, "scope": {"basis": "explicit", "subjects": ["tooling"]},
                             "body_sha256": hashlib.sha256(body.encode()).hexdigest(), "legacy": {"path": path, "cells": {"document": path}}}, body)


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
    for identifier in fresh_ids:
        if rng.random() < 0.4:
            other = rng.choice([item for item in candidates if item != identifier])
            after[identifier] = row(identifier, f"superseded-by {other}", successors=(other,))
        else:
            after[identifier] = row(identifier, "active")
    # Migration-assigned records: the branch may insert a document sorting first and a compatibility row in the middle,
    # renumbering the rest in its export; an unchanged tracked row and a finding cite later ones by those numbers.
    main_paths, main_names = ["docs/specs/c.md", "docs/specs/e.md"], ["Alpha", "Gamma"]
    branch_paths = ["docs/specs/a.md", *main_paths] if rng.random() < 0.5 else main_paths
    branch_names = ["Alpha", "Beta", "Gamma"] if rng.random() < 0.5 else main_names
    # A repeated name: two new rows of one name, or a second row of an accepted name.
    repeated = rng.choice([[], [], ["Dup", "Dup"], ["Gamma"]])
    branch_names = [*branch_names, *repeated]
    for paths, names, export in ((main_paths, main_names, before), (branch_paths, branch_names, after)):
        refs, cps = numbered(paths, names)
        export.update({refs[path]: document(refs[path], path) for path in paths})
        export.update({f"CP-{number:03d}": surface(f"CP-{number:03d}", name) for number, name in enumerate(names, 1)})
        export["M-500"] = tracked("M-500", "docs/specs/e.md", refs["docs/specs/e.md"])
        cited = export[base_ids[0]].to_json() | {"refs": [cps["Gamma"]]}
        export[base_ids[0]] = Record.from_json(cited)
    if "docs/specs/a.md" in branch_paths and rng.random() < 0.5:
        after["M-501"] = tracked("M-501", "docs/specs/a.md", numbered(branch_paths, branch_names)[0]["docs/specs/a.md"])
    if "Beta" in branch_names and rng.random() < 0.5:
        after["F-299"] = Record.from_json(row("F-299", "active").to_json() | {"refs": [numbered(branch_paths, branch_names)[1]["Beta"]]})
    revisions = {identifier: Revision.of(record) for identifier, record in before.items()}
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
                        {identifier: entry.heads for identifier, entry in state.entries.items()}, "journal", segments)
    return after, before, accepted


class GeneratedSalvageTests(unittest.TestCase):
    def test_every_generated_case_keeps_the_invariants_and_replays_cleanly(self) -> None:
        for seed in SEEDS:
            raw_after, raw_before, accepted = case(seed)
            after, before = raw_after, raw_before
            (after, before), unmapped, shared = renumber((after, before), accepted.records)
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
            if sum(1 for record in raw_after.values() if record.text("name") == "Gamma") > 1:
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
            (after_again, before_again), unmapped_again, shared_again = renumber((raw_after, raw_before), now.records)
            try:
                _ = classify(again, after_again, before_again, now, unmapped_again, shared_again)
            except SalvageError as error:
                self.fail(f"seed {seed}, second run: {error}")
            self.assertEqual([], [revision.record.id for revision in again.proposals], f"seed {seed}: second run proposes again")
            held = {name: kind for name, kind in result.kinds.items() if kind in ("supersede", "undecided", "conflict")}
            self.assertEqual(held, {name: kind for name, kind in again.kinds.items() if name in held}, f"seed {seed}: second run")
            changed = {name for name, record in after.items() if before.get(name) != record}
            proposed = {revision.record.id for revision in result.proposals} & changed
            self.assertEqual({name: "accepted" for name in proposed}, {name: again.kinds.get(name) for name in proposed},
                             f"seed {seed}: what run 1 proposed is accepted in run 2")
            self.assertEqual(set[str](), {name for name, kind in again.kinds.items() if kind == "conflict"} - set(held), f"seed {seed}: new conflicts")


if __name__ == "__main__":
    _ = unittest.main()
