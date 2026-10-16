# Retired findings: Save format

Rows retired from [the active ledger](../save-format.md): superseded by a summary row,
execution receipts, or historical session state. Full text is kept and IDs stay
permanent; cite the superseding row where one exists.

Last reconciled: 2026-10-15

| ID | Claim | Kind | Evidence | Scope and limits | Unknowns | Status |
| --- | --- | --- | --- | --- | --- | --- |
| F-206 | Float/Text (af8cf48) passed root integration, source review vs frozen contract, full manual suite and release build; prior outputs and input hashes kept. | observed | review; M-112; af8cf48 | high; two supplied copies; No supplied Float/Text values qualified |  | retired: receipt |
| F-208 | Prior release executable/library were hashed before rebuild; historical private helpers and snapshots still bind the earlier nine-kind reader. | observed | tool-receipt; M-112; af8cf48 | high; release artifacts; Historical helpers are not rebound to the new reader |  | retired: receipt |
| F-259 | Second selected class (C028 case): all 1,294 records frame (29,762 fields, 4,326,044 bytes); all 9,058 arrays empty; detail belongs to C028 topic. | observed | tool-receipt; M-148, M-143, M-149, M-150, M-119; build 25621760 | high; both copies, E-025 to E-036; No populated-element coverage; deeper child encoding/values open | U-048 | superseded-by F-488 |
| F-263 | Save producing build is user-reported as 25793767 (3.0.31.158478-rel-DangerousHorizons); preserved server executable is older 25621760 (3.0.30.158174). | inferred | review; M-118 | low; user report; Not an embedded build marker or matched reference load | U-040 | superseded-by F-277 |
| F-324 | Historical native helper binaries still carry the earlier nine-kind reader and remain preserved | observed | tool-receipt; M-112 | high; helper binaries; Not current library |  | retired: receipt |
| F-498 | All 38 supplied-save class identifiers are registered as U-prefixed UTF-16 literals (8 also bare) in read-only data of both server 25621760 and client 25793767; class identity cannot discriminate the producing build | observed | static-bytes; M-278..M-280 | high; whole-string presence only; names private | U-040 | superseded-by F-635 |
| F-277 | User reports the supplied save's producing release as build 25793767, version 3.0.31.158478-rel-DangerousHorizons; preserved server is build 25621760. | inferred | review | low; user statement; unverified pairing with any reference | U-040 | superseded-by F-685 |
