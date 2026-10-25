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
(F-751, F-845, F-849). Outgoing bunches carry export-only prefixes and
split into partial slices; incoming ones reassemble and keep dormant actors
(F-709 to F-711); content blocks frame per-object replicator input (F-846,
F-849). Addresses and limits are in the [spec](../../specs/network/actor-channel.md).

Offline schema derivation is authorized ([gates](../../gates.md)). The
gameplay-tag net-index dictionary derives from the client's tag table and
configuration per release (F-702; U-065 resolved); for 3.0.32 it, the class
checksums, cooked schemas and row indices all derive from the dedicated-server
distribution, which is what the shipped server may use (F-828, F-832).
Native descriptors, cooked Blueprint schemas, `FClassNetCache` bounds and
qualified codecs replay M-707 completely (F-762; U-064, U-066 resolved).

Server and client builds match in every scheduling stage, whose constants and
guards the [spec](../../specs/network/actor-channel.md) lists (F-745, F-748,
F-749, F-751, F-788, F-851); a native stage drives them over supplied actor
positions (F-790). Export checksums compute natively (F-853; U-067) and exports
repeat until acked (F-796). Native composers rebuild the captured
PlayerController (F-852), GameState (F-794) and pawn (F-854) opens bit for bit,
including session pawn contents (F-854) and saved GameState prospect/mode
values (F-855); `run` sends all four opens after Join (F-804). TimeOfDay
advances on the original by a per-world hourly rate curve setting
SecondsPerGameDay (F-805, F-855, F-856; U-071 resolved); `run` sends GameState
updates at the captured cadence with constant-rate TimeOfDay (F-807),
`ServerFPS` (F-808, F-809), locally saving `NetworkingStatus` with player pings
(F-810, F-811, F-815, F-816), and `LockedMissions` from `D_FactionMissions`
(F-834, F-835). Fresh-pawn vitals, inventories and stats: F-836, F-839.
Saved manager/quest opens, bitwise capture comparisons and the 15-prospect
census: F-859, F-861, F-862, F-864, F-866, F-867, F-914, F-933 to F-935;
[world managers](../../specs/network/world-managers.md). U-074 and U-075 are
resolved; unsupported quest classes and `Check()` graphs fail closed (U-076).
Joining players are not yet appended to `History` (F-865).

Explicit native prospect refreshes retain current values and future-open
templates, clear defaults and repair loss after open ACK; invalid combined
opens reject atomically and temporary pressure preserves clocks (F-936).
These are authored loopback tests; original trigger timing remains U-070.

## Open

- Two Blueprint class checksums with user-struct arrays (U-067).
- Authenticated runtime relevancy and priority exercise (U-038).
- Item, stat and character values of a played character at open (U-069).
  M-893 retains the unfinished UID-source investigation.
- Saved `C019` quests using the 270 `CPF_Net`-extending classes, 365 impure
  `Check` graphs, or true-valued 44 `AllowChecksAfterCompletion` rules, which
  fail closed in `run` (F-934, F-935; U-076).
- Saved container lists with empty positions (which `run` skips) and live
  `C016::StitchingOperation` nulling of unreferenced containers, whose wire
  encoding is unobserved (F-866, F-867).
- When the active prospect is refreshed after open at runtime; its direct
  callers are spawn completion, dropship leave and backend, setting or
  difficulty updates (F-822; U-070).

## Artifacts

M-420..M-449, M-455..M-470, M-570..M-576, M-581..M-588, M-600..M-605, M-707..M-717, M-720, M-728..M-748, M-776, M-777, M-782, M-794, M-795, M-797..M-799, M-801, M-802, M-804, M-805, M-807, M-813..M-819, M-832, M-834 to M-836, M-838 to M-848, M-852, M-853, M-860, M-863, M-868, M-869, M-879, M-881 to M-887, M-891 to M-896, M-901 to M-913, M-1027 to M-1030, M-1034.
