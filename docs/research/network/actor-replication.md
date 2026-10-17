# Actor replication after login

Static and recorded-wire evidence for how the client receives replicated state
once the control channel has admitted it; the spec is
[actor channel bunches](../../specs/network/actor-channel.md).

## Question cluster

- How does the client spawn an actor from an open actor bunch?\n- How are per-object content blocks and their payloads framed?\n- How are replicated properties encoded inside a content block?

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
blocks parse the original's recorded traffic (F-693, F-694); typed class
layouts, RPC caches and tag dictionaries are verified across 100% of native
and cooked Blueprint actor and component blocks in M-707 (F-702 to F-708).

Offline schema derivation is authorized ([gates](../../gates.md)). The runtime
gameplay-tag net-index table is derived solely from `Data/Tags/D_Tags` (130
rows; 3 row keys differ from `Tag`) and `DefaultGameplayTags.ini` (1,886 tags
in 3.0.31; 1,889 in 3.0.32): expanding dot-separated parent prefixes yields
2,299 and 2,302 unique nodes, sorted ASCII-case-insensitively (`total_bits` 12,
clamped `first_segment` 12, `container_bits` 6), and their chained lowercase
UTF-16 CRC-32 matches the logged `NetworkGameplayTagNodeIndexHash` (`dc780a27`
and `6d0f676f`, F-702; U-065 resolved). Native `UE4CodeGen_Private` descriptors
across all 4,000 `ConstructUClass` sites and cooked `UBlueprintGeneratedClass` /
`UserDefinedEnum` pak exports fix replicated property order, types, offsets,
`CPF_RepSkip` exclusions, enum maxima (`ConstructUEnum` `0x14213b210` appends
`<Enum>::_MAX = max_explicit + 1` when absent) and `FClassNetCache` field-index
bounds (`GetMaxIndex() + 1`: 5 for `UIcarusStatContainer`, 11 for `UInventory`,
7 for `BP_UIProjectionComponent_MountStatus_C`, 38 for `BP_IcarusPlayerState_C`,
84..86 for mounts/tames, 183 for `BP_IcarusPlayerControllerSurvival_C`,
`0x141fff0e0` and `0x141fec960`) across 64 actor and component schemas in
`experimental::rep_layout` and `experimental::rep_rpc` (F-703 to F-708; U-066
resolved). `FNumericProperty::NetSerializeItem` (1/2/4/8B), `FVector`,
`FVector2D`, `FQuat::NetSerialize` (three `f32`s `X, Y, Z` with `W >= 0`
reconstructed), `FRotator`, `FUniqueNetIdRepl`, `FRowHandle::NetSerialize`
(293-table `SetupTables` row-count oracle), table-specific `F*RowHandle` 1B/2B
serializers, `FStatPairRepState::NetSerialize` (1..4B length + BE bytes),
`FStatsRepArray` and `FInventorySlotsFastArray` delta serializers, subobject
and actor RPCs (`ForceProjectionUpdate`, `Server_UpdateHealthValue`,
`Multicast_PlayReplicatedMontage`, `Multicast_StopReplicatedMontage`, and 11
`AController`/`APlayerController` RPCs), zero-bit subobject lifecycle headers,
`ACharacter` `MovementLevel::Hundredths` location quantization and the 1-bit
`bDoChecksum` preamble of `FRepLayout::ReceiveProperties` replay all `20,407 / 20,407`
`with_rep_layout` blocks (`100%`), all `3,072 / 3,072` `has_rep_layout == false`
blocks (`100%`, including `220` actor RPCs and `295` zero-bit subobject headers)
and all `7 / 7` `APlayerController` open-hook blocks (`23,486 / 23,486` total
typed blocks, `2,607` RPC calls, `66,859` property entries, `2,361` stat-pair
items) in M-707 bit-identically with 0 unclassified targets and 0 failures
(F-704 to F-708).

## Open

- Remaining native structs not exercised in M-707 (e.g., `FDateTime`/`FTimespan`, `FPlane`) and movement level defaults (U-064); server send policy (U-044).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707, M-708, M-709..M-717, M-720, M-728..M-742.
