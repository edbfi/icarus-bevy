# Implementation status

The current checkpoint: what stands, milestone state and the next tasks. Claims
cite rows in the [findings ledgers](ledgers/findings/); open questions live in
the [unknowns ledger](ledgers/unknowns.md); preconditions live only in
[gates](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-10-23

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
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; client PE/PDB identity and MSF streams verified (F-878; U-034 resolved); producing save build log pending (U-040) |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; no official bug is marked reproduced or fixed (F-217); controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; the official server's data.pak fetches natively and its tables drive row handles (F-830, F-831); authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.1 / Bevy 0.20.0 foundation tested; opt-in loopback session layer admits authored probes (F-875); stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults pass; real saves preserve losslessly in native generations and run through autosaves (F-567, F-873); randomized inputs never panic the readers or the peer lab, and corrupted stores load a committed snapshot or fail closed (F-604 to F-607, F-611); every supplied value types losslessly and a dry-run import plan reports field-level blockers (F-766, F-898); value meanings and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

Offline work, in order; connected proof keeps its gates.

1. Data sources (F-828, F-829; U-072, U-073 resolved): the shipped server takes nothing from the game client. `icarus-server install-official-data` fetches the official server's data.pak anonymously into the data directory, where `run` uses it (F-831), and every protocol table `run` consumes already derives from the server distribution (F-832). Saved items and the faction mission in progress replicate as their data rows (F-861, F-866). Next: give further replicated row handles their rows as more opens need them, and fetch further server files only as they are needed.
2. Import (U-045, U-025): every recorder value types losslessly (F-766) and the dry-run import plan blocks only on unmapped content, now split by role; per-instance object names never block, actor variable records name class properties and fields with read restores classify as rows or world tiles by owning struct, with 23..26 unclassified in the measurements before root-content resolution (F-898, F-897). `import-save --catalog` resolves references by role against an authored catalog file. The earlier `--data-pak` measurements resolved every data-row reference then counted (F-898); root map, faction and setting references now also participate, so those counts do not describe the current plan. The capture save's actor classes are shipped Blueprints or native classes and its actor paths name shipped level packages (F-860). Next: author the catalog for root map references, actor classes, paths, tiles and class variables with independently authored world data, then write native world records behind the plan's acceptance conditions; packaged row meanings stay out of scope (F-660).
3. Player state: candidates and construction grids frame (F-668, F-643); no saved identifier or version string discriminates the producing build (F-635, F-896); write times place the save in the 3.0.29 window (F-896, F-685, F-878), and the producing server's log would confirm it (U-040).
4. Network sessions: `run` hosts an opt-in loopback session layer over the verified framing, control vocabulary and admission mirror (F-753 to F-759, F-875, F-841) and pins the Hello version to the release profile (F-875; [control negotiation](specs/network/control-negotiation.md)). Class checksums are native (F-853); `run` sends the PlayerController, pawn, GameState and PlayerState opens after Join in the capture's order and framing (lab values), repeating exports until acked (F-784, F-794, F-796, F-804, F-852, F-854), the shutdown notice and channel-failure close (F-782, F-876). The pawn's content blocks compose natively from session values (F-854), and a preserved world's active prospect and game mode handles 34, 35, 39, 41, 42 and 46 replicate from its save, matching the capture bit for bit (F-855). The original server advances TimeOfDay by a per-world rate curve (F-856); `run` follows the GameState open with its updates, advancing the time of day at the saved day length's constant rate instead of that curve (F-807), its own frame rate as ServerFPS (F-808, F-809) and a locally saving NetworkingStatus listing its players (F-810, F-811). Session flags, locked missions, the open-world and outpost flags and a fresh pawn's vitals, inventories and stats replicate from the save and data.pak (F-830, F-835, F-836, F-839, F-911); the GameState draws a fresh RichPresenceGUID per run (F-912), and a player joining with no other joined player is host (F-913). After the PlayerState, `run` opens the GameState's radiation, world-talent and disaster managers bit for bit as captured (F-859), then the QuestManager, InventoryContainerManager (with saved items; 96 of 98 captured inventories exact), PlayerHistoryTracker and FLOD manager from a save's records (F-861, F-862, F-866, F-914). Next: a loaded inventory's weight (U-075), the quest actors the QuestManager references (U-074), prospect refreshes, whose direct callers in the original are spawn completion, dropship leave and backend or setting changes (F-822; U-070), and imported pawn content, including its IcarusUID and tags, with the player's saved items (U-069); stock-client sessions stay gated (U-038, U-052).
5. Replication: the codecs replay M-707 completely (F-762) and server scheduling matches across builds (F-761, [actor channel](specs/network/actor-channel.md)). A native scheduling stage drives them over supplied actor positions (F-788, F-790, F-851), and its selections open, close and move archetyped actors on lab sessions (F-876, F-791, F-793). Next: other properties with the qualified codecs.
6. Reliability: 72-hour schedules pass in compressed time (F-872, F-873) and sessions do not leak over 1,000 in-process or 200 real-binary cycles (F-875). Held sessions keep steady GameState updates, one for 8 h and eight for 1 h, and eight held 10.1 h until a host reboot, after which the world reloaded at its last save (F-833); in 300 s sweeps of 8 to 64 sessions tick p99 stays under 80 us, and a NetworkingStatus over one bunch now goes out as unreliable partials that all 64 receive (F-877). Next: longer paced wall-clock holds and physical power loss (U-035); none of it is ICARUS performance (F-871).
7. Connected scenarios: this host fetches and runs the matched official server (F-687), the pin tracks the weekly builds (F-700, [architecture](architecture.md#release-tracking)), and S1 on 3.0.32 verified join, drop pod, harvest/replant and byte-identical save preservation (F-701); later scenarios await approval (U-047).
