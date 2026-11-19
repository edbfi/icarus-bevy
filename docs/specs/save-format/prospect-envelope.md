# Prospect envelope

## Status

Experimental: envelope schema observed statically in build 25621760 (M-168) and
confirmed against all twelve supplied prospect documents; the inner world is
not interpreted. Last reconciled: 2026-11-19.
Full text: d5b0ae6:docs/specs/save-format/prospect-envelope.md.

## Scope

The prospect JSON envelope, integrity chain and native inspection policy.
The [save runbook](../../runbooks/native-server.md#supplied-saves) covers
recorder comparison and read-only `import-save --dry-run` (F-898).
Import publication remains closed. See [tagged properties](tagged-properties.md)
and [prospect loading](prospect-loading.md) for inner-world framing and planning.

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

- Original decode order: Base64, zlib over `TotalLength` bytes, SHA-1, then
  archive deserialization (F-283, F-737, F-1256 to F-1259).
  Native model: `experimental::restore_rules::evaluate_prospect_blob_decode`.
- All twelve supplied BOM UTF-16LE prospects (eleven distinct) have one
  complete zlib stream, canonical Base64 and `DataLength == TotalLength`.
  Intake and sizes: M-118, F-881. Each lists seven distinct account/character-slot
  tuples including the claimed owner (F-262). Twelve identical 97-byte UTF-8
  documents (`Accolades.json` and 11 backups) match `FAccoladeSaveData`
  (`0x145dc1790` / `0x1419edb20`, size `0xB0`, align `0x08`: empty
  `CompletedAccolades: TArray<FAccoladeCompletedState>` `+0x00`,
  `PlayerTrackers: TMap<FPlayerTrackersRowHandle,int32>` `+0x10`, and
  `PlayerTaskListTrackers: TMap<FPlayerTrackersRowHandle,FTrackerTaskListProgress>`
  `+0x60`; F-738). The release inspector reports `world_imported: false` (F-881), and
  `inspect-save --stage-check --expect-prospect-name` passes all 12 supplied
  prospect files and 3 reference-server re-saves (`15 / 15`, `2,664..2,850`
  recorders, F-738).
- No explicit outer build or save-version marker exists, and neither the
  payload nor the original server's re-save holds a version string (F-896).
- F-1271 measures official empty report to save start with both timers at
  120/240 s. F-1272 measures native startup to SIGTERM. Across the official
  save pairs, C015 level rose 14 s, game time 14.0625/14.09375 and root
  elapsed 593 s. Whole-recorder matches leave object identity open.
- Dedicated build 26613688 declares a direct C008 array with float32 time
  and signed i32 state in each element. A search of the four direct
  element fields found no explicit plot ID (M-1593). Native readback of the
  F-1271 common input and both zero-player lobby outputs retained 109
  losslessly typed C008 records per save, each with one exact array element
  and one explicit finite float32/signed i32 pair. The unordered, bit-exact
  pair multisets differed in all three comparisons (F-1274, M-1595).
- Provenance: the heavy-world archive is preserved privately with an inventory
  (M-166); the writes date from the 3.0.29 window, before the user-reported
  3.0.31 client (F-896, F-685, F-878). Possession completes no compatibility,
  bug-reproduction or performance gate.

## Candidate

- Inspector policy (research limits, not recovered ones; F-880): raw input at
  most 4 MiB, declared output at most 32 MiB; UTF-8 or BOM-marked UTF-16LE
  only, rejecting odd UTF-16 lengths and unpaired surrogates; the entire UTF-8
  input is validated before unknown fields are skipped (N-005); exact field
  names and types, duplicates rejected; named objects required at root and
  blob boundaries, positional arrays rejected; malformed-JSON diagnostics keep
  only error category/line/column. It rejects negative lengths and malformed
  SHA-1 or Base64; requires canonical padded Base64, the exact compressed
  count, one complete zlib stream, no trailing bytes and the exact output
  length; verifies the legacy hash without retaining the world; and reports
  `DataLength` without enforcing equality with `TotalLength` (F-737).
  Output labels: `experimental_envelope_only`, `experimental_root_tags_only`
  (fixed research profile, F-882), and `experimental_stage_only`. Staging
  checks the `FProspectSaveState` root schema, base recorder framing, tag
  order, `PostLoadSerialize` migration, 17 `FProspectInfo` tags and optional
  `ProspectID` matching. It overlays tags on 65-class defaults,
  applies per-recorder `Validate` ([restore rules](restore-rules.md)), and
  counts `C033` `bIsPhysicsDynamic` across all 15 saves (F-680, F-736, F-738,
  F-746, F-747).
- The original reader checks overrun only and normalizes narrow/wide strings
  lossily (`F-893`; U-026 resolved), whereas the strict inspector fails closed;
  the entry-path failure matrix (`LoadProspectByName` vs `SetupLoadedProspect`)
  and `ReadFromProspectBlob` decode contract are verified across missing, empty,
  malformed-JSON, ID-mismatch and corrupt-`BinaryBlob` inputs (`F-899`,
  `F-733`, `F-737`; `U-056` to `U-058`, `U-063` resolved). Schema agreement does
  not establish cross-build compatibility (U-025, U-040).

## Open questions

- Producing-build log confirmation for the supplied save (U-040); official
  restoration and newer-build compatibility (U-025).
- C008 restoration needs stable plot identity, validated clock units and
  state semantics, and per-plot progression measured under matched official
  and native loads and restarts. Aggregate differences leave continuity,
  growth rate, cause and restoration open (F-1274).

## Evidence

- Static: M-169 (`M-169/E-004`, `M-169/E-008`, `M-169/E-009`, `M-169/E-018` to
  `M-169/E-025`, `M-169/E-028` to `M-169/E-030`), M-757, M-758, M-772, M-773,
  M-776, M-777. Supplied-save triage with hashes, CRCs and bounded member
  checks: M-118.
- Permissively licensed dependencies and a Rust zlib backend.
- Native admission, diagnostics and source preservation: M-182 and the
  [envelope fixture](../../../tests/fixtures/prospect-envelope.json),
  [parser tests](../../../tests/prospect.rs) and [CLI tests](../../../tests/cli.rs).
