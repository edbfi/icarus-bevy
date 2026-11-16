# Authored survival

Authored-only F-1183/F-1200: schema-14/15 [players](../architecture.md#persistence);
rules default absent.

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

Rates: integer vital units/s, each 0..=10000000; ≥1 positive.
Needs clamp at zero. Optional `[network.player_depletion_damage]` needs
consumption/persistence; integer `oxygen`, `water`, `food` health rates
share those bounds. Sum tick deltas for pre-batch empty needs; clamp health at
zero. Newly empty needs start next tick. Prior Use prevents damage from
replenished needs; it never restores health. No death/respawn/Use gate; maxima/placement/biome/growth/bags/allocators
unchanged. Private proof rejects damage.
Clock: minute 0..=1439, positive i32 seconds/day; server-matching `tick_hz` 1..=240.
Clock binds world input; rates/frequency bind declaration. Damage uses a distinct
domain; omission preserves legacy digests. Rule changes refuse reopen without
reset/migration. Release changes are provenance.

## Activation and time

Authenticated activated attachments only; pending/revoked/detached
players incur nothing. Bounded pre-batch auth drain only detaches ineligible
epochs; later events wait for next cut.

Tick `t`: `floor((t+1)*r/hz) - floor(t*r/hz)` debit/damage. One atomic batch/revision
follows Gameplay, before Finish; unchanged currents use none.
Schema 15 preflights the full generation at `t+1`; direct APIs retain current
tick. Overflow/failure refuses mutation. Projections follow Finish;
polling/pressure/NAKs never replay effects. Saved tick owns day/fractional phase;
no wall-time catch-up/extra accumulator. Offline ticks advance day, charging no
detached player. Restored sampling precedes Join/open/update; authored and legacy
monotonic clocks exclude each other.

## Failed health

Owner/Fixed-timestep checks precede Time/polling/batches, even offline without a
clock. Failure stops admission, refuses Finish/capture/publication and fails
the runner; reinsertion cannot clear it. Diagnose before restart. Auth-backend
loss closes peers/detaches epochs before batches; committed state stays capturable
for shutdown.

## Checkpoint leave

F-1188: `request_leave` freezes active key/epoch/revision/location/vitals; no
survival/edits/reattachment. Limit: 64 operations, including detached/failed/durable
barriers; ≤60 monotonic seconds/attempt. Capture/storage failure or timeout needs
`retry_leave`; old attempts cannot complete it. Worker loss is terminal; timeout
never cancels or rolls back I/O. Poll `leave_status`; `settle_leave` needs
durability and original detachment.

`network.authored_controller_leave` defaults false; needs qualified
codec/controller/checksum and checkpoints; declarations grant no dispatch.
Quit (1), CharacterSelect (2), Titlescreen (4) cut off before fixed ticks, even
zero-tick updates. Missing/None, dropship, disconnected, no-save do nothing.
Missing projection/refused cutoff drops intent; no retry. Duplicates keep
first type/cutoff/deadline.

Matching writer receipt alone permits reliable completion; pressure/loss retains
it. Entire-prefix ACK allows detach, excluding later traffic. Connection/epoch
checks prevent redirection after revocation/reuse. Detached durable barriers settle;
pending/failed remain. Pre-admission Cancel suppresses completion, keeping the
frozen obligation; admitted traffic cannot retract. Failed/suppressed leaves stay
connected/frozen until disconnect/revocation/expiry, without automatic retry,
failure-close or resume. ACKs can prolong freezing. Transport ACK/authored tests
prove neither full player-save nor client menus.

## Player inventory

Set `network.player_inventory` before creation; omitted: schema 14.
`check-config` checks shape; `run` loads recipes. Recipe changes need fresh storage,
without migration. V1 requires `format`, `items`, `inventories`, owner-free
`starter`; V2 adds nonempty `consumables` requiring item/oxygen/water/food.
Authored gains: integers 0..=10000000, ≥1 positive/item; unknown/duplicate items refuse.
Example: `config/player-inventory-consumables.example.json`. Canonical
item/inventory/eligibility/effect order binds identity; starter order binds routing.
V1 bytes/digest unchanged.

Player/bag CAS, aggregate revision and stack IDs differ (F-1200). World wrappers
preflight edits/activation; registry bypasses/private proof refuse. Leave pins record/bag.

With the server stopped:

```bash
icarus-server apply-player-inventory --data-dir STATE --request PRIVATE.json --dry-run
icarus-server apply-player-inventory --data-dir STATE --request PRIVATE.json
```

Named JSON objects only; unknown fields refuse. Required: `player_id`,
`expected_player_revision`, `expected_bag_revision`, `action`.
Actions: move/split/merge/consume/place_overflow; format-2 `use` takes only `source`.
Endpoints: matching `owner: {"player_id": ID, "incarnation": 1}`, inventory,
stack_id or slot; overflow slot null appends. Use atomically consumes one item,
capping oxygen/water/food at maxima. All-full Use checks source/CAS then returns
NoChange without consumption/revisions. Raw consume has no vital effect;
health never gates Use.

Requests: regular, ≤64 KiB, no symlink components. Lock spans load/save; only
Applied saves. Dry-run/NoChange/rejection preserve bytes. Success prints
SaveReceipt, errors none. Reopen before retrying uncertain writes.

Caps: 4 MiB recipe, 16 MiB encoded inventory, 65,536 stacks, 64 MiB generation.
Graph charge reserves eight copies/128 baseline pins; admission may fail below caps. No stock dispatch/import/trade/removal.
