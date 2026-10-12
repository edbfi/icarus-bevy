# Experimental root property tags

This reader checks outer record boundaries in an already verified prospect payload.
It does not decode recorder bodies, validate gameplay values, apply version
migrations or import a world. Its explicit `MemoryArchive522` research profile is
based on the preserved build 25621760; selecting it does not identify a save's
producing build. The user reports build 25793767 for the supplied saves.

```bash
cargo run --locked -- inspect-save --source /path/to/copied-prospect.json --inner-tags
```

The command verifies envelope integrity first, then reports
`experimental_root_tags_only`, profile `experimental_memory_archive_522`, aggregate
record sizes and `world_imported: false`. Without `--inner-tags`, the existing
envelope-only behavior and result format are unchanged.

## Observed framing

The [loading investigation](prospect-loading.md) resolves the concrete reader to
tagged-property traversal `0x14200f5b0`. Its tag reader `0x14215f530` obtains names
through archive virtual `+0x140`, resolved to `0x140962300`, which uses the string
reader at `0x141e15db0`. The archive resets to version 522 with byte swapping off.

A canonical name uses a little-endian signed 32-bit string count. Positive counts
describe narrow bytes, negative counts describe UTF-16 code units, and nonzero
counts include a terminating zero. The native strict subset reuses the
[archive string codec](archive-string.md); it rejects malformed terminators and
unsupported normalizations instead of reproducing the original's corrections.

| Record component | Encoding in this profile |
| --- | --- |
| Property name | Archive string; canonical `None` terminates the root sequence |
| Property type | Archive string, absent after the terminator |
| Payload size | Little-endian signed 32-bit byte count |
| Array index | Little-endian signed 32-bit value |
| Type-specific metadata | Described below |
| Property GUID presence | One byte; followed by 16 bytes when present |
| Opaque value | Exactly the declared payload size |

Struct tags add a struct name and 16-byte struct GUID. Array tags add an inner type
name. Byte and enum tags add an enum name. Boolean tags carry a one-byte value in
the header. Integer and string tags have no extra type-specific metadata.
The older tag reader gates struct GUIDs at archive version 441, array inner types
at 282 and property GUIDs at 503; all apply at 522. Other profiles are unsupported.

The size counts **only the value bytes after the complete header**. Function
`0x142166850` measures the property reader's position change against this value.
It does not make a C++ object offset a file offset. A struct array also has its own
body framing: a count followed by a shared inner tag before its elements in this
profile. The root scanner deliberately leaves that entire body opaque.

## Conservative reader policy

- Require a complete root sequence ending with canonical `None` and exact EOF.
- Bound input to 32 MiB, root records to 4,096 and each header string to 1,024 units
  including any terminator; check counts before allocating strings.
- Reject negative sizes or indices, truncated headers/values, unsupported type
  headers and noncanonical boolean/GUID-presence bytes.
- Accept canonical `ArrayProperty`, `StructProperty`, `StrProperty`, `IntProperty`,
  `ByteProperty`, `EnumProperty` and `BoolProperty` headers. Empty header names and
  other property kinds are unsupported by this initial subset.
- Preserve original header and value spans, including unknown field names, without
  rewriting, normalizing or interpreting their contents.
- Report aggregate sizes and record counts only. No property names or values are
  included in CLI results or parser errors.

These are research limits and accepted subsets, not recovered universal format
constraints. A successful scan does not establish required-field coverage, value
types, ownership, reference integrity, recorder counts or migration fidelity.
Opaque integer or array values are not semantically validated by their tag names.

## Evidence and verification

Private static evidence, exact commands and selected exports are retained under
`work/save-schema-20261012/`: E-009 covers header/name primitives and E-010 covers
payload sizes and array dispatch. Independent inspection of only the first 256 decoded
bytes of two private copies, in `work/save-triage-20261012/` E-013/E-014, identifies
`StateRecorderBlobs` / `ArrayProperty`, an inner `StateRecorderBlob` tag and the first
`ComponentClassName` / `StrProperty` tag. Both envelopes passed independent length,
Base64, zlib and SHA-1 checks first. No private field values were published.

This prefix agreement supports investigating the same framing in the newer saves;
it does not prove the complete newer schema or replace a matched official load.
Tests use authored byte fixtures for headers and opaque values, including unknown
field names. Private saves remain outside tests, source control and CI.

A subsequent independent Node scan and the release Rust reader reached the root
terminator at exact EOF on both copies. Each contains six root tags and 378 header
and terminator bytes. Opaque payload totals are 18,512,856 bytes for the current
copy and 18,492,297 for the selected backup. These are byte/record measurements,
not counts of gameplay objects. Envelope-only output remained byte-identical and
both input hashes remained unchanged. Evidence: save-triage E-015/E-016 and
`work/native-tagged-20261012/`; strict case review and artifact hashes pass.

Six reader tests cover all supported headers, GUID variants, opaque embedded
terminators, every truncated prefix, exact record/string limits, invalid sizes and
flags, unsupported kinds and raw wide-name preservation. Fifteen prospect tests
include integrity-before-framing and CLI privacy/no-write checks. The full manual
suite, release build and independent code/static-contract review pass. No original
runtime or stock-client migration test was performed.
