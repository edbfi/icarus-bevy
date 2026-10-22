# ICARUS Bevy server research

An independent native Linux dedicated-server project in Rust and Bevy.
**This currently runs a headless foundation, not an ICARUS-compatible server.**
Networking, authentication, gameplay, original-world compatibility and
official-save import remain unimplemented and unvalidated. Offline research has resumed; current results and remaining gates are in the
[checkpoint](docs/implementation-status.md#summary).

## Quick start

Requires native Linux and Rust 1.99.1 (pinned with Rustfmt and Clippy). Bevy
0.20.0 runs headless; no GPU, display, original game files or Proton are needed.

```bash
cargo build --release --locked
target/release/icarus-server check-config --config config/server.example.toml
target/release/icarus-server run --data-dir work/smoke-world --max-ticks 10
```

The [native server runbook](docs/runbooks/native-server.md) covers configuration,
persistence, recovery and save inspection.

**Planned install requirement:** like the official Windows server, a compatible
server will need basic data (such as data tables) that ships with the free
official ICARUS dedicated server (Steam app 2089300). Run
`icarus-server install-official-data --data-dir DIR` once (and after official
updates): it downloads those files anonymously from Steam without steamcmd or
prompts. Nothing original is bundled with this repository.

## Documentation

| Read | For |
| --- | --- |
| [docs/README.md](docs/README.md) | map of every document and its rules |
| [Plan](docs/linux-dedicated-server-plan.md) | goal, constraints, milestones, release targets |
| [Status](docs/implementation-status.md) | summary, milestone state, next tasks |
| [Gates](docs/gates.md) | preconditions that block actions |
| [Ledgers](docs/ledgers/) | findings by area, unknowns, bugs, compatibility matrix |
| [Specs](docs/specs/) | save-format and network facts |
| [Research](docs/research/) | one digest per question cluster |
| [Runbooks](docs/runbooks/) | server, synthetic workload, original-server launcher, REA tooling, research helpers |

## Contributing

```bash
prek install
prek run --all-files --hook-stage manual
```

Rules for agents and people are in [`.agents/rules/`](.agents/rules/): project
workflow in `project.md`, documentation in `docs.md`. Rust checks use `--locked`;
Python helpers pass basedpyright in recommended mode; commits are signed-off
Conventional Commits on feature branches. CI runs the same hooks plus a release
build and headless smoke test with authored fixtures only.

Original files stay under ignored `icarus-orig/`, supplied saves under ignored
`reference-data/`, and generated analysis under ignored `work/`. No original
assets are extracted or reused. Historical research approvals are indexed in
[docs/history](docs/history/authorizations-index.md); none extends a runtime window.

Project source uses the repository's [AGPL-3.0 license](LICENSE); third-party
dependencies and vendored tools retain their own licenses.
