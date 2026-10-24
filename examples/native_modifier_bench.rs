//! Bounded, independently authored schema-11 modifier workload. No source saves,
//! official data, network, filesystem storage, or compatibility comparison.
//!
//! Each timed observation is ONE public advance_native_modifiers call, including
//! its capture/preflight cost. Setup and correctness capture are outside timing.
//! Samples start from fresh identical apps; no app.update or warm-up mutates time.
//! All three modes run: empty owner state, four indefinite entries per owner,
//! and four armed timed entries per owner. Timed entries stay armed: at most
//! 3.3333333 seconds advances against initial durations of at least 60 seconds.
//! Empty/indefinite advances preserve revision; timed advances increment it.
//! Reported simulation progress is per sample, not wall time or simulation ticks.
//! Nearest-rank quantiles follow native_lifecycle_bench; these are small samples.
//!
//! One cooperative deadline includes setup/verification and is never reset.
//! Synchronous calls cannot be interrupted: the lead must also impose external
//! process time/CPU/memory bounds. At most 60 apps (one live at a time), 6,000
//! timed calls, 1,024 live owners and 4,096 modifiers. One <=8 KiB JSON report:
//! twelve mode/size records with individual timing summaries can exceed the former
//! four-record 4 KiB allowance. The cap includes the final newline.
//! No RSS, allocation, throughput, or improvement claim is produced.
//!
//! Build with the project's pinned toolchain. Suggested commands:
//! cargo check --locked --example native_modifier_bench
//! cargo run --release --locked --example native_modifier_bench -- \
//!   --sizes 1,32,256,1024 --samples 3 --steps 10 --seed 1 --seconds 120
//! Apply a hard process timeout to the built example separately from compilation.

use std::{
    io::{self, Write},
    process::ExitCode,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use clap::Parser;
use icarus_bevy::{
    config::ServerConfig,
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, OwnerHandle, Presence,
        RecorderComponentRecord, RecorderKind, SlotDeclaration, SpawnComponent,
        prepare_lifecycle_world,
    },
    native_lifecycle_runtime::advance_native_modifiers,
    native_modifiers::{
        FlatStatContribution, MAX_MODIFIER_OWNERS, MAX_MODIFIERS_PER_OWNER, MAX_WORLD_MODIFIERS,
        ModifierAction, ModifierDefinition, ModifierDuration, ModifierRequest, ModifierTimer,
        NANOS_PER_SECOND, enable_modifier_owners, prepare_modifiers,
    },
    native_world::{
        Aabb3, CompiledNativeCatalog, NativeCatalogDefinition, NativeClassDefinition,
        NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement, NativeTileDefinition,
        NativeTransform, prepare_authored_world,
    },
    simulation::{SNAPSHOT_SCHEMA_MODIFIER_OWNERS, WorldSnapshot, build_app, capture},
};
use serde_json::{Value, json};

const MODIFIERS_PER_OWNER: usize = 4;
const STEP_NANOS: u64 = 33_333_333;
const MAX_REPORT_BYTES: usize = 8192;
const MAX_APPS: usize = 60;
const MAX_TIMED_CALLS: usize = 6000;
const OWNER_COUNTS: [usize; 4] = [1, 32, 256, 1024];
const MODES: [Mode; 3] = [Mode::Empty, Mode::Indefinite, Mode::ArmedTimed];

#[derive(Clone, Copy)]
enum Mode {
    Empty,
    Indefinite,
    ArmedTimed,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Indefinite => "indefinite",
            Self::ArmedTimed => "armed_timed",
        }
    }

    fn entries_per_owner(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Indefinite | Self::ArmedTimed => MODIFIERS_PER_OWNER,
        }
    }
}

// Fixed storage prevents an unbounded comma list from allocating a workload.
#[derive(Clone)]
struct Sizes {
    values: [usize; 4],
    len: usize,
}

fn parse_sizes(text: &str) -> std::result::Result<Sizes, &'static str> {
    if text.len() > 19 {
        return Err("size list too long");
    }
    let mut sizes = Sizes {
        values: [0; 4],
        len: 0,
    };
    for token in text.split(',') {
        if sizes.len == sizes.values.len() {
            return Err("at most four sizes");
        }
        let count = token.parse::<usize>().map_err(|_| "invalid size")?;
        if !OWNER_COUNTS.contains(&count)
            || count > MAX_MODIFIER_OWNERS
            || MODIFIERS_PER_OWNER > MAX_MODIFIERS_PER_OWNER
            || count
                .checked_mul(MODIFIERS_PER_OWNER)
                .is_none_or(|n| n > MAX_WORLD_MODIFIERS)
            || sizes.values[..sizes.len].contains(&count)
        {
            return Err("size outside public caps or duplicate");
        }
        sizes.values[sizes.len] = count;
        sizes.len += 1;
    }
    Ok(sizes)
}

#[derive(Parser)]
#[command(about = "Bounded authored native modifier measurements: empty, indefinite, armed timed")]
struct Options {
    /// Unique subset of 1,32,256,1024; all three modes always run.
    #[arg(long, default_value = "1,32,256,1024", value_parser = parse_sizes)]
    sizes: Sizes,
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(3..=5))]
    samples: u32,
    #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=100))]
    steps: u32,
    /// Bounded deterministic workload variation; never a source identifier.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=4_294_967_295))]
    seed: u64,
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u32).range(1..=120))]
    seconds: u32,
}

struct Budget {
    start: Instant,
    limit: Duration,
}

impl Budget {
    fn check(&self) -> Result<()> {
        ensure!(self.start.elapsed() < self.limit, "deadline reached");
        Ok(())
    }
}

fn fixture(owners: usize, mode: Mode, seed: u64, budget: &Budget) -> Result<WorldSnapshot> {
    budget.check()?;
    // Recheck before the first workload allocation, independently of CLI parsing.
    ensure!(
        OWNER_COUNTS.contains(&owners)
            && owners <= MAX_MODIFIER_OWNERS
            && MODIFIERS_PER_OWNER <= MAX_MODIFIERS_PER_OWNER
            && owners
                .checked_mul(MODIFIERS_PER_OWNER)
                .is_some_and(|n| n <= MAX_WORLD_MODIFIERS)
            && (1..=4_294_967_295).contains(&seed),
        "workload outside bounds"
    );
    let key = |i: usize| format!("owner{i:04}");
    let catalog = Arc::new(CompiledNativeCatalog::compile(NativeCatalogDefinition {
        native_format: 1,
        frame: NativeFrame::RightHandedZUpMeters,
        classes: vec![NativeClassDefinition {
            key: "authored_box".into(),
            kind: NativeClassKind::SpatialObjectV1,
            local_bounds: Aabb3 {
                min: [-0.5; 3],
                max: [0.5; 3],
            },
            variables: vec![],
        }],
        maps: vec![NativeMapDefinition {
            key: "authored_map".into(),
            tiles: vec![NativeTileDefinition {
                key: "authored_tile".into(),
                bounds: Aabb3 {
                    min: [-100.0; 3],
                    max: [100.0; 3],
                },
            }],
            placements: (0..owners)
                .map(|i| NativePlacement {
                    key: key(i),
                    class: "authored_box".into(),
                    tile: "authored_tile".into(),
                    transform: NativeTransform {
                        translation: [(i % 32) as f32 * 2.0, (i / 32) as f32 * 2.0, 3.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0; 3],
                    },
                    initial_values: vec![],
                })
                .collect(),
        }],
    })?);
    budget.check()?;
    let lifecycle = CompiledNativeLifecycle::compile(
        LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "authored_map".into(),
            slots: (0..owners)
                .map(|i| SlotDeclaration {
                    placement: key(i),
                    owner_name: key(i).encode_utf16().collect(),
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: Presence::Live,
                    initial_component: Some(RecorderComponentRecord {
                        kind: RecorderKind::SpatialStateV1,
                        guid: 10_000 + i as i32,
                    }),
                    spawn_component: SpawnComponent::DeclaredKind,
                })
                .collect(),
        },
        &catalog,
    )?;
    budget.check()?;
    let base = prepare_authored_world(catalog, "authored_map")?.initial_snapshot();
    let lifecycle_world = prepare_lifecycle_world(base, &lifecycle)?.into_snapshot();
    budget.check()?;
    let mut snapshot = enable_modifier_owners(lifecycle_world)?;
    let state = snapshot
        .native_modifiers
        .as_mut()
        .context("modifier state missing")?;
    ensure!(state.owners.len() == owners, "owner count mismatch");
    for (i, owner) in state.owners.iter_mut().enumerate() {
        budget.check()?;
        for index in 0..mode.entries_per_owner() {
            let request = ModifierRequest {
                owner: owner.owner,
                expected_revision: owner.revision,
                action: ModifierAction::Add {
                    definition: ModifierDefinition {
                        name: format!("authored_modifier{index}"),
                        duration: match mode {
                            Mode::Empty => unreachable!("empty mode has no entries"),
                            Mode::Indefinite => ModifierDuration::Indefinite,
                            // Integer seconds are exactly representable as f32.
                            // Minimum 60 seconds exceeds the full 100-step sample.
                            Mode::ArmedTimed => ModifierDuration::Timed {
                                seconds: (60
                                    + (seed + (i * MODIFIERS_PER_OWNER + index) as u64) % 61)
                                    as f32,
                            },
                        },
                        contributions: vec![FlatStatContribution {
                            stat: "authored_stat".into(),
                            amount: index as i32 + 1,
                        }],
                    },
                },
            };
            *owner = prepare_modifiers(owner, &request)?.apply(owner)?;
        }
    }
    snapshot.validate()?;
    budget.check()?;
    Ok(snapshot)
}

fn pinned_handles(snapshot: &WorldSnapshot, owners: usize) -> Result<Vec<OwnerHandle>> {
    let lifecycle = snapshot
        .native_lifecycle
        .as_ref()
        .context("lifecycle missing")?;
    ensure!(lifecycle.slots.len() == owners, "slot count mismatch");
    lifecycle
        .slots
        .iter()
        .map(|slot| {
            ensure!(slot.presence == Presence::Live, "owner not live");
            Ok(OwnerHandle {
                id: slot.id,
                incarnation: slot.incarnation,
            })
        })
        .collect()
}

// Only the expected timer/revision deltas are synthesized for an equality check;
// no alternate implementation is timed and no verification mutates the app.
fn verify(
    actual: &WorldSnapshot,
    initial: &WorldSnapshot,
    handles: &[OwnerHandle],
    mode: Mode,
    steps: u32,
) -> Result<()> {
    ensure!(
        actual.schema_version == SNAPSHOT_SCHEMA_MODIFIER_OWNERS,
        "schema mismatch"
    );
    ensure!(
        pinned_handles(actual, handles.len())?.as_slice() == handles,
        "owner handle changed"
    );
    let elapsed = STEP_NANOS * u64::from(steps);
    let mut expected = initial.clone();
    let state = expected
        .native_modifiers
        .as_mut()
        .context("modifier state missing")?;
    ensure!(
        state.owners.len() == handles.len(),
        "modifier owner count mismatch"
    );
    let expected_cache = [FlatStatContribution {
        stat: "authored_stat".into(),
        amount: 10,
    }];
    for (owner, handle) in state.owners.iter_mut().zip(handles) {
        ensure!(owner.owner == *handle, "modifier owner handle mismatch");
        ensure!(
            owner.entries.len() == mode.entries_per_owner(),
            "modifier count mismatch"
        );
        ensure!(
            owner.cache.as_slice()
                == match mode {
                    Mode::Empty => &[],
                    Mode::Indefinite | Mode::ArmedTimed => expected_cache.as_slice(),
                },
            "modifier cache mismatch"
        );
        ensure!(
            owner.revision == mode.entries_per_owner() as u64
                && owner.next_id == mode.entries_per_owner() as u64 + 1,
            "initial cursor mismatch"
        );
        // prepare_modifiers advances revision only when the state changes.
        // Empty and indefinite owners have no armed timer to change.
        if matches!(mode, Mode::ArmedTimed) {
            owner.revision += u64::from(steps);
        }
        for (index, entry) in owner.entries.iter_mut().enumerate() {
            ensure!(entry.id.get() == index as u64 + 1, "initial ID mismatch");
            if matches!(mode, Mode::Indefinite) {
                ensure!(
                    entry.timer == ModifierTimer::Indefinite
                        && entry.definition.duration == ModifierDuration::Indefinite,
                    "indefinite timer mismatch"
                );
                continue;
            }
            let ModifierTimer::Timed {
                remaining_nanos,
                armed: true,
            } = &mut entry.timer
            else {
                anyhow::bail!("timer not armed");
            };
            ensure!(
                *remaining_nanos >= 60 * NANOS_PER_SECOND,
                "initial timer too short"
            );
            *remaining_nanos = remaining_nanos
                .checked_sub(elapsed)
                .context("timer expired")?;
            ensure!(*remaining_nanos > 0, "timer expired");
        }
    }
    // Includes exact cache, IDs, definitions, owner incarnation, remaining nanos,
    // count, and every non-modifier snapshot field (including simulation tick).
    ensure!(
        *actual == expected,
        "modifier progress or snapshot continuity mismatch"
    );
    Ok(())
}

fn timing_summary(nanos: &mut [u64]) -> Value {
    nanos.sort_unstable();
    let percentile = |percent: usize| nanos[(nanos.len() * percent).div_ceil(100) - 1];
    json!({
        "call_count": nanos.len(),
        "median_ns": percentile(50),
        "p95_ns": percentile(95),
        "max_ns": nanos.last(),
        "call_elapsed_ns": nanos.iter().map(|n| u128::from(*n)).sum::<u128>(),
    })
}

fn run(options: &Options, budget: &Budget) -> Result<Value> {
    let app_count = MODES.len() * options.sizes.len * options.samples as usize;
    let timed_call_count = app_count * options.steps as usize;
    ensure!(
        app_count <= MAX_APPS && timed_call_count <= MAX_TIMED_CALLS,
        "aggregate workload outside bounds"
    );
    let mut cases = Vec::with_capacity(MODES.len() * options.sizes.len);
    for mode in MODES {
        for &owners in &options.sizes.values[..options.sizes.len] {
            budget.check()?;
            let mut nanos = Vec::with_capacity((options.samples * options.steps) as usize);
            for _ in 0..options.samples {
                let initial = fixture(owners, mode, options.seed, budget)?;
                let handles = pinned_handles(&initial, owners)?;
                let mut app = build_app(&ServerConfig::default(), &initial)?;
                budget.check()?;
                verify(&capture(app.world_mut())?, &initial, &handles, mode, 0)?;
                for step in 1..=options.steps {
                    budget.check()?;
                    let start = Instant::now();
                    let result =
                        advance_native_modifiers(app.world_mut(), Duration::from_nanos(STEP_NANOS));
                    let elapsed = start.elapsed();
                    result?;
                    budget.check()?;
                    verify(&capture(app.world_mut())?, &initial, &handles, mode, step)?;
                    budget.check()?;
                    // Admit the timing only after correctness and budget checks pass.
                    nanos.push(u64::try_from(elapsed.as_nanos())?);
                }
            }
            cases.push(json!({
                "mode": mode.name(),
                "owners": owners,
                "modifiers": owners * mode.entries_per_owner(),
                "samples": options.samples,
                "verified_calls": nanos.len(),
                "steps_per_sample": options.steps,
                "simulation_elapsed_ns_per_sample": u64::from(options.steps) * STEP_NANOS,
                "timing": timing_summary(&mut nanos),
            }));
            budget.check()?;
        }
    }
    Ok(json!({
        "complete": true,
        "schema": SNAPSHOT_SCHEMA_MODIFIER_OWNERS,
        "phase": "public_advance_native_modifiers",
        "percentile_method": "nearest_rank",
        "seed": options.seed,
        "warmup_calls": 0,
        "app_count": app_count,
        "timed_call_count": timed_call_count,
        "step_ns": STEP_NANOS,
        "deadline_seconds": options.seconds,
        "wall_elapsed_ns": u64::try_from(budget.start.elapsed().as_nanos())?,
        "cases": cases,
    }))
}

fn emit(record: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(record)?;
    bytes.push(b'\n');
    ensure!(bytes.len() <= MAX_REPORT_BYTES, "report limit");
    let mut out = io::stdout().lock();
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}

fn main() -> ExitCode {
    let start = Instant::now();
    // Do not expose caller-supplied arguments, paths, or error chains in reports.
    let options = match Options::try_parse() {
        Ok(options) => options,
        Err(_) => {
            let _ = emit(&json!({"complete": false, "cli_errors": 1}));
            return ExitCode::FAILURE;
        }
    };
    let budget = Budget {
        start,
        limit: Duration::from_secs(u64::from(options.seconds)),
    };
    match run(&options, &budget) {
        Ok(report) if budget.check().is_ok() => {
            if emit(&report).is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        _ => {
            let _ = emit(&json!({
                "complete": false,
                "errors": 1,
                "deadline_reached": budget.check().is_err(),
            }));
            ExitCode::FAILURE
        }
    }
}
