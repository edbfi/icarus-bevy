//! Bounded authored-native measurements; no ICARUS workload or compatibility claim.
//!
//! Caller creates a work/<case> directory and passes an ABSENT child as --data-dir.
//! Stores are retained, including after errors or budget expiry. No subprocesses,
//! networking, cleanup, production optimizations, or original inputs are involved.
//! Budgets are cooperative: synchronous public calls finish and are checked before
//! another call starts. The caller must impose process CPU/memory/hard time bounds.
//! Every reported sample has passed its correctness check, outside the timer.
//! Action timings include the API's internal capture/preflight without subtraction.
//! Load timings use the same open SnapshotStore, with its normal verification cache.
//! Selected operations share the case budget in canonical order. Select one late
//! operation to give it the full case allowance; deadlines are never reset.
//!
//! Validation commands:
//! cargo check --locked --example native_lifecycle_bench
//! cargo test --locked --example native_lifecycle_bench
//! cargo run --release --locked --example native_lifecycle_bench -- \
//!   --data-dir work/<case>/absent-store --slots 3 --repeats 1 --warmups 0 --samples 1

#[path = "common/native_lifecycle_fixture.rs"]
mod fixture;

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use bevy::prelude::App;
use clap::{Parser, ValueEnum};
use fixture::{DEPLOYABLE, FOUNDATION, Fixture, STARTING_USES};
use icarus_bevy::{
    config::ServerConfig,
    native_lifecycle::{OwnerHandle, RestoreBatch, prepare_owner_restore},
    native_lifecycle_runtime::{
        NativeRestoreResult, lifecycle_foundation_of, native_owner_handle,
        reassign_lifecycle_foundation, restore_native_owners, scalar_owner_state, set_enabled,
        try_use, update_lifecycle_actor,
    },
    native_scalar_owners::{ScalarOwnerState, ScalarUseOutcome},
    native_world::{NativeTransform, NativeValue},
    persistence::SnapshotStore,
    simulation::{WorldSnapshot, build_app, capture},
};
use serde_json::{Value, json};

#[derive(Debug, Parser)]
#[command(about = "Bounded authored native lifecycle benchmark; not an ICARUS workload")]
struct Options {
    /// Absent child directory of an existing work/<case>; never deleted by this program.
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(long, value_delimiter = ',', default_value = "7,8,9")]
    schemas: Vec<u32>,
    #[arg(long, value_delimiter = ',', default_value = "3,64,256,1024")]
    slots: Vec<usize>,
    /// Run this subset in canonical order, sharing one case budget (default: all).
    #[arg(long, value_enum, value_delimiter = ',', default_values = OPERATIONS.map(Operation::name))]
    operations: Vec<Operation>,
    #[arg(long, default_value_t = 3)]
    repeats: usize,
    #[arg(long, default_value_t = 5)]
    warmups: usize,
    #[arg(long, default_value_t = 50)]
    samples: usize,
    /// Includes fixture construction, activation, resetting and verification.
    #[arg(long, default_value_t = 20.0)]
    case_seconds: f64,
    /// Includes CLI validation, destination reservation, output and all case setup.
    #[arg(long, default_value_t = 900.0)]
    total_seconds: f64,
}

impl Options {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.schemas.is_empty() && self.schemas.len() <= 3,
            "schema count invalid"
        );
        ensure!(
            self.schemas.iter().all(|s| (7..=9).contains(s)),
            "schemas must be 7,8,9"
        );
        ensure!(
            self.schemas.iter().collect::<BTreeSet<_>>().len() == self.schemas.len(),
            "duplicate schemas"
        );
        ensure!(
            !self.slots.is_empty() && self.slots.len() <= 4,
            "slot case count must be 1..4"
        );
        ensure!(
            self.slots.iter().all(|s| (3..=1024).contains(s)),
            "slots must be 3..1024"
        );
        ensure!(
            self.slots.iter().collect::<BTreeSet<_>>().len() == self.slots.len(),
            "duplicate slot cases"
        );
        ensure!(
            !self.operations.is_empty() && self.operations.len() <= OPERATIONS.len(),
            "operation count invalid"
        );
        ensure!(
            self.operations.iter().collect::<BTreeSet<_>>().len() == self.operations.len(),
            "duplicate operations"
        );
        ensure!((1..=3).contains(&self.repeats), "repeats must be 1..3");
        ensure!(self.warmups <= 5, "warmups must be 0..5");
        ensure!((1..=50).contains(&self.samples), "samples must be 1..50");
        ensure!(
            self.case_seconds.is_finite() && self.case_seconds > 0.0 && self.case_seconds <= 20.0,
            "case seconds must be finite, positive and <=20"
        );
        ensure!(
            self.total_seconds.is_finite()
                && self.total_seconds > 0.0
                && self.total_seconds <= 900.0,
            "total seconds must be finite, positive and <=900"
        );
        Ok(())
    }

    fn selected_operations(&self) -> impl Iterator<Item = Operation> + '_ {
        OPERATIONS
            .into_iter()
            .filter(|op| self.operations.contains(op))
    }
}

fn emit(record: Value) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer(&mut out, &record)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

/// create_dir is an atomic absent-path reservation, including dangling symlinks.
/// Existing parents are required; the program never creates a work case itself.
fn reserve_data_dir(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .context("data-dir requires a parent")?
        .canonicalize()?;
    let work_root = std::env::current_dir()?.join("work").canonicalize()?;
    ensure!(
        parent.starts_with(&work_root) && parent != work_root,
        "data-dir must be below an existing work case"
    );
    let name = path.file_name().context("data-dir requires a child name")?;
    let reserved = parent.join(name);
    fs::create_dir(&reserved).context("data-dir must be absent; refusing existing destination")?;
    Ok(reserved)
}

#[derive(Clone, Copy)]
struct Budget {
    total_start: Instant,
    case_start: Instant,
    total_limit: Duration,
    case_limit: Duration,
}
impl Budget {
    fn reason(&self) -> Option<&'static str> {
        if self.total_start.elapsed() >= self.total_limit {
            Some("total_budget")
        } else if self.case_start.elapsed() >= self.case_limit {
            Some("case_budget")
        } else {
            None
        }
    }
    fn active(&self) -> bool {
        self.reason().is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
#[value(rename_all = "snake_case")]
enum Operation {
    Capture,
    Preflight,
    FirstPublication,
    DurableSave,
    Load,
    #[value(name = "scalar_use")]
    Use,
    ChangedEnable,
    EqualEnable,
    DisabledUse,
    ExhaustedUse,
    PoseEdit,
    FoundationEdit,
    #[value(name = "owner_replacement")]
    Replacement,
    #[value(name = "exact_retry_after_edits")]
    ExactRetry,
}
const OPERATIONS: [Operation; 14] = [
    Operation::Capture,
    Operation::Preflight,
    Operation::FirstPublication,
    Operation::DurableSave,
    Operation::Load,
    Operation::Use,
    Operation::ChangedEnable,
    Operation::EqualEnable,
    Operation::DisabledUse,
    Operation::ExhaustedUse,
    Operation::PoseEdit,
    Operation::FoundationEdit,
    Operation::Replacement,
    Operation::ExactRetry,
];
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::Capture => "capture",
            Self::Preflight => "preflight",
            Self::FirstPublication => "first_publication",
            Self::DurableSave => "durable_save",
            Self::Load => "load",
            Self::Use => "scalar_use",
            Self::ChangedEnable => "changed_enable",
            Self::EqualEnable => "equal_enable",
            Self::DisabledUse => "disabled_use",
            Self::ExhaustedUse => "exhausted_use",
            Self::PoseEdit => "pose_edit",
            Self::FoundationEdit => "foundation_edit",
            Self::Replacement => "owner_replacement",
            Self::ExactRetry => "exact_retry_after_edits",
        }
    }
    fn supported(self, schema: u32) -> bool {
        schema == 9
            || !matches!(
                self,
                Self::Use
                    | Self::ChangedEnable
                    | Self::EqualEnable
                    | Self::DisabledUse
                    | Self::ExhaustedUse
            )
    }
}

#[derive(Default)]
struct Samples {
    nanos: Vec<u64>,
    warmups: usize,
}
impl Samples {
    fn summary(&mut self) -> Value {
        // Retain acquisition order for independent analysis of drift/outliers.
        let samples_ns = self.nanos.clone();
        self.nanos.sort_unstable();
        let percentile = |percent: usize| {
            if self.nanos.is_empty() {
                None
            } else {
                Some(self.nanos[(self.nanos.len() * percent).div_ceil(100) - 1])
            }
        };
        json!({
            "median_ns":percentile(50), "p95_ns":percentile(95),
            "max_ns":self.nanos.last(), "sample_count":self.nanos.len(),
            "verified_sample_count":self.nanos.len(), "warmups_completed":self.warmups,
            "sample_elapsed_ns":self.nanos.iter().map(|n| u128::from(*n)).sum::<u128>(),
            "samples_ns":samples_ns,
            "percentile_method":"nearest_rank"
        })
    }
}

fn elapsed_ns(start: Instant) -> u64 {
    start.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64
}

/// The closure does setup, checks the budget immediately before the timed call,
/// then verifies its result. None means setup consumed the remaining budget.
fn measure(
    budget: Budget,
    warmups: usize,
    samples: usize,
    mut sample: impl FnMut(Budget) -> Result<Option<u64>>,
) -> Result<Samples> {
    let mut result = Samples::default();
    for index in 0..warmups + samples {
        if !budget.active() {
            break;
        }
        let Some(nanos) = sample(budget)? else {
            break;
        };
        if index < warmups {
            result.warmups += 1;
        } else {
            result.nanos.push(nanos);
        }
    }
    Ok(result)
}

fn verify_loaded(store: &SnapshotStore, expected: &WorldSnapshot) -> Result<()> {
    let loaded = store.load()?.context("acknowledged generation missing")?;
    ensure!(
        !loaded.recovered_previous && loaded.snapshot == *expected,
        "durable state mismatch"
    );
    Ok(())
}

struct Storage {
    store: SnapshotStore,
    path: PathBuf,
    expected: WorldSnapshot,
    generation_bytes: Option<u64>,
    published: bool,
    untimed_commits: usize,
}
impl Storage {
    fn new(path: PathBuf, snapshot: &WorldSnapshot) -> Result<Self> {
        // This path belongs to our exclusively reserved data directory.
        fs::create_dir(&path)?;
        let store = SnapshotStore::open(&path)?;
        ensure!(store.load()?.is_none(), "new store unexpectedly populated");
        Ok(Self {
            store,
            path,
            expected: snapshot.clone(),
            generation_bytes: None,
            published: false,
            untimed_commits: 0,
        })
    }
    fn first_publication(&mut self, budget: Budget) -> Result<Option<u64>> {
        ensure!(!self.published, "first publication repeated");
        if !budget.active() {
            return Ok(None);
        }
        let start = Instant::now();
        let saved = self.store.save_new(&self.expected);
        let nanos = elapsed_ns(start);
        let receipt = saved?;
        ensure!(
            receipt.generation == 1 && receipt.tick == self.expected.tick,
            "first receipt mismatch"
        );
        verify_loaded(&self.store, &self.expected)?;
        self.generation_bytes = Some(
            fs::metadata(
                self.path
                    .join(format!("snapshot-{}.json", receipt.generation)),
            )?
            .len(),
        );
        self.published = true;
        Ok(Some(nanos))
    }
    fn establish_steady(&mut self, budget: Budget) -> Result<()> {
        while self.published && self.untimed_commits < 2 && budget.active() {
            self.expected.tick += 1;
            let receipt = self.store.save(&self.expected)?;
            ensure!(
                receipt.tick == self.expected.tick,
                "untimed receipt mismatch"
            );
            verify_loaded(&self.store, &self.expected)?;
            self.untimed_commits += 1;
        }
        Ok(())
    }
    fn save(&mut self, budget: Budget) -> Result<Option<u64>> {
        if !budget.active() {
            return Ok(None);
        }
        ensure!(
            self.published && self.untimed_commits == 2,
            "steady save lacks two untimed commits"
        );
        self.expected.tick += 1;
        let start = Instant::now();
        let saved = self.store.save(&self.expected);
        let nanos = elapsed_ns(start);
        let receipt = saved?;
        ensure!(receipt.tick == self.expected.tick, "save receipt mismatch");
        verify_loaded(&self.store, &self.expected)?;
        self.generation_bytes = Some(
            fs::metadata(
                self.path
                    .join(format!("snapshot-{}.json", receipt.generation)),
            )?
            .len(),
        );
        Ok(Some(nanos))
    }
    fn load(&self, budget: Budget) -> Result<Option<u64>> {
        if !budget.active() {
            return Ok(None);
        }
        ensure!(self.published, "load lacks publication");
        let start = Instant::now();
        let loaded = self.store.load();
        let nanos = elapsed_ns(start);
        let loaded = loaded?.context("measured load missing")?;
        ensure!(
            !loaded.recovered_previous && loaded.snapshot == self.expected,
            "measured load mismatch"
        );
        Ok(Some(nanos))
    }
}

/// Publication and two steady-state commits are prerequisites, never samples.
/// This also permits save/load to be selected without first_publication.
fn prepare_storage(
    storage: &mut Option<Storage>,
    path: PathBuf,
    snapshot: &WorldSnapshot,
    budget: Budget,
) -> Result<()> {
    if storage.is_none() {
        if !budget.active() {
            return Ok(());
        }
        *storage = Some(Storage::new(path, snapshot)?);
    }
    let store = storage.as_mut().unwrap();
    if !store.published {
        if !budget.active() {
            return Ok(());
        }
        store.first_publication(budget)?;
    }
    if !budget.active() {
        return Ok(());
    }
    store.establish_steady(budget)
}

fn verify_restore(
    result: NativeRestoreResult,
    operation: Operation,
    expected: &WorldSnapshot,
) -> Result<()> {
    let receipt = match (operation, result) {
        (Operation::Replacement, NativeRestoreResult::AppliedInMemory(receipt))
        | (Operation::ExactRetry, NativeRestoreResult::AlreadyCompleted(receipt)) => receipt,
        _ => anyhow::bail!("restore outcome mismatch"),
    };
    ensure!(
        expected
            .native_lifecycle
            .as_ref()
            .and_then(|life| life.completed.as_ref())
            == Some(&receipt),
        "restore receipt mismatch"
    );
    Ok(())
}

fn set_expected_scalar(snapshot: &mut WorldSnapshot, enabled: bool, uses: i32) {
    let actor = snapshot
        .native_world
        .as_mut()
        .unwrap()
        .actors
        .iter_mut()
        .find(|a| a.id == FOUNDATION)
        .unwrap();
    actor
        .variables
        .iter_mut()
        .find(|v| v.key == "enabled")
        .unwrap()
        .value = NativeValue::Bool(enabled);
    actor
        .variables
        .iter_mut()
        .find(|v| v.key == "remaining_uses")
        .unwrap()
        .value = NativeValue::I32(uses);
}

fn verify_scalar(
    state: ScalarOwnerState,
    owner: OwnerHandle,
    enabled: bool,
    uses: i32,
) -> Result<()> {
    ensure!(
        state.owner == owner
            && state.enabled == enabled
            && state.remaining_uses == uses
            && state.available == (enabled && uses > 0),
        "scalar state mismatch"
    );
    Ok(())
}

/// One fresh activation per action attempt. No resets, handle lookups, request
/// construction or correctness capture enter the measured region.
struct ActionSample {
    app: App,
    owner: OwnerHandle,
    deployable: OwnerHandle,
    pose: NativeTransform,
    batch: RestoreBatch,
    expected: WorldSnapshot,
}
impl ActionSample {
    fn prepare(
        snapshot: &WorldSnapshot,
        operation: Operation,
        budget: Budget,
    ) -> Result<Option<Self>> {
        Self::prepare_with_budget_check(snapshot, operation, || budget.active())
    }

    // Inject only the cooperative check so expiry tests need neither sleeps nor
    // wall-clock races. Each expensive setup call has its own immediate guard.
    fn prepare_with_budget_check(
        snapshot: &WorldSnapshot,
        operation: Operation,
        mut active: impl FnMut() -> bool,
    ) -> Result<Option<Self>> {
        if !active() {
            return Ok(None);
        }
        let mut app = build_app(&ServerConfig::default(), snapshot)?;
        if !active() {
            return Ok(None);
        }
        let mut owner = native_owner_handle(app.world(), FOUNDATION)?;
        if !active() {
            return Ok(None);
        }
        let deployable = native_owner_handle(app.world(), DEPLOYABLE)?;
        if !active() {
            return Ok(None);
        }
        let mut expected = snapshot.clone();
        if !active() {
            return Ok(None);
        }
        let batch = fixture::replacement(snapshot);
        if !active() {
            return Ok(None);
        }
        let mut pose = snapshot
            .native_world
            .as_ref()
            .unwrap()
            .actors
            .iter()
            .find(|a| a.id == FOUNDATION)
            .unwrap()
            .transform
            .clone();
        pose.translation[0] += 0.25;
        if !active() {
            return Ok(None);
        }
        match operation {
            Operation::Use => set_expected_scalar(&mut expected, true, STARTING_USES - 1),
            Operation::ChangedEnable => set_expected_scalar(&mut expected, false, STARTING_USES),
            Operation::DisabledUse => {
                if !active() {
                    return Ok(None);
                }
                verify_scalar(
                    set_enabled(app.world_mut(), owner, false)?,
                    owner,
                    false,
                    STARTING_USES,
                )?;
                set_expected_scalar(&mut expected, false, STARTING_USES);
            }
            Operation::ExhaustedUse => {
                for remaining in (0..STARTING_USES).rev() {
                    if !active() {
                        return Ok(None);
                    }
                    let ScalarUseOutcome::Used(state) = try_use(app.world_mut(), owner)? else {
                        anyhow::bail!("exhaustion setup did not use");
                    };
                    verify_scalar(state, owner, true, remaining)?;
                }
                set_expected_scalar(&mut expected, true, 0);
            }
            Operation::PoseEdit => {
                expected
                    .native_world
                    .as_mut()
                    .unwrap()
                    .actors
                    .iter_mut()
                    .find(|a| a.id == FOUNDATION)
                    .unwrap()
                    .transform = pose.clone();
            }
            Operation::FoundationEdit => {
                expected
                    .native_relationships
                    .as_mut()
                    .unwrap()
                    .foundations
                    .iter_mut()
                    .find(|f| f.owner == DEPLOYABLE)
                    .unwrap()
                    .foundation = None;
            }
            Operation::Replacement | Operation::ExactRetry => {
                if !active() {
                    return Ok(None);
                }
                expected = prepare_owner_restore(snapshot, &batch)?.into_snapshot();
                ensure!(
                    expected
                        .native_lifecycle
                        .as_ref()
                        .unwrap()
                        .slots
                        .iter()
                        .find(|s| s.id == FOUNDATION)
                        .unwrap()
                        .incarnation
                        == owner.incarnation + 1,
                    "replacement preparation did not advance incarnation"
                );
                if operation == Operation::ExactRetry {
                    if !active() {
                        return Ok(None);
                    }
                    verify_restore(
                        restore_native_owners(app.world_mut(), &batch)?,
                        Operation::Replacement,
                        &expected,
                    )?;
                    if !active() {
                        return Ok(None);
                    }
                    owner = native_owner_handle(app.world(), FOUNDATION)?;
                    // Later state deliberately differs from the request. An exact
                    // retry must preserve edits and consumption, not replay it.
                    if snapshot.schema_version == 9 {
                        if !active() {
                            return Ok(None);
                        }
                        let ScalarUseOutcome::Used(state) = try_use(app.world_mut(), owner)? else {
                            anyhow::bail!("retry setup did not use");
                        };
                        verify_scalar(state, owner, true, STARTING_USES - 1)?;
                        set_expected_scalar(&mut expected, true, STARTING_USES - 1);
                    }
                    if !active() {
                        return Ok(None);
                    }
                    let actor = expected
                        .native_world
                        .as_mut()
                        .unwrap()
                        .actors
                        .iter_mut()
                        .find(|a| a.id == FOUNDATION)
                        .unwrap();
                    actor.transform = pose.clone();
                    let variables = actor.variables.clone();
                    if !active() {
                        return Ok(None);
                    }
                    update_lifecycle_actor(app.world_mut(), owner, pose.clone(), variables)?;
                    if !active() {
                        return Ok(None);
                    }
                    reassign_lifecycle_foundation(app.world_mut(), deployable, None)?;
                    expected
                        .native_relationships
                        .as_mut()
                        .unwrap()
                        .foundations
                        .iter_mut()
                        .find(|f| f.owner == DEPLOYABLE)
                        .unwrap()
                        .foundation = None;
                    ensure!(
                        capture(app.world_mut())? == expected,
                        "retry setup state mismatch"
                    );
                }
            }
            Operation::EqualEnable => {}
            _ => anyhow::bail!("unsupported action preparation"),
        }
        if !active() {
            return Ok(None);
        }
        Ok(Some(Self {
            app,
            owner,
            deployable,
            pose,
            batch,
            expected,
        }))
    }

    fn run(mut self, operation: Operation, budget: Budget) -> Result<Option<u64>> {
        if !budget.active() {
            return Ok(None);
        }
        let variables = self
            .expected
            .native_world
            .as_ref()
            .unwrap()
            .actors
            .iter()
            .find(|a| a.id == FOUNDATION)
            .unwrap()
            .variables
            .clone();
        if !budget.active() {
            return Ok(None);
        }
        // The match dispatch and result inspection stay outside each timer.
        let nanos = match operation {
            Operation::Use | Operation::DisabledUse | Operation::ExhaustedUse => {
                let start = Instant::now();
                let result = try_use(self.app.world_mut(), self.owner);
                let elapsed = elapsed_ns(start);
                let state = match (operation, result?) {
                    (Operation::Use, ScalarUseOutcome::Used(state)) => state,
                    (Operation::DisabledUse, ScalarUseOutcome::Disabled(state)) => state,
                    (Operation::ExhaustedUse, ScalarUseOutcome::Exhausted(state)) => state,
                    _ => anyhow::bail!("scalar use outcome mismatch"),
                };
                let (enabled, uses) = match operation {
                    Operation::Use => (true, STARTING_USES - 1),
                    Operation::DisabledUse => (false, STARTING_USES),
                    _ => (true, 0),
                };
                verify_scalar(state, self.owner, enabled, uses)?;
                elapsed
            }
            Operation::ChangedEnable | Operation::EqualEnable => {
                let enabled = operation == Operation::EqualEnable;
                let start = Instant::now();
                let result = set_enabled(self.app.world_mut(), self.owner, enabled);
                let elapsed = elapsed_ns(start);
                verify_scalar(result?, self.owner, enabled, STARTING_USES)?;
                elapsed
            }
            Operation::PoseEdit => {
                let start = Instant::now();
                let result =
                    update_lifecycle_actor(self.app.world_mut(), self.owner, self.pose, variables);
                let elapsed = elapsed_ns(start);
                result?;
                elapsed
            }
            Operation::FoundationEdit => {
                let start = Instant::now();
                let result =
                    reassign_lifecycle_foundation(self.app.world_mut(), self.deployable, None);
                let elapsed = elapsed_ns(start);
                result?;
                ensure!(
                    lifecycle_foundation_of(self.app.world(), self.deployable)?.is_none(),
                    "foundation edit mismatch"
                );
                elapsed
            }
            Operation::Replacement | Operation::ExactRetry => {
                let start = Instant::now();
                let result = restore_native_owners(self.app.world_mut(), &self.batch);
                let elapsed = elapsed_ns(start);
                verify_restore(result?, operation, &self.expected)?;
                if operation == Operation::Replacement {
                    let new = native_owner_handle(self.app.world(), FOUNDATION)?;
                    ensure!(
                        new.incarnation == self.owner.incarnation + 1,
                        "replacement incarnation mismatch"
                    );
                }
                elapsed
            }
            _ => anyhow::bail!("unsupported action"),
        };
        let actual = capture(self.app.world_mut())?;
        ensure!(actual == self.expected, "action snapshot mismatch");
        if actual.schema_version == 9 {
            let requested_owner = native_owner_handle(self.app.world(), FOUNDATION)?;
            let state = scalar_owner_state(self.app.world(), requested_owner)?;
            let expected_actor = self
                .expected
                .native_world
                .as_ref()
                .unwrap()
                .actors
                .iter()
                .find(|a| a.id == FOUNDATION)
                .unwrap();
            let enabled = expected_actor
                .variables
                .iter()
                .find(|v| v.key == "enabled")
                .unwrap();
            let uses = expected_actor
                .variables
                .iter()
                .find(|v| v.key == "remaining_uses")
                .unwrap();
            let (NativeValue::Bool(enabled), NativeValue::I32(uses)) =
                (&enabled.value, &uses.value)
            else {
                anyhow::bail!("fixture scalar kinds mismatch");
            };
            verify_scalar(state, requested_owner, *enabled, *uses)?;
        }
        Ok(Some(nanos))
    }
}

fn run_case(
    options: &Options,
    root: &Path,
    schema: u32,
    slots: usize,
    repeat: usize,
    budget: Budget,
) -> Result<bool> {
    emit(
        json!({"event":"case_started", "schema":schema, "slots":slots,
        "repeat":repeat, "complete":false, "icarus_workload":false}),
    )?;
    let fixture = Fixture::new(schema, slots).context("fixture construction failed")?;
    emit(
        json!({"event":"fixture", "schema":schema, "slots":slots, "repeat":repeat,
        "identity":fixture.identity, "complete":budget.active(), "icarus_workload":false}),
    )?;
    let mut storage: Option<Storage> = None;
    let mut complete = true;
    for operation in options.selected_operations() {
        let supported = operation.supported(schema);
        let (warmups, requested) = if operation == Operation::FirstPublication {
            (0, 1)
        } else {
            (options.warmups, options.samples)
        };
        let mut samples = Samples::default();
        let mut skip_reason = if !supported {
            Some("unsupported_schema")
        } else {
            budget.reason()
        };
        if skip_reason.is_none() {
            emit(
                json!({"event":"operation_started", "schema":schema, "slots":slots,
                "repeat":repeat, "operation":operation.name(), "complete":false, "icarus_workload":false}),
            )?;
            samples = match operation {
                Operation::Capture => {
                    if !budget.active() {
                        Samples::default()
                    } else {
                        let mut app = build_app(&ServerConfig::default(), &fixture.snapshot)?;
                        measure(budget, warmups, requested, |budget| {
                            if !budget.active() {
                                return Ok(None);
                            }
                            let start = Instant::now();
                            let captured = capture(app.world_mut());
                            let nanos = elapsed_ns(start);
                            ensure!(captured? == fixture.snapshot, "capture mismatch");
                            Ok(Some(nanos))
                        })?
                    }
                }
                Operation::Preflight => measure(budget, warmups, requested, |budget| {
                    if !budget.active() {
                        return Ok(None);
                    }
                    let start = Instant::now();
                    let result = SnapshotStore::preflight(&fixture.snapshot);
                    let nanos = elapsed_ns(start);
                    result?;
                    // Fixture validity and digest were checked outside this region;
                    // immutable borrowing makes preflight state changes impossible.
                    Ok(Some(nanos))
                })?,
                Operation::FirstPublication => {
                    if !budget.active() {
                        Samples::default()
                    } else {
                        storage = Some(Storage::new(
                            root.join(format!("schema{schema}-slots{slots}-repeat{repeat}")),
                            &fixture.snapshot,
                        )?);
                        measure(budget, 0, 1, |budget| {
                            storage.as_mut().unwrap().first_publication(budget)
                        })?
                    }
                }
                Operation::DurableSave => {
                    prepare_storage(
                        &mut storage,
                        root.join(format!("schema{schema}-slots{slots}-repeat{repeat}")),
                        &fixture.snapshot,
                        budget,
                    )?;
                    if let Some(store) = storage.as_mut().filter(|s| s.untimed_commits == 2) {
                        measure(budget, warmups, requested, |budget| store.save(budget))?
                    } else {
                        skip_reason = budget.reason().or(Some("steady_state_not_ready"));
                        Samples::default()
                    }
                }
                Operation::Load => {
                    prepare_storage(
                        &mut storage,
                        root.join(format!("schema{schema}-slots{slots}-repeat{repeat}")),
                        &fixture.snapshot,
                        budget,
                    )?;
                    if let Some(store) = storage.as_ref().filter(|s| s.untimed_commits == 2) {
                        measure(budget, warmups, requested, |budget| store.load(budget))?
                    } else {
                        skip_reason = budget.reason().or(Some("steady_state_not_ready"));
                        Samples::default()
                    }
                }
                action => measure(budget, warmups, requested, |budget| {
                    let Some(sample) = ActionSample::prepare(&fixture.snapshot, action, budget)?
                    else {
                        return Ok(None);
                    };
                    sample.run(action, budget)
                })?,
            };
        }
        let operation_complete = supported
            && samples.warmups == warmups
            && samples.nanos.len() == requested
            && budget.active();
        if supported {
            complete &= operation_complete;
        }
        let status = if !supported {
            "unsupported"
        } else if operation_complete {
            "complete"
        } else if samples.nanos.is_empty() {
            "skipped"
        } else {
            "incomplete"
        };
        let reason = skip_reason.or_else(|| {
            if operation_complete {
                None
            } else {
                budget.reason()
            }
        });
        emit(
            json!({"event":"operation", "schema":schema, "slots":slots, "repeat":repeat,
            "operation":operation.name(), "status":status, "reason":reason,
            "requested_samples":requested, "requested_warmups":warmups,
            "elapsed":samples.summary(), "identity":fixture.identity,
            "generation_encoded_bytes":storage.as_ref().and_then(|s| s.generation_bytes),
            "untimed_steady_commits":storage.as_ref().map_or(0, |s| s.untimed_commits),
            "case_elapsed_ns":budget.case_start.elapsed().as_nanos(),
            "total_elapsed_ns":budget.total_start.elapsed().as_nanos(),
            "complete":operation_complete, "icarus_workload":false}),
        )?;
    }
    complete &= budget.active();
    emit(
        json!({"event":"case_finished", "schema":schema, "slots":slots, "repeat":repeat,
        "complete":complete, "reason":budget.reason(),
        "case_elapsed_ns":budget.case_start.elapsed().as_nanos(),
        "total_elapsed_ns":budget.total_start.elapsed().as_nanos(), "icarus_workload":false}),
    )?;
    Ok(complete)
}

fn run(options: &Options, total_start: Instant) -> Result<()> {
    options.validate()?;
    let total_limit = Duration::from_secs_f64(options.total_seconds);
    let case_limit = Duration::from_secs_f64(options.case_seconds);
    ensure!(
        !total_limit.is_zero() && !case_limit.is_zero(),
        "budgets below timer resolution"
    );
    let root = reserve_data_dir(&options.data_dir)?;
    emit(
        json!({"event":"benchmark_started", "schemas":options.schemas, "slots":options.slots,
        "repeats":options.repeats, "warmups":options.warmups, "samples":options.samples,
        "operations":options.selected_operations().map(Operation::name).collect::<Vec<_>>(),
        "case_budget_seconds":options.case_seconds, "total_budget_seconds":options.total_seconds,
        "case_budget_scope":"shared_by_selected_operations_without_reset",
        "budget_mode":"cooperative_between_synchronous_calls", "load_mode":"same_open_store",
        "stores_retained":true, "compatibility_demonstrated":false,
        "complete":false, "icarus_workload":false}),
    )?;
    let mut complete = true;
    let mut completed_cases = 0;
    let mut skipped_cases = 0;
    for &schema in &options.schemas {
        for &slots in &options.slots {
            for repeat in 1..=options.repeats {
                let budget = Budget {
                    total_start,
                    case_start: Instant::now(),
                    total_limit,
                    case_limit,
                };
                if !budget.active() {
                    complete = false;
                    skipped_cases += 1;
                    emit(
                        json!({"event":"case_skipped", "schema":schema, "slots":slots,
                        "repeat":repeat, "reason":budget.reason(), "complete":false,
                        "icarus_workload":false}),
                    )?;
                    continue;
                }
                let case_complete = run_case(options, &root, schema, slots, repeat, budget)?;
                completed_cases += usize::from(case_complete);
                complete &= case_complete;
            }
        }
    }
    complete &= total_start.elapsed() < total_limit;
    emit(json!({"event":"benchmark_finished", "complete":complete,
        "completed_cases":completed_cases, "skipped_cases":skipped_cases,
        "incomplete_cases":options.schemas.len() * options.slots.len() * options.repeats - completed_cases - skipped_cases,
        "requested_cases":options.schemas.len() * options.slots.len() * options.repeats,
        "total_elapsed_ns":total_start.elapsed().as_nanos(),
        "compatibility_demonstrated":false, "icarus_workload":false}))?;
    Ok(())
}

fn main() -> ExitCode {
    let started = Instant::now();
    let options = Options::parse();
    match run(&options, started) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => {
            // Never expose payloads, names or arbitrary I/O error chains in reports.
            let _ = emit(
                json!({"event":"benchmark_error", "reason":"invalid_input_fixture_io_or_correctness",
                "complete":false, "total_elapsed_ns":started.elapsed().as_nanos(),
                "icarus_workload":false}),
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use icarus_bevy::native_lifecycle::{
        CompletedRestore, ComponentDisposition, Presence, RecorderSource, ResolvePolicy,
        RestoreDisposition,
    };
    use icarus_bevy::simulation::DomainId;

    fn options(operations: &str) -> Result<Options> {
        Ok(Options::try_parse_from([
            "bench",
            "--data-dir",
            "work/authored/absent",
            "--operations",
            operations,
            "--schemas",
            "9",
            "--slots",
            "3",
            "--repeats",
            "1",
            "--warmups",
            "0",
            "--samples",
            "1",
        ])?)
    }

    fn budget() -> Budget {
        let start = Instant::now();
        Budget {
            total_start: start,
            case_start: start,
            total_limit: Duration::from_secs(20),
            case_limit: Duration::from_secs(20),
        }
    }

    #[test]
    fn operation_selector_defaults_validates_and_canonicalizes() -> Result<()> {
        let defaults = Options::try_parse_from(["bench", "--data-dir", "work/authored/absent"])?;
        defaults.validate()?;
        assert_eq!(
            defaults.selected_operations().collect::<Vec<_>>(),
            OPERATIONS
        );
        let mut selected = options("exact_retry_after_edits,load,durable_save")?;
        selected.validate()?;
        assert_eq!(
            selected.selected_operations().collect::<Vec<_>>(),
            vec![
                Operation::DurableSave,
                Operation::Load,
                Operation::ExactRetry,
            ]
        );
        selected.operations.clear();
        assert!(selected.validate().is_err());
        assert!(options("").is_err());
        assert!(options("unknown").is_err());
        assert!(options("load,load")?.validate().is_err());
        Ok(())
    }

    #[test]
    fn selecting_late_actions_runs_without_earlier_operations() -> Result<()> {
        let fixture = Fixture::new(9, 3)?;
        for name in ["owner_replacement", "exact_retry_after_edits"] {
            let selected = options(name)?;
            selected.validate()?;
            let operations: Vec<_> = selected.selected_operations().collect();
            assert_eq!(operations.len(), 1);
            let operation = operations[0];
            let measured = measure(budget(), 0, 1, |budget| {
                let sample = ActionSample::prepare(&fixture.snapshot, operation, budget)?
                    .context("selected action setup expired")?;
                sample.run(operation, budget)
            })?;
            assert_eq!(measured.nanos.len(), 1);
            assert_eq!(measured.warmups, 0);
        }
        Ok(())
    }

    #[test]
    fn selecting_save_or_load_establishes_untimed_prerequisites() -> Result<()> {
        let fixture = Fixture::new(9, 3)?;
        for name in ["durable_save", "load"] {
            let selected = options(name)?;
            selected.validate()?;
            let operations: Vec<_> = selected.selected_operations().collect();
            assert_eq!(operations.len(), 1);
            let root = tempfile::tempdir()?;
            let path = root.path().join("store");
            let budget = budget();
            let mut storage = None;
            prepare_storage(&mut storage, path.clone(), &fixture.snapshot, budget)?;
            let store = storage.as_mut().unwrap();
            assert!(store.published);
            assert_eq!(store.untimed_commits, 2);
            assert_eq!(store.expected.tick, fixture.snapshot.tick + 2);
            assert_eq!(store.store.load()?.unwrap().snapshot, store.expected);
            assert!(store.path.join("snapshot-3.json").is_file());
            let measured = measure(budget, 0, 1, |budget| match operations[0] {
                Operation::DurableSave => store.save(budget),
                Operation::Load => store.load(budget),
                _ => anyhow::bail!("wrong selected storage operation"),
            })?;
            assert_eq!(measured.nanos.len(), 1);
            assert_eq!(store.untimed_commits, 2);
            let tick = store.expected.tick;
            // Reusing the prerequisites must neither republish nor add commits.
            prepare_storage(&mut storage, path, &fixture.snapshot, budget)?;
            assert_eq!(storage.as_ref().unwrap().expected.tick, tick);
        }
        Ok(())
    }

    #[test]
    fn restore_receipt_validation_rejects_every_changed_field_and_wrong_outcome() -> Result<()> {
        let fixture = Fixture::new(9, 3)?;
        let expected =
            prepare_owner_restore(&fixture.snapshot, &fixture::replacement(&fixture.snapshot))?
                .into_snapshot();
        let receipt = expected
            .native_lifecycle
            .as_ref()
            .unwrap()
            .completed
            .as_ref()
            .unwrap();
        for operation in [Operation::Replacement, Operation::ExactRetry] {
            let outcome = |receipt| match operation {
                Operation::Replacement => NativeRestoreResult::AppliedInMemory(receipt),
                _ => NativeRestoreResult::AlreadyCompleted(receipt),
            };
            verify_restore(outcome(receipt.clone()), operation, &expected)?;
            let mutations: [fn(&mut CompletedRestore); 14] = [
                |r| r.sequence += 1,
                |r| r.request_digest = "0".repeat(64),
                |r| r.outcomes.clear(),
                |r| r.outcomes[0].guid += 1,
                |r| r.outcomes[0].target = DomainId(2),
                |r| r.outcomes[0].policy = ResolvePolicy::FindOnly,
                |r| r.outcomes[0].source = RecorderSource::Bound,
                |r| r.outcomes[0].before_presence = Presence::Absent,
                |r| r.outcomes[0].before_incarnation += 1,
                |r| r.outcomes[0].owner.id = DomainId(2),
                |r| r.outcomes[0].owner.incarnation += 1,
                |r| r.outcomes[0].disposition = RestoreDisposition::Attached,
                |r| r.outcomes[0].component_disposition = ComponentDisposition::Created,
                |r| r.outcomes.push(r.outcomes[0].clone()),
            ];
            for mutate in mutations {
                let mut wrong = receipt.clone();
                mutate(&mut wrong);
                assert!(verify_restore(outcome(wrong), operation, &expected).is_err());
            }
        }
        assert!(
            verify_restore(
                NativeRestoreResult::AlreadyCompleted(receipt.clone()),
                Operation::Replacement,
                &expected
            )
            .is_err()
        );
        assert!(
            verify_restore(
                NativeRestoreResult::AppliedInMemory(receipt.clone()),
                Operation::ExactRetry,
                &expected
            )
            .is_err()
        );
        let mut no_receipt = expected.clone();
        no_receipt.native_lifecycle.as_mut().unwrap().completed = None;
        assert!(
            verify_restore(
                NativeRestoreResult::AppliedInMemory(receipt.clone()),
                Operation::Replacement,
                &no_receipt
            )
            .is_err()
        );
        let requested = OwnerHandle {
            id: FOUNDATION,
            incarnation: 1,
        };
        let wrong_owner = ScalarOwnerState {
            owner: OwnerHandle {
                incarnation: 2,
                ..requested
            },
            enabled: true,
            remaining_uses: STARTING_USES,
            available: true,
        };
        assert!(verify_scalar(wrong_owner, requested, true, STARTING_USES).is_err());
        Ok(())
    }

    #[test]
    fn setup_expiry_is_deterministic_at_each_cooperative_boundary() -> Result<()> {
        let fixture = Fixture::new(9, 3)?;
        // Counts pin the guards before activation, both handle lookups, cloning,
        // request/pose construction, and each operation-specific expensive step.
        for (operation, expected_checks) in [
            (Operation::DisabledUse, 9),
            (Operation::Replacement, 9),
            (Operation::ExactRetry, 15),
        ] {
            let mut checks = 0;
            let sample =
                ActionSample::prepare_with_budget_check(&fixture.snapshot, operation, || {
                    checks += 1;
                    true
                })?;
            assert!(sample.is_some());
            assert_eq!(checks, expected_checks);
            for allowed_checks in 0..expected_checks {
                let mut checks = 0;
                let sample =
                    ActionSample::prepare_with_budget_check(&fixture.snapshot, operation, || {
                        checks += 1;
                        checks <= allowed_checks
                    })?;
                assert!(sample.is_none());
                assert_eq!(checks, allowed_checks + 1);
            }
        }
        // No activation may start with an already expired budget, even if the
        // supplied snapshot would fail activation's validation.
        let mut invalid = fixture.snapshot.clone();
        invalid.schema_version = u32::MAX;
        assert!(
            ActionSample::prepare_with_budget_check(&invalid, Operation::DisabledUse, || false)?
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn authored_actions_smoke_and_retry_preserves_later_state() -> Result<()> {
        for schema in 7..=9 {
            let fixture = Fixture::new(schema, 3)?;
            for operation in OPERATIONS {
                if !operation.supported(schema)
                    || matches!(
                        operation,
                        Operation::Capture
                            | Operation::Preflight
                            | Operation::FirstPublication
                            | Operation::DurableSave
                            | Operation::Load
                    )
                {
                    continue;
                }
                let budget = budget();
                let sample = ActionSample::prepare(&fixture.snapshot, operation, budget)?
                    .context("smoke setup expired")?;
                ensure!(
                    sample.run(operation, budget)?.is_some(),
                    "smoke action expired"
                );
            }
            let mut app = build_app(&ServerConfig::default(), &fixture.snapshot)?;
            assert_eq!(capture(app.world_mut())?, fixture.snapshot);
            SnapshotStore::preflight(&fixture.snapshot)?;
        }
        Ok(())
    }

    #[test]
    fn cli_bounds_and_budget_skip_smoke() -> Result<()> {
        let args = [
            "bench",
            "--data-dir",
            "work/authored/absent",
            "--slots",
            "3",
            "--repeats",
            "1",
            "--warmups",
            "0",
            "--samples",
            "1",
        ];
        let mut options = Options::try_parse_from(args)?;
        options.validate()?;
        options.slots = vec![1025];
        assert!(options.validate().is_err());
        options.slots = vec![3];
        options.total_seconds = f64::NAN;
        assert!(options.validate().is_err());
        options.total_seconds = 1.0;
        options.samples = 51;
        assert!(options.validate().is_err());
        let mut budget = budget();
        budget.total_limit = Duration::ZERO;
        let measured = measure(budget, 0, 1, |_| {
            anyhow::bail!("expired budget invoked closure")
        })?;
        assert!(measured.nanos.is_empty());
        let mut samples = Samples {
            nanos: vec![3, 1, 2],
            warmups: 0,
        };
        let summary = samples.summary();
        assert_eq!(summary["median_ns"], 2);
        assert_eq!(summary["p95_ns"], 3);
        Ok(())
    }
}
