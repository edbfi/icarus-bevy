# Findings: Network pawn state

Restored stats, temperature transitions and placement. IDs remain in the shared finding space.

Before restructure: 9f684fb:docs/ledgers/findings/network.md

Last reconciled: 2026-11-15

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-1054 | Offline 3.0.32 restore sends 90 pawn stats (16 changed, 32 added); record 2's from item, set-bonus, modifier grants by F-839 formulas (23 keep 3.0.31 inputs), gear attachments, effectiveness-scaled grants, Overburdened, per-item tool-tag counts | observed | runtime-capture, static-bytes, static-decompile, authored-test; M-1110, M-1116..M-1120 | high; 1 run; 3.0.32 | U-069 | active |
| F-1055 | Native restores F-1054's 90 stats; deltas use replicated items, vital bounds use full authoritative maxima. Incomplete sources withhold stats and dependent saved levels; independent fields remain | measured | authored-test, runtime-capture; M-1110, M-1121, M-1122, M-1140, M-1388 | high; loopback; IDs, keys, bounded state, Overburdened differ; no stock client | U-069 | active |
| F-990 | Offline 3.0.32's restore sets a returning pawn's TotalExperience (handle 15) to its character profile's XP, FOnlineProfileCharacter's third property, XP_Debt not subtracted (run init-03: 41); no native setter read copies it. Native run does the same | observed | runtime-capture, static-bytes, authored-test; M-1110, M-1123 | high; 1 run; new players unread | U-069 | active |
| F-997 | 3.0.32 internal-temperature setter snapshots entry, reads GetStat upper then lower, stores its ordered clamp and requests the temperature delegate only if that result differs from entry | observed | static-bytes; M-1144/ev_3d47e345fc06, M-1145 | high; exact 186 B; stat/listener effects and full TemperatureTick unqualified | U-069 | active |
| F-999 | 3.0.32 TemperatureTick samples after a strict interval threshold, consumes one period and adjusts internal temperature with ordered callbacks and asymmetric fractional conversion | observed | static-bytes, authored-test; M-1147/ev_2cd387431009, M-1148, M-1149 | high; full 553 B; climate, defaults, listeners and scheduling unqualified | U-069 | active |
| F-1000 | Explicit native temperature/modifier ticks retain per-connection/UID timing, read latest values and publish final callback state without replay on polling or loss repair; disconnect drops timing | measured | authored-test; M-1149, M-1151 | high; caller inputs; no automatic clock, climate, expiry, destruction, persistence or stock-client proof | U-069 | active |
| F-1198 | One new-member player-created location is associated with its historical full-map load; that URL matches the frozen current-pin protected configuration | derived | save/log/config; M-1161, M-1422, M-1423, M-1504 | one candidate; no general name inversion, terrain/collision/clearance or current-client placement proof; F-1134 base absent | U-038, U-025 | active |
