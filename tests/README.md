# Integration harnesses

Cargo auto-discovers four domain roots with explicit modules under matching
subdirectories, plus the separate subprocess harnesses below. Select one moved
suite with, for example, `python3 tools/cargo_guard.py test --locked --test protocol handshake::`.
`-- --exact` needs the complete new module-qualified test name.

`harness-source-map.tsv` maps every old integration target/source to its new
target/source and libtest module prefix. The prefix is prepended to every old
test name, including any existing nested modules. An empty prefix means the
target stays separate. This is a source mapping, not an enumerated test count.

| Domain target | Source suites |
| --- | --- |
| `protocol` | 24 protocol, session, loopback and offline Steam suites |
| `replication_domain` | 28 actor, property, restore and gameplay encoding suites |
| `save_format` | 20 save reader, rule and preservation suites |
| `native_domain` | 26 authored native world, runtime and store suites |

## Isolation inventory

Keep these 20 targets separate; their source paths and crate roots are unchanged:

| Targets | Process-sensitive behavior |
| --- | --- |
| `cli`, `network` | Server children, signals, shutdown and crash recovery |
| `data_pak`, `import_plan`, `inspect_source`, `json_diagnostics`, `prospect`, `prospect_info`, `typed_check` | CLI children, diagnostics or child lifecycle |
| `native_import` | External `mkfifo` child for FIFO rejection |
| `native_c001_owner_cli`, `native_c001_scalar_owner_cli`, `native_inventory_cli`, `native_lifecycle_cli`, `native_modifiers_cli`, `native_object_admission`, `native_relationship_cli`, `native_import_runtime`, `native_world_runtime` | Server children; retain each suite's fixture mutex and process boundary |
| `native_replication` | Server subprocess, owned-child cleanup and opt-in cost output |

Grouped store tests use their own temporary directories and store handles;
file-lock/reopen checks remain in their original modules. Grouped loopback tests
bind ephemeral ports. No grouped suite installs environment/current-directory,
signal, logger or panic-hook globals. Bevy apps retain the existing shared task
pool initialization. Per-test resources and thread-local decode scopes remain
unchanged. Validate new groupings with the full runtime suite to detect contention.

`common/*`, `pak_writer/*` and `network.rs` stay in place. Library and example
fixture includes keep their original paths. The native/save harness roots supply
the crate-root `scalar`/`source` names expected by common inventory fixtures;
grouped callers use those same types. Shared fixture modules are declared once per harness; callers import aliases.

## Fast layout gate

Run `node tools/check_test_harnesses.cjs` before compiling. It validates each
unconditional root declaration against the mapping, every domain source file,
shared helper paths and Cargo's automatically discovered integration roots.
Unmapped/orphan modules, conditional declarations, extra top-level targets,
`tests/*/main.rs`, disabled discovery and explicit `[[test]]` entries fail.
To add a suite, add its source, domain-root declaration and reviewed mapping row;
a new isolated target needs its source and a `subprocess` row. No test count is
frozen. Run `node --test tests/harness-tools.test.cjs` for the gate's negative
cases and inventory comparison tests; they use injected listings, never binaries.

## Validation and measurement

For a layout change, preserve a baseline inventory before migration. M-1184
records this migration's baseline: 118 old targets, 236 agreeing executable
variants, 1,466 test names and one ignored test. Its fixity manifest includes
`baseline-inventory.json`. Do not rebuild that historical layout; retrieve the
preserved receipt when available. It is local-private evidence, not a fixture
shipped with a fresh checkout. These counts are not limits on future additions.

Use the same profile for baseline and candidate. Set `BASELINE_INVENTORY` to the
preserved baseline JSON for the change being qualified. The validating lead
builds and enumerates under normal guard admission and a runtime lease:

```bash
HARNESS_CASE="$PWD/work/test-harness-layout-YYYYMMDD"
mkdir -p "$HARNESS_CASE"
printf '%s\n' '{"state":"open","owner":"lead","purpose":"candidate harness inventory"}' > "$HARNESS_CASE/case.json"
python3 tools/cargo_guard.py test --locked --tests --no-run --message-format=json > "$HARNESS_CASE/after-build.jsonl"
python3 tools/cargo_guard.py --hold-target -- node tests/harness-inventory.cjs after "$HARNESS_CASE/after-build.jsonl" > "$HARNESS_CASE/after-inventory.json"
node tests/harness-inventory.cjs compare "$BASELINE_INVENTORY" "$HARNESS_CASE/after-inventory.json"
prek run --all-files --hook-stage manual
python3 tools/docs_lint.py
```

The helper runs only `--list --format terse`, once normally and once with
`--ignored`, with a 60-second timeout and 16 MiB output cap per call. The shared
target lease prevents cooperating cleanup during enumeration. It requires exact
artifact target coverage, canonicalizes module-qualified names through the source
map and compares all names, kinds and ignored flags. Nested names stay intact.
The comparison also accepts an older helper-generated `before` inventory.
Comparison reads JSON only and never opens the receipt's executable paths.

Candidate sizes sum logical executable bytes for emitted artifacts, excluding
stale executables. Baseline `recorded_variant_bytes` sums all 236 recorded
variants; it is not one generation's size or allocated cache use. Measure actual
cache allocation separately through the guard. Exact inventory equality proves
preservation of this checkpoint; intentional additions need a separately reviewed
baseline update with their inventory difference explained. Then run the full
suite to qualify grouping under concurrency.
