# Synthetic durability workload

`synthetic_soak` uses Bevy's fixed schedule and asynchronous writer to mutate
authored entity positions, then reopens its fresh store and checks the full snapshot.

```bash
cargo run --release --locked --example synthetic_soak -- \
  --data-dir work/soak-new --duration-seconds 7200 --entities 100000 --save-seconds 10
```

JSON output covers up to 259,200 seconds (72 hours). Tick timings exclude
snapshot capture and the outer loop. Save latency spans submission to observation,
including polling delay. Fixed-memory histograms count every sample: ticks at 1 µs through 131,072 µs,
saves/captures at 100 µs through 10 s. Quantiles use rank
`floor((n - 1) * percentile / 100)`, rounded down to resolution; out-of-range
ranks yield `null`, while maxima are exact.
`--warmup-seconds` (default 600) selects the first RSS baseline sample.
`rss_growth_after_warmup_percent` ends before recovery verification; only final
`rss_kib` includes its allocations. `rss_slope_kib_per_hour` fits all post-warm-up
status samples by least squares. Record affinity, commit, binary hash, host
resources and concurrent workloads.

`--compressed` uses simulated time: one fixed step per unpaced update, saves
every `--save-seconds`, awaiting in-flight saves (`save_waits`). It executes
exact tick/save counts as fast as the host allows. Warm-up, status and growth
use simulated time. `--start-tick` exercises long-uptime counters.

```bash
cargo run --release --locked --example synthetic_soak -- --compressed \
  --data-dir work/soak-new --duration-seconds 259200 --entities 100000 --save-seconds 10
```

Compression covers operation counts, retained growth per operation and counter
ranges. It excludes production-loop wall-clock autosaves, peer idle timeouts,
deadline pressure between ticks and storage/page-cache/host effects over days;
pair it with a short paced run.

Compatibility and official-server speedups require the
[plan](../linux-dedicated-server-plan.md)'s heavy ICARUS worlds.

Synchronous persistence:

```bash
cargo run --release --locked --example snapshot_bench -- \
  --data-dir work/bench-new --entities 100000 --iterations 20
```

The benchmark makes two warm-up commits, times save/load and checks all recovered
state. Repeat fresh runs at matched population, affinity and build settings;
retain baseline binaries and hashes under ignored `work/`.
`/usr/bin/time -v` includes verification allocations: its peak RSS must not be
labeled steady-state server memory.

For schemas 7–9, `native_lifecycle_bench` measures capture, preflight, storage
and owner actions, verifying full state outside each timer. Create the case
directory first; its store child must be absent:

```bash
cargo run --release --locked --example native_lifecycle_bench -- \
  --data-dir work/CASE/new-store --operations preflight --slots 3,64,256,1024
```

Run each operation separately within the shared 20s limit to avoid starving
later ones. Defaults: three repetitions, five warm-ups, 50 samples. JSON retains
raw times and incomplete markers. Enforce external CPU/memory/deadline limits.
Action times include capture/preflight; load reuses the open store. Compare only
complete cases with matched fixtures, builds and affinity.

Recovery alternates SIGKILL/SIGTERM near autosaves after a commit. The default
10,000-entity fixture checks checksums, clock and ACKs. Use fresh output and retain
provenance/logs; no mode qualifies I/O fault points, disk exhaustion or physical
power loss.

```bash
python3 tools/native_recovery_campaign.py --binary target/release/icarus-server \
  --output work/recovery-new --cycles 24
```

Schema14 fixture:

```bash
python3 tools/cargo_guard.py build --locked --example authored_player_recovery
python3 tools/native_recovery_campaign.py --mode authored-player \
  --binary target/debug/examples/authored_player_recovery \
  --output work/CASE/new-campaign --cycles 4
```

The 7 Hz schema14 fixture covers active/detached players (F-1183). Fresh Rust
verification checks typed state and eight resumed steps without downtime debit.
Identity/collection/exit failures, 20s child deadlines or the 30 GiB disk floor
deny success. Retain receipts; no production-loop/client qualification.

Schema15: `authored-player-inventory` checks sparse bags, conservation, revisions
and allocation;
`authored-player-consumables` checks format-2 Use, coupled vitals/bag changes,
stale retries and fractional restart.
`authored-player-depletion` adds health loss; each cycle needs new Use and damage ACKs.
All use a literal oracle and the exact
writer's leave cutoff. Consumable cycles wait for a new Use-bearing ACK before
arming interruption. Bounds: 1–4 cycles, 240s workload, 64 MiB output,
3 GiB available-RAM start floor; apply external CPU/memory limits.
F-1200: M-1516 covers four raw-inventory cycles; M-1519 covers four Use cycles.
Shared decoder; no production/client, power-loss or performance qualification.
