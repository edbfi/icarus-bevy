# Bug ledger

Reported original-server problems, static leads about the original, and native
foundation defects. No original defect is reproduced or fixed. Native
persistence tests are preventative engineering, not fixes of the original.
Full text before condensation: 16d1a9d:docs/ledgers/bugs.md.

Last reconciled: 2026-10-14

## Reported campaigns

| Priority | Reported problem | Status | Next evidence |
| --- | --- | --- | --- |
| 1 | Crashes, hangs and save integrity | Unreproduced | Preserved failing state, actions, dumps, committed save generations |
| 2 | Long-session degradation | Unreproduced | Fixed-workload memory, object/handle counts, tick and save distributions |
| 3 | Joining and reconnecting | Unreproduced | Paired client/server logs, lifecycle traces, session ownership |
| 4 | Large worlds, tames, bases, players | Unreproduced | Workload dimensions, profiles, Windows/Proton comparison |

A reproduction records build, configuration, hardware, starting-state hash,
actions, expected and actual behavior, attempts, reference platform, minimized
case, stack or call-flow evidence, confidence, root-cause class, proposed
replacement behavior and regression test, and separates server faults from
Proton, configuration and resource limits. The isolated offline startup (F-268,
M-169) logged Steam initialization failures and engine warnings while
initialization continued; these are environment-confounded leads, not
reproductions.

## Static leads on the original

Hypotheses to test, scoped to the cases named; none is a reported or reproduced
defect. SL-001 to SL-003 belong to M-170, SL-004 to M-169.

| Lead | Static evidence | What would establish or refute a problem |
| --- | --- | --- |
| SL-001: Different error checks in staged file readers | E-010/E-014/E-015 | Recover the shared helper contract, then test missing and truncated inputs |
| SL-002: From-file load failure enters a fatal-output facility | E-010/E-016/E-020; [output target](../research/archive/2026-10-04-save-failure-output.md) | Demonstrate from-file entry, runtime guard state and process outcome |
| SL-003: Requested prospect name compared against loaded data | E-009/E-018 | Test controlled name mismatches; a rejection alone is not a bug |
| SL-004: Blob result and declared compressed length may diverge | E-009/E-022: no visible branch on the Base64 result before `TotalLength` reaches decompression | Establish the reachable loader and decoder allocation contract, then compare small authored inputs |

By-name startup rejection runs found graceful rejection for one missing file
and two truncated JSON inputs, with stateless handshake liveness retained
(F-256); they narrow SL-001 but do not test the from-file branch.

## Native foundation defects

Defects in this repository's code, each with a regression test.

| ID | Trigger and former behavior | Change | Findings |
| --- | --- | --- | --- |
| N-001 | Save worker exit before acknowledging an in-flight request left `busy` set and read as a retryable failure | Typed terminal error, cleared in-flight flag, simulation stop; disconnected-channel test | F-232 |
| N-002 | A tick-10 submission after acknowledged tick 20 published generation 2, rolling back the persisted clock | Reject older ticks before publication; equal-tick final saves and explicit previous-generation recovery stay allowed | F-232 |
| N-003 | Real ENOSPC was misclassified because `tempfile` hid `raw_os_error()` behind path context | Classify `StorageFull` and `PermissionDenied` through the error chain; tmpfs run verified ENOSPC, EACCES, retries and reopen | F-239 |
| N-004 | Shutdown accepted a success receipt created after its deadline; synchronous final capture could exhaust the budget and still enqueue | Time checks before polling, completion, final capture and enqueueing; expired waits consume nothing | F-233 |
| N-005 | Envelope inspection accepted malformed UTF-8 inside ignored JSON values | Validate the whole bounded input before parsing; invalid, overlong and surrogate fixtures fail | F-243 |
| N-006 | Offline harness could report failed inspection as cleanup, used a predictable ownership label and could overrun its reserved lifetime | Current session, hash and mode bindings, locked quotas, random exact ownership, bounded phases, launch budget clamped to session expiry | F-252 |
| N-007 | Removing Position from a domain-identified entity silently dropped it from snapshot capture; generation 2 acknowledged a truncated world | Query every DomainId and require Position before capture; four regressions | F-003 |
| N-008 | FIFO manifest or selected generation blocked startup before signal handling | Nonblocking opens and opened-handle regular-file checks; bounded CLI rejection/preservation and valid preceding-generation recovery tests | F-405 |
| N-009 | Envelope structs accepted unlabeled positional arrays, bypassing named-object policy | Map-only root/blob boundaries; all modes reject at envelope parsing in both encodings | F-411 |
| N-010 | Wrong known-field types exposed input strings through Serde diagnostics on stderr | Category/line/column only; original cause discarded; CLI/API privacy and source-preservation regressions | F-412 |
| N-011 | Malformed checksum references caused rollback or left invalid previous metadata admitted | Validate both canonical digest strings before selection; load/save/CLI preservation and valid-mismatch recovery regressions | F-417 |
| N-012 | Native JSON parse causes exposed supplied strings/unknown keys, including both unusable generations | Shared static stage/category/location errors discard raw causes; API/CLI and nine release cases preserve files | F-423, F-425 |

Injected ENOSPC at the generation and manifest writer boundaries preserves the
acknowledged generation and permits retry; an error after publication returns
no receipt and keeps the readable generation as fallback (F-228). These are
injected outcomes, not physical power-loss tests (U-035).

## Research-harness corrections

Corrections to authored research tooling, not original-server behavior: the
common-scalar wrapper's receipt-ordering defect (F-171, M-068); the
instruction-context job that compiled fixture stubs beneath Ghidra's script
directory and was retried from a source-only leaf (M-040); the
scope-guard rejection of a session that paraphrased its authorization text
(F-201, M-051); the decoder checker that expected a four-byte write for a
zero-extending MOV (F-047, M-014).
