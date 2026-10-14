# Sources and verification

## Consolidation decisions

This is one self-contained skill with task-specific references. ECS, UI, assets,
and rendering frequently intersect; separate installable skills would duplicate
version selection and leave routing dependent on which siblings were installed.
Specialized topics are concise decision guides with upstream links, not a bundled
copy of every plugin manual or engine-porting tool.

The four requested repositories informed the coverage and correction audit:

| Source snapshot | Useful coverage | Treatment here |
|---|---|---|
| [gamedev-skills: bevy-ecs](https://github.com/gamedev-skills/awesome-gamedev-agent-skills/blob/d4b0e35550c55ae70bdfcab4ef5a0e94610438a9/skills/other-engines/bevy-ecs/SKILL.md) | ECS introduction, queries, scheduling | Consolidated core concepts; verified API destinations independently rather than repeating rename dates |
| [bfollington/terma: bevy](https://github.com/bfollington/terma/blob/75e1bac2317daecf42f5403f2e37c9b3bc434994/plugins/tsal/skills/bevy/SKILL.md) | Feature plugins, component design, UI, iteration pitfalls | Rewrote current guidance; removed unconditional dynamic-linking policy, entity-count performance thresholds, and mandatory subagent workflow |
| [sickn33: bevy-ecs-expert](https://github.com/sickn33/agentic-awesome-skills/blob/8bff3af5d2173c90b9ba41a3fe4bcb91bcb64305/skills/bevy-ecs-expert/SKILL.md) | Required components, query filters, resource access | Corrected old time API, incomplete examples, and the explanation of query conflicts |
| [chrisgliddon/bevy-skills](https://github.com/chrisgliddon/bevy-skills/tree/b1b4da5744ebbd5c526342b2351967411cd5ca61/skills) | Broad 0.19 engine, migration, testing, input, platform, and integration coverage | Reorganized by task; retained important distinctions, replaced sibling routing with local references, and avoided frozen third-party compatibility claims |

The upstream repositories declare Apache-2.0, CC-BY-SA-4.0, MIT, and MIT respectively.
This package contains newly written instructions and examples based on independently
checked engine behavior; it does not vendor their skill bodies, reference files,
or extraction scripts. Links retain research provenance. The new material uses
this repository's AGPL-3.0 license; upstream material retains its own license.

In particular, the synthesis does not carry forward these misleading shortcuts:

- “Events were replaced by observers”: buffered messages and observer events coexist.
- “A tuple runs top to bottom”: it needs explicit ordering when order matters.
- “Chain systems to fix conflicting queries”: within-system borrowing needs
  disjoint access or `ParamSet`.
- “All bundles were removed”: obsolete engine bundles differ from custom `Bundle`.
- “Never unwrap” or “always split components”: error policy and data shape depend
  on invariants and access, not engine-wide prohibitions.
- “Latest crate/plugin names imply compatible versions”: inspect resolved metadata.

## Primary baseline

Checked on **2026-10-12**:

- [crates.io metadata](https://crates.io/api/v1/crates/bevy), read through the browser:
  latest non-yanked stable **0.20.0**, published **2026-10-08**.
  `cargo info bevy@0.20.0` independently confirms Rust **1.97.1** minimum.
- [Final release](https://github.com/bevyengine/bevy/releases/tag/v0.20.0) and
  [Bevy 0.20.0 Rust docs](https://docs.rs/bevy/0.20.0/bevy/).
- [Release source](https://github.com/bevyengine/bevy/tree/b3bdd75becaffe9a16677cf5bae5bd6d198f052b),
  the `v0.20.0` tag: reviewed features, examples, and prose-only API claims across
  ECS, scheduling, input, assets, rendering, UI, animation, audio, and platforms.
- [0.19 → 0.20 migration guide](https://bevy.org/learn/migration-guides/0-19-to-0-20/):
  reconciled observer generics, pointer events, WESL, button widgets, entity-list
  queries, weak schedule ordering, and opt-in screen-space transmission.

The previous verification on **2026-10-04** recorded **0.19.1** (published
2026-08-13, Rust 1.95.0) and **0.20.0-rc.2** as a prerelease. That was dated
evidence: the final 0.20.0 release now supersedes it. The RC's Rust 1.96.0 minimum
also differs from the final release. New 0.20 guidance applies to that release
line; it does not justify applying 0.20 APIs to a project locked to 0.19.

Versioned official sources take precedence over every researched community skill.
An upstream example may demonstrate one API without supplying a complete product
behavior, such as keyboard navigation or a durable save format.

## Reproduce the executable checks

Verified on **2026-10-12** using Python 3.13.13, `rustc 1.99.0`, Cargo 1.99.0,
and `aarch64-apple-darwin`: **6 core tests passed and all 4 presentation blocks
type-checked** against exact Bevy **0.20.0**, with no skipped Rust blocks or compiler
warnings. The UI example was migrated to `ui_widgets::Button`, `Hovered`, and
`Pressed`; the presentation feature set now includes the widget and picking APIs.
The MSRV was read from package metadata, not separately tested on Rust 1.97.1.

The skill frontmatter validator, repository content contract, and CI's manual-stage
prek checks passed. These hygiene checks do not validate Bevy API behavior.

Historical installation validation on 2026-10-04 used Bun 1.4.2 and `skills` 1.7.0:
`bunx skills add /path/to/this/repository --list` found `bevy`, and an isolated
project install with `--skill bevy --agent codex --copy --yes` preserved all 14
skill files byte-for-byte. Discovery/installation was not rerun for this refresh.

From the repository root, with Python 3.10+ and a suitable Rust toolchain:

```sh
python3 bevy/bevy/scripts/verify-examples.py
python3 scripts/check-content.py
prek run --all-files --hook-stage manual
```

The first command extracts **every Rust fence** in the skill and its references.
Each must carry a `verify: core` or `verify: presentation` marker. It generates
temporary crates with an exact Bevy pin, so the documented examples themselves
are checked rather than separately maintained copies. It leaves Cargo's downloaded
crates and a reusable build cache, but no Cargo project in this content repository.
Use `--target-dir PATH` to relocate that cache or `--core-only` for a faster rerun.

- **Core:** `cargo test` with `std`, `async_executor`, `multi_threaded`, and
  `bevy_state`. Exercises required components/disjoint queries, buffered input,
  messages/observers, deferred command visibility, state cleanup, and fixed time.
- **Presentation:** `cargo check --tests` with the same core features plus
  `2d_api`, `3d_api`, `ui_api`, `bevy_ui_widgets`, `bevy_picking`, `bevy_pbr`,
  and `bevy_world_serialization`.
  Checks the asset loader, 3D setup, UI/widget/text, and glTF animation wiring.

The repository content checker only discovers Git-indexed skill files. When
validating a newly created skill before staging, include its files in a temporary
Git index or use a separate structural check; a success count that omits the new
skill is not evidence that it passed.

These checks do **not** run a GPU window, load the example font/glTF files, exercise
an audio device, compile third-party plugins, validate all WESL/WGSL paths, or launch a
browser. Platform and specialized workflow advice is source-reviewed, not a claim
that every integration was executed. Keep those distinctions in user-facing reports.

## Refreshing the baseline

Resolve the latest stable release and MSRV again, read intervening migration
guides, and run `verify-examples.py --bevy VERSION`. Update failing examples and
feature selections, then review prose-only APIs and all relevant primary links.
Update the recorded baseline/date only after the checks finish, retaining explicit
coverage limits. Do not globally substitute version strings without reviewing
behavioral changes or upgrade a user's project merely to match this skill.
