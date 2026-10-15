# Actor replication after login

Static evidence for how the client receives actors and their replicated state
once the control channel has admitted it; the spec is
[actor channel bunches](../../specs/network/actor-channel.md).

## Question cluster

- How does the client spawn an actor from an open actor bunch?
- How are per-object content blocks and their payloads framed?
- How are replicated properties encoded inside a content block?

## Current answer

An open actor bunch carries the actor NetGUID and, for dynamic actors, the
archetype, a gated level and optional location, rotation, scale and velocity
blocks (F-569). Later content blocks name the actor or a sub-object and carry a
packed payload-bit count for the object's replicator (F-570). Inside a block,
packed rep handles drive the layout, with nested streams for dynamic arrays,
and spawn vectors are raw floats or bias-packed integers in tenths, with
16-bit compressed rotators (F-571, F-573). Bool, Byte, Enum, object, native
struct and soft-object items are read; Str, Name, Text and native struct bodies
are not (F-574, U-064). These are
client-build observations only; no server writer has been read and nothing is
wire-validated.

## Open

- Property stream, rep handles and quantization (U-064); server writers (U-044).

## Artifacts

M-420..M-445.
