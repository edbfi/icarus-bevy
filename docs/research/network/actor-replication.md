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

Server and client builds also match across relevancy, priority, tick rate,
connection and channel readiness, `UActorChannel::ReplicateActor` and every
`UNetDriver::ServerReplicateActors` stage, whose constants and guards the
[spec](../../specs/network/actor-channel.md) lists (F-745, F-748, F-749,
F-751); the relevancy re-check window, force-relevance frame comparison and
mark step are refined by F-772, F-774 and F-788. A native scheduling stage
drives these evaluators over 2,626 supplied actor positions; with UE default
parameters it takes 0.3 to 5.2 ms per frame for 1 to 8 viewers (F-790). Export checksums are class network checksums computed natively
(F-776), reproducing 77 of 79 captured values (F-777; U-067), and the
original's first post-Join bunch opens the PlayerController channel (F-778),
which the native composer reproduces bit for bit (F-779). The GameState open
names its manager actors by GUIDs assigned on first reference, before their
own opens, and is also reproduced bit for bit (F-794); `run` sends the
PlayerController, GameState and PlayerState opens after Join (F-795).

## Open

- Two Blueprint class checksums with user-struct arrays (U-067).
- Authenticated runtime relevancy and priority exercise (U-038).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707..M-717, M-720, M-728..M-748, M-776, M-777, M-782, M-794, M-795, M-797..M-799, M-801, M-802, M-804, M-805, M-807, M-813..M-819, M-832.
