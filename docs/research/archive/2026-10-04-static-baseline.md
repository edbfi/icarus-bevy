> Archived research report, retained verbatim. Current conclusions live in [the ledgers](../../ledgers/findings.md). Moved from `docs/research/` on 2026-10-13.

# Static baseline: build 25621760

Historical baseline — 2026-10-04. Authorization status and unknowns below describe
this initial static batch. Later [startup observations](2026-10-04-offline-startup-report.md)
record the reported engine version. See the
[current checkpoint](../../implementation-status.md) for pending work and pause status.

This report records offline static observations. It does not establish a matched
client build, runtime compatibility, a reproduced failure, or a performance result.

## Scope and evidence

User-approved read-only metadata/import/library/string inspection of the inventoried
shipping executable and build manifest. Case: `work/icarus-server-baseline/`.
Generated JSON, strings and manifest copies remain ignored. Original assets are
not used by the replacement.

| Evidence | Artifact | Observation |
| --- | --- | --- |
| E-002 | `static/server-info.json` | PE32+, Windows, x86-64, image base `0x140000000` |
| E-003 | `static/server-imports.json`, `static/server-libraries.json` | 1,250 imported functions; platform, socket, filesystem and engine dependencies |
| E-004 | `static/appmanifest_2089300.acf` | App 2089300, build 25621760, depot 2089301, depot manifest 363076092815165210 |
| E-005 | `static/selected-strings.json` | 323 selected ASCII/UTF-16 diagnostic anchors with file offsets and virtual addresses |

Executable SHA-256:
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`.
The executable header timestamp is 2026-09-30 07:44:49; it is metadata, not proof
of installation date or release identity. The local Steam manifest provides the
build identifier. Client pairing remains unknown.

## Findings

- The import table includes Steam game-server initialization/callback functions,
  EOS initialization/authentication functions, `recvfrom`/`sendto`, and filesystem
  operations including `CreateFileW`, `FlushFileBuffers`, and `MoveFileW`.
  Imports establish linked interfaces, not which runtime paths execute.
- Library references include PhysX and rendering/audio components. Their presence
  does not establish a dedicated-server graphics requirement or explain crashes.
- Prospect-load and network-version diagnostic strings provide concrete anchors
  for bounded control-flow investigation. Neither error strings nor function names
  prove a defect or describe an entire protocol/save format.
- An incidental plugin string mentions UE4.27.1 requirements. It does not identify
  this executable's engine version; that remains unresolved.

| Candidate path | String address | Next question |
| --- | --- | --- |
| Prospect file-load validation | `0x1449f1550` | Which input validation branches lead to the failure? |
| Prospect deserialization | `0x1449f6660` | What file format and reference resolution are used? |
| Client version rejection | `0x14546d780` | How are local and remote network versions computed and compared? |

Evidence → Finding → Path: E-002 anchors addresses to a hashed build; E-003
identifies external interfaces; E-004 names the local build; E-005 locates diagnostic
sites. The next path is string references → owning functions → callers and data
flow → controlled runtime validation. Every behavioral conclusion remains a
candidate until supported by that investigation.

## String scan limitation and reproduction

The initial `rabin2 -zj` scan returned 16,443 strings from `.data` and `.rsrc`,
without relevant engine/gameplay diagnostics. A whole-file `-zzj` scan returned an
empty array. Neither result establishes absence of strings. A read-only Python
scan of ASCII and UTF-16 strings located the anchors instead; PE section mappings
translate file offsets to virtual addresses.

The reusable equivalent is `tools/pe_string_anchors.py`. Like other research tools,
its execution must be included in an approved batch. It requires an existing case
directory and a new output filename, rejects invalid PE32+ headers, and never writes
the original executable. Do not copy extracted strings or decompilation into
replacement source or content.

## Remaining gates

- Bounded decompilation: [proposed batch](../../history/decompilation-authorization.md).
- Dynamic reference runs, packet capture, crash reproduction and stock clients:
  not yet authorized or performed.
- Asset independence and full-world compatibility: unresolved feasibility gate.
- The original server's reported bugs remain unreproduced.
