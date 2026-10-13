# Prospect envelope

## Status

Experimental: envelope schema observed statically in build 25621760 (M-168) and
confirmed against all twelve supplied prospect documents; the inner world is
not interpreted. Last reconciled: 2026-10-13.
Full text: d5b0ae6:docs/specs/save-format/prospect-envelope.md.

## Scope

The outer JSON envelope of a prospect save, its integrity chain, and the
native `icarus-server inspect-save` policy. Not covered: the inner serialized
world ([tagged-properties](tagged-properties.md),
[prospect-loading](prospect-loading.md)), content references, version
migration, world import and stock-client round trips. `import-save` remains
unavailable.

## Confirmed

- JSON conversion at `0x141800e40` uses reflected `ProspectSaveData`
  (registration `0x141cfd8f0`, descriptor `0x145e9cb00`) with `ProspectInfo` at
  object offset 0 and `ProspectBlob` at `0xa0`. `ProspectBlob` (registration
  `0x143dbf3a0`, descriptor `0x145fae230`) has six properties; offsets are
  in-memory, not file offsets (F-283):

  | Name | Object offset | Use |
  | --- | --- | --- |
  | `Key` | `0x00` | String metadata, not validated |
  | `Hash` | `0x10` | Expected SHA-1 of uncompressed bytes, 40 lowercase hex digits |
  | `TotalLength` | `0x20` | Compressed byte count passed to the decompressor |
  | `DataLength` | `0x24` | Integer; semantics unresolved |
  | `UncompressedLength` | `0x28` | Output size passed to the decompressor |
  | `BinaryBlob` | `0x30` | Base64 input string |

- The reader at `0x1418e6240` calls the Base64 decoder `0x141efc520`, then the
  decompression dispatcher `0x141f140f0` with name index `0x101` (a zlib-shaped
  stream, window bits 15), hashes the output through `0x140accda0` (SHA-1 state
  initialized by `0x141f2cf00`), compares with `Hash`, and enters archive
  deserialization at `0x1418ee4a0` (F-283).
- Supplied saves (M-118, F-261): twelve BOM-marked UTF-16LE JSON prospect
  documents (one current, eleven backups), raw 2,154,484 to 2,239,534 bytes,
  Base64-decoded 805,856 to 837,751 bytes, decompressed 17,671,728 to
  19,131,027 bytes; all pass canonical Base64, exact lengths, one complete zlib
  stream and SHA-1; eleven distinct bodies, one backup equals current; seven
  distinct account/character-slot tuples per prospect with the claimed owner
  among them; `DataLength == TotalLength` in all twelve. Twelve identical 97-byte
  UTF-8 documents hold empty `CompletedAccolades`, `PlayerTrackers` and
  `PlayerTaskListTrackers`. The release inspector reports 18,513,234 and
  18,492,675 decoded bytes for the current prospect and one backup with
  `world_imported: false` (F-242).
- No explicit outer build or save-version marker exists; the producing build is
  user-reported (F-277).

## Candidate

- Inspector policy (research limits, not recovered ones): raw input at most
  4 MiB, declared output at most 32 MiB; accept UTF-8 or BOM-marked UTF-16LE
  only, rejecting odd UTF-16 lengths and unpaired surrogates; validate the
  entire UTF-8 input before skipping unknown fields (N-005, F-243); exact field
  names and types with duplicates rejected; reject negative lengths, malformed
  SHA-1 or Base64; require canonical padded Base64, exact compressed count, one
  complete zlib stream, no trailing bytes and exact output length; verify the
  legacy hash without retaining the world; report `DataLength` uninterpreted
  (F-241). Success is labeled `experimental_envelope_only`; `--inner-tags`
  adds `experimental_root_tags_only` aggregate counts under the fixed research
  profile (F-244).
- The original may accept other encodings or inconsistent inputs; the strict
  inspector does not reproduce those tolerances (U-026). Schema agreement does
  not establish cross-build compatibility (U-025, U-040).

## Open questions

- Inner archive semantics, ownership fidelity and character profile linkage
  (U-040); official restoration and newer-build compatibility (U-025); original
  tolerance of malformed envelopes (U-026).

## Evidence

- Static: M-169 (overnight E-004, E-008/E-009, E-018 to E-025, E-028 to E-030).
  Supplied-save triage with hashes, CRCs and bounded member checks: M-118.
- Dependencies in `Cargo.lock`: `base64` 0.23.1, `flate2` 1.1.10 (Rust backend,
  `miniz_oxide` 0.9.1, `crc32fast` 1.5.2, `adler2` 2.0.1, `simd-adler32`
  0.3.10), `sha1` 0.11.0; permissive licenses; no proprietary SDK.
- Fixture: Python `zlib.compress`, `base64.b64encode` and `hashlib.sha1` over
  the 42 bytes `Authored envelope fixture: no game state.\n`. Tests cover
  malformed and truncated input, duplicates, invalid lengths, trailing bytes,
  hash mismatches, bounded reads, UTF-16LE with BOM, non-BMP and malformed
  code units, rejected guessed encodings, exact bounds in both encodings,
  outputs above the former 16 MiB cap, over-expanding streams, invalid, overlong
  and surrogate UTF-8 in ignored fields, and a CLI check that preserves the
  source and creates no state directory.

```bash
cargo test --locked --test prospect --test cli
target/release/icarus-server inspect-save --source PATH [--inner-tags]
```
