# Cargo builds

```bash
python3 tools/cargo_guard.py build --locked
python3 tools/cargo_guard.py clippy --workspace --all-targets --locked -- -D warnings
python3 tools/cargo_guard.py --hold-target -- target/debug/icarus-server check-config --config config/server.example.toml
```

Supports `check`/`test`, filters, release, no-run, JSON/quiet and trailers.
Two jobs; unchanged profiles. Checkout-local targets; serialized builds.
Preserve guard locks/state. `--hold-target` leases exclude cleanup for bounded
short-lived commands (30 minutes maximum). Servers, soaks and `cargo run` need a separately scoped, bounded procedure;
they do not cooperate with leases. Process detection is best effort, not
cleanup protection.

Legacy targets need `--inventory`, then reviewed exact-path
`--adopt-target /absolute/checkout/target`; next build fully retires them using
Cargo. `--retire-target` requests retirement. Failed artifacts stay reusable; interrupted cleanup retries. Generation changes
retire dependencies too. Other caches: reports.

Own cap: 10 GiB (provisional); growth reserve: min(8 GiB, remaining cap), minimum
2 GiB headroom. Local disk floor: 30 GiB. CI explicitly sets
`ICARUS_CARGO_GUARD_POLICY=ci` for a 4 GiB floor; `CI=true` changes nothing.
Calibrate budgets. Contracts: [guard contracts](../../tools/cargo_guard.py).

Local measurements: M-1184.
