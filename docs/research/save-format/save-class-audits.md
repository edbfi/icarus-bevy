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
items and no Set occurs (F-559, F-562). The census began with C009, C013, C028
and C034 (F-496), then C001 (F-534, F-540, F-722; U-023 resolved), the
player-state candidates and the remaining classes (F-668; F-213). Server
25621760 matches client 25793767 across `FString`/`FName`, `FFloatProperty`,
`FVector`, `FArrayProperty`, tagged wrap-around, C013/C028 migration/restore,
C030 `TextProperty`, `ProspectInfo.ExpireTime` (`Int64Property`),
`C015`/`C022`/`C032` Map item serializers, all 65 `RecorderComponent`
`FClassParams`, ctor defaults, vtable/restore dispatch (`C001..C038` and
`N001..N027`), `UIcarusGameStateRecorder` (`ReadFromProspectSaveState`,
`RecordToProspectSaveState`, `BeginRecording`, `EndRecording`,
`FlushDeferredRecorder`, `TryResolveRecorder`, `ResolveOwnerRecorderState`,
`TryRestoreActorRecorder`, `RetainRecorder`, `PerformStitchingPass`,
`HasValidOwner`), and all `N001..N027` virtual slot overrides
([restore-rules spec](../../specs/save-format/restore-rules.md)), 6 of 7
populated C001 arrays, `CA06`/`DA04`, all 29,762 C028 spans, pin `b40c97e3...`,
entry-path failure matrices, `FProspectBlob` `SL-004` decode contracts,
`FAccoladeSaveData`, `FProspectInfo` (`17` props), and `15 / 15`
`inspect-save --stage-check` qualification (`dddd.json`: `2,744` -> `1` invalid
`C031`, `2,743` retained (`971` skip-owner, `52` `FindOnly`, `798` `RespawnOnly`,
`658` `FindOrRespawn`, `264` `ManuallyResolved`, `821` stitching), `2` dynamic
`C033` -> `2,741` on `M-691`; F-645 to F-680, F-715 to F-720, F-725 to F-728,
F-731 to F-738, F-741 to F-744, F-746, F-747, F-750, F-752; U-020 to U-024, U-026,
U-028 to U-033, U-039, U-046, U-048, U-050, U-053 to U-063 resolved). Every
opaque Struct payload is one of four fixed-width engine math types (F-565,
F-602), and construction grids nest per-type row names over placed pieces
sharing the grid's frame (F-643).

Under the hard tree limits every recorder value of all 15 prospects types
losslessly with no uninterpreted leaf (F-766), and a dry-run native import plan
then blocks only on name and path references that no native content catalog
yet defines (F-767). Per-instance object names are identity, not content, and
never block (F-813), and actor variable records name class properties
(F-817). Fields whose recorder restores were read classify by property and
enclosing struct: most name data rows through each table's row handle, and
FLOD tile records name landscape tiles (F-820). That leaves 23 to 26 of 3,528
to 3,782 distinct references unclassified, with 52 find-only owners per save
(F-821).

No saved class identifier, field or type name, or recorder version tells the
older server from the newer client, and the prospect carries no version string
(F-635, F-682). Recorder code gives every class's fields their meanings (F-645 to
F-666, F-676, F-677, F-734 to F-736, F-741 to F-744, F-746, F-747, F-750, F-752); item
stats, alterations and slots are row references (F-660). Readers use borrowed
strict string frames without success-path allocation (F-142); comparator limits
stay at 128 KiB (F-198).

## Open

- Packaged data-table row meanings remain out of scope by policy (F-660).
- Native content catalog and import writer: classify the F-767 references by role and author native world data for them; connected client compatibility (U-025, U-038, U-045).
- Heavy-world save work order: the preserved server's offline load/save/reload baseline (F-678 to F-681, F-736) and connected scenario S1 on server 25956137 (3.0.32, F-701) both frame and preserve byte-identically; build pairing of the original save remains open (U-040), and native interpreted import stays gated (U-038, U-045).

## Artifacts

M-047..M-077, M-079..M-108, M-118..M-130, M-278..M-280, M-351, M-352, M-376..M-384, M-389, M-390, M-401..M-404, M-407..M-410, M-606..M-613, M-616..M-618, M-626, M-627, M-636..M-638, M-727, M-755..M-760, M-766..M-769, M-772..M-777, M-780, M-782, M-784, M-786, M-788, M-790..M-792, M-794..M-808, M-811, M-850, M-851, M-854..M-859, M-873, M-878, M-880.
