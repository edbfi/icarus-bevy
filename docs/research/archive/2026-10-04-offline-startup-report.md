> Archived research report, retained verbatim. Current conclusions live in [the ledgers](../../ledgers/findings.md). Moved from `docs/research/` on 2026-10-13.

# Offline reference startup report

Historical report — 2026-10-04. Scope, authorization and next steps below describe
this completed batch; they do not authorize another run. See the
[current checkpoint](../../implementation-status.md) for pending work and pause status.

The original server reached its dedicated-server entry map under Proton during
the approved isolated run. It reported game version `3.0.30.158174-Shipping-DangerousHorizons`
and engine version `4.27.2-0+++UE4+Release-4.27`. Steam initialization failed in
this setup, so authenticated startup and stock-client play remain unverified.
The harness stopped the still-running container after approximately 36.93 seconds
and removed it. This experiment neither reproduces the reported server crashes
nor demonstrates any performance advantage for the replacement.

## Scope and environment

The user approved [this batch](../../history/offline-runtime-authorization.md), implemented
by [the launcher](../../../tools/run_reference_offline.sh). The local authorization
record is `work/icarus-offline-startup/scope.md`. The separate static-analysis case
continues to exclude original execution.

The reference is tool build **25621760**, with executable SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`.
Its imported interfaces are documented in the
[static baseline](2026-10-04-static-baseline.md); imports alone do not prove use.
Docker used the approved pinned image, whose runtime identified GE-Proton11-7.
Limits were two CPUs, 6 GiB memory and 512 processes, with no network, published
ports, image pull, real players or production saves. The installation was mounted
read-only; writable prefix and generated server files stayed in ignored `work/`.
No original assets or generated original data entered the Bevy project.

## Evidence

IDs below belong to `work/icarus-offline-startup/`, not the static-analysis case.
Exact artifact SHA-256 values are preserved in `evidence/index.md` and each
`evidence/E-*.md` record. Reproduction uses `timeout 90s bash tools/run_reference_offline.sh`
after case initialization and scope approval, as documented in the authorization.
The launcher refuses to reuse the existing container ID file; preserve this case
and prepare a fresh approved case for a repeat rather than overwriting evidence.

| ID | Case-local artifact | Observation |
| --- | --- | --- |
| E-001 | `evidence/container-inspect.json` | Running before stop; network `none`; original mount read-only |
| E-002 | `evidence/container-after-stop.json` | Exited after harness stop; exit 130; `OOMKilled=false` |
| E-003 | `evidence/container.log` | Proton version and Xalia display exception |
| E-004 | `runtime/Icarus/Saved/Logs/Icarus.log` | Engine/game versions, initialization and Steam failure diagnostics |
| E-005 | `evidence/resource-samples.jsonl` | Six startup resource samples, zero reported network bytes |

Read-only Docker inspection after cleanup returned no container matching the
recorded ID. The console artifact was captured before stopping; it is not a
complete post-stop console transcript. Server logs remained in the writable case.

## Findings

| ID | Severity / status / confidence | Evidence | Location and interpretation |
| --- | --- | --- | --- |
| F-001 | `n/a_re` / candidate / high | E-001, E-004 | Startup reaches entry-map initialization, but this is not authenticated gameplay readiness |
| F-002 | `n/a_re` / candidate / high | E-004 | Steam API fails repeatedly; offline networking, missing runtime prerequisites and read-only installation are unresolved confounders |
| F-003 | `n/a_re` / candidate / high | E-003, E-004 | Xalia reports no display, while the engine subsequently initializes; this is not evidence that ICARUS crashed |
| F-004 | `n/a_re` / candidate / high | E-001, E-002 | Container termination follows the harness stop; exit 130 and crash-reporter configuration files alone do not prove a spontaneous crash |

The server log reports a local `IpNetDriver` listener on port 17777 and loads
`StatelessConnectHandlerComponent` after Steam fails. This is evidence about this
fallback startup only: it does not establish the transport selected by a working
authenticated server. The entry world reports maximum tick rate 30, which does
not establish gameplay timing, replication frequency or all-mode compatibility.

The engine also logs FMOD and missing test-package diagnostics. They remain
untriaged observations because engine initialization continues; none is promoted
to a confirmed defect. No prospect was selected, no player connected and no
official save-format behavior was exercised.

## Observation path and timeline

P-001 (`solve`): start the pinned offline container (E-001) → observe Proton
initialization (E-003/F-003) → observe engine startup and Steam failures
(E-004/F-001/F-002) → sample resources (E-005) → stop the running container
(E-001/E-002/F-004) → verify removal. This describes the experiment, not a
recovered internal function call graph.

- 20:09:48.917 UTC: container started.
- 20:10:17.130 UTC: server reports engine initialization completed in 23.98 seconds.
- 20:10:25.846 UTC: container finished after harness stop.

The append-only case `timeline.md` records the batch review and evidence IDs.
The last six resource samples ranged from 67.02 MiB to 1.875 GiB and from 5.06%
to 148.01% Docker CPU. These are sparse container startup samples, including
Proton prefix setup; they are neither a measured peak nor a gameplay benchmark.
They must not be compared against the empty native Bevy loop as a speedup claim.

## Next compatibility gate

Establish a matching stock client and a working reference configuration in a
separately authorized private lab. Confirm legitimate Steam initialization before
classifying negotiation failures or collecting paired client/server traces. Record
the client build, reference configuration and actual loaded world. Reproduce the
user's reported failures under controlled workloads before claiming their causes
or fixes; retain the existing independent Rust durability tests as preventative
engineering evidence only.
