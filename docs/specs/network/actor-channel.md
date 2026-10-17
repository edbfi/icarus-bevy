# Actor channel bunches

## Status

Experimental: static observations of client build 25793767, with server
build 25621760 writers for content blocks and NetGUIDs; wire-checked against
one capture of the 3.0.31 server (F-693, F-704, F-705). Last reconciled: 2026-10-16.

## Scope

How the client reads actor-channel bunches after the control channel: the
spawn record of a new actor and the per-object content blocks that carry
replicated state. Not covered: the property stream inside a content block,
remote procedure calls, quantization helpers,
dormancy, channel close reasons and every server-side writer.

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
  payload ends is a `SerializeInt` index below `max_index + 1`, a packed bit
  count and that payload (unknown indices skipped). Custom-delta property
  payloads start with a bit (net version 11+) and, for a static array, a packed
  index below its dimension, then the struct's delta serializer. Fast-array
  bodies carry four 4-byte fields (array and base keys, delete and change counts
  capped at 2048 initially), a 4-byte ID per delete, and per change a 4-byte ID
  plus either item commands in order, a set bit and rep-layout property stream
  in struct-delta mode, or a native item serializer. Function fields are RPCs
  (requiring the net flag and, on a server, the server flag): bool parameters
  are a bare bit, others a presence bit plus handle-free item encodings, and
  arrays a 2-byte count (`<= 0xFFFE`) plus elements, consuming the payload
  exactly (`experimental::rep_rpc` re-encodes all, F-644).
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
- Native `UE4CodeGen_Private` descriptors (F-703): across all 4,000
  `ConstructUClass` call sites in the client executable, none of the inspected
  engine or Icarus classes sets class flag `0x80`, so each sorts its own net
  properties by `(offset, name)`:
  - `AActor` (10 net properties -> 15 leaf handles 1..15): handles 1..4 bool
    flags (`bHidden`, `bReplicateMovement`, `bTearOff` at `+0x58`;
    `bCanBeDamaged` at `+0x5a`), handle 5 `RemoteRole` (`ENetRole` at `+0x5f`,
    `ROLE_MAX = 4` -> 3-bit enum byte), handle 6 `ReplicatedMovement`
    (`FRepMovement` at `+0x60`, native `NetSerialize`), handles 7..12
    `AttachmentReplication` (`FRepAttachment` at `+0xa0`, flattened into
    `AttachParent` Object, `LocationOffset` and `RelativeScale3D`
    `VectorQuantize100`, `RotationOffset` `Rotator`, `AttachSocket` Name and
    `AttachComponent` Object), handle 13 `Owner` (`+0xe0`), handle 14 `Role`
    (`+0xf0`, 3-bit enum byte) and handle 15 `Instigator` (`+0x118`).
  - `AController` adds handle 16 `PlayerState` (`+0x228`) and handle 17 `Pawn`
    (`+0x250`); `APlayerController` adds `TargetViewRotation` (`FRotator`,
    `+0x2cc`) and `SpawnLocation` (`FVector`, `+0x558`); `APlayerState`
    (`AInfo` adds none) adds handles 16..26 (`Score` Float `+0x220`, `PlayerId`
    Int `+0x224`, `Ping` Byte `+0x228`, five bitfield bools at `+0x22a` sorted
    by name, `StartTime` Int `+0x22c`, `UniqueId` `FUniqueNetIdRepl` `+0x250`
    and `PlayerNamePrivate` Str `+0x300`). `UActorComponent` defines handle 1
    `bReplicates` (`+0x88`) and handle 2 `bIsActive` (`+0x8a`), and
    `experimental::rep_layout` models 17 native Icarus component and actor-state
    layouts (`UTraitComponent`, `UTraitBehaviour`, `UIcarusStatContainer`,
    `UDurableComponent`, `UItemableComponent`, `UFocusableComponent`,
    `UActionableComponent`, `UHighlightableComponent`, `UInteractableComponent`,
    `UInventoryComponent`, `UInventory`, `UProcessingComponent`,
    `UModifierStateComponent`, `UFLODActorComponent`, `UActorState`,
    `UPlayerTrackerComponent`, `UIcarusMapIconComponent`) (F-703, F-705).
- Spawn vectors (F-571, F-573): after the presence bit, archives at engine net
  version 13+ read a quantized bit; unquantized is three 4-byte floats,
  quantized a component bit count (`<= 24`) and three integers bounded by
  `1 << (bits + 2)`, decoded as `(v - (1 << (bits + 1))) * 0.1`.
- Property net items and command expansion (F-574, F-575, F-632, F-704, F-705):
  `FRepLayout::ReceiveCustomProperty` (`0x143b219b0`) calls `Property->NetSerializeItem`
  (`vtable + 0x98`) for every non-array command. Bool is 1 bit; Byte is 8 bits or
  `min(8, CeilLogTwo64(max + 1))` with an enum (`max <= 0` sends 0 bits); Enum
  is that width capped at its underlying integer's bits (at net version 17+);
  `FNumericProperty::NetSerializeItem` (`0x1421206f0`) calls `SerializeItem`
  (`0x142123c50`, `0x142123c90`, `0x142123ce0`, `0x142123d30`) to read 1, 2, 4
  or 8 archive bytes for `Int8`, `Int16`/`UInt16`, `Int`/`UInt32`/`Float` and
  `Int64`/`UInt64`/`Double`; object references are a NetGUID; structs with
  `STRUCT_NetSerializeNative` (`StructFlags & 0x400`, set from `ICppStructOps`
  slot `+0x68` in `PrepareCppStructOps` `0x141fe53e0`) emit one leaf command and
  call `ICppStructOps::NetSerialize` (`+0x78`), whereas structs without `0x400`
  recurse through `0x143af7340`; soft objects, Str, Name and Text use their
  archive/item operators below. `read_rep_properties` and `write_rep_properties`
  frame the 1-bit `bDoChecksum` preamble of `FRepLayout::ReceiveProperties`
  (`0x143b20720`, M-426) before the handle stream and, together with
  `FStatsRepArray::NetDeltaSerialize` (`0x14178f430`, `FClassNetCache` max index
  4, field index 2, `FastArrayItemLayout::Delta`), decode and re-encode all
  1,031 typed actor and subobject blocks in M-707 bit-identically (F-704, F-705).
- Native struct items (F-588, F-589, F-593, F-597, F-673, F-674, F-704, F-705):
  `FVector::NetSerialize` (`0x142100d00`) and `FVector2D::NetSerialize`
  (`0x142100bf0`) serialize three and two 4-byte float words; `FRotator::NetSerialize`
  (`0x142100890`) calls `SerializeCompressedShort` (`0x141ed7810`, the compressed
  rotator below); `FUniqueNetIdRepl::NetSerialize` (`0x143ddced0` -> `0x1439a23f0`)
  uses the control-channel unique-net-id framing (F-531, F-636); `FRowHandle::NetSerialize`
  (`0x143f8a530`) sends a 2-byte table index (`0xffff` unmapped, or `< 293` in the
  `UDatatableFunctionLibrary::SetupTables` table list at `0x1418193d0`, invariant
  across 3.0.31 and 3.0.32) and a 0/1/2/4-byte row index sized by that table's row
  count (`0` -> 0B, `<= 0xff` -> 1B, `<= 0xffff` -> 2B, else 4B); `FStatPairRepState::NetSerialize`
  (`0x141c10c50`) calls `Detail::WriteInteger` / `Detail::ReadInteger` (`0x141807bd0` /
  `0x1417e1b50`) for both `Stat` (`FIcarusStatEnum`) and `Value` (`i32`), writing a
  1-byte length `1..=4` (`< 0xff` -> 1, `< 0xffff` -> 2, `< 0xffffff` -> 3, else 4)
  followed by `len` big-endian bytes. `FVector_NetQuantize` is a bit count (`<= 20`) and three biased unscaled
  integers; `FVector_NetQuantizeNormal` is three 16-bit values biased by 32768
  and multiplied by an unread constant. `FRepMovement` and the `10`/`100`
  vector quantizers live in `TCppStructOps` with no class-method symbols;
  `FRepMovement` sends two flag bits, location, rotation (byte rotator at level
  0: per-axis nonzero bit plus `round(angle * 256 / 360)` byte; compressed
  rotator at level 1), linear velocity, and (when flag bit 1 is set) angular
  velocity; vector levels 0, 1 and 2 cap bits below 24, 27 and 30 with scales
  1, 0.1 and 0.01 (levels other than 1 and 2 behave as 0; `NetQuantize10` and
  `100` match levels 1 and 2). Hit results send an 8-bit flags byte (bits 2..6
  replace impact point/normal, item, face index and penetration depth), 4-byte
  time, location, normal, unreplaced impact fields, trace start/end, gated
  depth/item, three NetGUID references, bone network name, and gated face index
  and element byte.
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
  F-696), the actor's property block, then subobject blocks.
- The older server's `SerializeNewActor` (M-460) makes the same three object
  references and single rotation bit as the client's; its quantizers are
  inlined and unread.
- The block grammar, spawn order and handle stream match the stock engine
  shape of this generation; the remaining native struct serializers are
  unread (U-064).

## Open questions

- Other native structs and scalar items (U-064); which levels each movement property uses (a property default).
- Server writers for these records and their byte order and bounds (U-044).
- Cooked Blueprint class property extensions and typed capture replay (U-066).

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
  M-511, M-522, M-530, M-548; native class/struct/enum descriptors and scalar/struct net serializers M-710 to M-713, M-717, M-720, M-731 to M-736;
  layout source M-552 to M-555; send order M-461, M-556 to
  M-560; wire capture M-707, M-708.
