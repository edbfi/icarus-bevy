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

The callback 0x1419fe870 (305 B) and candidate constructor 0x141788530 (296 B)
are complete functions (.pdata 103538 and 95461, unwind v1 flags=2, push/pop
rbx) rather than fragments (F-471, F-721; U-005 resolved). The retained listing
query in M-568/M-569 rejected because the project was imported/opened with
`-noanalysis`, leaving 0x1419fe870 undisassembled so `getInstructionAt`
returned null with no API exception (F-470, F-721; U-003, U-042 resolved). The
native getter 0x1419fe200 (.pdata 103518 at 0x14655d468) is registered by CRT
init 0x1406a8ab0 -> 0x142187f70 and 0x141a00220 via 0x144abc1b0[0] with 48
`.text` references (F-472, F-513, F-722; U-006 resolved).

A standalone decoder that works on fixed buffers without a Program or database
(F-473) passes its authored fixtures, including one- and two-window variants
(F-469). With zero lookahead it decodes every physical byte of the callback,
the candidate, their common callee and that callee's two callees (F-474, F-475).
Retained-frame replay qualifies its inputs: unwind records, register and space
descriptors and public opcode constants (F-476, F-477, F-508). The old slot
preparation mis-stated two addresses (F-465); same-build base and offsets give
three candidate slots (F-495).

Decompilation and client-PDB correspondence resolve the complete bridge and
runtime restoration (F-509, F-513, F-669, F-670, F-722, F-727, F-728, F-733,
F-735; U-004, U-020 to U-023, U-041 resolved). Callback 0x1419fe870
dereferences `rbx = *(C001**)rcx` and inlines constructor 0x141788530: both
call base constructor 0x1418824d0 (which preserves `rbx`), store vftables
0x1449d9f90 (`+0x0`) and 0x1449da448 (`+0x28`), and initialize `+0xd8..+0x1b8`.
Slot `+0x4a0` (0x1418a9bb0) calls base `RestoreState` 0x1418a9c80 (696 B,
matching client 0x1418ae6c0 + log), which constructs `FMemoryReader` at version
522 and dispatches `+0xa0` (`Serialize` 0x1418aaf70) then `+0x488`
(`UpgradeVersion` 0x14189d380), before conditionally invoking `+0x4b0`
(`ApplyRecorderTransform`); `+0x4a8` is 0x14189a760, and
`UpdateOwnerToDatabaseState` (0x1418ba040) restores modifiers, `D01` inventory
slots (`0x1418b8f20`) and `CPF_SaveGame` variables (`0x1418bcff0`, F-735).

## Open

- Native interpreted import and newer-build compatibility (U-025, U-045).

## Artifacts

M-003, M-004, M-007, M-008, M-011, M-013, M-014, M-176, M-177, M-179..M-181, M-185..M-190, M-193..M-199, M-201, M-202, M-205, M-206, M-209..M-211, M-221..M-239, M-242..M-250, M-253..M-268, M-272..M-276, M-284..M-286, M-298..M-314, M-348..M-350, M-761, M-762, M-768, M-769, M-772 to M-775.
