# Experimental prospect envelope inspection

`icarus-server inspect-save --source PATH` reads at most 1 MiB and checks a
candidate JSON save envelope without writing files, importing a world or exporting
payload bytes. Success is labeled `experimental_envelope_only`, with
`world_imported: false`. `import-save` remains unavailable.

This is based on static observations from build 25621760, executable SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`.
It has passed independently authored fixtures, **not an original save or a
stock-client round trip**. The inner serialized world, content references and
version migration remain unresolved.

## Observed schema and call path

The JSON conversion at `0x141800e40` uses the reflected `ProspectSaveData` type.
Its registration at `0x141cfd8f0` references descriptor `0x145e9cb00`, whose
property array identifies `ProspectInfo` at object offset 0 and `ProspectBlob` at
offset `0xa0`. These are in-memory offsets, not binary file offsets.

The `ProspectBlob` registration at `0x143dbf3a0` references `0x145fae230`.
Its six property descriptors identify:

| Name | Object offset | Observed use |
| --- | --- | --- |
| `Key` | `0x00` | String metadata; not validated by this inspector |
| `Hash` | `0x10` | Expected hash of uncompressed bytes |
| `TotalLength` | `0x20` | Compressed byte count passed to decompressor |
| `DataLength` | `0x24` | Integer; semantics unresolved |
| `UncompressedLength` | `0x28` | Output size passed to decompressor |
| `BinaryBlob` | `0x30` | Encoded input string |

The reader at `0x1418e6240` calls the Base64 decoder at `0x141efc520`, then the
decompression dispatcher at `0x141f140f0` with name index `0x101`. That branch uses
a zlib-shaped stream with window bits 15. The reader hashes the output through
`0x140accda0`; its helper `0x141f2cf00` initializes the five SHA-1 state words,
and the result becomes 40 lowercase hexadecimal digits. The reader compares this
with `Hash` before entering archive deserialization at `0x1418ee4a0`.

Private evidence: overnight case E-004, E-008/E-009, E-018–E-025 and E-028–E-030.
Static decompiler types are provisional; arithmetic and call sites were inspected
alongside metadata. The complete schema and dynamic error behavior remain unknown.

## Inspector policy

The limits are conservative research choices, not recovered ICARUS limits:

- Maximum input: 1 MiB; maximum declared output: 16 MiB.
- Required known fields use exact names and types, with duplicates rejected.
  Unrelated JSON fields, including `ProspectInfo` and `Key`, are skipped.
- Reject negative lengths, malformed lowercase SHA-1 and malformed Base64.
- Require canonical padded Base64 and an exact compressed byte count.
- Require one complete zlib stream, no trailing bytes and exact output length.
- Verify the legacy hash but do not interpret or retain the decoded world.
- Report `DataLength` as an uninterpreted observation. Do not assert that it is
  validated against any other field.

The original may accept other encodings or tolerate some inconsistent inputs.
This strict inspector does not claim to reproduce those tolerances. SHA-1 serves
only legacy-format inspection; it is not authentication or the native save hash.

## Dependencies and tests

The implementation uses registry `base64` 0.23.1, `flate2` 1.1.10 with its Rust
backend, and `sha1` 0.11.0, all locked in `Cargo.lock`. Their package metadata gives
MIT/Apache-2.0 licensing. Compression dependencies include `miniz_oxide` 0.9.1
(MIT/Zlib/Apache-2.0), `crc32fast` 1.5.2 (MIT/Apache-2.0), `adler2` 2.0.1
(0BSD/MIT/Apache-2.0) and `simd-adler32` 0.3.10 (MIT). No proprietary SDK or assets
are linked. Metadata/license review is not a full dependency security audit.

The low-level decompressor exposes consumed and produced byte counts, allowing
the inspector to reject trailing input and length discrepancies. See the
[versioned flate2 API](https://docs.rs/flate2/1.1.10/flate2/struct.Decompress.html).
SHA-1's legacy-only purpose follows the
[crate guidance](https://docs.rs/sha1/0.11.0/sha1/).

The fixture is independently generated with Python `zlib.compress`,
`base64.b64encode` and `hashlib.sha1` over the 42 bytes
`Authored envelope fixture: no game state.\n`. It contains no game state.
Tests cover malformed/truncated input, duplicate fields, invalid lengths,
trailing compressed bytes, hash mismatches, bounded reads and a CLI check that
preserves the source and creates no state directory.
