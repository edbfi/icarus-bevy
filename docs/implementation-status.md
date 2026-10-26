# Implementation status

The current checkpoint: what stands, milestone state and the next tasks. Claims
cite rows in the [findings ledgers](ledgers/findings/); open questions live in
the [unknowns ledger](ledgers/unknowns.md); preconditions live only in
[gates](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-10-26

## Summary

This is an independent native server foundation, not an ICARUS-compatible
server (F-487). One bounded stateless exchange has been observed against the
original (F-253). Protocol interoperability, authentication, gameplay,
world-data parity, official-save import and the release reliability and
performance targets are not demonstrated.

Offline research resumed under the user's execution instruction (F-404);
offline original-server save-load runs are now authorized, connected-client
tests only per approved scenario ([gates](gates.md)). Input parity: the native
server needs no more than the official dedicated server needs, never the game
client (F-828). The user reports the local client/save pair compatible, with
authenticated and reference access deferred (F-421).

The native server types every supplied recorder value losslessly and plans
imports with field-level blockers by role (F-766, F-898); `run` hosts an
opt-in loopback session layer tested only with authored probes (F-875, F-833).
`install-official-data` fetches the official server's data tables anonymously
without steamcmd (F-831), and replicated row handles take their indices from
them (F-830).

What the foundation does is in the
[foundation digest](research/foundation/foundation.md). Research state is in the
[C001](research/save-format/c001-constructor.md),
[save-class](research/save-format/save-class-audits.md),
[control-store](research/network/control-stores.md) and
[actor-replication](research/network/actor-replication.md) digests.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; server/client PE/PDB identities and MSF streams verified (F-878; U-034 resolved); producing save build log pending (U-040) |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; no official bug is marked reproduced or fixed (F-217); controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; the official server's data.pak fetches natively and its tables drive row handles (F-830, F-831); authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.1 / Bevy 0.20.0 foundation tested; opt-in loopback session layer admits authored probes (F-875); stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults pass; real saves preserve losslessly in native generations and run through autosaves (F-567, F-873); randomized inputs never panic the readers or the peer lab, and corrupted stores load a committed snapshot or fail closed (F-604 to F-607, F-611); every supplied value types losslessly and a dry-run import plan reports field-level blockers (F-766, F-898); value meanings and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

Offline work, in order; connected proof keeps its gates.

1. Data sources (F-828, F-829; U-072, U-073 resolved): the shipped server takes nothing from the game client. `icarus-server install-official-data` fetches the official server's data.pak anonymously into the data directory, where `run` uses it (F-831), and every protocol table `run` consumes already derives from the server distribution (F-832). Saved items and the faction mission in progress replicate as their data rows (F-861, F-866). Next: give further replicated row handles their rows as more opens need them, and fetch further server files only as they are needed.
2. Import (U-045, U-025): typing/coverage are not restoration (F-766, F-916).
   Retained closed Bool/I32 candidates: F-920, F-927, F-929; component
   lifecycle/scaling/init/callbacks: F-930; local removal/comparator call/read
   models: F-937; the server comparator helper compares receiver u64s at +8
   (F-940). Owner/row and subclass effects remain open. Authored reload/retry
   timers/cache persist (F-931). Next: overrides, owner/row bindings and
   coordinates before all-or-nothing import.
   `import-save` stays closed; coverage grants no publication; packaged row
   meanings stay unsupported (F-660). [Procedures](runbooks/authored-worlds.md);
   native costs: F-926.
3. Player state: candidates and construction grids frame (F-668, F-643); no saved identifier or version string discriminates the producing build (F-635, F-896); write times place the save in the 3.0.29 window (F-896, F-685, F-878), and the producing server's log would confirm it (U-040).
4. Network sessions: `run` hosts the opt-in loopback session layer and pins Hello to the release profile (F-753 to F-759, F-875, F-841; [control negotiation](specs/network/control-negotiation.md)), computes class checksums natively (F-853), sends PlayerController, pawn, GameState and PlayerState opens with saved prospect/mode handles, session flags, locked missions, fresh RichPresenceGUID, host assignment and fresh-pawn vitals/inventories/stats (F-782, F-784, F-794, F-796, F-804, F-830, F-835, F-836, F-839, F-852, F-854, F-855, F-876, F-911 to F-913), advances constant-rate TimeOfDay (vs the original's curve, F-856), ServerFPS and NetworkingStatus (F-807 to F-811), and opens the radiation, world-talent and disaster managers, stitched C019 quest actors (unsupported Check() graphs/classes fail closed, U-076) and the Quest, InventoryContainer (98/98 captured exact), PlayerHistoryTracker and FLOD managers from saved records (F-859, F-861, F-862, F-864, F-866, F-914, F-933 to F-935; U-074, U-075 resolved). Explicit prospect refresh transport is tested with authored peers (F-936). Next: original refresh triggers (F-822; U-070), fail-closed quest classes (U-076) and imported pawn content/UID/tags/items (U-069); stock-client sessions stay gated (U-038, U-052).
5. Replication: capture replay and scheduling: F-762, F-761 ([actor channel](specs/network/actor-channel.md)). `run` installs the opt-in authored schema-4 bridge with explicit bindings and frame, atomic validation and retirement (F-938; [configuration](runbooks/native-server.md#network-sessions-experimental)). Archetype checks reduce measured debug idle cost at 1,024 actors (F-939). Next: measure release costs, qualify further properties and saved-pose mappings (U-025); import, physics/rotation/velocity source mappings and stock-client behavior remain unqualified.
6. Reliability: compressed 72-hour schedules pass (F-872, F-873); 1,000 in-process/200 real-binary cycles show no session leaks (F-875). Paced hold history/limits: F-833, F-932. A 48 h attempt stopped at its disk guard 8.57 h after admission without a final probe summary; copied reload passes (M-1055). In 300 s sweeps of 8–64 sessions, tick p99 stays under 80 us and all 64 receive unreliable NetworkingStatus partials (F-877). Next: multi-day wall-clock holds with controller receipts and physical power loss (U-035); no ICARUS performance proof (F-871).
7. Connected scenarios: this host fetches and runs the matched official server (F-687), the pin tracks the weekly builds (F-700, [architecture](architecture.md#release-tracking)), and S1 on 3.0.32 verified join, drop pod, harvest/replant and byte-identical save preservation (F-701); later scenarios await approval (U-047).
