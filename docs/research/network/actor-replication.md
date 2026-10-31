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
qualified codecs replay M-707 completely: 66,859 property entries, 2,607 RPC
calls and 2,361 stat pairs (F-762; U-064, U-066 resolved).

Server and client builds match in every scheduling stage, whose constants and
guards the [spec](../../specs/network/actor-channel.md) lists (F-745, F-748,
F-749, F-751, F-788, F-851); a native stage drives them over supplied actor
positions (F-790); the authored motion/flags bridge, including hidden-aware
priority and per-field loss repair, is qualified in F-938.
Export checksums compute natively (F-853; U-067) and exports
repeat until acked, including returning-pawn component restores (F-796, F-964).
Native composers rebuild the captured
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
New-player `History` appends coalesce across temporary per-peer send pressure
(F-865, F-967).

Native prospect refresh: F-936. Profile phases retry send pressure (F-975).
Initial opens retain pending stages, Begin and pawn UID ownership (F-1011).
A saved member's Join refreshes who is playing by login ID, as the original's
spawn completion does (F-822, F-945); unassociated joins, which the original adds
from character records, change nothing (F-946). These are authored loopback
tests; original trigger timing remains U-070. Each pawn open claims a distinct
IcarusUID by the original's search order around saved actor UIDs (F-943, F-944,
F-948); a returning player's controller and pawn recorders match by PlayerCharacterID,
and the pawn then retakes its saved UID (F-947, F-949, F-950); the client
sends its character slot and profiles (F-951). `run` follows that sequence on authored
loopback peers: UID/state sends retry temporary queue pressure (F-964),
and new players join History (F-954, F-967, F-975). The original
finalises players only after every controller's friends list (F-973); the probe,
initialising as a saved character, captured its restore of a returning pawn (F-972, F-974); `run` restores inventories, weights, modifiers and stats; incomplete inputs withhold dependent levels (F-978 to F-989);
modifier and temperature call models preserve local order
(F-991, F-994, F-996, F-997). Native time/temperature updates share reliable delivery
(F-995, F-998); temperature tick order and connection-owned temperature/modifier
ticks are qualified (F-999, F-1000). Automatic scheduling and climate remain open. GUID-tagged replay distinguishes countdown,
increases and zero without inferring their causes (F-992). Profile answers use the PlayerState's trees and verified backend component
(F-955); retry/kick and loadout/bestiary gates precede prospect initialization
(F-956 to F-959).

## Open

- Two Blueprint class checksums with user-struct arrays (U-067).
- Authenticated runtime relevancy and priority exercise (U-038).
- Item, stat and character values of a played character at open (U-069).
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

M-707, M-813, M-819, M-832, M-838..M-848, M-1083..M-1093, M-1096, M-1099, M-1102, M-1105..M-1108, M-1110, M-1112, M-1126..M-1149. Earlier observations remain indexed by the cited finding rows.
