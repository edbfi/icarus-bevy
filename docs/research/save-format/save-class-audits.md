# Supplied-save class audits

Structural qualification of selected supplied-save classes (C009 with nested
D01, C013, C028, C034) against both copies, and the enabling reader changes;
values stay opaque. Preconditions: [gates](../../gates.md#before-new-save-class-work).

Before restructure: 96ddb0b:docs/research/save-format/2026-10-13-save-class-audits.md

## Question cluster

- Which selected classes have exact, individually qualified header, width and string-frame structure in both supplied copies?
- Which descriptor families lack populated coverage?
- What lies outside the selection, and what remains unknown about meanings, restoration and original behavior?

## Current answer

Every body of the save's 38 classes round-trips byte-identically through a
lossless value-opaque tree in both copies (F-556, F-559, F-900). Census: F-496,
F-901, F-722 (U-023 resolved), F-668.
Server 25621760 matches client 25793767 on audited serializers and load paths
(gates); recorder registrations, defaults, dispatch and overrides:
[restore rules](../../specs/save-format/restore-rules.md) (F-645..F-680,
F-741, F-746, F-747, F-750, F-902..F-904). Selected coverage: F-728, F-737,
F-738. Reference-load recorder changes: F-894; load timings: F-731; opaque
math Structs: F-900, F-602; construction grids: F-643.

All 15 prospects type recorder values losslessly within tree limits (F-766).
Reference plans: F-897, F-898, F-916; candidate retention, not restoration:
F-920.

Strict authored C001 adapters: F-927; ordinary import stays closed.
Reload/lifecycle: F-930; removal and comparator models and their open limits:
F-937, with the server helper F-940
([boundary](../../specs/save-format/restore-rules.md#confirmed)). Row-handle
declarations join FName/FNameEntryId; Number/Value declare direct unsigned32.
Removal joins signature/forward owner, matching counts, ordered
refs, pointer/forward AActor and complete row-handle header (F-1195).
Storage, ABI, comparator/native owner, runtime/import stay open.

Saved identifiers do not discriminate the producing build (F-635, F-896).
Row references: F-660; reader allocation/comparator limits: F-142, F-198.

## Open

- Packaged data-table row meanings remain out of scope by policy (F-660).
- Further recorder adapters and qualified coordinates precede an all-or-nothing official-save writer; authored consumers and reference coverage do not qualify original behavior (U-025, U-038, U-045).
- Reuse the heavy-world baseline (F-894, F-736) and connected S1 (F-701); producing-build pairing remains U-040.

## Artifacts

Earlier artifacts: cited findings; M-1425 to M-1428, M-1431 to M-1458, M-1471 to M-1476.
