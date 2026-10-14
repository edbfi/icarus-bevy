> Archived research report, retained verbatim. Current conclusions live in [the ledgers](../../ledgers/findings/). Moved from `docs/research/` on 2026-10-13.

# Bounded decompilation result

Historical report — 2026-10-04. The later
[targeted analysis](2026-10-04-targeted-static-report.md) recovered selected
references without changing this unsuccessful result. See the
[current checkpoint](../../implementation-status.md) for pending work and pause status.

The approved Ghidra pass did not recover any of the selected functions. Its
automatic analysis reached the 180-second limit; most analysis time was spent
disassembling entry points. The database was saved, but a successful process exit
does not establish completed analysis. No save or negotiation specification can
be derived from this pass.

## Scope and reproduction

The [approved command](../../history/decompilation-authorization.md) used Ghidra 12.1.4,
Java 21, two analysis CPUs and the local shipping executable identified in the
[static baseline](2026-10-04-static-baseline.md). PDB Universal was disabled;
the import loaded no additional library files. No original execution or asset
extraction was part of this batch.

Local scope: `work/icarus-server-baseline/scope.md`. Evidence, the database and
decompiler artifacts remain ignored. Reproduction requires that authorized local
sample and the exact documented command; preserve this case before any rerun.

## Evidence and finding

| Evidence | Case-local artifact | Observation |
| --- | --- | --- |
| E-006 | `ghidra-analysis.log` | Analysis timed out; export count zero; database saved |
| E-007 | `ghidra-export/references.tsv` | All three selected anchors have unresolved references |

SHA-256 values and observation times are recorded in the respective local
`evidence/E-006.md` and `evidence/E-007.md` records. The source executable's hash
remains the one in the static baseline.

F-001 (severity `n/a_re`, category `other`, status `candidate`, confidence high):
the current analysis is insufficient to identify callers of the selected save
and version diagnostic strings. Evidence: E-006 and E-007; location: the three
addresses passed to the exporter. These artifacts come from one analysis run,
not independent corroboration of original-server behavior. Missing references
must not be interpreted as unreachable code or absence of functionality.

P-001 (`solve`): select diagnostic addresses from static evidence → import with
external symbol analysis disabled → reach the analysis limit (E-006/F-001) →
export unresolved references (E-007/F-001) → retain the database for a targeted
follow-up. No original function call path was recovered.

## Next investigation

Prepare a bounded pass that locates candidate code references and PE runtime
function boundaries before requesting decompilation of specific functions. Check
instruction boundaries and cross-references; byte-pattern matches alone are not
proof. Keep exports private and translate observations into independent behavior
specifications, never replacement source copied from decompiled code.

The local `timeline.md` records the review and `workitems.md` marks the batch
complete with its unsuccessful recovery result. The broader research remains
open; increasing a timeout alone is not evidence of progress.
