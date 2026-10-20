# Actor replication after login

Static and recorded-wire evidence for how the client and server exchange
replicated actor state once the control channel has admitted a connection; the
normative wire and scheduling rules live in
[actor channel bunches](../../specs/network/actor-channel.md).

## Question cluster

- How does the client spawn an actor from an open actor bunch?
- How are per-object content blocks, exports, partials, and dormancy framed?
- How are replicated properties encoded inside a content block?
- How does the server evaluate relevancy, priority, connection readiness, and the consider/prioritize/process stages?

## Current answer

Open actor bunches frame the actor NetGUID and, for dynamic actors, the
archetype, level, and optional location, rotation, scale, and velocity blocks
(`UPackageMapClient::SerializeNewActor` `0x1437201a0` / `0x1439be280`, F-569,
F-739), followed by `OnActorChannelOpen` / `OnSerializeNewActor` (`AActor`
empty; `APlayerController` 1-byte `NetPlayerIndex` at `+0x3d4`, F-610, F-709,
F-751). Outgoing `UChannel::SendBunch` (`0x14353c300` / `0x1437c9690`) enforces
the 64 KiB partial cap, prepends export-only bunches (`ExportNetGUIDHeader` /
`ExportNetGUID`, `<= 2048` entries) and `bHasMustBeMappedGUIDs` prefixes, and
splits bunches `> 7,625` bits into `7,624`-bit (`953` B) slices (F-709 to F-711).
Incoming `ReceiveNetGUIDBunch`, `ReceivedNextBunch`, `StartBecomingDormant`, and
`UActorChannel::CleanUp` reassemble partial sequences and preserve dormant
actors on `CloseReason == 1` (`Dormancy`, F-710). Content blocks carry a
rep-layout bit, is-actor bit, optional sub-object/class NetGUIDs, and packed bit
count for `FObjectReplicator::ReplicateProperties` (F-576, F-586).

Offline schema derivation is authorized ([gates](../../gates.md)). The runtime
gameplay-tag net-index dictionary derives from `Data/Tags/D_Tags` (130 rows) and
`DefaultGameplayTags.ini` (1,886 tags in 3.0.31; 1,889 in 3.0.32 -> 2,299 and
2,302 sorted nodes, `12`-bit indices, `6`-bit containers, CRC-32 `dc780a27` and
`6d0f676f` in `experimental::tag_dictionary`, F-702; U-065 resolved). Native
`UE4CodeGen_Private` descriptors (4,000 classes), cooked Blueprint/enum pak
exports, 64 actor/component schemas, `FClassNetCache` bounds (`5, 11, 7, 38,
84..86, 183`), and the `3,960`-struct `TCppStructOps` `NetSerialize` census
(`3,637` stub `0`, `323` custom incl. `293` `F*RowHandle` structs and
`FStatPairRepState`) in `experimental::rep_layout`, `experimental::rep_rpc`,
`experimental::sequencing`, and `experimental::actor_channel` replay and
re-encode `317 / 317` export bunches, `300 / 300` partial sequences, `136 / 136`
dormancy closes, `288 / 288` actor opens, `20,407 / 20,407` `with_rep_layout`
blocks, and `3,072 / 3,072` `has_rep_layout == false` blocks (`220` actor RPCs,
`295` zero-bit subobject headers; `23,486 / 23,486` total typed blocks, `2,607`
RPC calls, `66,859` property entries, `2,361` stat-pair items) in M-707 with 0
failures (F-571 to F-644, F-703 to F-711, F-739; U-064, U-066 resolved).

Server and client builds also match across `AActor`/`APawn`/`APlayerController`/
`AOnlineBeacon::IsNetRelevantFor`, `IsWithinNetRelevancyDistance`,
`AActor`/`APlayerController`/`AMatineeActor::GetNetPriority`,
`UGameEngine::GetMaxTickRate` (`30` Hz default, `[1, 1000]` clamp, `750,000` B/s
max client rate in `DefaultEngine.ini`), `UNetConnection::IsNetReady`
(`Saturate` sets `QueuedBits = -SendBuffer.GetNumBits()`, then
`QueuedBits + SendBuffer.GetNumBits() <= 0` or adaptive `+0x1ba0` byte/packet
budget), `UChannel::IsNetReady` (`NumOutRec < 255`),
`UActorChannel::ReplicateActor` (6 early-exit guards, `FReplicationFlags`,
`!bNetOwner` `ROLE_AutonomousProxy -> ROLE_SimulatedProxy` downgrade,
`SerializeNewActor` on `bNetInitial && SpawnAcked`, `SendBunch` iff bits
written, and unconditional `LastUpdateTime = ElapsedTime` at `+0x80`), and all
`UNetDriver::ServerReplicateActors` (`ServerTickTime = (MaxTickRate <= 0) ?
DeltaSeconds : 1 / MaxTickRate`, `bCPUSaturated = DeltaSeconds > 1.2 *
ServerTickTime`, `ClientConnections` rotation) stages (`PrepConnections`
`< 1.5` s `LastReceiveTime` gate; `BuildConsiderList` `> 2.0` s `* 0.2`
adaptive `OptimalNetUpdateDelta` lerp with `MinNetUpdateFrequency = 2.0`;
`PrioritizeActors` `cvtss2si` `round_ties_even(65536 * p)` and
`DestroyedStartupOrDormantActors` `cvttss2si` `250k`/`4M`/`9.998M` `* 0.4`/`0.2`
scaling; `ProcessPrioritizedActors` `<= 1.0` s relevancy fast-path, `+ 0.5 *
SRand` jitter, `< 1.0` Hz `+ 0.2 * srands` delay, and `CloseReason` `4`/`3` when
`!bLevelInitializedForActor || !bNetStartup`; and inlined `MarkRelevantActors`
`<= 1.0` s channel fast-path, `+ 0.5 * SRand` `RelevantTime`, and
`ForceRelevantFrame = ReplicationFrame + 1` when `>= LastProcessedFrame`,
F-745, F-748, F-749, F-751); force relevance compares with the connection's
`LastProcessedFrame`, unsigned (F-772). A native scheduling stage drives these
evaluators over 2,626 supplied actor positions in 0.12 to 1.64 ms per frame
for 1 to 8 viewers (F-773).

## Open

- Authenticated runtime relevancy and priority exercise (U-038).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707..M-717, M-720, M-728..M-748, M-776, M-777, M-782, M-794, M-795, M-797..M-799, M-801, M-802, M-804, M-805, M-807, M-813, M-814.
