# Experimental prospect envelope inspection

`icarus-server inspect-save --source PATH` reads at most 4 MiB and checks a
candidate JSON save envelope without writing files, importing a world or exporting
payload bytes. Success is labeled `experimental_envelope_only`, with
`world_imported: false`. `import-save` remains unavailable.

This is based on static observations from build 25621760, executable SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`.
The supplied save's outer schema agrees with these static observations, but its
producing build is user-reported as 25793767, version
`3.0.31.158478-rel-DangerousHorizons`; this has not been independently paired with
a reference load. Independent bounded inspection of all twelve prospect
documents verified their envelopes. The inner serialized world, content references,
version migration and stock-client round trips remain unresolved.

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

- Maximum raw input: 4 MiB; maximum declared output: 32 MiB.
- Accept UTF-8 JSON or explicitly BOM-marked UTF-16LE JSON. Reject odd UTF-16 byte
  lengths and unpaired surrogates; do not guess encodings or replace malformed text.
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

## Supplied save baseline (2026-10-12)

Read-only archive checks matched all 24 file entries against the preserved private
inventory's sizes, CRCs and SHA-256 values. The ZIP also has two directory entries.
The case checked bounded member reads, traversal/symlink/duplicate destinations and
aggregate sizes before inspecting bodies. Preserved source/archive/inventory hashes
remained unchanged. Private evidence and reproducible command batches reside under
`work/save-triage-20261012/`; no private body, member name or account identifier is
included in authored fixtures or tracked reports.

The native release inspector also passes a bounded disposable current prospect and
one distinct backup, reporting 18,513,234 and 18,492,675 decoded bytes respectively.
Their source bytes remain unchanged and both results explicitly report
`world_imported: false`; the payload was not exported by the inspector.

| Observation | Measured result |
| --- | --- |
| Prospect family | Twelve BOM-marked UTF-16LE JSON documents; one current file and eleven backups |
| Raw prospect bytes | 2,154,484–2,239,534 |
| Base64-decoded compressed bytes | 805,856–837,751 |
| Decompressed binary archive bytes | 17,671,728–19,131,027 |
| Integrity | All twelve pass canonical Base64, exact lengths, complete single zlib stream and SHA-1 |
| Backup differences | Eleven distinct prospect bodies/payloads; one backup equals current |
| Member metadata | Seven distinct account/character-slot tuples per prospect; claimed owner tuple matches a member |
| Other family | Twelve identical 97-byte UTF-8 JSON documents with empty accolade/tracker collections |

The previous 1 MiB/16 MiB caps rejected valid supplied dimensions. The new fixed
4 MiB/32 MiB bounds cover these measured inputs with headroom; they are still
research policy, not recovered format limits. `DataLength == TotalLength` in all
twelve documents is an observation, not a general decoder rule.

The smaller documents contain `CompletedAccolades`, `PlayerTrackers` and
`PlayerTaskListTrackers`; they do not establish complete character profiles.
Prospect membership metadata varies across five distinct values, while all copies
share the same prospect identity and claimed owner. File names and ZIP timestamps
do not establish backup chronology. Seven member tuples do not imply seven
concurrent players. Actor, structure, inventory, animal and mission counts remain
unknown because the inner archive is binary and has not been schema-decoded.

No explicit outer build/save-version marker was found. The user identifies the
save's “latest” release as build 25793767/version `3.0.31.158478-rel-DangerousHorizons`.
The preserved executable and static addresses belong to older build 25621760;
schema agreement does not establish cross-build compatibility. Ownership fidelity, character
profile linkage, content references, binary field semantics and unknown fields
remain unsupported; envelope success must never be reported as world import.

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
preserves the source and creates no state directory. Additional authored tests
cover BOM-marked UTF-16LE, non-BMP characters, malformed surrogates/code units,
rejected guessed encodings, exact raw bounds in both encodings, output above the
former 16 MiB cap and streams that expand beyond their declared size.
