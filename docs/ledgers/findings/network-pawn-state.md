# Findings: Network pawn state

Restored stats and temperature transitions. IDs remain in the shared finding space.

Before restructure: 9f684fb:docs/ledgers/findings/network.md

Last reconciled: 2026-11-10

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-1054 | Offline 3.0.32's restore sends 90 pawn stats (16 changed, 32 added); record 2's all follow from item, set-bonus and modifier grants via F-839 formulas, gear attachments, effectiveness-scaled grants, Overburdened and per-item tool-tag counts | observed | runtime-capture, static-bytes, static-decompile, authored-test; M-1110, M-1116..M-1120 | high; 1 run; 3.0.32 | U-069 | active |
| F-1055 | Native stat_restore models record 2's 90 restored stats from F-1054's sources (all equal run init-03's); the restore sends deltas and bounds; incomplete inputs withhold stats and dependent saved levels | measured | authored-test, runtime-capture; M-1110, M-1121, M-1122, M-1140 | high; loopback; IDs, keys, stat-bounded state, Overburdened object differ; no stock client | U-069 | active |
| F-990 | Offline 3.0.32's restore sets a returning pawn's TotalExperience (handle 15) to its character profile's XP, FOnlineProfileCharacter's third property, XP_Debt not subtracted (run init-03: 41); no native setter read copies it. Native run does the same | observed | runtime-capture, static-bytes, authored-test; M-1110, M-1123 | high; 1 run; new players unread | U-069 | active |
| F-997 | 3.0.32 internal-temperature setter snapshots entry, reads GetStat upper then lower, stores its ordered clamp and requests the temperature delegate only if that result differs from entry | observed | static-bytes; M-1144/ev_3d47e345fc06, M-1145 | high; exact 186 B; stat/listener effects and full TemperatureTick unqualified | U-069 | active |
| F-999 | 3.0.32 TemperatureTick samples after a strict interval threshold, consumes one period and adjusts internal temperature with ordered callbacks and asymmetric fractional conversion | observed | static-bytes, authored-test; M-1147/ev_2cd387431009, M-1148, M-1149 | high; full 553 B; climate, defaults, listeners and scheduling unqualified | U-069 | active |
| F-1000 | Explicit native temperature/modifier ticks retain per-connection/UID timing, read latest values and publish final callback state without replay on polling or loss repair; disconnect drops timing | measured | authored-test; M-1149, M-1151 | high; caller inputs; no automatic clock, climate, expiry, destruction, persistence or stock-client proof | U-069 | active |
