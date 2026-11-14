# Controller receive diagnostics

Lifetime metadata is authored-qualified (F-1182); a protected stock-client stall
is observed (F-1184). Counters establish no client call execution, geometry,
world entry or compatibility (F-1045, U-038).

## Outputs and lifetime

Production `status` and `stopped` expose `controller_receive`, cumulative since
network bind across departures/replacements and teardown. Snapshots overlap;
never add them. `null` means networking was never installed.

Probe `controller_fields` (final `controller_fields_before_shutdown`) and library
`controller_field_totals`/`controller_field_diagnostics` remain live-peer views:
removal drops their rows, so absence after departure proves no historical absence.

## Bounds and parser metadata

`controller_receive.fields.entries` retains the first 64 distinct keys:
`target`, `index`, `payload_bits`, `disposition`, with occurrence `count`.
Targets/dispositions are enums; other values are numeric. No player IDs,
addresses, GUIDs, names, profiles, passwords, payloads, sequences or timestamps
are exposed. Counts and overflow saturate.

`fields.overflow` counts discarded row occurrences. Absent Begin (actor field
124) is inconclusive with overflow, unknown-subobject skips or parse failures.
`body_parse_failures` counts actor-body failures; `malformed_bunches` counts
failed controller parses. A block fitting available bits but exceeding the cap
increments `block_payload_limit_rejections`; `max_rejected_block_payload_bits`
retains the largest declaration. Length beyond available bits increments
`block_payload_length_rejections`. `unknown_subobject_blocks` counts skipped
blocks without identifying their source (F-1045).

Outer counters cover packet acceptance/rejection/malformation, ignored/dropped/
malformed actor bunches, partial parts/completions/rejections, reliable duplicates
and ignored controller fields. Partial completion precedes domain validation.
Transport totals include non-controller traffic and identify no RPC or caller.

## Recognition and application

`Recognized` means a field parsed, not that its call applied. A malformed tail
can retain a Begin row while applying none of the bunch. Reliable duplicates
neither classify nor apply the call twice.

`sessions.initialisation_slots_recorded` counts accepted Begin slots once
PlayerState is available, including a retained Begin processed after its open.
It counts neither parsed fields, ACKs nor world entries. Compare actor field 124,
malformation/overflow, player phases and request/publication counters; settled
opens/possession alone establish no client startup (F-1181).

`sessions.profile_{user,character}_responses_recognized` counts parsed calls
dispatched after whole-bunch validation, including failure/wrong-phase calls.
Wrong phase also increments `profile_responses_ignored`; malformed bunches apply
none. `profile_{user,character}_publications_deferred` counts attempts held by
drainable pressure, not extra ingress. `profile_{user,character}_responses_received`
counts successful PlayerState publication admission, not ingress. Zero alone
does not prove no response; skipped unknown-subobject blocks remain unattributed.

## Skipped-block and drop metadata

Authored-tested (F-1182): `fields.skipped_blocks` retains at most 16 rows
with `relation` (`ConfiguredControllerComponent`, `Unmapped`, `MappingOverflow`),
optional u8 `component_ordinal`, `has_rep_layout`, `payload_bits` and `count`;
`fields.skipped_blocks_overflow` counts excess occurrences. A private mapping
retains the first 64 committed component GUIDs; missing/overflowed mapping reports
`MappingOverflow`. Ordinals/relations establish no runtime ownership; GUIDs stay
private. Bounded numeric/enum metadata preserves parsing and dispatch.

`actor_bunch_drop_reasons` classifies existing drops as `unreliable`,
`reliable_queue_refusal`, `unsupported_header`, `partial_policy` or
`partial_reassembly`. Classification accompanies the existing drop total;
reason counts saturate independently. Connected interpretation remains unqualified.

## Connected observation and stopping

A skipped block leaves Begin absence inconclusive (F-1184). A later protected run
accepted Begin and reached entry/locomotion (F-1185); checkpoint and normal-leave
qualification remain open (F-1186).

If Cancel is absent, use Alt+F4 and confirm client closure before the owned
server stop. Supervisor 130 denotes that intentional stop, not baseline
completion (M-1418). An unloaded unit's default ExecMainStatus 0 is not a
native exit receipt.
