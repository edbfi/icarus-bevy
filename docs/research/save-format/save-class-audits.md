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
descriptors exactly, leaving 33 classes and 2,041 records (F-534), and its
nested Struct and Array children frame in both copies (F-540).
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

Every one of the save's 38 class identifiers and 332 framed field and type
names occurs in both the older server and the newer client, and both assign
the five saved recorder versions the same literals, so no saved identifier
confirms the reported producing build (F-635). Eight player-state candidates
frame to depth 4 in both copies apart from two size ceilings (F-550), as do 23
of the 25 remaining classes (F-551). With statically qualified Map, Set and
Int64 headers (F-556) and a 1 MiB component ceiling, every body of every class
frames and round-trips byte-identically through a lossless value-opaque tree
in both copies (F-557); all 163 Maps per copy also frame into items under the
client item layout, and no Set occurs (F-559, F-562). Every Struct payload
that stays opaque is one of four fixed-width engine math types (F-565), each
three or four ordered 4-byte components with no count or tail (F-602).
Construction candidates frame too, building grids nesting per-type row names
over placed pieces whose transforms share the grid's frame (F-643). Client recorder code
gives the deposit, voxel, deployable and FLOD fields their meanings (F-645, F-646).
With struct arrays bounded at the 1 MiB component
ceiling, a 646,614-byte grid array frames and both copies still round-trip
(F-634).

Counts and maxima are samples from the two supplied copies, not format limits.
No decoded value, text or number was emitted or interpreted.

## Open

- Meanings and Name identity for every qualified value (U-020); every saved
  enum property value and item dynamic-data Type already resolves in both builds'
  identical tables (F-649).
- Unpopulated families: 17 C028 array-child descriptors, DA04, CA06 Text/Name and the empty inherited arrays (U-021, U-046).
- Map item meanings and binary Struct component values (U-055, U-020); meanings of the classes outside the audited selections (U-022); C001's nested values and runtime dispatch (U-023).
- Individual C028 native top spans and the historical library snapshot (U-024).
- Restoration and import, client and newer-build compatibility (U-025); original normalization, EOF and error equivalence (U-026); original performance (U-028).
- Heavy-world save work order after this structural work: a matched official-server load/save/reload baseline on disposable copies once build pairing and a runtime scope exist (U-040, U-047); then authenticated negotiation, replication and the gameplay slice, validating a small supported import before the heavy dataset (U-038, U-045). Generic framing agreement is not an importable world; no partial importer.

## Artifacts

M-047..M-077, M-079..M-108, M-118..M-130, M-278..M-280, M-351, M-352, M-376..M-384, M-389, M-390, M-401..M-404, M-407..M-410, M-606..M-613, M-616..M-618, M-626, M-627, M-636..M-638.
