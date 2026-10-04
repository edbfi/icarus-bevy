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
