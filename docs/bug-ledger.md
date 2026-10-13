# Bug investigation ledger

These campaigns originate from the user's experience. None is currently a
reproduced defect or a claimed fix. Do not infer root cause from bundled DLL names.

| Priority | Reported problem | Status | Next evidence |
| --- | --- | --- | --- |
| 1 | Crashes, hangs and save integrity | Unreproduced | Preserved failing state, actions, dumps, committed save generations |
| 2 | Long-session degradation | Unreproduced | Fixed-workload memory, object/handle counts, tick and save distributions |
| 3 | Joining and reconnecting | Unreproduced | Paired client/server logs, lifecycle traces, session ownership |
| 4 | Large worlds, tames, bases, players | Unreproduced | Workload dimensions, profiles, Windows/Proton comparison |

For each reproduction, record build/configuration/hardware, starting-state hash,
actions, expected/actual behavior, failures/attempts, reference platform, minimized
case, stack or call-flow evidence, confidence, root-cause classification, proposed
replacement behavior and regression test. Separate server faults from Proton,
configuration and resource limits. Native persistence tests are preventative
engineering; they are not evidence of fixing an original-server defect.

The [offline startup batch](research/2026-10-04-offline-startup-report.md) recorded
Steam initialization failures, an auxiliary Xalia display exception and engine
warnings. Engine initialization continued, and the harness stopped the container.
These observations remain environment-confounded leads, not reproductions of the
reported crashes. No cause or fix is assigned to them.

## Static investigation leads

These are hypotheses to test, not additional reported or reproduced defects.
SL-001 through SL-003 are scoped to `icarus-server-baseline`; see the
[targeted report](research/2026-10-04-targeted-static-report.md).

| Lead | Static evidence | What would establish or refute a problem |
| --- | --- | --- |
| SL-001: Different error checks in staged file readers | E-010/E-014/E-015 | Recover the shared helper contract, then test missing/truncated inputs; later conversion may already reject them correctly |
| SL-002: From-file load failure enters a fatal-output facility | Baseline E-010/E-016/E-020 plus [resolved output target](research/2026-10-04-save-failure-output.md) | Forced-exit and exception branches identified statically; demonstrate actual from-file entry, runtime guard state and process outcome before claiming a defect |
| SL-003: Requested prospect name compared against loaded data | E-009/E-018 | Test controlled name mismatches and document the intended rejection; a rejection alone is not a bug |
| SL-004: Encoded blob result and declared compressed length may diverge | Overnight E-009/E-022: caller does not visibly branch on Base64 result before passing `TotalLength` to decompression | Establish reachable loader and decoder allocation contract, then compare small authored valid/malformed inputs; do not infer a crash from decompilation |

The [prospect-loading experiment matrix](specs/prospect-loading.md) defines the
next evidence to collect. No native fix is attributed to these leads.
SL-004 belongs to `icarus-overnight`, unlike the earlier leads. The independent
envelope inspector checks decoded length before decompression, but that defensive
choice is not proof of reproducing or fixing an original-server defect.

The [by-name startup rejection tests](research/2026-10-04-prospect-rejection.md)
found graceful rejection for one missing file and two identical truncated JSON
inputs. The repeated malformed case retained stateless handshake liveness. These
negative crash results narrow SL-001's investigation but do not test the separate
from-file branch, deeper blob decoding, or preservation of an already loaded world.

## Native foundation defects and fault tests

These concern this repository's code, not the official server.

The common-scalar research wrapper inherited a separate receipt-ordering defect:
zero exit and unchanged bindings could record `success=true` before validating
returned JSON/counts. Independent review reproduced this with mocked malformed
output before any private execution. The fresh scalar variant now checks the
qualified summary and deadline before recording success; regressions require
failure receipts and no summary for malformed JSON, missing results, wrong counts,
changed bindings or expiry. Historical frozen wrappers remain evidence, and their
earlier successful outputs were valid. This is a research-harness correction, not
an original-server fix. Evidence: `work/c009-common-scalar-values-20261013/`.

The first bounded instruction-context job also failed due to authored fixture API
sources beneath Ghidra's production script search directory. Its compiled bundle
referenced the stub `ProgramStub` field type and threw `NoSuchFieldError` against
the real engine. No result was accepted; all 476 bindings and owned cleanup passed.
The fresh retry retains the unchanged reviewed helper in a source-only leaf,
rejecting classes, subdirectories and symlinks before launch. All 523 bindings and
cleanup subsequently pass. This corrects research tooling, not an official-server
defect. Evidence: `work/control-caller-instructions-20261013/`, including the
preserved failed attempt and `execution-v2/`.

The first full-population C028 child-header invocation was rejected by the scope
guard before private fixity or probe launch: the lead-authored session repeated
the scope's authorization text with different wording. No native calls or accepted
output occurred. The original session, bindings and rejected-attempt receipt are
preserved; a separate reviewed retry derives the exact authorization source from
its scope and validates it before creating a fresh session. This is a session
construction error, not an original-server defect. Evidence:
`work/c028-all-struct-child-headers-20261013/rejected-attempt.json`.
The separately reviewed path-only retry passes with all 2,401 bindings unchanged;
its actual session-creator and entry fixtures reproduce the mismatch rejection and
verify exact scope-derived authority before exclusive session creation.

The new bounded x64 decoder's first real authored-engine batch exposed a checker
assumption: it expected a four-byte p-code register write for an encoded 32-bit
immediate MOV. Ghidra correctly emits an eight-byte zero-extending write. The
separate corrected checker verifies both encoded operands and exact p-code
register/constants; nine real recipes and five injected guard cases pass. The
failed checker/source/review/results remain preserved. No original bytes were
read, and no original-server behavior was tested by this correction. Evidence:
`work/control-client-decoder-api-validation-v2-20261013/` and its fresh v3 retry.

| ID | Trigger and former behavior | Change and evidence |
| --- | --- | --- |
| N-001 | Save worker exits before acknowledging an in-flight request; polling returned an untyped error while leaving `busy` true, and the main loop treated it as a retryable write failure | Return a typed terminal error, clear the stale in-flight flag and stop simulation. A deterministic disconnected-channel test covers missing acknowledgement and rejected subsequent submission |
| N-002 | Direct snapshot-store submission at tick 10 after acknowledged tick 20 succeeded as generation 2, allowing stale work to roll back the persisted simulation clock | Reject older ticks before any file publication. The regression first reproduced the rollback, then verified unchanged manifest/state and no orphan file on rejection. Equal-tick final saves and explicit previous-generation recovery remain permitted |
| N-003 | Native filesystem probe rejected a real ENOSPC because `tempfile` wraps the OS error with path context, retaining `ErrorKind` but hiding `raw_os_error()` | Classify typed `StorageFull`/`PermissionDenied` through the error chain. A real tempfile failure reproduces the lost errno; regressions reject misleading text. The isolated 2026-10-12 tmpfs run verified ENOSPC, EACCES, unchanged acknowledged state, both retries and reopened generation 4 |
| N-004 | Shutdown accepted a success receipt created after its deadline because it polled before checking time; synchronous final capture could also exhaust the budget and still enqueue a new write | Check time before polling and before returning completion; check the budget before final capture and again before enqueueing. Deterministic channel regressions first reproduced overdue success, then verified rejection, preserved pending state, later draining and no expired submission |
| N-005 | Envelope inspection accepted malformed UTF-8 in ignored JSON values because the byte-oriented JSON parser skipped those strings without validating their encoding | Validate the entire bounded UTF-8 input before parsing; authored invalid-byte, overlong and surrogate fixtures reproduce the former false success and now fail, while valid Unicode metadata remains accepted |
| N-006 | Offline reference harness could report failed inspection as successful cleanup, used only a predictable ownership label, and could spend more than the reserved lifetime; pre-launch hashing could also consume authorization slack | Require current session/hash/mode bindings, locked quotas and random exact ownership; preserve cleanup errors, bound each phase and clamp the final launch budget to fresh session expiry. Mocked daemon failures, repeated interrupts, ownership mismatches and delayed hashing/preparation verify the corrected behavior without executing the original |
| N-007 | Removing Position from a domain-identified entity silently excluded it from snapshot capture; a regression reproduced durable generation 2 acknowledging the truncated world | Query every DomainId and require Position before returning a snapshot. Four regressions cover unchanged committed bytes, no worker submission/receipt, repair and exact next generation, duplicate identities and ignored transient entities. This is a native-foundation defect, not an original-server bug |

Publication tests also inject Linux `ENOSPC` at the generation and manifest writer
boundaries. Both preserve the acknowledged generation across reopening and permit
a successful retry. A separate test models an error after publication: it returns
no success receipt, preserves the readable generation, and retains it as a fallback
on the next save. These are injected I/O outcomes, not real disk exhaustion or
physical power-loss tests. An actual rename obstruction verifies asynchronous
write-error reporting and recovery without a false save receipt.

On 2026-10-12, the real unprivileged 256 KiB tmpfs probe completed: 245,760 filler
bytes exhausted storage after acknowledged generation 2/tick 20; the failed save
preserved manifest bytes and the full snapshot. Removing the filler allowed
generation 3. Removing directory write permission rejected the next save without
changing manifest/state; restoring permission allowed generation 4/tick 40 with
1,100 entities, verified after reopening. Both failures returned errors, not save
receipts. This qualifies those native filesystem paths, not physical power loss.

Evidence: ignored `work/native-io-20261012/native-io-faults/` contains the exact
invocation, helper/binary hashes, container configuration, probe events and verified
cleanup. The case's scope/session records the current user authorization; the
historical failed run remains under `work/icarus-overnight/native-io-faults/`.
The reusable launcher now requires explicit case/session/probe inputs, current
timezone-aware deadlines, matching scope/probe hashes and exact container ownership.
It retains the offline, read-only, unprivileged resource contract and a total
120-second maximum including cleanup. Unit tests cover invalid scope/deadlines,
foreign containers, launch/log/removal/verification failures and missing probe events.

N-004's regression creates the deadline before creating the receipt, proving the
completion is late rather than merely consumed late. `finish_before` now refuses
an expired pending wait without consuming its queued result. If time expires while
polling consumes a completion, it returns no success receipt. An idle worker still
has nothing to wait for; the shutdown caller checks its remaining capture budget.
`submit_before` rejects a snapshot whose synchronous capture exhausted that budget,
without enqueueing a request. Neither method cancels an already running filesystem
operation or prevents OS scheduling pauses; these are honest deadline checks, not
a hard process-exit-time guarantee. Private red/green regression output is retained
under `work/native-deadline-20261012/`.
The updated native shutdown path and synthetic workload also pass startup/restart
smoke and a 120-second, 100,000-entity run with 60 commits and verified final state.
The current-revision recovery campaign then passed 24 cycles (twelve SIGKILL,
twelve SIGTERM) with 10,000 entities, preserving checksums, complete entity state
and acknowledged ticks. Its exact commands and binary/helper provenance reside
under `work/native-deadline-20261012/recovery/`; no original-server workload ran.
