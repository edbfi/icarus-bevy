# Actor channel bunches

## Status

Experimental: static observations of client build 25793767 and server build
25621760, wire-checked against the 3.0.31 capture (F-704 to F-711). Last reconciled: 2026-10-18.

## Scope

How client and server frame actor-channel bunches after the control channel:
export prefixes, outgoing partial-bunch segmentation and incoming reassembly,
actor spawn records, per-object content blocks, and channel close / dormancy
transitions. Not covered: server relevancy and priority heuristics.

## Confirmed

- NetGUID references (F-572, F-692): a packed GUID value (`0` null, recursion
  capped at 16); GUID 1 or an export-enabled archive adds an export-flags byte:
  bit 0 a recursive outer GUID and path string, bit 2 a 4-byte network checksum
  after the path, bit 1 no-load. Nonzero even GUIDs are dynamic.
- Gameplay-tag net-index dictionary (F-702; U-065 resolved):
  `ConstructGameplayTagTree` populates the tree from `Data/Tags/D_Tags` (130
  rows; 3 row keys differ from `Tag`) and `DefaultGameplayTags.ini` (1,886
  explicit tags in 3.0.31; 1,889 in 3.0.32), expanding parent prefixes into
  2,299 and 2,302 unique nodes. `ConstructNetIndex` sorts nodes
  ASCII-case-insensitively (`GetAllNodes` + `FName::LexicalLess`), derives
  `InvalidTagNetIndex = N + 1`, `total_bits = 12` and clamped
  `first_segment = 12` (`container_bits = 6`), and logs the chained lowercase
  UTF-16 CRC-32 `NetworkGameplayTagNodeIndexHash` (`dc780a27` in 3.0.31,
  `6d0f676f` in 3.0.32; `experimental::tag_dictionary`).
- NetGUID exports, partial-bunch split/reassembly and must-be-mapped prefixes
  (F-709 to F-711): `UChannel::SendBunch` (`0x1437c9690`) enforces
  `NetMaxConstructedPartialBunchSizeBytes = 65,536`, computes
  `MAX_SINGLE_BUNCH_SIZE_BITS = (MaxPacket * 8) - MaxPacketHandlerBits - 565`
  and `MAX_PARTIAL_BUNCH_SIZE_BITS = MAX_SINGLE_BUNCH_SIZE_BITS & ~7` (`7,625`
  and `7,624` bits / `953` B for `MaxPacket = 1024`, `MaxPacketHandlerBits = 2`),
  calls `AppendExportBunches` (`0x1437b03b0` / `0x1439a5290`) to prepend
  export-only bunches (`bHasPackageMapExports = 1`) and
  `AppendMustBeMappedGuids` (`0x1437b0490`) to prepend `uint16` count + packed
  GUIDs (`bHasMustBeMappedGUIDs = 1`), slices bunches above
  `MAX_SINGLE_BUNCH_SIZE_BITS` into `MAX_PARTIAL_BUNCH_SIZE_BITS` byte-aligned
  parts (`bPartial = 1`, `bPartialInitial = 1` + `bOpen` on entry `0`,
  `bPartialFinal = 1` + `bClose` on the last; `experimental::sequencing` and
  `experimental::actor_channel`). `ReceivedRawBunch` (`0x1437c5cc0`) runs
  `ReceiveNetGUIDBunch` (`0x1439b8bb0`) when `bHasPackageMapExports` is set
  before reliable ordering (`InRec`, max 255). Export-only bunches frame a
  1-bit `bHasRepLayoutExport` flag (`0` for GUID exports), a 4-byte `int32`
  count (`<= 2048`), and `InternalLoadObject` (`0x1439af1c0`) references.
  `ReceivedNextBunch` (`0x1437c5500`) reassembles `bPartial` bunches in
  `InPartialBunch` (`PartialBunchReassembler`), and `UActorChannel::ReceivedBunch`
  (`0x1437c43f0`) reads the `bHasMustBeMappedGUIDs` prefix before `ProcessBunch`
  (`0x1437c0c30`).
- Open bunch, actor hook and dormancy transitions (F-569, F-610, F-709 to F-711):
  `SerializeNewActor` reads the actor NetGUID; for a dynamic GUID (nonzero,
  even) it reads the archetype NetGUID, a level reference gated on archive
  flags, and four optional presence-bit-gated blocks: location, rotation, scale
  and velocity. `ProcessBunch` and `ReplicateActor` (`0x1437c7f00`) then invoke
  `OnActorChannelOpen` / `OnSerializeNewActor`: empty on `AActor`
  (`0x1408c9710`), and 1 byte (`uint8 NetPlayerIndex` at `+0x3d4`) on
  `APlayerController` (`0x143ae73a0` / `0x143ae79d0`,
  `PlayerControllerOpenBunchBody`). `StartBecomingDormant` (`0x1437ce5b0`) sets
  `bPendingDormancy = 1` (`+0x30 | 0x80`) across `ReplicationMap` (`+0xa0`), and
  on `bClose` with `CloseReason == 1` (`ChannelCloseReason::Dormancy`, `136 / 136`
  closes in M-707), `UActorChannel::CleanUp` (`0x1437b27e0`) sets
  `Actor->NetDormancy = DORM_DormantAll` (`2`) without destroying the world actor.
- Content blocks (F-576, F-581, F-709): after the open, until the bunch ends,
  each block is a rep-layout bit and an is-actor bit; a non-actor block adds the
  sub-object NetGUID and a stably-named bit, and a dynamic sub-object a class
  NetGUID whose zero value deletes it and ends the block. Otherwise a packed
  payload-bit count follows and those bits go to `FObjectReplicator::ReplicateProperties`
  (`0x1437f6b80`).
- `experimental::actor_channel`, `experimental::sequencing` and
  `experimental::rep_layout` read, re-encode, reassemble and re-split export
  prefixes (`317 / 317`), `PackageMapState` mappings, partial-bunch sequences
  (`300 / 300`), must-be-mapped GUID lists, open bunches, content blocks,
  property streams and `ChannelCloseReason` codes (`136 / 136`) bit-identically
  without panicking on randomized inputs (F-577, F-604, F-673, F-709 to F-711).
- Replicator property and field loops (F-571, F-586, F-609, F-644, F-703 to F-709):
  `FRepLayout::ReceiveProperties` (`0x143b20720`) reads a 1-bit `bDoChecksum`
  flag and a zero-terminated packed-handle stream (dynamic arrays carry a 2-byte
  element count and nested handle stream) before the field-header loop. Each
  field is a `SerializeInt` index below `FClassNetCache::GetMaxIndex() + 1`
  (`0x1437c2650`), a packed bit count and payload (`experimental::rep_rpc`).
  Native `ConstructUClass` descriptors sort own `CPF_Net` properties by
  `(offset, name)`, whereas cooked `UBlueprintGeneratedClass` exports
  (`ClassFlags & 0x80`) keep `ChildProperties` order (`experimental::rep_layout`
  models 64 schemas covering all `20,407 / 20,407` `with_rep_layout` blocks in
  M-707). Custom-delta fast arrays carry a 1-bit header, packed static-array
  index, four 4-byte header words, 4-byte delete IDs, and changed elements.
- Spawn vectors, property net items and struct/scalar serializers (F-571, F-573
  to F-575, F-583, F-588, F-589, F-591 to F-593, F-595, F-597, F-599, F-632,
  F-639, F-673 to F-675, F-704 to F-709; U-064, U-066 resolved):
  - Spawn vectors read a quantized bit (unquantized: three floats; quantized:
    bit count `<= 24` and three integers scaled by `0.1`), and compressed
    rotators send per-axis nonzero bits and 16-bit angles.
  - `FRepLayout::ReceiveCustomProperty` (`0x143b219b0`) dispatches Bool (1 bit),
    Byte/Enum (`min(8, CeilLogTwo64(max + 1))` bits with `_MAX = max_explicit + 1`),
    `FNumericProperty` (1/2/4/8 B), object NetGUIDs, and structs (`StructFlags & 0x400`
    calls `ICppStructOps::NetSerialize`: `3,637` of `3,960` `TCppStructOps` use
    the `return 0` stub `0x1408cb520` and recurse through `0x143af7340`, while
    `323` define custom `NetSerialize`, including `293` typed `F*RowHandle`
    structs, `FRowHandle`, `FVector`, `FVector2D`, `FQuat`, `FPlane`,
    `FDateTime`, `FTimespan`, `FRotator`, `FUniqueNetIdRepl`, `FStatPairRepState`,
    `FVector_NetQuantize*`, `FRepMovement`, and `FHitResult`).
  - Gameplay tags use 12-bit net indices (`container_bits = 6`), Name items use
    hardcoded (`<= 702`) or archive-string + number network names, Str and
    soft-object paths use archive strings, and Text items use the UE4 version
    522 `FText` history framing (`0..12`, `0xFF`; `experimental::rep_text`),
    replaying all `23,486 / 23,486` (`100%`) typed blocks in M-707 bit-identically.

## Candidate

- The older server's `SerializeNewActor` (M-460) makes the same three object
  references and single rotation bit as the client's; its quantizers are
  inlined and unread.

## Open questions

- Server relevancy and priority heuristics (authenticated session U-038).

## Evidence

- Symbols and pseudocode M-420 to M-437; net items and writers M-438 to M-449,
  M-455 to M-470; movement, tags, Name/Str, hit results, soft objects and Text
  M-475 to M-548, M-570 to M-588, M-602 to M-605; enum widths, RPCs and fast
  arrays M-614, M-615, M-620 to M-624, M-628 to M-635; native descriptors,
  cooked Blueprint exports, export/open hooks, partial/close lifecycle and wire
  replay M-552 to M-560, M-707, M-708, M-710 to M-713, M-717, M-720, M-731 to M-748.
