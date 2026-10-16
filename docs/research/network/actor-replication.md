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
layouts and tag dictionaries remain unvalidated.

Offline schema derivation is authorized ([gates](../../gates.md)). The runtime
gameplay-tag net-index table is derived solely from `Data/Tags/D_Tags` (130
rows; 3 row keys differ from `Tag`) and `DefaultGameplayTags.ini` (1,886 tags
in 3.0.31; 1,889 in 3.0.32): expanding dot-separated parent prefixes yields
2,299 and 2,302 unique nodes, sorted ASCII-case-insensitively (`total_bits` 12,
clamped `first_segment` 12, `container_bits` 6), and their chained lowercase
UTF-16 CRC-32 matches the logged `NetworkGameplayTagNodeIndexHash` (`dc780a27`
and `6d0f676f`, F-702; U-065 resolved). `experimental::tag_dictionary` models
both ordered lists and declaration expansion with `net_index_hash`. Native
`UE4CodeGen_Private` descriptors across all 4,000 `ConstructUClass` sites fix
replicated property order, types, offsets and static array sizes for `AActor`
(10 net properties -> 15 leaf handles: `FRepAttachment` flattened at 7..12,
3-bit `ENetRole` at 5 and 14), `AController` (handles 16..17),
`APlayerController` (handles 18..19), `APlayerState` (handles 16..26),
`UActorComponent` (handles 1..2) and native Icarus subclasses (F-703), with
layout constructors in `experimental::rep_layout`.

## Open

- Other native structs and scalar items (U-064); server send policy (U-044).
- Cooked Blueprint class property extensions and typed capture replay (F-702, F-703; U-066).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707, M-709..M-717, M-720, M-728..M-732.
