# Actor replication after login

After control admission: [actor channel rules](../../specs/network/actor-channel.md).

## Question cluster

- How does the client spawn an actor from an open actor bunch?
- How are per-object content blocks, exports, partials, and dormancy framed?
- How are replicated properties encoded inside a content block?
- How does the server evaluate relevancy, priority, connection readiness, and the consider/prioritize/process stages?

## Current answer

Actor opens, content blocks and partial reassembly: F-709 to F-711, F-751,
F-845, F-846, F-849; addresses and limits are in the
[spec](../../specs/network/actor-channel.md).
Component bounds correct F-958 (F-1015); matched 3.0.34 controller and component
bounds are F-1016; RPC identities: F-1019. Eight opens settle without Begin
(F-1017). Conditional outer presence/custom order and name codec: F-1020;
caller-qualified parsing/staging: F-1021; private capture: F-1022. Actual payload,
live profile/layout/ops and callbacks remain U-079; no cause proof.

Gameplay-tag indexing: F-702 (U-065 resolved). Dedicated-server sources for
3.0.32 tags, checksums, cooked schemas and row indices: F-828, F-832;
[research gates](../../gates.md).
Native descriptors, cooked Blueprint schemas, `FClassNetCache` bounds and
qualified codecs replay M-707 completely: 66,859 property entries, 2,607 RPC
calls and 2,361 stat pairs (F-762; U-064, U-066 resolved).

Scheduling correspondence: F-745, F-748, F-749, F-751, F-788, F-851;
native scheduling over supplied positions: F-790; authored motion/flags bridge:
F-938.
Export checksums compute natively (F-853; U-067) and exports
repeat until acked, including returning-pawn component restores (F-796, F-964).
Native captured-open reconstruction: F-852, F-794, F-854, F-855; four opens after
Join: F-804. Original TimeOfDay: F-805, F-855, F-856 (U-071 resolved);
native cadence/TimeOfDay: F-807; ServerFPS: F-808, F-809; NetworkingStatus/pings:
F-810, F-811, F-815, F-816; LockedMissions: F-834, F-835.
Fresh-pawn vitals, inventories and stats: F-836, F-839.
Saved manager/quest opens, bitwise capture comparisons and the 15-prospect
census: F-859, F-861, F-862, F-864, F-866, F-867, F-914, F-933 to F-935;
[world managers](../../specs/network/world-managers.md). U-074 and U-075 are
resolved; unsupported quest classes and `Check()` graphs fail closed (U-076).
New-player `History` appends coalesce across temporary per-peer send pressure
(F-865, F-967).

Prospect refresh/membership and initialization: F-936, F-1014, F-1011, F-975;
original membership: F-822, F-945, F-946 (U-070). UID/state matching and profiles:
F-943, F-944, F-947 to F-951; retry/History: F-954, F-964, F-967, F-975.
Original friends-list finalization and returning-pawn capture: F-972 to F-974.
Native restore/withheld levels: F-978 to F-989; modifier/temperature order and
ticks: F-991, F-992, F-994 to F-1000. Automatic scheduling/climate remain open.
Profile/backend, retry/kick, loadout/bestiary: F-955 to F-959;
[world managers](../../specs/network/world-managers.md).

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

M-707, M-813, M-819, M-832, M-838..M-848, M-1083..M-1093, M-1096, M-1099, M-1102, M-1105..M-1108, M-1110, M-1112, M-1126..M-1149, M-1197, M-1198, M-1207 to M-1209, M-1211 to M-1225, M-1227 to M-1230.
