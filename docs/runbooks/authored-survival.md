# Authored survival

Survival F-1183, inventory F-1200; authored-only. Requires schema-14/15
[player setup](../architecture.md#persistence). Both rules default absent.

## Configuration and reopening

```toml
[network.player_consumption]
oxygen = 1
water = 1
food = 1

[network.authored_world.clock]
initial_minute = 480
seconds_per_day = 3600
tick_hz = 30
```

Rates: integer vital units/second, each 0..=10000000, at least one positive.
Oxygen/water/food clamp at zero; other vitals, maxima, placement, biome and growth
stay unchanged.
Clock: minute 0..=1439, positive i32 seconds/day, server-matching `tick_hz` 1..=240.
Clock binds world input; rates/frequency bind the declaration
([contract](../architecture.md#persistence)). Changing/enabling/disabling bound
rules refuses reopen; no reset/migration. Release changes remain provenance.

## Activation and time

Only authenticated activated attachments consume; pending/revoked/detached players
incur nothing. A bounded pre-debit auth-event drain detaches ineligible epochs;
later events wait for the next cut. The cut never activates or publishes.

Completed tick `t` debits `floor((t+1)*r/hz) - floor(t*r/hz)`. The batch is atomic
after Gameplay/before Finish; projections follow successful Finish. Polling,
pressure retries and NAKs never repeat debit. Unchanged currents use no revision.
Saved tick owns day/fractional phase; restart has no wall-time catch-up or second
accumulator. Offline ticks advance day; downtime charges nothing. Restored sampling
precedes Join/open/update; authored and legacy monotonic clocks exclude each other.

## Failed health

Owner/Fixed-timestep checks precede Time/polling and every debit, including offline
without a clock. Failure latches: stop admission, refuse Finish/capture/publication,
fail the runner. Reinsertion cannot clear failure; diagnose before restart.
Auth-backend loss closes peers/detaches epochs before debit; committed state remains
capturable for shutdown.

## Checkpoint leave

F-1188: `request_leave` freezes active key/epoch/revision/location/vitals, blocking
consumption/edits/reattachment. The 64-operation cap includes detached/failed/durable
barriers; attempts last ≤60 monotonic seconds. Capture/storage failure or timeout
needs `retry_leave`; old attempts cannot complete it. Worker loss is terminal.
Timeout never cancels/rolls back I/O. Poll `leave_status`; `settle_leave` requires
durability and original detachment.

Default-false `network.authored_controller_leave` separately enables a qualified
codec/controller/checksum plan with checkpoints. Codec declarations alone never
enable dispatch. Quit (1), CharacterSelect (2)
and Titlescreen (4) cut off before fixed ticks, including zero-tick updates.
Missing/None, dropship, disconnected and no-save do nothing. Missing projection
or refused cutoff drops intent without delayed retry. Duplicates keep the first
type/cutoff/deadline.

Only the matching writer receipt permits reliable completion; pressure/loss retains
it. Entire-prefix ACK permits detach, excluding later traffic. Connection/epoch
checks prevent redirection after revocation/reuse. Detached durable barriers settle
automatically; pending/failed remain. Pre-admission Cancel suppresses completion,
retaining the frozen obligation; admitted traffic cannot retract.
Failed/suppressed leaves stay connected and frozen until disconnect/revocation/expiry,
with no automatic retry, failure-close or resume. Continued ACKs can keep them frozen.
Transport ACK and authored tests prove neither full player-save nor client menus.

## Player inventory

Set `network.player_inventory = "authored-bags.json"` before store creation.
`check-config` is shape-only: it neither loads nor validates this recipe; `run` does.
Omission retains schema 14; changed/enabled/removed recipes refuse mismatched
stores. Strict JSON format 1: `items` (key/stack_limit), `inventories` (slot rules),
owner-free `starter` (inventory/requested_slot/item/count). Use independent values.
Canonical item/inventory/eligibility order binds the digest; starter order binds routing.

Schema 15 issues once per sparse player ID; detach/reconnect retains bags and each
player's starter conservation/consumed totals. Player/bag CAS, aggregate revision
and monotonic stack IDs are separate. Exclusive world wrappers preflight all live
edits/activation; registry bypasses and private proof refuse. Leave pins only that
player's exact state while others mutate.

With the server stopped:

```bash
icarus-server apply-player-inventory --data-dir STATE --request PRIVATE.json --dry-run
icarus-server apply-player-inventory --data-dir STATE --request PRIVATE.json
```

Request and owner must be named-field JSON objects, never positional arrays;
unknown fields refuse. Request: `player_id`, `expected_player_revision`,
`expected_bag_revision`, `action`. Actions: move/split/merge/consume/place_overflow.
Endpoints name matching `owner: {"player_id": ID, "incarnation": 1}`,
inventory and stack_id or slot.
Overflow slot null appends. Private regular requests: ≤64 KiB, no symlink components.
Existing-store exclusive lock spans load/save; only Applied saves. Dry-run/NoChange/
rejection preserve bytes. Success prints SaveReceipt; errors print none.
Reopen before retry: publication can be uncertain.

Caps: 4 MiB recipe, 16 MiB encoded inventory, 65,536 stacks, 64 MiB generation.
The 64 MiB inventory charge reserves eight aggregate copies/128 baseline pins;
admission can fail below logical caps. No stock dispatch, import, trade or removal.
