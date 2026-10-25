# Implementation status

The current checkpoint: what stands, milestone state and the next tasks. Claims
cite rows in the [findings ledgers](ledgers/findings/); open questions live in
the [unknowns ledger](ledgers/unknowns.md); preconditions live only in
[gates](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-10-25

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
2. Import (U-045, U-025): typing and coverage do not establish restoration (F-766, F-916). Retained candidates (F-920) support closed Bool/I32 assessment (F-929); authored lifecycle/scalar/inventory state persists (F-927). `UModifierStateComponent` vftable, `Activate`/`Deactivate`/`UpdateStats` and `Begin`/`EndPlay` match across builds while BP overrides and scaling internals remain open (F-930); authored modifier timers/cache survive reload and retries (F-931). Next: qualify remaining modifier/property internals, owner/row bindings and coordinates before all-or-nothing import. Ordinary `import-save` stays closed; coverage grants no publication and packaged row meanings remain unsupported (F-660). See [procedures](runbooks/authored-worlds.md); native costs remain F-926.
3. Player state: candidates and construction grids frame (F-668, F-643); no saved identifier or version string discriminates the producing build (F-635, F-896); write times place the save in the 3.0.29 window (F-896, F-685, F-878), and the producing server's log would confirm it (U-040).
4. Network sessions: `run` hosts the opt-in loopback session layer and pins Hello to the release profile (F-753 to F-759, F-875, F-841; [control negotiation](specs/network/control-negotiation.md)), computes class checksums natively (F-853), sends PlayerController, pawn, GameState and PlayerState opens with saved prospect/mode handles, session flags, locked missions, fresh RichPresenceGUID, host assignment and fresh-pawn vitals/inventories/stats (F-782, F-784, F-794, F-796, F-804, F-830, F-835, F-836, F-839, F-852, F-854, F-855, F-876, F-911 to F-913), advances constant-rate TimeOfDay (vs the original's curve, F-856), ServerFPS and NetworkingStatus (F-807 to F-811), and opens the radiation, world-talent, disaster, Quest, InventoryContainer (96/98 captured exact), PlayerHistoryTracker and FLOD managers from saved records (F-859, F-861, F-862, F-866, F-914). Next: loaded inventory weight (U-075: owner/tick flow F-915, F-917; authored cache/explicit composition F-918; bind restored flags, owner and dirty-write order), QuestManager quest actors (U-074), prospect refreshes (F-822; U-070), and imported pawn content/UID/tags/items (U-069); stock-client sessions stay gated (U-038, U-052).
5. Replication: the codecs replay M-707 completely (F-762) and server scheduling matches across builds (F-761, [actor channel](specs/network/actor-channel.md)). A native scheduling stage drives them over supplied actor positions (F-788, F-790, F-851), and its selections open, close and move archetyped actors on lab sessions (F-876, F-791, F-793). Next: other properties with the qualified codecs.
6. Reliability: compressed 72-hour schedules pass (F-872, F-873); 1,000 in-process/200 real-binary cycles show no session leaks (F-875). Paced GameState updates: one peer held 1 and 8 h, eight held 1 and 12 h, plus 10.1 h until reboot (F-833). The 12 h controller exit receipts are missing; copied reload passes. Earlier metrics/limits: F-932. In 300 s sweeps of 8–64 sessions, tick p99 stays under 80 us and all 64 receive unreliable NetworkingStatus partials (F-877). Next: multi-day wall-clock holds with controller receipts and physical power loss (U-035); no ICARUS performance proof (F-871).
7. Connected scenarios: this host fetches and runs the matched official server (F-687), the pin tracks the weekly builds (F-700, [architecture](architecture.md#release-tracking)), and S1 on 3.0.32 verified join, drop pod, harvest/replant and byte-identical save preservation (F-701); later scenarios await approval (U-047).
