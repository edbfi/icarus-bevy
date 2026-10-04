# Prospect loading: static observations and investigation contract

Status: **observed call paths; schema and runtime behavior unresolved**. Applies
only to dedicated server tool build 25621760. Evidence IDs below belong to
`work/icarus-server-baseline/` and are described in the
[targeted static report](../research/2026-10-04-targeted-static-report.md).

## Keep the two entry paths distinct

| Path | Evidence | Observed stages | Unknowns |
| --- | --- | --- | --- |
| By-name candidate, `0x14146e6c0` | E-009/E-017/E-018 | Empty-name check, file lookup, preliminary load, subsequent load/name comparison, continuation toward map loading | Dedicated command entry, field identities, case rules, complete outcomes |
| From-file candidate, `0x1414baf60` | E-010/E-014/E-015/E-019 | Two reader results gate apparent state application; either failure enters reporting | Caller, reader contracts, schemas, atomicity, dynamic error behavior |

The by-name path constructs names with a `.json` suffix. That alone does not prove
that every save is plain JSON or describe compressed/encoded fields, companion
files, versioning, checksums, references or content requirements. The shared
archive conversion includes indirect calls; its full schema is still unknown.

In the from-file path, both reader candidates receive the same path object and
call the same initial helper. One immediately checks that helper's return; the
other proceeds to a conversion whose return becomes its result. There is no proof
yet that this difference loses an error or permits inconsistent reads.

On failure, the outer path calls a helper that sets a global flag and forwards
to output formatting and a virtual call (E-016/E-020). Resolve the dynamic output
target before deciding whether that path terminates the process. Cleanup and a
false-return path exist in the caller; their presence alone does not prove that
the reporting call returns.

## Controlled reproduction matrix

These are proposed experiments, not executed tests or confirmed bugs. They require
a separate authorized runtime case and disposable fixtures. Do not use production
saves, package assets, or extracted game data.

| Scenario | Question to resolve | Evidence to retain |
| --- | --- | --- |
| Missing file | Do both paths report failure and preserve current state? | Selected command/path, logs, exit status, before/after state |
| Empty or truncated file | Which reader rejects the file, and does the process continue? | Input hash, truncation point, error and process outcome |
| Malformed field or unsupported version | Is invalid data distinguished from missing data? | Minimized player-created fixture, schema/version, outcome |
| Embedded name differs from requested name | Which comparison fails; does case matter? | Both names, selected path, resulting state |
| File changes during load | Do staged readers consume one coherent generation? | Exact timing, file generations, resulting state; only after reader contracts are known |
| Interrupted save followed by restart | What recovery behavior is provided by the original? | Last acknowledged generation, partial files, restart result |

For every candidate, repeat with an unchanged valid fixture, minimize the failure,
and separate original-server behavior from Proton, missing platform services and
resource constraints. A failed startup in the isolated container is not a substitute
for these prospect-load experiments.

## Independent replacement requirements

The importer must load into staging state, resolve references and validate the
complete supported schema before publishing a playable world. Missing, malformed,
unsupported and inconsistent inputs need distinct actionable errors. Preserve
the source and existing native state on failure. A failed import must never silently
become an empty new world. Content-dependent fields require an explicit mapping
to independently authored data, with unresolved mappings reported as incompatibility.

The native snapshot store already has its own versioned format and durability
tests. It is not an official-save reader. `import-save` intentionally remains
unavailable until field coverage and stock-client validation can demonstrate a
correct migration. Those preventative design choices are not evidence of fixing
an original-server bug.
