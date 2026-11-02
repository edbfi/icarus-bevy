# Cargo builds

Requires Git and pinned Python 3.15.

```bash
python3 tools/cargo_guard.py build --locked
```

Supports `check`/`test`, filters, release, no-run, JSON/quiet and trailers.
Two jobs; unchanged profiles. Checkout-local targets; serialized builds.
Preserve guard locks/state. `--hold-target -- COMMAND` leases exclude cleanup for bounded
short-lived commands (30 minutes maximum). Servers, soaks and `cargo run` need a separately scoped, bounded procedure;
they do not cooperate with leases. Process detection is best effort, not
cleanup protection.

Legacy targets need `--inventory`, then reviewed exact-path
`--adopt-target /absolute/checkout/target`; next build fully retires them using
Cargo. `--retire-target` requests retirement. Failed artifacts stay reusable; interrupted cleanup retries. Adding, removing or renaming a target root also
forces a cold rebuild, including dependencies. Add ordinary tests as domain
modules to retain warm reuse.

Own cap: 10 GiB (provisional); growth reserve: min(8 GiB, remaining cap), minimum
2 GiB headroom. Local disk floor: 30 GiB. CI explicitly sets
`ICARUS_CARGO_GUARD_POLICY=ci` for a 4 GiB floor; `CI=true` changes nothing.
Contracts: [guard contracts](../../tools/cargo_guard.py).

Local measurements: M-1184.
