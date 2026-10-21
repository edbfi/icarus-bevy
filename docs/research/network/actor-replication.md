# Actor replication after login

Static and recorded-wire evidence for how the client and server exchange
replicated actor state once the control channel has admitted a connection; the
normative wire and scheduling rules live in
[actor channel bunches](../../specs/network/actor-channel.md).

## Question cluster

- How does the client spawn an actor from an open actor bunch?
- How are per-object content blocks, exports, partials, and dormancy framed?
- How are replicated properties encoded inside a content block?
- How does the server evaluate relevancy, priority, connection readiness, and the consider/prioritize/process stages?

## Current answer

Open actor bunches frame the actor NetGUID and, for dynamic actors, the
archetype, level and optional spawn vectors, then the actor-channel open hook
(F-569, F-610, F-739, F-751). Outgoing bunches carry export-only prefixes and
split into partial slices; incoming ones reassemble and keep dormant actors
(F-709 to F-711); content blocks frame per-object replicator input (F-576,
F-586). Addresses and limits are in the [spec](../../specs/network/actor-channel.md).

Offline schema derivation is authorized ([gates](../../gates.md)). The
gameplay-tag net-index dictionary derives from the client's tag table and
configuration per release (F-702; U-065 resolved). Native descriptors, cooked
Blueprint schemas, `FClassNetCache` bounds and the `NetSerialize` census in
`experimental::rep_layout`, `rep_rpc`, `sequencing` and `actor_channel` replay
and re-encode every export (317), partial sequence (300), dormancy close (136),
actor open (288) and typed content block (23,486: 20,407 with a rep layout,
3,072 without; 66,859 property entries, 2,607 RPC calls, 2,361 stat pairs) of
M-707 with 0 failures (F-571 to F-644, F-703 to F-711, F-739; U-064, U-066
resolved).

Server and client builds also match across relevancy, priority, tick rate,
connection and channel readiness, `UActorChannel::ReplicateActor` and every
`UNetDriver::ServerReplicateActors` stage, whose constants and guards the
[spec](../../specs/network/actor-channel.md) lists (F-745, F-748, F-749,
F-751); the relevancy re-check window, force-relevance frame comparison and
mark step are refined by F-772, F-774 and F-788. A native scheduling stage
drives these evaluators over 2,626 supplied actor positions; with UE default
parameters it takes 0.3 to 5.2 ms per frame for 1 to 8 viewers (F-790). Export checksums are class network checksums computed natively
(F-776), reproducing 77 of 79 captured values (F-777; U-067), and the
original's first post-Join bunch opens the PlayerController channel (F-778),
which the native composer reproduces bit for bit (F-779). The GameState open
names its manager actors by GUIDs assigned on first reference, before their
own opens, and is also reproduced bit for bit (F-794). Paths are exported
again until a packet carrying them is acked (F-796), and the pawn open, with
its components and inventory subobjects, is reproduced bit for bit by the
same serialization-order rule (F-797). Its contents (component bools,
character state, stat and inventory fast arrays) are composed natively from
session values, again bit for bit (F-799); `run` sends the PlayerController,
pawn, GameState and PlayerState opens after Join (F-800).

## Open

- Two Blueprint class checksums with user-struct arrays (U-067).
- Authenticated runtime relevancy and priority exercise (U-038).
- Item, stat and character values of a played character at open (U-069).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707..M-717, M-720, M-728..M-748, M-776, M-777, M-782, M-794, M-795, M-797..M-799, M-801, M-802, M-804, M-805, M-807, M-813..M-819, M-832, M-834 to M-836, M-838.
