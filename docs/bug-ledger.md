# Bug investigation ledger

These campaigns originate from the user's experience. None is currently a
reproduced defect or a claimed fix. Do not infer root cause from bundled DLL names.

| Priority | Reported problem | Status | Next evidence |
| --- | --- | --- | --- |
| 1 | Crashes, hangs and save integrity | Unreproduced | Preserved failing state, actions, dumps, committed save generations |
| 2 | Long-session degradation | Unreproduced | Fixed-workload memory, object/handle counts, tick and save distributions |
| 3 | Joining and reconnecting | Unreproduced | Paired client/server logs, lifecycle traces, session ownership |
| 4 | Large worlds, tames, bases, players | Unreproduced | Workload dimensions, profiles, Windows/Proton comparison |

For each reproduction record: build/configuration/hardware, starting-state hash,
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
Evidence is scoped to `icarus-server-baseline`; see the
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

| ID | Trigger and former behavior | Change and evidence |
| --- | --- | --- |
| N-001 | Save worker exits before acknowledging an in-flight request; polling returned an untyped error while leaving `busy` true, and the main loop treated it as a retryable write failure | Return a typed terminal error, clear the stale in-flight flag and stop simulation. A deterministic disconnected-channel test covers missing acknowledgement and rejected subsequent submission |

Publication tests also inject Linux `ENOSPC` at the generation and manifest writer
boundaries. Both preserve the acknowledged generation across reopening and permit
a successful retry. A separate test models an error after publication: it returns
no success receipt, preserves the readable generation, and retains it as a fallback
on the next save. These are injected I/O outcomes, not real disk exhaustion or
physical power-loss tests. An actual rename obstruction verifies asynchronous
write-error reporting and recovery without a false save receipt.
