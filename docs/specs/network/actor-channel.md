# Actor channel bunches

## Status

Experimental: static observations of client build 25793767, with server
build 25621760 writers for content blocks and NetGUIDs; wire-checked against
one capture of the 3.0.31 server (F-693, F-704 to F-708). Last reconciled: 2026-10-17.

## Scope

How the client reads actor-channel bunches after the control channel: the
spawn record of a new actor and the per-object content blocks that carry
replicated state. Not covered: dormancy, channel close reasons and every
server-side writer.

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
- NetGUID exports (F-694, wire-consistent, not statically read) ride
  export-only partial bunches: a rep-layout bit, an i32 count, then that many
  exported references.
- Open bunch (F-569): `SerializeNewActor` reads the actor NetGUID through the
  package map. For a dynamic GUID (nonzero, even) it then reads the archetype
  NetGUID, a level reference gated on archive flags, and four optional blocks
  in this order, each a presence bit followed by its data: location (quantized
  vector), rotation (compressed rotator), scale (quantized vector) and
  velocity (quantized vector). Static GUIDs carry none of these.
- Content blocks (F-576): after the open, until the bunch ends, each block is a
  rep-layout bit and an is-actor bit; a non-actor block adds the sub-object
  NetGUID and a stably-named bit, and a dynamic sub-object a class NetGUID
  whose zero value deletes it and ends the block. Otherwise a packed count of
  payload bits follows, and exactly those bits go to the object's replicator.
- The older server's writers mirror these layouts: content-block headers with
  packed payload counts, and NetGUIDs with export flags, path and checksum
  (F-581).
- `experimental::actor_channel` reads and re-encodes these structures
  bit-identically from authored fixtures, with payloads opaque (F-577), and
  `experimental::rep_layout` reads and re-encodes property streams from a
  caller-supplied layout (F-673). Randomized and mutated inputs never panic
  either reader, and every accepted input re-encodes exactly (F-604).
- The replicator hands rep-layout properties to `FRepLayout::ReceiveProperties`
  (an optional bit, a packed handle, an optional checksum word, then a handle
  loop) before its field-header loop (M-426, M-427; F-644). Each field until the
  payload ends is a `SerializeInt` index below `max_index + 1` (`FClassNetCache::GetMaxIndex() + 1`,
  `0x1437c2650`), a packed bit count and that payload (unknown indices skipped).
  `UClass::SetUpRuntimeReplicationData` (`0x141fec960`) populates `ClassReps`
  with `CPF_Net` properties and `NetFields` with own `FUNC_Net` functions
  (`SuperStruct == NULL`) sorted by `FName`, and `FClassNetCacheMgr::GetClassNetCache`
  (`0x141fff0e0`) numbers own `ClassReps` then own `NetFields` from
  `SuperNetCache->FieldsBase + SuperNetCache->Fields.Num()` (`SerializeInt` bounds:
  `UIcarusStatContainer` 5, `UInventory` 11, `BP_UIProjectionComponent_MountStatus_C` 7,
  `BP_IcarusPlayerState_C` 38, `BP_Mount_Base_C` 84, `BP_Mount_Horse_C` 85,
  `BP_Tame_Base_C` 86, `BP_IcarusPlayerControllerSurvival_C` 183; F-708).
  Custom-delta payloads start with a bit (net version 11+) and, for a static
  array, a packed index below its dimension, then the struct's delta serializer.
  Fast-array bodies carry four 4-byte fields (array and base keys, delete and
  change counts capped at 2048 initially), a 4-byte ID per delete, and per
  change a 4-byte ID plus either item commands in order, a set bit and
  rep-layout property stream in struct-delta mode, or a native item serializer.
  Function fields are RPCs: bool parameters are a bare bit, others a presence
  bit plus handle-free item encodings, and arrays a 2-byte count (`<= 0xFFFE`)
  plus elements, consuming the payload exactly (`experimental::rep_rpc`
  re-encodes all, F-644, F-708).
- Rep-layout stream (F-571): packed handles select layout commands and handle
  0 ends a stream; a dynamic array carries a 2-byte element count and a nested
  zero-terminated handle stream; each value goes through its property's net
  serializer, and a 4-byte checksum may follow each handle.
- Layout source and handle walk (F-586, F-609): a class's layout orders the
  super class's list before its own net properties (field order under class flag
  `0x80`, else ascending offset with name tie-break), one entry per static array
  element, then a terminating return command and handle table. Handles strictly
  increase within a stream; an `N`-command array element layout numbers element
  commands consecutively (`(h - 1) / N`), and a nested array is one handle.
- Native `UE4CodeGen_Private` descriptors and cooked Blueprint class extensions
  (F-703, F-705 to F-707): none of the 4,000 native `ConstructUClass` sites sets
  class flag `0x80`, so native classes sort own net properties by
  `(offset, name)`, whereas cooked `UBlueprintGeneratedClass` exports
  (`ClassFlags & 0x80`) order own net properties in `ChildProperties` order
  after super handles:
  - `AActor` (10 net properties -> 15 leaf handles 1..15): handles 1..4 bool
    flags (`bHidden`, `bReplicateMovement`, `bTearOff`, `bCanBeDamaged`),
    handle 5 `RemoteRole` (3-bit `ENetRole`), handle 6 `ReplicatedMovement`
    (`FRepMovement`; `ACharacter` subclasses use `MovementLevel::Hundredths`
    for location), handles 7..12 `AttachmentReplication` (`FRepAttachment`),
    handle 13 `Owner`, handle 14 `Role` (3-bit) and handle 15 `Instigator`.
  - `AController` adds handles 16..17 (`PlayerState`, `Pawn`),
    `APlayerController` adds 18..19 (`TargetViewRotation`, `SpawnLocation`),
    `APlayerState` adds 16..26 (`Score`, `PlayerId`, `Ping`, five bools,
    `StartTime`, `UniqueId`, `PlayerNamePrivate`), and `UActorComponent`
    defines handles 1..2 (`bReplicates`, `bIsActive`). `experimental::rep_layout`
    models 64 native and cooked Blueprint actor/component schemas covering all
    `20,407 / 20,407` `with_rep_layout` blocks (`100%`) in M-707 across player
    controllers/states/characters, mounts (`BP_Mount_Base_C`, `BP_Tame_Base_C`,
    `BP_Mount_Horse_C`), buildings (`BP_Building_Base_C`), world managers,
    world actors, dropships/rocket parts, deployables,
    `UWeatherForecastBarComponent` and 28 inventory/trait/state components
    (F-703, F-705 to F-707).
- Spawn vectors (F-571, F-573): after the presence bit, archives at engine net
  version 13+ read a quantized bit; unquantized is three 4-byte floats,
  quantized a component bit count (`<= 24`) and three integers bounded by
  `1 << (bits + 2)`, decoded as `(v - (1 << (bits + 1))) * 0.1`.
- Property net items and command expansion (F-574, F-575, F-632, F-704 to F-708):
  `FRepLayout::ReceiveCustomProperty` (`0x143b219b0`) calls `Property->NetSerializeItem`
  (`vtable + 0x98`) for every non-array command. Bool is 1 bit; Byte is 8 bits or
  `min(8, CeilLogTwo64(max + 1))` with an enum (`max <= 0` sends 0 bits; native
  `ConstructUEnum` `0x14213b210` and cooked `UserDefinedEnum` assets append
  `_MAX = max_explicit + 1` unless already present); Enum is that width capped at
  its underlying integer's bits (net version 17+); `FNumericProperty::NetSerializeItem`
  (`0x1421206f0`) reads 1, 2, 4 or 8 archive bytes via `SerializeItem`
  (`0x142123c50`..`0x142123d30`); object references are a NetGUID; structs with
  `STRUCT_NetSerializeNative` (`StructFlags & 0x400`) emit one command and call
  `ICppStructOps::NetSerialize` (`+0x78`), whereas structs without `0x400`
  recurse through `0x143af7340` (skipping inner `CPF_RepSkip` fields such as
  `FItemData::CachedStats` and `FFastArraySerializerItem` base fields). Framing
  the 1-bit `bDoChecksum` preamble of `ReceiveProperties` (`0x143b20720`, M-426)
  with `FStatsRepArray` (bound 5, index 3), `FInventorySlotsFastArray` (bound 11,
  index 3), `ForceProjectionUpdate` (bound 7, index 5),
  `Server_UpdateHealthValue` (bound 38, index 34), `AIcarusCharacter` montage
  RPCs (bounds 84..86, indices 51/53), 11 `APlayerController` RPCs (bound 183,
  indices 13, 18, 21, 23, 40, 46, 49, 51, 54, 62, 63) and zero-bit subobject
  headers replays all `20,407 / 20,407` `with_rep_layout` blocks, all
  `3,072 / 3,072` `has_rep_layout == false` blocks and all `7 / 7` channel-2
  open-hook blocks (`23,486 / 23,486` total typed blocks, `2,607` RPC calls,
  `66,859` property entries, `2,361` stat-pair items, 0 failures; F-704 to F-708;
  U-066 resolved).
- Native struct items (F-588, F-589, F-593, F-597, F-673, F-674, F-704 to F-708):
  `FVector` (`0x142100d00`), `FVector2D` (`0x142100bf0`) and `FQuat::NetSerialize`
  (`0x1421003f0` -> `0x141ed3a00`, normalizing `W >= 0` on save and reconstructing
  `W = sqrt(1 - (X^2+Y^2+Z^2))` on load, so `FTransform` flattens into three
  `Vector` commands) serialize three, two and three 4-byte float words;
  `FRotator::NetSerialize` (`0x142100890`) calls `SerializeCompressedShort`;
  `FUniqueNetIdRepl::NetSerialize` (`0x143ddced0` -> `0x1439a23f0`) uses the
  control-channel unique-net-id framing (F-531, F-636); `FRowHandle::NetSerialize`
  (`0x143f8a530`) sends a 2-byte table index (`0xffff` unmapped, or `< 293` in
  `SetupTables` `0x1418193d0`) and a 0/1/2/4-byte row index sized by table row
  count, whereas typed `F*RowHandle` subclasses serialize only their 1-byte or
  2-byte (`FQuestsRowHandle` `0x14165b3d0`) row index (`FIcarusStatReplicated`
  flattens into `[NetworkName, Int]`, F-706, F-707); `FStatPairRepState::NetSerialize`
  (`0x141c10c50`) writes a 1-byte length `1..=4` (`0x141807bd0`) followed by
  `len` big-endian bytes for `Stat` and `Value` (`0x1417e1b50`).
  `FVector_NetQuantize` is a bit count (`<= 20`) and three biased unscaled
  integers; `FVector_NetQuantizeNormal` is three 16-bit values biased by 32768.
  `FRepMovement` sends two flag bits, location, rotation (byte rotator at
  level 0; compressed rotator at level 1), linear velocity, and (when flag bit 1
  is set) angular velocity; vector levels 0, 1 and 2 cap bits below 24, 27 and 30
  with scales 1, 0.1 and 0.01 (`NetQuantize10` and `100` match levels 1 and 2).
  Hit results send an 8-bit flags byte, 4-byte time, quantized/normal vectors,
  gated depth/item, three NetGUID references, bone network name, and gated face
  index and element byte.
- Gameplay tags (F-583, F-591, F-592, F-674): sent as names unless fast
  replication is set; then as a net index (`T` raw bits when `F < 1` or
  `F >= T`, else `F` low bits, a continuation bit and `T - F` high bits when
  set; per-connection table flag sends a packed 16-bit index, slot meaning
  inferred). Containers send an empty bit, a `W`-bit count (`<= 2^W - 1`) and
  that many tags.
- Name, Str, soft-object and Text items (F-595, F-599, F-639, F-675): Name items
  call the archive name operator (`SerializeName` or `StaticSerializeName`, the
  network-name form below). Str items and net soft-object paths are archive
  strings (signed 4-byte count, then narrow bytes or 2-byte units with a
  terminator). Text items use the shared FText serializer at UE4 archive version
  522 (`0x170+` 4-byte flags word, then history byte `0..12` or `0xFF` none;
  Names here use archive slot `0x140`, whose net routing is unread, U-064):
  - `0` Base sends namespace and key as text keys (empty key saved as count 1
    plus terminator), then source string; `0xFF` none (Dev-Editor custom version
    40 >= 32) sends a 4-byte culture-invariant bool and optional string (F-639).
  - `1` NamedFormat nests a format Text and a count-prefixed map of string keys
    to argument values (type byte: `0/1` int64, `2` float, `3` double, `4` Text);
    `2` OrderedFormat and `3` ArgumentDataFormat send a format Text, a 4-byte
    count and argument records (`3` carries name string, type byte and int32,
    float, Text or gender byte; Dev-Editor 40 >= 5) (F-639).
  - `4` AsNumber, `5` AsPercent and `6` AsCurrency (`6` prefixes currency code)
    send a format-argument value, a 4-byte has-options bool, optional number
    options (AlwaysSign bool at Dev-Editor > `0x14`, UseGrouping bool, rounding
    byte, four int32 digit bounds) and target-culture string (F-639).
  - `7` AsDate, `8` AsTime and `9` AsDateTime send an 8-byte date-time, style
    byte(s), time-zone string (from version `0x1a6` on Date, always on Time and
    DateTime) and culture string; `10` Transform sends a nested Text and
    transform byte; `11` StringTableEntry sends a network-name table Name and
    key string; `12` TextGenerator sends a generator Name and, unless `None`, a
    4-byte byte count and payload (`experimental::rep_text` re-encodes all,
    F-639).
- Network names (F-575): a hardcoded bit; when set, the hardcoded name index
  (packed at engine net version 6 or later, otherwise bounded by 411; indices
  above 702 are rejected), which senders use only for indices up to 410; when
  clear, an archive string and a 4-byte number.
- Compressed rotator (F-573): pitch, yaw and roll in that order, each a bit set
  when its 16-bit quantized angle is nonzero and then, if set, that 2-byte
  value; quantization is `angle * 65536 / 360` rounded, decoding multiplies by
  `360 / 65536`.

## Candidate

- Server send order (F-610): the older server writes an actor bunch in the
  order the client reads it: the spawn record (when the initial and
  locally opened flags are set) and an actor hook that is empty in the base
  class (nonempty 8-bit net player index on channel 2's player controller open,
  followed by 7 content blocks that all decode and re-encode bit-identically,
  F-696, F-708), the actor's property block, then subobject blocks.
- The older server's `SerializeNewActor` (M-460) makes the same three object
  references and single rotation bit as the client's; its quantizers are
  inlined and unread.
- The block grammar, spawn order and handle stream match the stock engine
  shape of this generation; the remaining native struct serializers are
  unread (U-064).

## Open questions

- Other native structs not exercised in M-707 (e.g., `FDateTime`/`FTimespan`, `FPlane`) and server writers for these records (U-044, U-064).

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
  M-511, M-522, M-530, M-548; native class/struct/enum descriptors, cooked Blueprint exports, RPC caches and scalar/struct net serializers M-710 to M-713, M-717, M-720, M-731 to M-742;
  layout source M-552 to M-555; send order M-461, M-556 to
  M-560; wire capture M-707, M-708.
