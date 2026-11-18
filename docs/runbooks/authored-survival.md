# Authored survival

Authored [players](../architecture.md#persistence), schemas 14–16.
Rules default absent; F-1183/F-1200 cover 14/15.

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

Integer need units/s: each 0..=10000000, ≥1 positive; clamp at zero.
`[network.player_depletion_damage]` requires consumption/checkpoints; integer
oxygen/water/food health rates share these bounds. Sum deltas for pre-batch empty
needs; clamp health at zero. Newly empty needs start next tick; prior Use
prevents replenished-need damage, never heals. No death/respawn/Use gate; maxima/placement/biome/growth/bags/allocators
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
Schemas 15/16 preflight the full generation at `t+1`; direct APIs retain current
tick. Overflow/failure refuses mutation. Projections follow Finish;
polling/pressure/NAKs never replay effects. Saved tick owns day/fractional phase;
no wall-time catch-up/extra accumulator. Offline ticks advance day, charging no
detached player. Restored sampling precedes Join/open/update; authored and legacy
monotonic clocks exclude each other.

Before fixed ticks, polling can use a newly freed send slot to carry the ACK
with already staged, changed pawn vitals for a current, non-leaving epoch
(F-1261). Bag-only revisions and new tick output follow the projection pass.

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

## Saved locations

`network.checkpoint_saved_locations` (default false; needs player persistence
and `pawn_move_acks`; F-1206) writes the last accepted move location into the
checkpoint at the leave cut-off, every 10 s and at teardown. It saves only while
the pawn's position descends from the stored one (new checkpoint, pawn opened
there, or relocation applied); otherwise the save is withheld and the stored
location kept. `network.checkpoint_location_correction` (default false; needs
`checkpoint_pawn_before_identity` and `pawn_move_acks`; F-1207) moves a
returning player's live pawn to the stored location plus 50 cm with a Falling
movement correction answering a client move: at most one per second, five
attempts or 30 s, applied when a move reports within 300 cm; the frozen release
waits at most 5 s. [Counters](network-readiness.md):
`sessions.checkpoint_saved_locations` and `sessions.checkpoint_relocation`.
`--private-leave-proof` refuses both.

## Player inventory

`network.player_inventory` selects fresh schema 15/section 1; omission: schema 14.
True `network.player_inventory_transfers` requires recipe/checkpoints and fresh
schema 16/section 2. False/default keeps legacy bytes/digests. No upgrade,
downgrade, reset or migration; policy/recipe/context changes refuse reopen.
`check-config` checks shape; `run` prepares recipes before state. Recipe 1:
format/items/inventories/owner-free starter. Recipe 2 adds nonempty consumables
(item/oxygen/water/food); gains are integers 0..=10000000, ≥1 positive/item.
Unknown/duplicate items refuse. Example: `config/player-inventory-consumables.example.json`.
Canonical item/inventory/eligibility/effect order binds identity; starter order
binds routing. Recipe 2 Use and section 2 accounting are independent versions.

Local move/split/merge/consume/place_overflow remain. Recipe 2 use takes only
source, consumes one on change and caps oxygen/water/food at maxima. All-full
Use validates source/CAS: exact NoChange without increment headroom. Consume
has no vital effect; health never gates Use. Distinct player/bag CAS, aggregate
revision and global stack IDs (F-1200); world preflight, no registry bypass/private
proof. Section 2 keeps sparse cumulative received/sent/consumed per owner:
starter+received=contents+consumed+sent; global received=sent. Consumption stays
with its consumer; flows never net. Leave pins owner bag/flows/mode/context,
excluding global revision/allocator; unrelated transfers remain settleable.

`network.player_inventory_projection = "BINDING.json"` (default absent; needs
player persistence and `player_inventory`; F-1202) sends each owner's bag to its
own client: bound inventories open empty, then slot deltas precede the record.
The binding names pak rows per recipe key (example:
`config/player-inventory-projection.example.json`); unknown, duplicate or
ambiguous rows, changeable slot counts and overflow refuse at startup.

Format 1 omits `unit_weight` and sends slots only. Format 2 requires an
independently authored integer `unit_weight` from 0 to 2,147,483,647 for each
item. Each inventory's `CurrentWeight` is the sum of unit weight times stack
count. Startup rejects a binding if the sum of the heaviest permitted stack in
each slot exceeds 2,147,483,647. Each reliable page carries a changed total
for the slots it leaves on the client, including explicit zero after a
nonzero total. Unchanged totals are omitted. A split update can expose
intermediate totals until all its slots arrive.

The binding is session-only and can be changed across restarts of the same
player store. Client inventory actions are unhandled. Returning-pawn inputs and
`--private-leave-proof` refuse both formats.

Stopped server: `apply-player-inventory` or `apply-player-inventory-transfer`,
with `--data-dir STATE --request PRIVATE.json [--dry-run]`. Requests: named JSON
objects, regular files ≤64 KiB, no symlink components; unknown/duplicate/positional
fields refuse. Local fields: player_id/expected_player_revision/expected_bag_revision/action.
Endpoints: owner {player_id, incarnation:1}, inventory, stack_id or slot;
local overflow slot null appends. Transfers require source/destination/positive
amount/source_cas/destination_cas; both CAS: expected_player_revision/
expected_bag_revision. Only section 2 transfers: normal/overflow source to another
owner's empty normal slot. Whole retains ID; partial allocates one; both player/
bag revisions +1, aggregate +1, no NoChange. Live calls need two independently
held key/epoch authorities after queued-auth reconciliation; IDs grant none.

Lock spans load/save; Applied saves once. Dry-run/NoChange/rejection preserve
bytes. Success prints SaveReceipt after publication, errors none; reopen before
uncertain-write retry. Caps: 4 MiB recipe, 16 MiB section, 65536 stacks, 64 MiB
generation/graph. Eight copies/128 baseline pins; section 2 also reserves pin
roots/accounting scratch/64 KiB requests. Admission may fail below caps.
No stock RPC/consent/trade/import/player removal or client proof.
