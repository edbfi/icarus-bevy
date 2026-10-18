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

Every body of every one of the save's 38 classes frames and round-trips
byte-identically through a lossless value-opaque tree in both copies, under
statically qualified Map, Set and Int64 headers and 1 MiB component and
struct-array bounds (F-556, F-557, F-634); all 163 Maps per copy frame into
items and no Set occurs (F-559, F-562). The census behind this began with
C009, C013, C028 and C034, qualified per boundary in both copies (F-496),
then C001 (F-534, F-540, F-722; U-023 resolved), the player-state candidates
and the remaining classes (F-668); C034 copies are compared in separate batches
because the original 8 MiB component transport cannot hold both (F-213). The
static contracts behind the measured widths are separate rows (F-655, F-369 to
F-373, F-664, F-386, F-388, F-391), and server 25621760 matches client 25793767
on `FString`/`FName` normalization (`ParseNumber` `_<1..10 digits>`),
`FFloatProperty`, `FVector`, `FArrayProperty` 32-bit size diffs, immutable
`CPF_SaveGame` flags, tagged property wrap-around, and C013/C028 migration and
restore hooks, plus all 28 C030 `TextProperty` bodies (`history = -1`),
`ProspectInfo.ExpireTime` (`Int64Property`), and `C015`/`C022`/`C032` Map item
serializers (F-715 to F-720, F-725, F-726; U-026, U-029 to U-031, U-033, U-039,
U-053 to U-057, U-059 to U-062 resolved). Every opaque Struct payload is one of
four fixed-width engine math types of three or four ordered 4-byte components
(F-565, F-602). Construction candidates frame too, grids nesting per-type row
names over placed pieces that share the grid's frame (F-643).

No saved class identifier, field or type name, or recorder version tells the
older server from the newer client, and the prospect carries no version string
(F-635, F-682). Client recorder code gives every
class's fields their meanings (F-645, F-646, F-648, F-650 to F-654, F-656 to
F-661, F-665, F-666, F-676, F-677); item stats, alterations and slots are row references
(F-660). The validate, stitch and restore rules are in the
[restore-rules spec](../../specs/save-format/restore-rules.md) (F-663, F-715 to F-719). The
readers rest on borrowed strict string frames with no success-path allocation
(F-142); comparator limits stay at 128 KiB (F-198).

Counts and maxima are samples from the two supplied copies, not format limits.
No decoded value, text or number was emitted or interpreted.

## Open

- Meanings and Name identity for every qualified value (U-020); every saved
  enum property value and item dynamic-data Type already resolves in both builds'
  identical tables (F-649).
- Unpopulated families: 17 C028 array-child descriptors, DA04, CA06 Text/Name and the empty inherited arrays (U-021, U-046).
- Binary Struct component values (U-020) and packaged row meanings of the classes outside the audited selections (U-022).
- Individual C028 native top spans and the historical library snapshot (U-024).
- Restoration and import, client and newer-build compatibility (U-025); original performance (U-028).
- Heavy-world save work order: the preserved server's offline load/save/reload baseline (F-678 to F-681) and connected scenario S1 on server 25956137 (3.0.32, F-701) both frame and preserve byte-identically; build pairing of the original save remains open (U-040), and native interpreted import stays gated (U-038, U-045). Generic framing agreement is not an importable world; no partial importer.

## Artifacts

M-047..M-077, M-079..M-108, M-118..M-130, M-278..M-280, M-351, M-352, M-376..M-384, M-389, M-390, M-401..M-404, M-407..M-410, M-606..M-613, M-616..M-618, M-626, M-627, M-636..M-638, M-727, M-755..M-760, M-766, M-767.
