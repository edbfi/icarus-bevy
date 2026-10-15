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
packed payload-bit count for the object's replicator (F-576). Inside a block,
packed rep handles drive the layout, with nested streams for dynamic arrays,
and spawn vectors are raw floats or bias-packed integers in tenths, with
16-bit compressed rotators (F-571, F-573). Bool, Byte, Enum, object, native
struct and soft-object items are read; Str, Name, Text and native struct bodies
are not (F-574, U-064). Several native struct serializers are read on the
server: quantized and normal vectors, tag containers and hit results (F-593).
Handles walk the layout in strictly increasing order, numbering array elements
consecutively (F-586), and the movement struct sends flags, level-quantized
location and velocities and a byte or 16-bit rotation, its levels being
property defaults that never cross the wire (F-588, F-589). Gameplay tags go
as names or as split net indices whose widths, like the container count
width, are tag configuration (F-591, F-592). Name items take the
network-name form through the package map, and Str items are archive strings
(F-595), as are soft-object paths on net archives (F-599). Hit results carry a flags byte that elides duplicate or default
fields, quantized vectors, NetGUID references and a bone name (F-597).
Text items take the shared FText serializer at the archive version client
bunches are built with, 522, and every history body but TextGenerator is
read (F-624, F-614); Base histories add their namespace, key and source
(F-624); the codec re-encodes every history but TextGenerator (F-629). These
are
client-build observations, with the older server's content-block and NetGUID
writers mirroring them (F-581) and composing actor bunches in the client's
read order (F-610); nothing is wire-validated.

## Open

- Text Name routing (F-624, F-627) and other native
  structs (U-064); server send policy (U-044).
- Tag replication by net index would need an identical tag table (F-583, U-065).
- Rep handles follow each class's replicated-property list (F-609), so a server
  needs the same lists as the client; their source is a user decision (U-066).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-603.
