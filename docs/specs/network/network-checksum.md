# Default version checksum

## Status

Experimental: static interpretation with independent arithmetic tests and one
matching early-startup original value. Scope: build 25621760 (M-168); addresses
are preferred virtual addresses in that file. Last reconciled: 2026-10-13.
Full text: d5b0ae6:docs/specs/network/network-checksum.md.

## Scope

The default construction path of the network version checksum and its cache,
setter and delegate structure. Not covered: datagram framing, identity
validation, the active admission decision, or any stock-client negotiation. The
native server does not use this module for admission.

## Confirmed

- Default construction at `0x141f2b220` formats project/version inputs, calls
  `0x141e28f90` and then `0x14139af80`. The transformation lowercases UTF-16
  units `A` to `Z` only. The checksum consumes each nonzero UTF-16 unit as a
  little-endian 32-bit value, stops at the first zero unit, complements the seed
  before processing and complements the result afterward. All 256 table entries
  at `0x145eed270` equal independently generated reflected CRC-32 entries for
  polynomial `0xedb88320` (F-281).
- The independent primitive and a Python `zlib.crc32` oracle over each UTF-16
  unit expanded to four little-endian bytes reproduce the offline log's early
  checksum **1447163286** for project `Icarus`, version `1.0.0`, NetCL 0,
  EngineNetVer 17, GameNetVer 0. Supplementary characters contribute two
  separately expanded surrogate units, so UTF-32 encoding is not equivalent
  (F-254).
- The log then sets ProjectVersion to `3.0.30.158174-rel-DangerousHorizons` and
  invalidates the cached checksum; the early value must not be hard-coded as
  the accepted game version (F-254).
- Cache-valid storage is `0x146210cf0`, the cached checksum `0x146210cf4`, and
  the generation/comparison delegates `0x1462116a0` / `0x1462116b0`. Generation
  checks the cache before its callback-enable argument; an uncached call needs
  nonzero occupancy, a nonnull object and a successful virtual predicate.
  Comparison has its own delegate checks and otherwise falls back to exact
  32-bit equality (F-255).
- Setter `0x141f39000` ignores null/empty strings; nonempty input updates
  project-version storage and clears cache-valid, even when the input already
  points at the stored buffer. Caller `0x143996c54` supplies a string from a
  `GeneralProjectSettings` default object (F-255).
- The two initializer candidates found by a broader direct-reference scan
  register delegate teardown through CRT on-exit imports; they do not install
  compatibility callbacks (F-255).

## Candidate

- The string getter at `0x141f2bae0` has fallback literal `1.0.0`; mutable
  state and a generator callback can replace defaults, so it does not prove the
  active version input.
- Bounded RIP-relative and direct-reference searches found getter/comparator
  reads and destructor writes for the delegate globals but cannot prove
  registration absent: indirect addressing, dynamic modules and runtime writes
  lie outside them. The negative search does not justify dropping the callback
  requirement.

## Open questions

- Active checksum/admission policy and indirect runtime callback registration;
  progress requires observing generation, comparison and delegate state in a
  matched reference session, not repeating the negative search (U-037).

## Evidence

- Private cases: M-146 (`work/control-version-20261012/`, E-001 to E-003, strict
  review with zero issues), M-169 (`decompile-01/`, selected disassemblies and
  `checksum-table-verification.json`).
- Authored tests cover ASCII case conversion, embedded NUL, nonzero seeds and
  incremental processing against the independent oracle:

```bash
cargo test --locked --test checksum
```
