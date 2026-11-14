# Controller receive diagnostics

Source contract for qualified diagnostics builds; these counters establish no
stock-client execution, geometry, world entry or compatibility (F-1045, U-038).

## Outputs and lifetime

Production `status` and `stopped` records include `controller_receive`: cumulative
metadata since this network bind, including departed and replaced peers. Each
observation updates a lifetime table; removal and teardown keep it unchanged. Repeated snapshots
overlap: do not add them together. `null` means networking was never installed.

Probe `controller_fields` (final `controller_fields_before_shutdown`) and library
`controller_field_totals`/`controller_field_diagnostics` retain live-peer semantics:
removal drops their rows. Their absence after departure is not a historical claim.

## Bounds and parser metadata

`controller_receive.fields.entries` keeps the first 64 distinct rows, keyed by
`target`, `index`, `payload_bits` and `disposition`, with a `count`. Targets and
dispositions are fixed enums; all other values are counts or numeric metadata.
There are no player IDs, addresses, GUIDs, names, profile values, passwords,
payloads, sequences or timestamps. Counts and overflow saturate.

`fields.overflow` counts row occurrences that could not fit. With overflow,
an absent Begin row (actor field 124) is inconclusive: its key may have been
discarded. Unknown subobjects and parse failures also limit absence claims.

`fields.body_parse_failures` counts actor-body failures. If a declared block
fits the available bits but exceeds the cap, `block_payload_limit_rejections`
increments and `max_rejected_block_payload_bits` retains the largest declaration.
Lengths beyond available bits increment `block_payload_length_rejections`.
`unknown_subobject_blocks` counts skipped blocks without identifying their source.
`malformed_bunches` counts failed controller parses (F-1045).

The outer counters summarize packet acceptance/rejection/malformation, actor
bunches ignored/dropped/malformed, partial parts/completions/rejections,
reliable duplicates and ignored controller fields. Partial completion precedes
domain validation. These transport totals also cover non-controller traffic;
they do not identify an RPC or its caller.

## Recognition and application

A `Recognized` field row records successful parsing of that field, not application
of its call. A malformed tail may leave a recognized Begin row while applying
none of the bunch. Reliable duplicates do not classify or apply that call twice.

`sessions.initialisation_slots_recorded` counts accepted Begin slots once the
PlayerState is available, including a retained Begin processed after its open.
It is not a count of parsed fields, acknowledgements or completed world entries.
Compare it with field 124 metadata, malformed/overflow counts, player phases
and request/publication counters. Settled opens or possession alone do not
establish Begin or client startup (F-1181).

`sessions.profile_{user,character}_responses_recognized` counts parsed profile
calls dispatched after whole-bunch validation, including failure/wrong-phase
calls. Wrong phase also increments `profile_responses_ignored`; malformed bunches
apply none. `profile_{user,character}_publications_deferred` counts attempts held
behind drainable pressure, not extra ingress. `profile_{user,character}_responses_received`
counts successful PlayerState publication admission, not RPC ingress. Zero alone
does not prove no response; skipped unknown-subobject blocks are unattributed.
