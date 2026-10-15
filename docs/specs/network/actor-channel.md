# Actor channel bunches

## Status

Experimental: static observations of client build 25793767, with server
build 25621760 writers for content blocks and NetGUIDs; not wire-validated. Last reconciled: 2026-10-15.

## Scope

How the client reads actor-channel bunches after the control channel: the
spawn record of a new actor and the per-object content blocks that carry
replicated state. Not covered: the property stream inside a content block,
remote procedure calls, NetGUID export bunches, quantization helpers,
dormancy, channel close reasons and every server-side writer.

## Confirmed

- NetGUID references (F-572): a packed GUID value, 0 meaning none and
  recursion capped at 16 levels; GUID 1 or an export-enabled archive adds an
  export-flags byte: bit 0 a recursive outer GUID and a path string, bit 2 a
  4-byte network checksum, bit 1 no-load. Nonzero even GUIDs are dynamic.
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
  caller-supplied layout (F-585).
- The replicator hands rep-layout properties to `FRepLayout::ReceiveProperties`
  (an optional bit, a packed handle, an optional checksum word, then a handle
  loop) before its field-header loop (M-426, M-427).
- Rep-layout stream (F-571): packed handles select layout commands and handle
  0 ends a stream; a dynamic array carries a 2-byte element count and a nested
  zero-terminated handle stream; each value goes through its property's net
  serializer, and a 4-byte checksum may follow each handle.
- Spawn vectors (F-571): after the presence bit, archives at engine net
  version 13 or later read a quantized bit; an unquantized vector is three
  4-byte floats, a quantized one a component bit count (at most 24) and three
  integers bounded by `1 << (bits + 2)`, each decoded as the value minus
  `1 << (bits + 1)` times 0.1 (F-573).
- Property net items (F-574): Bool is one bit; Byte is 8 bits, or with an
  enum a width derived from its maximum value, at most 8 bits (formula split at
  engine net version 16); Enum a width derived from its maximum (split at
  version 12); object references a NetGUID; structs with the native
  net-serialize flag their own serializer, other structs and arrays are
  flattened by the layout; soft objects the archive soft-path operator. Other
  types (Str, Name, Text) use the base item path, which forwards to the
  property's ordinary item serializer (F-575).
- Native struct items on the older server (F-582): `FVector_NetQuantize` is a
  component bit count (at most 20) and three integers with the same bias and
  bound as spawn vectors but no scale; `FVector_NetQuantizeNormal` three
  16-bit values biased by 32768 and multiplied by an unread constant; gameplay
  tag containers an empty bit, a count byte and packed tags; hit results a
  flags byte followed by those vectors and flag-gated fields. The movement
  struct and the 10/100 vector quantizers are not public symbols.
- Gameplay tags (F-583): sent as network names unless the tag manager's
  fast-replication flag is set; then as a net index into the runtime tag
  table, which both sides must share in the same order.
- Network names (F-575): a hardcoded bit; when set, the hardcoded name index
  (packed at engine net version 6 or later, otherwise bounded by 411; indices
  above 702 are rejected), which senders use only for indices up to 410; when
  clear, an archive string and a 4-byte number.
- Compressed rotator (F-573): pitch, yaw and roll in that order, each a bit set
  when its 16-bit quantized angle is nonzero and then, if set, that 2-byte
  value; quantization is `angle * 65536 / 360` rounded, decoding multiplies by
  `360 / 65536`.

## Candidate

- The older server's `SerializeNewActor` (M-460) makes the same three object
  references and single rotation bit as the client's; its quantizers are
  inlined and unread.
- The block grammar, spawn order and handle stream match the stock engine
  shape of this generation; whether Name items reach the network-name path,
  the exact enum-width formulas and native struct serializers are unread
  (U-064).

## Open questions

- The rep-handle loop, property encodings and quantization helpers (U-064).
- Server writers for these records and their byte order and bounds (U-044).
- Gameplay-tag replication mode and an independent tag table (U-065).

## Evidence

- Symbols M-420 (failed-closed attempts M-421, M-422); pseudocode M-423 to
  M-431, M-433; constants M-432, M-435 to M-437 (scope deviation M-434); net items M-438 to
  M-449; server writers M-455 to M-462;
  server struct items M-463 to M-470.
