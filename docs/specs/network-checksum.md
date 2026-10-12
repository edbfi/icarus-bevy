# Experimental default version checksum

Status: static interpretation, independent primitive tests and **one matching
early-startup original checksum**. No stock-client negotiation has been validated.
The native server does not use this module for admission.

Reference build 25621760, shipping executable SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`.
Addresses are preferred virtual addresses for this exact file.

The default construction path at `0x141f2b220` formats project/version inputs,
calls `0x141e28f90` and then `0x14139af80`. The transformation changes UTF-16 units
`A` through `Z` to their ASCII lowercase values; it does not lowercase arbitrary
Unicode. The checksum processes each nonzero UTF-16 unit as a little-endian
32-bit value. It stops at the first zero unit, complements the input seed before
processing and complements the result afterward. All 256 entries in its table at
`0x145eed270` match independently generated reflected CRC-32 polynomial
`0xedb88320` entries. The implementation generates the arithmetic directly and
contains no copied table or original code.

Tests use authored strings and Python's `zlib.crc32` as an independent arithmetic
oracle. The oracle input is each UTF-16 code unit expanded to four little-endian
bytes. Supplementary characters contribute two separately expanded surrogate
units, so ordinary UTF-32 encoding is **not** equivalent. Tests also cover ASCII
case conversion, embedded NUL, nonzero seeds and incremental processing.

The string getter at `0x141f2bae0` has a fallback literal `1.0.0`. This is not proof
of the active game version input: mutable state and a generator callback can
replace defaults. The comparison callback is separate. Callback registration and
the final admission-time inputs remain unresolved; see
[control negotiation](control-negotiation.md).

The offline original log first reports project `Icarus`, version `1.0.0`, NetCL 0,
EngineNetVer 17 and GameNetVer 0 with checksum **1447163286**. Applying the independent
Rust primitive and Python zlib oracle to the observed formatted input gives that
same value. The log then sets ProjectVersion to
`3.0.30.158174-rel-DangerousHorizons` and explicitly invalidates the cached checksum.
The early value must therefore **not** be hard-coded as the accepted game version.
No later recalculated value or active compatibility callback was observed.

A bounded RIP-relative reference search found getter/comparator reads and destructor
writes for the two delegate globals. It did not establish registration or its
absence: indirect accesses and instruction encodings outside the search remain
possible. This negative search does not justify removing the callback requirement.

The 2026-10-12 follow-up resolves cache-valid storage to `0x146210cf0`, the cached
checksum to `0x146210cf4`, and separate generation/comparison delegates to
`0x1462116a0` / `0x1462116b0`. Generation checks the cache **before** its callback-enable
argument: disabling that argument does not bypass an already cached result. On an
uncached call, the delegate also needs nonzero occupancy, a nonnull object and a
successful virtual predicate. Comparison has its own delegate checks and otherwise
falls back to exact 32-bit equality.

Setter `0x141f39000` ignores null/empty strings. Nonempty input updates project-version
storage and clears cache-valid, including when the input already points at the
stored buffer. Caller `0x143996c54` supplies a string from a `GeneralProjectSettings`
default object. This corroborates the observed late version update without proving
initialization order or the final admission-time checksum.

A broader direct-reference scan checked neighboring delegate words, possible trailing
immediates and absolute pointers. The two initializer candidates register delegate
**teardown** through verified CRT on-exit imports; they do not install compatibility
callbacks. Indirect addressing, dynamic modules and runtime writes remain outside
that search. Further progress requires observing actual generation/comparison and
delegate state in a working matched reference session, rather than repeating the
same negative search. Private case `work/control-version-20261012/` E-001–E-003
retains exact commands, hashes and a strict review with zero issues.

Private evidence: `work/icarus-overnight/decompile-01/`, selected disassemblies in
`static/`, and `static/checksum-table-verification.json`. These observations do not
recover datagram framing, identity validation or the rest of the connection state
machine.
