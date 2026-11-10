# C001 constructor decoder and exact-window listing

Older-server (build 25621760) C001 construction callback and candidate
constructor: metadata bounds, the retained listing rejection, the standalone
decoder and its fixtures, exact-window decodes and retained-frame replay.
Preconditions for further cases are in
[gates](../../gates.md#before-any-c001-original-code-case).

Before restructure: 96ddb0b:docs/research/save-format/2026-10-13-c001-decoder.md

## Question cluster

- Where are the C001 construction callback and candidate constructor, and what bounds them?
- Why does the retained listing query reject?
- Can a database-free decoder decode the exact windows faithfully?
- Does a construction/vtable bridge, receiver continuity or registration flow connect them to C001 archive slots?

## Current answer

Callback 0x1419fe870 and candidate constructor 0x141788530 are complete
functions (F-905; U-005 resolved); the M-568/M-569 listing query rejected
because `-noanalysis` left the callback undisassembled (F-905; U-003, U-042
resolved). CRT init registers native getter 0x1419fe200 (F-906, F-722; U-006
resolved).

A database-free standalone decoder passes its authored fixtures (F-907) and,
with zero lookahead, decodes every byte of the callback, the candidate, their
common callee and its two callees (F-474, F-908). Retained-frame replay
qualifies its inputs (F-476, F-477, F-908). The old slot preparation
mis-stated two addresses; same-build base and offsets give three candidate
slots (F-910).

Decompilation and client-PDB correspondence resolve the bridge and runtime
restoration (F-906, F-908, F-909, F-722, F-902, F-728, F-733, F-735; U-004,
U-020 to U-023, U-041 resolved). Callback 0x1419fe870 dereferences
`rbx = *(C001**)rcx` and inlines the candidate; both call the `rbx`-preserving
base constructor, store F-908's vftables and initialize `+0xd8..+0x1b8`. Slot
`+0x4a0` reaches base `RestoreState` (F-722), which reads through
`FMemoryReader` at version 522, dispatches `Serialize` then `UpgradeVersion`
and may invoke `ApplyRecorderTransform` (`+0x4b0`, F-909);
`UpdateOwnerToDatabaseState` restores modifiers, `D01` slots and
`CPF_SaveGame` variables (F-735).

## Open

- Native interpreted import and newer-build compatibility (U-025, U-045).

## Artifacts

M-003, M-004, M-007, M-008, M-011, M-013, M-014, M-176, M-177, M-179..M-181, M-185..M-190, M-193..M-199, M-201, M-202, M-205, M-206, M-209..M-211, M-221..M-239, M-242..M-250, M-253..M-268, M-272..M-276, M-284..M-286, M-298..M-314, M-348..M-350, M-761, M-762, M-768, M-769, M-772 to M-775.
