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
start validated exception-metadata intervals that may be procedure fragments
(F-471). The native getter has one retained non-call reference and no proven
registration flow (F-472). The retained listing query rejects at its first
listing-entry check, and the diagnostic variant cannot name the cause (F-470).

A standalone decoder that works on fixed buffers without a Program or database
(F-473) passes its authored fixtures, including one- and two-window variants
(F-469). With zero lookahead it decodes every physical byte of the callback,
the candidate, their common callee and that callee's two callees (F-474). Both
windows call the common callee, and the callback stores a table constant through
post-call RBX; the declared CFGs stay open and receiver continuity across calls
is unproved (F-475).

Retained-frame replay qualifies its inputs: unwind records, register and space
descriptors and public opcode constants (F-476). Every case stops at an
unsupported operation before any receiver or frame claim. The common callee's
conditional model completes; with both vector registers admitted, the callback
and candidate stop at the same narrow store of RAX's low 16 bits, and admitting
that read grows the report past the 64 KiB bundle cap (F-477). Publishing per
role, the candidate qualifies its conditional model: after its one call it
stores two table constants at +0 and +0x28 of its entry RCX, while the callback
stops at its own output cap; an independent replay confirms the split outputs
(F-508). Normal-return
ABI inference could preserve RBX only with local stack, save-slot, home-area and
argument joins (F-431). The old slot preparation mis-stated two addresses
(F-465); same-build base and offsets give three candidate slots (F-495).

The client's PDB resolves the bridge by correspondence. The native getter is
C001's registration getter, whose constructor field is the callback (F-513);
the callback and
candidate match the client's named C001 InternalConstructor and default
constructor apart from image displacements, so the two stored tables are C001's
primary and IInterface_AssetUserData vftables and the common callee is its base
recorder's default constructor (F-509). In the client those vftable slots at the
three save-path offsets are the base's Serialize, C001's version-upgrade hook
and its state restore (F-510). The server's three slot targets match those
functions, and C028 inherits the last two (F-512). The static bridge is thus
named; runtime dispatch and register preservation remain conditions. Its load
path reads no archive beyond the base recorder's restore and Serialize: the
version hook is call-free and the restore then only applies a transform (F-532,
F-533).

## Open

- Construction/vtable bridge and receiver continuity to concrete archive slots (U-004, U-041).
- Cause of the listing rejection (U-003, U-042) and whether the intervals are complete functions (U-005).
- Registration argument flow and other callers of getter 0x1419fe200 (U-006); C001's place in the save census (U-023).

## Artifacts

M-003, M-004, M-007, M-008, M-011, M-013, M-014, M-176, M-177, M-179..M-181, M-185..M-190, M-193..M-199, M-201, M-202, M-205, M-206, M-209..M-211, M-221..M-239, M-242..M-250, M-253..M-268, M-272..M-276, M-284..M-286, M-298..M-314, M-348..M-350.
