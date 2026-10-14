> Archived research report, retained verbatim. Current conclusions live in [the ledgers](../../ledgers/findings/). Moved from `docs/research/` on 2026-10-13.

# Original prospect-startup rejection observations

Historical report — 2026-10-04. The runtime windows below have ended. See the
[current checkpoint](../../implementation-status.md) for later findings, pending work
and pause status.

The original build **25621760** rejected a missing prospect file once and an
independently authored truncated JSON file twice. All three runs remained running
until the harness stopped them after about 150 seconds. The repeated malformed
case also completed the independent stateless handshake after the rejection.
**These tests did not reproduce a crash or demonstrate a valid world load.**

The documented command-line option is `-LoadProspect=NAME`; it selects
`Saved/PlayerData/DedicatedServer/Prospects/NAME.json` beneath the configured UserDir.
This is specified in RocketWerkz's
[server launch documentation](https://github.com/RocketWerkz/IcarusDedicatedServer/wiki/Server-Config-%26-Launch-Parameters)
and confirmed by the observed lookup path. The earlier
[ExecCmds experiment](2026-10-04-overnight-runtime-checkpoint.md) remains inconclusive.

| Private run | Input | Observed outcome |
| --- | --- | --- |
| `run-04-missing-save` | Name `overnight-authored`, no file | Attempted by-name load, reported file not found, entry server remained running |
| `run-05-truncated-save` | 34 authored bytes, incomplete JSON | Found file, JSON conversion reported parse failure, load failure logged, entry server remained running |
| `run-06-truncated-save-live` | Same preserved input | Same rejection; subsequent request/challenge/echo/ack exchange passed |

The malformed input contains only an empty `ProspectInfo` object and an unfinished
`ProspectBlob` member. It contains no extracted game content or production save
state. Each run preserves the pristine input separately from the writable copy.
Source bytes, hashes, exact command vectors, Docker inspection, pre/post-stop logs,
resource samples and cleanup results are in `work/icarus-overnight/`. Evidence
E-077–E-080 and E-089–E-092 record these runs.

All used the approved pinned image, read-only originals, disabled networking,
unprivileged user, dropped capabilities and bounded resources. Exit code 130
followed the harness stop; Docker reported no OOM kill. Each exact container was
removed. The successful probe ran inside the same isolated namespace and sent no
identity material or control messages. Steam authentication was inactive there,
so liveness is limited to the stateless layer.

## What these observations resolve

The by-name command reaches file lookup and JSON conversion in this environment.
For these two small failure cases, rejection preserves a responsive entry server.
This narrows the bug investigation: merely supplying a missing or truncated
by-name file did not reproduce the user's reported instability.

The distinct `LocalSavePath`/from-file path, deeper blob decoding, valid world load,
state application, interrupted saves and full prospect restart remain untested.
There was no previously loaded world whose atomic preservation could be checked.
Do not generalize these results to all corrupt saves or to the separate fatal-output
lead. The [loading specification](../../specs/save-format/prospect-loading.md) retains those gates.
