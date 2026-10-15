# Actor channel bunches

## Status

Experimental: static observations of client build 25793767, not wire-validated
and not compared with the server. Last reconciled: 2026-10-15.

## Scope

How the client reads actor-channel bunches after the control channel: the
spawn record of a new actor and the per-object content blocks that carry
replicated state. Not covered: the property stream inside a content block,
remote procedure calls, NetGUID export bunches, quantization helpers,
dormancy, channel close reasons and every server-side writer.

## Confirmed

- Open bunch (F-569): `SerializeNewActor` reads the actor NetGUID through the
  package map. For a dynamic GUID (nonzero, even) it then reads the archetype
  NetGUID, a level reference gated on archive flags, and four optional blocks
  in this order, each a presence bit followed by its data: location (quantized
  vector), rotation (compressed rotator), scale (quantized vector) and
  velocity (quantized vector). Static GUIDs carry none of these.
- Content blocks (F-570): after the open, until the bunch ends, each block is a
  rep-layout bit and an is-actor bit; a non-actor block adds the sub-object
  NetGUID and a stably-named bit, and a dynamic sub-object a class NetGUID
  whose zero value deletes it. A packed count of payload bits follows, and
  exactly those bits go to the object's replicator.
- The replicator hands rep-layout properties to `FRepLayout::ReceiveProperties`
  (an optional bit, a packed handle, an optional checksum word, then a handle
  loop) before its field-header loop (M-426, M-427).

## Candidate

- The block grammar and the spawn order match the stock engine shape of this
  generation; quantization precision and the handle loop are unread (U-064).

## Open questions

- The rep-handle loop, property encodings and quantization helpers (U-064).
- Server writers for these records and their byte order and bounds (U-044).

## Evidence

- Symbols M-420 (failed-closed attempts M-421, M-422); pseudocode M-423 to
  M-427.
