# Actor channel bunches

## Status

Experimental: static observations of client build 25793767, with server
build 25621760 writers for content blocks and NetGUIDs; wire-checked against
one capture of the 3.0.31 server (F-704 to F-710). Last reconciled: 2026-10-17.

## Scope

How the client reads actor-channel bunches after the control channel: export
prefixes, partial-bunch reassembly, the spawn record of a new actor, the
per-object content blocks that carry replicated state, and channel close /
dormancy cleanup. Not covered: every server-side send-policy heuristic.

## Confirmed

- NetGUID references (F-572): a packed GUID value, 0 meaning none and
  recursion capped at 16 levels; GUID 1 or an export-enabled archive adds an
  export-flags byte: bit 0 a recursive outer GUID and a path string, bit 2 a
  4-byte network checksum after the path (none without one, F-692), bit 1
  no-load. Nonzero even GUIDs are dynamic.
- Gameplay-tag net-index dictionary (F-702; U-065 resolved): `ConstructGameplayTagTree`
  populates the tree solely from `Data/Tags/D_Tags` (130 rows; 3 row keys differ
  from `Tag`) and `DefaultGameplayTags.ini` (1,886 explicit tags in 3.0.31; 1,889
  in 3.0.32), expanding dot-separated parent prefixes into 2,299 and 2,302 unique
  nodes (`LegacyNativeTags` and `NativeTagsToAdd` are empty). `ConstructNetIndex`
  sorts nodes ASCII-case-insensitively (`GetAllNodes` + `FName::LexicalLess`),
  derives `InvalidTagNetIndex = N + 1`, `total_bits = 12` and clamped
  `first_segment = 12` (`container_bits = 6`), and logs the chained lowercase
  UTF-16 CRC-32 `NetworkGameplayTagNodeIndexHash` (`dc780a27` in 3.0.31,
  `6d0f676f` in 3.0.32). `experimental::tag_dictionary` models both supplied
  ordered lists and declaration expansion with `net_index_hash`.
- NetGUID exports, partial-bunch reassembly and must-be-mapped prefixes (F-709, F-710): `UChannel::ReceivedRawBunch` (`0x1437c5cc0`) invokes
  `ReceiveNetGUIDBunch` (`0x1439b8bb0`) when `bHasPackageMapExports` is set
  before reliable ordering (`InRec`, max 255). Export-only partial bunches frame
  a 1-bit `bHasRepLayoutExport` flag (0 for GUID exports), a 4-byte `int32`
  count (`<= 2048`), and that many `InternalLoadObject` (`0x1439af1c0`)
  references (`PackageMapState` in `experimental::actor_channel`).
  `UChannel::ReceivedNextBunch` (`0x1437c5500`) reassembles `bPartial` bunches
  in `InPartialBunch`: `bPartialInitial` skips payload append when
  `bHasPackageMapExports` is set, non-final parts without exports must be
  non-empty and 8-bit aligned, continuation `ChSequence` steps by 1 (capped at
  `NetMaxConstructedPartialBunchSizeBytes = 65,536` on a server), and
  `bPartialFinal` rejects exports and copies `bClose`, `CloseReason`, `bDormant`
  and `bHasMustBeMappedGUIDs` (`PartialBunchReassembler` in
  `experimental::sequencing`). When `bHasMustBeMappedGUIDs` is set,
  `UActorChannel::ReceivedBunch` (`0x1437c43f0`) reads a 2-byte `uint16` count
  and that many `SerializeIntPacked` GUIDs before `ProcessBunch` (`0x1437c0c30`).
- Open bunch, actor hook and dormancy close (F-569, F-610, F-709, F-710):
  `SerializeNewActor` reads the actor NetGUID through the package map; for a
  dynamic GUID (nonzero, even) it reads the archetype NetGUID, a level reference
  gated on archive flags, and four optional presence-bit-gated blocks: location
  (quantized vector), rotation (compressed rotator), scale (quantized vector) and
  velocity (quantized vector). `UActorChannel::ProcessBunch` (`0x1437c0c30`) and
  `ReplicateActor` (`0x1437c7f00`) then invoke `OnActorChannelOpen` /
  `OnSerializeNewActor`: empty on `AActor` (`0x1408c9710`), and 1 byte
  (`uint8 NetPlayerIndex` at `+0x3d4`) on `APlayerController` (`0x143ae73a0` /
  `0x143ae79d0`), followed by content blocks (`PlayerControllerOpenBunchBody` in
  `experimental::actor_channel`). When a reassembled bunch sets `bClose`,
  `ReceivedNextBunch` sets `bDormant = (CloseReason == 1)` and calls
  `ConditionalCleanUp(false, CloseReason)` (`0x1437b37f0`); on `CloseReason == 1`
  (`EChannelCloseReason::Dormancy`, `ChannelCloseReason::Dormancy` in
  `experimental::packet`, `136 / 136` closes in M-707), `UActorChannel::CleanUp`
  (`0x1437b27e0`) sets `Actor->NetDormancy = DORM_DormantAll` (`2`) and removes
  the network actor without destroying the world actor, freeing the channel slot
  for reuse.
- Content blocks (F-576, F-581, F-709): after the open, until the bunch ends,
  each block is a rep-layout bit and an is-actor bit; a non-actor block adds the
  sub-object NetGUID and a stably-named bit, and a dynamic sub-object a class
  NetGUID whose zero value deletes it and ends the block (`ReplicateActor`
  `0x1437c7f00`). Otherwise a packed payload-bit count follows, and those bits
  go to the object's replicator.
- `experimental::actor_channel` and `experimental::sequencing` read and
  re-encode export prefixes, `PackageMapState` mappings, `PartialBunchReassembler`
  sequences, must-be-mapped GUID lists, open bunches (including the
  `APlayerController` open hook), content blocks and `ChannelCloseReason` codes
  bit-identically (F-577, F-709, F-710), and `experimental::rep_layout` reads and
  re-encodes property streams (F-673). Randomized and mutated inputs never panic
  either reader (F-604).
- The replicator hands rep-layout properties to `FRepLayout::ReceiveProperties`
  (an optional bit, a packed handle, an optional checksum word, then a handle
  loop) before its field-header loop (M-426, M-427; F-644). Each field until the
  payload ends is a `SerializeInt` index below `max_index + 1` (`FClassNetCache::GetMaxIndex() + 1`,
  `0x1437c2650`), a packed bit count and that payload (unknown indices skipped).
  `UClass::SetUpRuntimeReplicationData` (`0x141fec960`) and
  `FClassNetCacheMgr::GetClassNetCache` (`0x141fff0e0`) number own `CPF_Net`
  properties then own `FUNC_Net` functions (`SuperStruct == NULL`, sorted by
  `FName`) from `SuperNetCache->FieldsBase + SuperNetCache->Fields.Num()`
  (`SerializeInt` bounds: 5, 11, 7, 38, 84..86, 183; F-708). Custom-delta
  payloads start with a bit (net version 11+) and a static array's packed index,
  then the struct's delta serializer (all 7 gameplay fast arrays in the binary
  use `FastArrayDeltaSerialize`, F-709). Fast-array bodies carry four 4-byte
  header words (counts capped at 2048 initially), a 4-byte ID per delete, and
  per change a 4-byte ID plus item commands, a set bit and rep-layout property
  stream in struct-delta mode, or a native item serializer. Function fields are
  RPCs (`experimental::rep_rpc` re-encodes all, F-644, F-708).
- Rep-layout stream and handle walk (F-571, F-586, F-609): packed handles select
  layout commands and handle 0 ends a stream; a dynamic array carries a 2-byte
  element count and a nested zero-terminated handle stream. A class's layout
  orders the super class's list before its own net properties (field order under
  class flag `0x80`, else ascending offset with name tie-break), one entry per
  static array element, then a return command and handle table.
- Native `UE4CodeGen_Private` descriptors and cooked Blueprint class extensions
  (F-703, F-705 to F-707): none of the 4,000 native `ConstructUClass` sites sets
  class flag `0x80` (own properties sort by `(offset, name)`), whereas cooked
  `UBlueprintGeneratedClass` exports (`ClassFlags & 0x80`) use `ChildProperties`
  order:
  - `AActor` (handles 1..15): 1..4 bool flags, 5 `RemoteRole` (3-bit), 6
    `ReplicatedMovement` (`ACharacter` subclasses use `MovementLevel::Hundredths`
    for location), 7..12 `AttachmentReplication`, 13 `Owner`, 14 `Role` (3-bit),
    15 `Instigator`.
  - `AController` adds 16..17 (`PlayerState`, `Pawn`), `APlayerController` adds
    18..19 (`TargetViewRotation`, `SpawnLocation`), `APlayerState` adds 16..26,
    and `UActorComponent` defines 1..2 (`bReplicates`, `bIsActive`).
    `experimental::rep_layout` models 64 native and cooked Blueprint schemas
    covering all `20,407 / 20,407` `with_rep_layout` blocks in M-707 (F-703,
    F-705 to F-707).
- Spawn vectors (F-571, F-573): after the presence bit, archives at engine net
  version 13+ read a quantized bit; unquantized is three 4-byte floats,
  quantized a component bit count (`<= 24`) and three integers bounded by
  `1 << (bits + 2)`, decoded as `(v - (1 << (bits + 1))) * 0.1`.
- Property net items and command expansion (F-574, F-575, F-632, F-704 to F-709):
  `FRepLayout::ReceiveCustomProperty` (`0x143b219b0`) calls `Property->NetSerializeItem`
  (`vtable + 0x98`). Bool is 1 bit; Byte is 8 bits or `min(8, CeilLogTwo64(max + 1))`
  with an enum (`max <= 0` sends 0 bits; native `ConstructUEnum` `0x14213b210`
  and cooked `UserDefinedEnum` assets append `_MAX = max_explicit + 1` unless
  present); Enum is that width capped at its underlying integer's bits;
  `FNumericProperty::NetSerializeItem` (`0x1421206f0`) reads 1/2/4/8 bytes;
  object references are a NetGUID; structs with `STRUCT_NetSerializeNative`
  (`StructFlags & 0x400`) emit one command and call `ICppStructOps::NetSerialize`
  (`+0x78`), whereas structs without `0x400` recurse through `0x143af7340`
  (skipping `CPF_RepSkip` fields). Across all `3,960` `TCppStructOps` in the
  binary, `3,637` use the `return 0` stub (`0x1408cb520`, including `FColor`,
  `FLinearColor`, `FIntPoint`, `FIntVector`, `FGuid`, `FTransform`, `FBox` and
  `FBox2D`) and `323` define custom `NetSerialize` (`293` typed `F*RowHandle`
  structs, `FRowHandle`, and `29` engine/game structs, F-709). Framing the 1-bit
  `bDoChecksum` preamble of `ReceiveProperties` (`0x143b20720`, M-426) with
  custom-delta fast arrays, subobject/actor RPCs, zero-bit subobject headers,
  partial-bunch reassembly, dormancy closes and the `APlayerController` open
  hook replays all `23,486 / 23,486` (`100%`) typed blocks, `317 / 317` export
  bunches, `300 / 300` partial sequences and `136 / 136` dormancy closes in
  M-707 bit-identically with 0 failures (F-704 to F-710; U-064, U-066 resolved).
- Native struct items (F-588, F-589, F-593, F-597, F-673, F-674, F-704 to F-709):
  `FVector` (`0x142100d00`), `FVector2D` (`0x142100bf0`) and `FQuat` (`0x1421003f0`
  -> `0x141ed3a00`, `X, Y, Z` with `W >= 0` reconstructed) serialize three, two
  and three 4-byte floats; `FPlane` (`0x142100980`) serializes four 2-byte
  `int16` words; `FDateTime` and `FTimespan` (`0x142100050`) serialize one
  8-byte `int64` (`Ticks`); `FRotator` (`0x142100890`) calls
  `SerializeCompressedShort`; `FUniqueNetIdRepl` (`0x143ddced0` -> `0x1439a23f0`)
  uses the control-channel unique-net-id framing (F-531, F-636); `FRowHandle`
  (`0x143f8a530`) sends a 2-byte table index (`< 293` or `0xffff`) and a
  0/1/2/4-byte row index sized by table row count, whereas typed `F*RowHandle`
  subclasses serialize only their 1-byte or 2-byte (`FQuestsRowHandle`
  `0x14165b3d0`) row index (F-705 to F-707); `FStatPairRepState` (`0x141c10c50`)
  writes a 1-byte length `1..=4` plus big-endian bytes for `Stat` and `Value`.
  `FVector_NetQuantize` is a bit count (`<= 20`) and three biased unscaled
  integers; `FVector_NetQuantizeNormal` is three 16-bit values biased by 32768.
  `FRepMovement` sends two flag bits, location, rotation (byte rotator at
  level 0; compressed rotator at level 1), linear velocity, and optional angular
  velocity; vector levels 0, 1 and 2 cap bits below 24, 27 and 30 with scales 1,
  0.1 and 0.01. Hit results send an 8-bit flags byte, 4-byte time,
  quantized/normal vectors, gated depth/item, three NetGUID references, bone
  network name, and gated face index and element byte.
- Gameplay tags (F-583, F-591, F-592, F-674): sent as names unless fast
  replication is set; then as a net index (`T` raw bits when `F < 1` or
  `F >= T`, else `F` low bits, a continuation bit and `T - F` high bits when
  set). Containers send an empty bit, a `W`-bit count (`<= 2^W - 1`) and that
  many tags.
- Name, Str, soft-object and Text items (F-595, F-599, F-639, F-675): Name items
  call the archive name operator (`SerializeName` or `StaticSerializeName`, the
  network-name form below). Str items and net soft-object paths are archive
  strings (signed 4-byte count, then narrow bytes or 2-byte units with a
  terminator). Text items use the shared FText serializer at UE4 archive version
  522 (`0x170+` 4-byte flags word, then history byte `0..12` or `0xFF` none;
  `experimental::rep_text` re-encodes all, F-639):
  - `0` Base sends namespace and key as text keys, then source string; `0xFF`
    none (Dev-Editor 40 >= 32) sends a 4-byte culture-invariant bool and
    optional string.
  - `1` NamedFormat, `2` OrderedFormat and `3` ArgumentDataFormat send a format
    Text and argument map/records (Dev-Editor 40 >= 5).
  - `4` AsNumber, `5` AsPercent and `6` AsCurrency send a format-argument value,
    optional number options (Dev-Editor 40 > `0x14`) and target-culture string.
  - `7` AsDate, `8` AsTime and `9` AsDateTime send an 8-byte date-time, style
    byte(s), time-zone string and culture string; `10` Transform sends a nested
    Text and transform byte; `11` StringTableEntry sends a network-name table
    Name and key string; `12` TextGenerator sends a generator Name and, unless
    `None`, a 4-byte byte count and payload.
- Network names (F-575): a hardcoded bit; when set, the hardcoded name index
  (packed at engine net version 6+, else bounded by 411; `> 702` rejected, saved
  up to 410); when clear, an archive string and a 4-byte number.
- Compressed rotator (F-573): pitch, yaw and roll in that order, each a bit set
  when its 16-bit quantized angle is nonzero and then that 2-byte value
  (`round(angle * 65536 / 360)`).

## Candidate

- The older server's `SerializeNewActor` (M-460) makes the same three object
  references and single rotation bit as the client's; its quantizers are
  inlined and unread.

## Open questions

- Server send-policy heuristics and control-bunch framing past Hello (U-044).

## Evidence

- Symbols M-420 (failed-closed attempts M-421, M-422); pseudocode M-423 to
  M-431, M-433; constants M-432, M-435 to M-437 (scope deviation M-434); net items M-438 to
  M-449; server writers M-455 to M-462;
  server struct items M-463 to M-470; handle walk M-428; movement and scaled
  vectors M-477 to M-491; tags M-493 to M-495; Name and Str routing M-497 to
  M-510; hit results M-468, M-512 to M-521; soft objects M-445, M-523 to
  M-529; Text M-531 to M-536, M-545 to M-547, M-570;
  net archive version M-571 to M-576;
  display strings M-581 to M-584; Text codec M-585, M-602 to M-605; Text callees M-586,
  M-587; custom versions M-588; enum widths M-614, M-615; fields and RPCs M-620 to M-624; fast arrays M-628 to M-635; codec tests M-475, M-476, M-492, M-496,
  M-511, M-522, M-530, M-548; native class/struct/enum descriptors, cooked Blueprint exports, RPC caches, export/open hooks, partial/close lifecycle and scalar/struct net serializers M-710 to M-713, M-717, M-720, M-731 to M-746;
  layout source M-552 to M-555; send order M-461, M-556 to
  M-560; wire capture M-707, M-708.
