# Retired findings: Network presence

Rows superseded by consolidated active findings; IDs and full claims remain.

Last reconciled: 2026-11-08

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-1012 | Readiness retains open/startup and initialization transport receipts, queue/channel counts, player phases and blockers; probe preserves progress/final counts | measured | authored-test; M-1181, M-1186, M-1199, M-1202, M-1203, M-1205 | high; opens need reliable-empty; initialization ACKs use batch spans; no client execution/world-entry proof | U-079, U-038 | superseded-by F-1040 |
| F-1037 | Frozen matched capture sent no configured controller startup actions; existing RestartPawn sends field40 after pawn/controller ACK | measured | source/capture; M-1293, M-1302 | high; omitted restart is a candidate; ACK is not execution or world entry | U-038, U-079 | superseded-by F-1038 |
| F-1018 | Visibility internal handling changes name sets, checks packages and can close; validation checks name bounds and long-package validity | observed | static-decompile; M-1208/ev_9e15bbc246e0, M-1208/ev_d7d3d915cd91 | bounded; public server 26313077; callbacks, name codec and complete contract unresolved (M-1209) | U-079 | superseded-by F-1020 |
| F-1040 | Diagnostics retain open/startup/initialization transport, player phases and parsed/rejected/deferred/admitted profile boundaries; unknown blocks retain no identity | measured | authored-test; M-1181, M-1186, M-1199, M-1202, M-1203, M-1205, M-1308 | high; ACK is not execution; unknown blocks are not identified profile traffic; no world-entry proof | U-038, U-079 | superseded-by F-1045 |
