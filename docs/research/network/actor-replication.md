# Actor replication after login

Static and recorded-wire evidence for how the client receives replicated state
once the control channel has admitted it; the spec is
[actor channel bunches](../../specs/network/actor-channel.md).

## Question cluster

- How does the client spawn an actor from an open actor bunch?\n- How are per-object content blocks and their payloads framed?\n- How are replicated properties encoded inside a content block?

## Current answer

An open actor bunch carries the actor NetGUID and, for dynamic actors, the
archetype, a gated level and optional location, rotation, scale and velocity
blocks (F-569), followed by `OnActorChannelOpen` / `OnSerializeNewActor` (empty
on `AActor`, 1-byte `NetPlayerIndex` at `+0x3d4` on `APlayerController`, F-610,
F-709). Export-only partial bunches carry a 1-bit `bHasRepLayoutExport` flag, a
4-byte `int32` count (`<= 2048`), and that many `InternalLoadObject` references
(`ReceiveNetGUIDBunch` `0x1439b8bb0` / `ExportNetGUIDHeader` `0x1439ab670`), and
bunches with `bHasMustBeMappedGUIDs` prefix a 2-byte `uint16` count and that
many `SerializeIntPacked` GUIDs (`ReceivedBunch` `0x1437c43f0`, F-709). Later
content blocks name the actor or a sub-object and carry a packed payload-bit
count for its replicator (F-576). Inside a block, packed rep handles walk the
layout in strictly increasing order, numbering array elements consecutively,
with nested streams for dynamic arrays (F-586); spawn vectors are raw floats or
bias-packed tenths, with 16-bit rotators (F-571, F-573). Bool, Byte, Enum,
object and soft-object items are read (F-574), enum widths deriving from the
enum's maximum (F-632). Name items take the network-name form, Str items and net
soft-object paths are archive strings (F-595, F-599), and Text items use the
shared serializer at archive version 522, every history read (F-639). Native
struct serializers read across all `3,960` `TCppStructOps` (`3,637` stub
`return 0`, `323` custom `NetSerialize`, F-709): quantized/normal vectors, tag
containers, hit results, movement, `FVector`, `FVector2D`, `FQuat`, `FPlane`
(`4 x i16`), `FDateTime`/`FTimespan` (`i64`), `FRotator`, `FUniqueNetIdRepl`,
`FRowHandle`, `293` typed `F*RowHandle` structs and `FStatPairRepState` (F-588
to F-597, F-704 to F-709). Field headers then carry RPCs, custom-delta payloads
and fast arrays (F-644). Generic framing, export bunches, open hooks and typed
class layouts, RPC caches and tag dictionaries replay 100% of M-707 traffic
(F-693, F-702 to F-709).

Offline schema derivation is authorized ([gates](../../gates.md)). The runtime
gameplay-tag net-index table is derived solely from `Data/Tags/D_Tags` (130
rows; 3 row keys differ from `Tag`) and `DefaultGameplayTags.ini` (1,886 tags
in 3.0.31; 1,889 in 3.0.32): expanding dot-separated parent prefixes yields
2,299 and 2,302 unique nodes, sorted ASCII-case-insensitively (`total_bits` 12,
clamped `first_segment` 12, `container_bits` 6), matching logged CRC-32 hashes
`dc780a27` and `6d0f676f` (F-702; U-065 resolved). Native `UE4CodeGen_Private`
descriptors across all 4,000 `ConstructUClass` sites and cooked
`UBlueprintGeneratedClass` / `UserDefinedEnum` pak exports fix replicated
property order, types, offsets, `CPF_RepSkip` exclusions, enum maxima and
`FClassNetCache` field-index bounds (`GetMaxIndex() + 1`: 5, 11, 7, 38, 84..86,
183; `0x141fff0e0` and `0x141fec960`) across 64 actor and component schemas in
`experimental::rep_layout`, `experimental::rep_rpc` and
`experimental::actor_channel` (F-703 to F-709; U-066 resolved). Together with
`read_export_prefix`, `read_must_be_mapped_guids` and
`PlayerControllerOpenBunchBody`, they replay all `317 / 317` export bunches
(`3,994` GUIDs), all `288 / 288` actor opens (including the 472-bit channel-2
`APlayerController` open bunch and its `7` content blocks), all `20,407 / 20,407`
`with_rep_layout` blocks (`100%`) and all `3,072 / 3,072` `has_rep_layout == false`
blocks (`100%`, including `220` actor RPCs and `295` zero-bit subobject headers;
`23,486 / 23,486` total typed blocks, `2,607` RPC calls, `66,859` property
entries, `2,361` stat-pair items) in M-707 bit-identically with 0 failures
(F-704 to F-709).

## Open

- Server send policy and dormancy/close lifecycle on actor channels (U-044, U-064).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707, M-708, M-709..M-717, M-720, M-728..M-744.
