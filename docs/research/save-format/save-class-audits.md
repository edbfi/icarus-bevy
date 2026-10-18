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
`CPF_SaveGame` flags, tagged property wrap-around, C013/C028 migration and
restore hooks, all 28 C030 `TextProperty` bodies (`history = -1`),
`ProspectInfo.ExpireTime` (`Int64Property`), `C015`/`C022`/`C032` Map item
serializers, all 65 `RecorderComponent` `FClassParams` (including all 38
`StateRecorderBlobs` classes' `+0xa0`/`+0x488`/`+0x4a0`/`+0x4b0` overrides), 6 of
7 populated C001 inherited arrays, `CA06`/`DA04` schemas, all 29,762 C028 top
spans, historical pin `b40c97e3...`, `C035`/`C002` clocks, `C033` dynamic tree
drop, `D01` inventory/variable restoration (`14` `EDynamicItemProperties`),
entry-path failure matrices, `WriteToProspectBlob`/`ReadFromProspectBlob`
`DataLength` (`+0x24`) and `SL-004` decode contracts, `FAccoladeSaveData`
(`0x145dc1790`), `FProspectInfo` (`0x145fae2d0`, 17 properties including
`CustomSettings`), and `15 / 15` `inspect-save --stage-check` qualification
across all 12 supplied and 3 re-saved prospects (F-715 to F-720, F-725 to F-728,
F-731 to F-738; U-020 to U-024, U-026, U-028 to U-033, U-039, U-046, U-048,
U-050, U-053 to U-063 resolved). Every opaque Struct payload is one of four
fixed-width engine math types of three or four ordered 4-byte components (F-565,
F-602). Construction candidates frame too, grids nesting per-type row names over
placed pieces that share the grid's frame (F-643).

No saved class identifier, field or type name, or recorder version tells the
older server from the newer client, and the prospect carries no version string
(F-635, F-682). Recorder code gives every class's fields their meanings (F-645 to
F-666, F-676, F-677, F-734 to F-736); item stats, alterations and slots are row
references (F-660). The validate, stitch and restore rules are in the
[restore-rules spec](../../specs/save-format/restore-rules.md). The readers rest
on borrowed strict string frames with no success-path allocation (F-142);
comparator limits stay at 128 KiB (F-198).

Counts and maxima are samples from the two supplied copies, not format limits.
No decoded value, text or number was emitted or interpreted.

## Open

- Packaged data-table row meanings remain out of scope by policy (F-660).
- Native interpreted import and connected client compatibility (U-025, U-038, U-045).
- Heavy-world save work order: the preserved server's offline load/save/reload baseline (F-678 to F-681, F-736) and connected scenario S1 on server 25956137 (3.0.32, F-701) both frame and preserve byte-identically; build pairing of the original save remains open (U-040), and native interpreted import stays gated (U-038, U-045). Generic framing agreement is not an importable world; no partial importer.

## Artifacts

M-047..M-077, M-079..M-108, M-118..M-130, M-278..M-280, M-351, M-352, M-376..M-384, M-389, M-390, M-401..M-404, M-407..M-410, M-606..M-613, M-616..M-618, M-626, M-627, M-636..M-638, M-727, M-755..M-760, M-766..M-769, M-772..M-777.
