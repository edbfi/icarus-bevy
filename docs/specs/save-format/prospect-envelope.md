# Prospect envelope

## Status

Experimental: envelope schema observed statically in build 25621760 (M-168) and
confirmed against all twelve supplied prospect documents; the inner world is
not interpreted. Last reconciled: 2026-10-18.
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
  in-memory, not file offsets (F-283, F-737):

  | Name | Object offset | Use |
  | --- | --- | --- |
  | `Key` | `0x00` | String metadata (`"actors"` in `WriteToProspectBlob`), not validated on load |
  | `Hash` | `0x10` | Expected SHA-1 of uncompressed bytes, 40 lowercase hex digits |
  | `TotalLength` | `0x20` | Compressed byte count passed to `FCompression::UncompressMemory` |
  | `DataLength` | `0x24` | Redundant copy of `CompressedSize` written by `WriteToProspectBlob` (`0x1418fa200`); unread by `ReadFromProspectBlob` (`0x1418e6240`, F-737) |
  | `UncompressedLength` | `0x28` | Output size passed to `FCompression::UncompressMemory` |
  | `BinaryBlob` | `0x30` | Base64 input string |

- `WriteToProspectBlob` (`0x1418fa200` server / `0x1418fa530` client) writes
  `"actors"` into `Key`, compresses via `FCompression::CompressMemory` (`NAME_Zlib`
  `0x101`), and at `0x1418fa48c..0x1418fa49a` stores `CompressedSize` to both
  `TotalLength` (`+0x20`) and `DataLength` (`+0x24`). `ReadFromProspectBlob`
  (`0x1418e6240` server / `0x1418e7680` client) leaves `DataLength` unread,
  pre-allocates `CompressedData` to `TotalLength`, calls `FBase64::Decode`
  (`0x141efc520`, which resizes `CompressedData` to `GetDecodedDataSize`
  `0x141f02ee0`), ignores `FBase64::Decode`'s `bool` return, sign-extends
  `UncompressedLength` into `memset`, and passes `CompressedSize = TotalLength`
  (not `CompressedData.Num()`) to `FCompression::UncompressMemory` (`0x141f140f0`,
  `NAME_Zlib` `0x101`, window bits 15), hashes the output through `0x140accda0`
  (SHA-1 state initialized by `0x141f2cf00`), compares with `Hash`, and enters
  archive deserialization at `0x1418ee4a0` (F-283, F-737;
  `evaluate_prospect_blob_decode` in `experimental::restore_rules`).
- Supplied saves (M-118, F-261, F-738): twelve BOM-marked UTF-16LE JSON prospect
  documents (one current, eleven backups), raw 2,154,484 to 2,239,534 bytes,
  Base64-decoded 805,856 to 837,751 bytes, decompressed 17,671,728 to
  19,131,027 bytes; all pass canonical Base64, exact lengths, one complete zlib
  stream and SHA-1; eleven distinct bodies, one backup equals current; seven
  distinct account/character-slot tuples per prospect with the claimed owner
  among them; `DataLength == TotalLength` in all twelve. Twelve identical 97-byte
  UTF-8 documents (`Accolades.json` and 11 backups) match `FAccoladeSaveData`
  (`0x145dc1790` / `0x1419edb20`, size `0xB0`, align `0x08`: empty
  `CompletedAccolades: TArray<FAccoladeCompletedState>` `+0x00`,
  `PlayerTrackers: TMap<FPlayerTrackersRowHandle,int32>` `+0x10`, and
  `PlayerTaskListTrackers: TMap<FPlayerTrackersRowHandle,FTrackerTaskListProgress>`
  `+0x60`; F-738). The release inspector reports 18,513,234 and 18,492,675
  decoded bytes for the current prospect and one backup with
  `world_imported: false` (F-242), and `inspect-save --stage-check
  --expect-prospect-name` passes across all 12 supplied prospect files and 3
  reference-server re-saves (`15 / 15`, `2,664..2,850` recorders, F-738).
- No explicit outer build or save-version marker exists, and neither the
  payload nor the original server's re-save holds a version string (F-682).
- Provenance: the user supplied the archive from a heavy ICARUS world; it is
  preserved privately with an inventory (M-166). Intake: 24 members (2 JSON, 22
  backups), 26,511,334 bytes, all member CRCs pass (F-276). The writes
  date from the 3.0.29 release window, a week before the user-reported 3.0.31,
  which is the installed client's version (F-683, F-685, F-486). A stock Windows
  client exists on the user's separate machine, outside the offline lab.
  Possession of the archive completes no compatibility, bug-reproduction or
  performance gate.

## Candidate

- Inspector policy (research limits, not recovered ones): raw input at most
  4 MiB, declared output at most 32 MiB; accept UTF-8 or BOM-marked UTF-16LE
  only, rejecting odd UTF-16 lengths and unpaired surrogates; validate the
  entire UTF-8 input before skipping unknown fields (N-005, F-243); exact field
  names and types with duplicates rejected; require named objects at root and
  blob boundaries, rejecting positional arrays (F-411); keep only error
  category/line/column in malformed JSON diagnostics, with raw causes removed
  (F-412); reject negative lengths, malformed
  SHA-1 or Base64; require canonical padded Base64, exact compressed count, one
  complete zlib stream, no trailing bytes and exact output length; verify the
  legacy hash without retaining the world; report `DataLength` uninterpreted
  (F-241). Success is labeled `experimental_envelope_only`; `--inner-tags`
  adds `experimental_root_tags_only` aggregate counts under the fixed research
  profile (F-244), and `--stage-check` adds `experimental_stage_only` root,
  `ProspectInfo`, migration and recorder base-component staging (`F-738`).
- The original reader checks overrun only and normalizes narrow/wide strings
  lossily (`F-719`; U-026 resolved), whereas the strict inspector fails closed;
  the entry-path failure matrix (`LoadProspectByName` vs `SetupLoadedProspect`)
  and `ReadFromProspectBlob` decode contract are verified across missing, empty,
  malformed-JSON, ID-mismatch and corrupt-`BinaryBlob` inputs (`F-717`, `F-732`,
  `F-733`, `F-737`; `U-056` to `U-058`, `U-063` resolved). Schema agreement does
  not establish cross-build compatibility (U-025, U-040).

## Open questions

- Producing-build log confirmation for the supplied save (U-040); official
  restoration and newer-build compatibility (U-025).

## Evidence

- Static: M-169 (`M-169/E-004`, `M-169/E-008`, `M-169/E-009`, `M-169/E-018` to
  `M-169/E-025`, `M-169/E-028` to `M-169/E-030`), M-757, M-758, M-772, M-773,
  M-776, M-777. Supplied-save triage with hashes, CRCs and bounded member
  checks: M-118.
- Dependencies in `Cargo.lock`: `base64` 0.23.1, `flate2` 1.1.10 (Rust backend,
  `miniz_oxide` 0.9.1, `crc32fast` 1.5.2, `adler2` 2.0.1, `simd-adler32`
  0.3.10), `sha1` 0.11.0; permissive licenses; no proprietary SDK.
- Native admission and diagnostic regressions: M-182; UTF-8/BOM UTF-16LE,
  all inspection modes, exact failure boundary and source preservation.
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
target/release/icarus-server inspect-save --source PATH [--inner-tags | --recorders | --stage-check [--expect-prospect-name NAME] | --preserve-check]
```
