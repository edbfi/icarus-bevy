# Implementation status

The current checkpoint: what stands, milestone state and the next tasks. Claims
cite rows in the [findings ledgers](ledgers/findings/); open questions live in
the [unknowns ledger](ledgers/unknowns.md); preconditions live only in
[gates](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-10-18

## Summary

This is an independent native server foundation, not an ICARUS-compatible
server (F-487). One bounded stateless exchange has been observed against the
original (F-253). Protocol interoperability, authentication, gameplay,
world-data parity, official-save import and the release reliability and
performance targets are not demonstrated.

Offline research resumed under the user's execution instruction (F-404);
offline original-server save-load runs are now authorized, connected-client
tests only per approved scenario ([gates](gates.md)). The user
reports the local client/save pair compatible, with authenticated and reference
access deferred (F-421).

What the foundation does is in the
[foundation digest](research/foundation/foundation.md). Research state is in the
[C001](research/save-format/c001-constructor.md),
[save-class](research/save-format/save-class-audits.md),
[control-store](research/network/control-stores.md) and
[actor-replication](research/network/actor-replication.md) digests.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; client identity checked (F-486); exact build pairing pending |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; no official bug is marked reproduced or fixed (F-217); controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.1 / Bevy 0.20.0 foundation tested; stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults pass; real saves preserve losslessly in native generations and run through autosaves (F-567, F-584); randomized inputs never panic the readers or the peer lab, and corrupted stores load a committed snapshot or fail closed (F-604 to F-607, F-611); value meanings and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

Offline work, in order; connected proof keeps its gates.

1. Save classes: every body of every class frames and round-trips in both copies (F-557, F-565, F-602), recorder fields have meanings (F-645 to F-661, F-665, F-666, F-676, F-677), and C001's complete functions, registration and vtable/archive bridge plus server `FString`/`FName` normalization (`ParseNumber` `_<1..10 digits>`), `FFloatProperty`/`FVector`/`FArrayProperty` correspondence, immutable `CPF_SaveGame` flags, tagged property wrap-around, and C013/C028 migration and restore rules are verified (F-715 to F-722; U-003 to U-006, U-023, U-026, U-029 to U-031, U-033, U-039, U-041, U-042, U-054, U-056, U-057, U-059 to U-062 resolved); Name identities and row meanings sit in packaged tables (F-660), whose use is your call (U-020).
2. Player state: candidates and construction grids frame (F-668, F-643); no saved identifier or version string discriminates the producing build (F-635, F-682); write times place the save in the 3.0.29 window, older than both installations (F-683, F-685), and the producing server's log would confirm it (U-040).
3. Control stores: owner, complete `UWorld::UWorld` (1,332 B, `.pdata` 255917..255921) and `UPendingNetGame::UPendingNetGame` (71 B, 245246) constructors, `UWorld::~UWorld` (`TARGET02`, 757 B, 255950), `UObject`/`FURL` receiver preservation, `/GR-` abutting vftables (`0x145467ef0`/`0x145468180`), `InitBase` (`Driver + 0x208`), `ReceivedBunch` dispatch and both builds' `NotifyControlMessage` correspond statically (F-505, F-506, F-520, F-544, F-546, F-579, F-723, F-724; U-007 to U-009, U-013, U-015 to U-018, U-043 resolved); authenticated runtime dispatch needs a connected session (U-010, U-038).
4. Packet framing and transport: formats, reliability, partial-bunch split/reassembly, close/dormancy codes, control vocabulary, `UControlChannel` byte-order persistence (`Connection + 0x158`), 19 fallback skip layouts, 32,767 queued-bunch bounds, unique net ids, `IsClientMsgTypeValid` login-state gating, `PreLogin` + `SpawnPlayActor`/`InitNewPlayer` checks (`NetPlayerIndex`, 20-unit `Name` truncation) and 0 bound version/PreLogin callbacks across both binaries (F-712 to F-714; U-014, U-037, U-044 resolved) are recorded, with experimental codecs, a loopback peer lab and a native admission mirror ([control negotiation](specs/network/control-negotiation.md)); the offline original accepts the framing, answers Hello (F-688, F-689) and welcomes a synthetic Login and Join (F-690); authenticated Steam handler sessions remain (U-038).
5. World data: both copies and S1's saved prospect preserve losslessly and seeded worlds autosave (F-567, F-584, F-701); restore, migration and entry rules are authored in `experimental::restore_rules` and `experimental::archive` (F-663, F-715 to F-719); the preserved server loads, saves and reloads a supplied copy offline, dropping only two physics-dynamic tree records and one fully mined respawned voxel record (F-678 to F-681); an interpreted import stays gated (U-045).
6. Reliability: 72-hour schedules pass in compressed time (F-615, F-616); paced wall-clock effects and physical power loss remain (U-035), and none of it is ICARUS performance (F-485).
7. Replication: actor-channel codecs decode and re-encode a 240 s original capture (F-692, F-709 to F-711). Schema derivation is authorized; gameplay-tag sources (`Data/Tags/D_Tags` and `DefaultGameplayTags.ini`), prefix expansion, case-insensitive sorting, 12-bit net-index widths and logged CRC-32 hashes (`dc780a27` in 3.0.31, `6d0f676f` in 3.0.32) are verified in `experimental::tag_dictionary` (F-702; U-065 resolved). Native `UE4CodeGen_Private` descriptors across 4,000 classes, cooked Blueprint `FProperty`/`UserDefinedEnum` pak exports, 64 native/Blueprint actor and component layouts, `FClassNetCache` field-index bounds (`GetMaxIndex() + 1`: 5, 11, 7, 38, 84..86, 183), the 3,960-struct `TCppStructOps` `NetSerialize` census, export-bunch and must-be-mapped GUID prefixes, `PackageMapState` (`encode_export_bunch_payload`, `prepend_must_be_mapped_guids`), `PartialBunchReassembler`, `split_outgoing_bunch` (`7,625`-bit threshold, `7,624`-bit slices), `ChannelCloseReason::Dormancy`, `APlayerController` 1-byte `NetPlayerIndex` open-bunch hooks, subobject/actor RPCs, zero-bit subobject headers and the `ReceiveProperties` checksum preamble in `experimental::actor_channel`, `experimental::sequencing`, `experimental::rep_layout`, `experimental::rep_rpc` and `examples/capture_decode` replay and re-encode 100% (`317 / 317` export bunches, `300 / 300` partial sequences, `136 / 136` dormancy closes, `288 / 288` actor opens and `23,486 / 23,486` content blocks) of M-707 bit-identically with 0 failures (F-703 to F-711; U-064, U-066 resolved; [actor channel](specs/network/actor-channel.md)).
8. Connected scenarios: this host fetches and runs the matched official server (F-687), and the release pin tracks the weekly builds (now 25956137 / 3.0.32, F-700, [architecture](architecture.md#release-tracking)); a synthetic player lifts the empty-world pause offline (F-690, F-691), and connected scenario S1 with the user's client on 3.0.32 verified character join, drop pod, crop harvest/replant and byte-identical save preservation (F-701); later scenarios await approval (U-047).
