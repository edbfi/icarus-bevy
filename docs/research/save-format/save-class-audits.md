# Supplied-save class audits

Structural qualification of selected supplied-save classes (C009 with its D01
nested arrays, C013, C028 and C034) against both supplied copies, and the
reader changes that enabled it. Values stay opaque throughout. Preconditions for
new class work are in [gates](../../gates.md#before-new-save-class-work).

Before restructure: 96ddb0b:docs/research/save-format/2026-10-13-save-class-audits.md

## Question cluster

- Which selected classes have exact, individually qualified header, width and string-frame structure in both supplied copies?
- Which descriptor families lack populated coverage?
- What lies outside the selection, and what remains unknown about meanings, restoration and original behavior?

## Current answer

The census is closed for its selection: C009, C013, C028 and C034 cover 3,440
of 5,485 records, and every observed populated tagged boundary in them is
qualified in both copies; 34 classes with 2,045 records lay outside (F-496). C001
now joins them at the top level: its four bodies match its 20 save-selected
descriptors exactly, leaving 33 classes and 2,041 records (F-534).
The qualified populations are C028 bodies, top frames and direct Structs (F-488, F-489); C034 top, own and
inherited populations (F-490), compared in per-copy batches because the original
8 MiB component transport cannot hold both copies (F-213); C009 top, own and
parent structure (F-491) and its common children with ND01 and D01 nested
arrays (F-492); and C013's two singleton bodies (F-493). The static contracts
behind the measured widths are separate rows, for example F-308, F-367, F-369
to F-373, F-375, F-380, F-386, F-388 and F-391.

Two library changes enabled the work: borrowed strict string frames with no
success-path allocation (F-142) and explicit struct-array and comparator limits
up to a 128 KiB research ceiling while the original API keeps 64 KiB (F-178,
F-198).

Every one of the save's 38 class identifiers is registered in both the older
server and the newer client, so class identity cannot confirm the reported
producing build (F-498).

Counts and maxima are samples from the two supplied copies, not format limits.
No decoded value, text or number was emitted or interpreted.

## Open

- Meanings, Name identity and Enum lookup for every qualified value (U-020).
- Unpopulated families: 17 C028 array-child descriptors, DA04, CA06 Text/Name and the empty inherited arrays (U-021, U-046).
- The 33 classes and 2,041 records still outside (U-022); C001's nested Struct and Array values and runtime dispatch (U-023).
- Individual C028 native top spans and the historical library snapshot (U-024); opaque C028 binary Struct bodies (U-027).
- Restoration and import, client and newer-build compatibility (U-025); original normalization, EOF and error equivalence (U-026); original performance (U-028).
- Heavy-world save work order after this structural work: a matched official-server load/save/reload baseline on disposable copies once build pairing and a runtime scope exist (U-040, U-047); then authenticated negotiation, replication and the gameplay slice, validating a small supported import before the heavy dataset (U-038, U-045). Generic framing agreement is not an importable world; no partial importer.

## Artifacts

M-047..M-077, M-079..M-108, M-118..M-130, M-278..M-280, M-351, M-352.
