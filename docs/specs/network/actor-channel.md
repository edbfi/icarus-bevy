# Actor channel bunches

## Status

Experimental: static observations of client build 25793767 and server build
25621760, wire-checked against the 3.0.31 capture (F-704 to F-711, F-739). Last reconciled: 2026-10-18.

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
  (F-709 to F-711): `UChannel::SendBunch` (`0x14353c300` / `0x1437c9690`;
  `UActorChannel::SendBunch` `0x143522160` / `0x1437b2680`) enforces
  `NetMaxConstructedPartialBunchSizeBytes = 65,536`, computes
  `MAX_SINGLE_BUNCH_SIZE_BITS = 7,625` and `MAX_PARTIAL_BUNCH_SIZE_BITS = 7,624`
  (`953` B for `MaxPacket = 1024`, `MaxPacketHandlerBits = 2`), calls
  `AppendExportBunches` (`0x14351fe30` / `0x143701ae0` server; `0x1437b03d0` /
  `0x1439a5290` client) and `AppendMustBeMappedGuids` (`0x14351ff00` /
  `0x1437b0490`), and slices bunches `> 7,625` bits into `7,624`-bit parts
  (`bPartialInitial` + `bOpen` on 0, `bPartialFinal` + `bClose` on the last;
  `experimental::sequencing` and `experimental::actor_channel`).
  `ReceivedRawBunch` (`0x143538460` / `0x1437c5cc0`) runs `ReceiveNetGUIDBunch`
  (`0x143719700` / `0x1439b8bb0`) when `bHasPackageMapExports` is set before
  reliable ordering (`InRec`, max 255). Export-only bunches
  (`ExportNetGUIDHeader` `0x1437089e0` / `0x1439ab670`, `ExportNetGUID`
  `0x143707e50` / `0x1439aabb0`) frame a 1-bit `bHasRepLayoutExport` flag (`0`
  for GUID exports), an `int32` count (`<= 2048`), and `InternalLoadObject`
  (`0x1439af1c0`) references. `ReceivedNextBunch` (`0x143537690` / `0x1437c5500`)
  reassembles `bPartial` bunches in `InPartialBunch`, and
  `UActorChannel::ReceivedBunch` (`0x143535c60` / `0x1437c43f0`) reads the
  `bHasMustBeMappedGUIDs` prefix before `ProcessBunch` (`0x143530b30` /
  `0x1437c0c30`).
- Open bunch, actor hook and dormancy transitions (F-569, F-610, F-709 to F-711,
  F-739): `UPackageMapClient::SerializeNewActor` (`0x1437201a0` server /
  `0x1439be280` client) frames the actor NetGUID and, for dynamic GUIDs (nonzero,
  even), the archetype NetGUID, a flag-gated level reference, and four optional
  presence-bit-gated blocks: location, rotation, scale and velocity
  (`SerializeOptionalValue<FVector>` `0x1436feda0` / `0x14399cac0` with
  `bQuantized = 1` for Location/Velocity and `0` for Scale3D,
  `Write/ReadPackedVector<10, 24>` `0x143471e90` / `0x1436fee6c`, and
  `FRotator::NetSerialize` `0x141ee74a0` / `0x141ed36b0` ->
  `SerializeCompressedShort` `0x141eeb7d0` / `0x141ed7810`, F-739). `ProcessBunch`
  and `ReplicateActor` (`0x14353a820` / `0x1437c7f00`) then invoke
  `OnActorChannelOpen` / `OnSerializeNewActor`: empty on `AActor` (`0x1408c9710`),
  and 1 byte (`uint8 NetPlayerIndex` at `+0x3d4`) on `APlayerController`
  (`0x1438384d0` / `0x143ae73a0` and `0x143838b70` / `0x143ae79d0`).
  `StartBecomingDormant` (`0x1435419d0` / `0x1437ce5b0`) sets `bPendingDormancy = 1`
  across `ReplicationMap` (`+0xa0`), and on `bClose` with `CloseReason == 1`
  (`ChannelCloseReason::Dormancy`, `136 / 136` closes in M-707),
  `UActorChannel::CleanUp` (`0x1435222c0` / `0x1437b27e0`) sets
  `Actor->NetDormancy = DORM_DormantAll` (`2`) without destroying the actor.
- Content blocks (F-576, F-581, F-709): after the open, until the bunch ends,
  each block is a rep-layout bit and an is-actor bit; a non-actor block adds the
  sub-object NetGUID and a stably-named bit, and a dynamic sub-object a class
  NetGUID whose zero value deletes it and ends the block. Otherwise a packed
  payload-bit count follows and those bits go to `FObjectReplicator::ReplicateProperties`
  (`0x1437f6b80`).
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
  F-639, F-673 to F-675, F-704 to F-709, F-739; U-064, U-066 resolved):
  - Spawn vectors read/write a quantized bit at `EngineNetVer >= 13`
    (unquantized: three floats; quantized: bit count `<= 24` and three integers
    scaled by `0.1`), and compressed rotators send per-axis nonzero bits and
    16-bit angles (`182.04445f` = `65536 / 360`) identically in both builds (F-739).
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

- `experimental::actor_channel`, `experimental::sequencing` and
  `experimental::rep_layout` read, re-encode, reassemble and re-split export
  prefixes (`317 / 317`), `PackageMapState` mappings, partial-bunch sequences
  (`300 / 300`), must-be-mapped GUID lists, open bunches, content blocks,
  property streams and `ChannelCloseReason` codes (`136 / 136`) bit-identically
  without panicking on randomized inputs (F-577, F-604, F-673, F-709 to F-711,
  F-739).

## Open questions

- Server relevancy and priority heuristics (authenticated session U-038).

## Evidence

- Symbols and pseudocode M-420 to M-437; net items and writers M-438 to M-449,
  M-455 to M-470, M-776, M-777, M-782, M-791; movement, tags, Name/Str, hit
  results, soft objects and Text M-475 to M-548, M-570 to M-588, M-602 to M-605;
  enum widths, RPCs and fast arrays M-614, M-615, M-620 to M-624, M-628 to M-635;
  native descriptors, cooked Blueprint exports, export/open hooks, partial/close
  lifecycle and wire replay M-552 to M-560, M-707, M-708, M-710 to M-713, M-717,
  M-720, M-731 to M-748.
