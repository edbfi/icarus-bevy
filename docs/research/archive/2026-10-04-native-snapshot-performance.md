> Archived research report, retained verbatim. Current conclusions live in [the ledgers](../../ledgers/findings/). Moved from `docs/research/` on 2026-10-13.

# Native snapshot decoding measurement

Historical measurement — 2026-10-04. Results and test claims apply to the source
and workload recorded below. See the [current checkpoint](../../implementation-status.md)
for later implementation changes and validation.

The snapshot reader previously decoded a full `serde_json::Value` tree and then
converted it into typed state. Every periodic save also verifies the preceding
generation, so this allocation affected both save and load time. The reader now
probes the schema while skipping entity values, then decodes directly into typed
records. It still hashes the complete file, rejects unsupported schemas, validates
domain state and verifies the preceding generation; no trust cache was introduced.

## Measured scope

On this Linux VPS, `snapshot_bench` used 100,000 authored entities, two warm-up
commits and 20 measured save/load iterations per process. Three fresh-directory
processes ran for each implementation, with CPU affinity 2–3. The baseline was
commit `9a7c90d`; the changed persistence source and both executable hashes are
recorded in `work/icarus-overnight/commands.jsonl`. Builds used Rust 1.99.0,
release optimization and the locked dependency graph.

A separate synthetic soak was active on CPUs 0–1 during both groups. No original
container or Ghidra job ran during these measurements. This is a shared VPS;
storage, scheduling and cache state were not controlled beyond the stated setup.
Each save and load verified the entire recovered state. All 132 warm-up/measured
cycles across the six processes passed.

| Implementation | Per-process median save, ms | Per-process median load, ms | Whole-process peak RSS, KiB |
| --- | --- | --- | --- |
| JSON tree, run 1 | 175.864 | 130.609 | 96,304 |
| JSON tree, run 2 | 167.177 | 128.286 | 96,180 |
| JSON tree, run 3 | 167.974 | 122.702 | 96,308 |
| Typed records, run 1 | 59.512 | 32.325 | 13,936 |
| Typed records, run 2 | 60.316 | 32.645 | 13,976 |
| Typed records, run 3 | 59.953 | 35.838 | 13,976 |

The median of the three run medians fell by about 64% for saving and 75% for
loading. The median peak RSS fell by about 85%. RSS includes the benchmark's
verification allocations and is not a steady-state server memory measurement.
Raw JSON lines and `/usr/bin/time -v` outputs remain in the private case's `bench/`.

These are improvements over this repository's earlier native snapshot code for
one authored workload. They are **not** ICARUS gameplay results or a comparison
with the official dedicated server. The ongoing soak began before this change and
cannot be used as sustained qualification of the optimized reader.

## Correctness and parser behavior

The persistence and worker tests still pass, including prior-generation fallback,
future-schema rejection, single-writer exclusion, failed publication and retry.
Direct typed decoding also rejects duplicate known JSON fields, which the old
intermediate object tree could collapse before validation. An ambiguous current
generation is now treated as corrupt and may recover a valid previous generation.
The native writer never produces duplicate fields.

Reproduce with the [snapshot workload instructions](../../runbooks/synthetic-workload.md).
Keep executable hashes, source revision, population, affinity and concurrent work
with results. Re-evaluate on real implemented gameplay and larger supported saves
before setting production capacity targets.
