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
configuration per release (F-702; U-065 resolved); for 3.0.32 it, the class
checksums, cooked schemas and row indices all derive from the dedicated-server
distribution, which is what the shipped server may use (F-828, F-832).
Native descriptors, cooked Blueprint schemas, `FClassNetCache` bounds and the
`NetSerialize` census in
`experimental::rep_layout`, `rep_rpc`, `sequencing` and `actor_channel` replay
and re-encode every export (317), partial sequence (300), dormancy close (136),
actor open (288) and typed content block (23,486: 20,407 with a rep layout,
3,072 without; 66,859 property entries, 2,607 RPC calls, 2,361 stat pairs) of
M-707 with 0 failures (F-571 to F-644, F-703 to F-711, F-739; U-064, U-066
resolved).

Server and client builds match in every scheduling stage, whose constants
and guards the [spec](../../specs/network/actor-channel.md) lists (F-745,
F-748, F-749, F-751, refined by F-772, F-774, F-788); a native stage drives
them over supplied actor positions (F-790). Export checksums are computed
natively (F-776, F-777; U-067) and exports repeat until acked (F-796). The
native composers rebuild the captured PlayerController (F-778, F-779),
GameState (F-794) and pawn (F-797) opens bit for bit, including the pawn's
contents from session values (F-799) and the GameState's active prospect
and game mode values from the save the capture's server loaded (F-801,
F-803); `run` sends all four opens after Join, with a preserved world's
saved state (F-804). On the capture's server build, as on the client,
TimeOfDay advances by a per-world rate curve of the hour, and prospect setup
writes SecondsPerGameDay from that curve; the open carried the saved
SecondsPerGameDay (F-803, F-805, F-806; U-071 resolved). `run` follows the
open with GameState updates in the captured form and cadence, its time of
day advancing at the saved day length's constant rate (F-807), and its
own frame rate as `ServerFPS` by the original's rule (F-808, F-809), and a
locally saving server's `NetworkingStatus` listing its players with pings
measured by the original's rule (F-810, F-811, F-815, F-816). The
original's `LockedMissions` come from the save's mission history and the
server's `D_FactionMissions` by a fixed rule, which `run` follows (F-834,
F-835). A fresh player's pawn vitals and empty inventories are data.pak rows,
which `run` sends (F-836); after each PlayerState it opens the GameState's
radiation, world-talent and disaster managers as the capture did (F-859); its stats are mostly starting grants, with 8
virtual stats whose formulas the server declares in code; `run` sends them
from data.pak by the same formulas (F-839).

## Open

- Two Blueprint class checksums with user-struct arrays (U-067).
- Authenticated runtime relevancy and priority exercise (U-038).
- Item, stat and character values of a played character at open (U-069).
- When the active prospect is refreshed after open at runtime; its direct
  callers are spawn completion, dropship leave and backend, setting or
  difficulty updates (F-822; U-070).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707..M-717, M-720, M-728..M-748, M-776, M-777, M-782, M-794, M-795, M-797..M-799, M-801, M-802, M-804, M-805, M-807, M-813..M-819, M-832, M-834 to M-836, M-838 to M-848, M-852, M-853, M-860, M-863, M-868, M-869.
