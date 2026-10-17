# Actor replication after login

Static and recorded-wire evidence for how the client receives replicated state
once the control channel has admitted it; the spec is
[actor channel bunches](../../specs/network/actor-channel.md).

## Question cluster

- How does the client spawn an actor from an open actor bunch?
- How are per-object content blocks and their payloads framed?
- How are replicated properties encoded inside a content block?

## Current answer

An open actor bunch carries the actor NetGUID and, for dynamic actors, the
archetype, a gated level and optional location, rotation, scale and velocity
blocks (F-569). Later content blocks name the actor or a sub-object and carry
a packed payload-bit count for its replicator (F-576). Inside a block, packed
rep handles walk the layout in strictly increasing order, numbering array
elements consecutively, with nested streams for dynamic arrays (F-586); spawn
vectors are raw floats or bias-packed tenths, with 16-bit rotators (F-571,
F-573). Bool, Byte, Enum, object and soft-object items are read (F-574), enum
widths deriving from the enum's maximum (F-632). Name items take the
network-name form, Str items and net soft-object paths are archive strings
(F-595, F-599), and Text items use the shared serializer at archive version
522, every history read (F-639). Native struct serializers read so far:
quantized and normal vectors, tag containers and hit results (F-593, F-597),
and the movement struct, whose quantization levels are property defaults
(F-588, F-589). Gameplay tags go as names or as split net indices with
configured widths (F-591, F-592). Field headers then carry RPCs, custom-delta
payloads and fast arrays (F-644). These are client-build observations; the
older server's content-block and NetGUID writers mirror them (F-581) and
compose actor bunches in client read order (F-610). Generic framing and content
blocks now parse the original's recorded traffic (F-693, F-694); typed class
layouts and tag dictionaries are verified on native and cooked Blueprint actor
and component blocks (F-702 to F-706).

Offline schema derivation is authorized ([gates](../../gates.md)). The runtime
gameplay-tag net-index table is derived solely from `Data/Tags/D_Tags` (130
rows; 3 row keys differ from `Tag`) and `DefaultGameplayTags.ini` (1,886 tags
in 3.0.31; 1,889 in 3.0.32): expanding dot-separated parent prefixes yields
2,299 and 2,302 unique nodes, sorted ASCII-case-insensitively (`total_bits` 12,
clamped `first_segment` 12, `container_bits` 6), and their chained lowercase
UTF-16 CRC-32 matches the logged `NetworkGameplayTagNodeIndexHash` (`dc780a27`
and `6d0f676f`, F-702; U-065 resolved). `experimental::tag_dictionary` models
both ordered lists and declaration expansion with `net_index_hash`. Native
`UE4CodeGen_Private` descriptors across all 4,000 `ConstructUClass` sites and
cooked `UBlueprintGeneratedClass` / `UserDefinedEnum` exports in
`pakchunk0-WindowsServer.pak` and `pakchunk0_s1-WindowsServer.pak` fix
replicated property order, types, offsets, `CPF_RepSkip` exclusions and enum
maxima (`ConstructUEnum` `0x14213b210` appends `<Enum>::_MAX = max_explicit + 1`
when absent) across 36 actor and component schemas in `experimental::rep_layout`
(F-703, F-705, F-706). `FNumericProperty::NetSerializeItem` (1/2/4/8 archive
bytes for `Int8`..`UInt64`, `Float`, `Double`), `FVector`, `FVector2D`,
`FRotator` (`SerializeCompressedShort`), `FUniqueNetIdRepl`,
`FRowHandle::NetSerialize` (`UDatatableFunctionLibrary::SetupTables` 293-table
row-count oracle, 0/1/2/4-byte row index), table-specific `F*RowHandle`
1-byte/2-byte row-index serializers, `FStatPairRepState::NetSerialize`
(`Detail::WriteInteger`/`Detail::ReadInteger` 1..4-byte length prefix +
big-endian bytes), `FStatsRepArray::NetDeltaSerialize` (bound 4),
`FInventorySlotsFastArray::NetDeltaSerialize` (bound 10),
`BP_UIProjectionComponent_MountStatus_C::ForceProjectionUpdate` field headers
(bound 6), `ACharacter` `MovementLevel::Hundredths` location quantization and
the 1-bit `bDoChecksum` preamble of `FRepLayout::ReceiveProperties` are
qualified and modeled in `experimental::rep_layout` and `examples/capture_decode`,
replaying all 22,892 attempted actor and subobject blocks (`58,368` property
entries and `2,361` stat-pair items; `20,486` of `20,618` `with_rep_layout`
blocks, `99.36%`) in M-707 bit-identically with 0 failures (F-704 to F-706).

## Open

- Remaining native structs (e.g., `FDateTime`/`FTimespan`, `FPlane`) and movement level defaults (U-064); server send policy (U-044).
- Remaining 132 world/quest/deployable content blocks in M-707 (F-702 to F-706; U-066).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707, M-708, M-709..M-717, M-720, M-728..M-738.
