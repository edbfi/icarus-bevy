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
Import planning: row names (F-897, F-898), world context (F-916),
synthetic quest coverage (F-1263) and candidate retention (F-920).
F-651 qualifies C016 list positions. F-1266 identifies the historical server stitching slot.

Authored adapters F-927 leave import closed. Lifecycle F-930; server operand/
append chains F-937/F-940. Reuse F-1195/M-1426's row-handle list/base join;
M-1517 qualifies the repeated reader.
F-1260 binds a 24-byte copy into one removal caller’s stack argument.
A later opaque call may alter the destination. Its final contents, owner/receiver, element storage,
getter/class identity, ABI, aliases, lifetimes and client/runtime/import
remain open ([boundary](../../specs/save-format/restore-rules.md#confirmed)).

Saved identifiers do not discriminate the producing build (F-635, F-896).
Row references: F-660; reader allocation/comparator limits: F-142, F-198.

## Open

- Open C016 bindings: [restore rules](../../specs/save-format/restore-rules.md#open-questions).
- Packaged data-table row meanings remain out of scope by policy (F-660).
- Further recorder adapters and qualified coordinates precede an all-or-nothing official-save writer; authored consumers and reference coverage do not qualify original behavior (U-025, U-038, U-045).
- Reuse the heavy-world baseline (F-894, F-736) and connected S1 (F-701); producing-build pairing remains U-040.

## Artifacts

Cited findings; M-1425 to M-1428, M-1431 to M-1458, M-1471 to M-1476; M-1508, M-1517.
