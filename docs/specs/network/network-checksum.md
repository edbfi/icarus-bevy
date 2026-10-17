# Default version checksum

## Status

Experimental: static interpretation with independent arithmetic tests, one
matching early-startup value, both builds' wire values and exhaustive delegate
cross-reference scans across both binaries (F-689, F-713; U-037 resolved). Scope: build 25621760 (M-168); addresses
are preferred virtual addresses in that file. Last reconciled: 2026-10-17.
Full text: d5b0ae6:docs/specs/network/network-checksum.md.

## Scope

The default construction path of the network version checksum and its cache,
setter and delegate structure. Not covered: datagram framing, identity
validation, or any stock-client negotiation. The native server does not use
this module for admission.

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
- On the wire both builds compare a Hello's version exactly: 3.0.31 answers its
  own 2406501229 with a Challenge and 1379612455 with Upgrade, 3.0.30 the
  reverse, each logging its value (F-689). A connected client needs a server of
  its own build; the comparison is the server's, so a native server sets its
  own policy (F-529).
- Client build 25793767 builds the same string from its own inputs: image
  project name `Icarus`, engine protocol 17, a zero-filled game protocol
  global, and NetCL taken from `-networkversionoverride=` or the
  `networkversionoverride` console variable, else 0. Hello sends this value
  with the override delegate allowed; its project version is runtime state, so
  a matching value needs the client's active version (F-541, F-542).
- Exhaustive `.text` RIP-relative and `.rdata`/`.data` pointer scans across both
  binaries prove that `GetLocalNetworkVersionOverride` (`0x1463251b0` /
  `0x1462116a0`) and `IsNetworkCompatibleOverride` (`0x1463251c0` /
  `0x1462116b0`) are referenced only by their reader functions (`0x141f13b30` /
  `0x141f2b220`, `0x141f16ef0` / `0x141f2e6e0`) and CRT `atexit` destructors
  (`0x144800cb0..d10` / `0x14461eb50..b0`), with 0 callbacks bound in either
  binary (F-255, F-713; U-037 resolved).

## Candidate

- The getter's `1.0.0` fallback is superseded by `GeneralProjectSettings` at
  startup, and with 0 override callbacks bound in either binary, both builds
  compare their computed CRC-32 values by exact equality (F-689, F-713).

## Open questions

- Authenticated stock-client session against the native server (U-038).

## Evidence

- Private cases: M-146 (`work/control-version-20261012/`, E-001 to E-003, strict
  review with zero issues), M-169 (`decompile-01/`, selected disassemblies and
  `checksum-table-verification.json`), M-751 and M-752 (`join-spawn-and-admission-20261017/`).
- Authored tests cover ASCII case conversion, embedded NUL, nonzero seeds and
  incremental processing against the independent oracle:

```bash
cargo test --locked --test checksum
```
