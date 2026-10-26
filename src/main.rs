use std::{
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};
use bevy::time::TimeUpdateStrategy;
use clap::{Parser, Subcommand};
use icarus_bevy::{
    config::ConfigOptions,
    network,
    persistence::{SaveReceipt, SaveWorker, SaveWorkerDisconnected, SnapshotStore},
    simulation::{RunClock, SimulationClock, TickMetrics, build_app, capture},
};
use serde_json::json;

#[derive(Parser)]
#[command(
    version,
    about = "Independent headless Bevy foundation; ICARUS connectivity is not implemented"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the headless foundation. Opens no socket unless the experimental,
    /// loopback-only `[network]` session layer is enabled; no gameplay runs.
    Run {
        #[command(flatten)]
        options: ConfigOptions,
        /// Exit after this many additional simulation ticks (smoke testing).
        #[arg(long)]
        max_ticks: Option<u64>,
        /// Qualification only: run the ticks as simulated time, one fixed step
        /// per update without pacing; autosave and status intervals count
        /// simulated seconds and a due autosave awaits one still in flight.
        #[arg(long, requires = "max_ticks")]
        compressed: bool,
    },
    /// Fetch the basic data the server needs from the free official ICARUS
    /// dedicated server (Steam app 2089300) anonymously, without steamcmd,
    /// into `<data-dir>/official/` (F-828, F-831): its data tables
    /// (`Icarus/Content/Data/data.pak`). Unattended; rerunning refreshes it.
    InstallOfficialData {
        #[arg(long)]
        data_dir: PathBuf,
        /// Steam branch of the official server.
        #[arg(long, default_value = "public")]
        branch: String,
    },
    /// Validate and print effective native configuration without writing files.
    CheckConfig {
        #[command(flatten)]
        options: ConfigOptions,
    },
    /// Prepare every placement in an independently authored native map.
    /// This creates spatial state, not an imported or gameplay-ready world.
    CreateNativeWorld {
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        map: String,
        /// Independently authored identities, capabilities and foundation links.
        #[arg(long)]
        relationships: Option<PathBuf>,
        /// Authored owner slots, initial presence and recorder components.
        #[arg(long)]
        lifecycle: Option<PathBuf>,
        /// Apply one complete authored owner restore batch before publication.
        #[arg(long, requires = "lifecycle")]
        restore: Option<PathBuf>,
        /// Enable empty modifier state on every live owner (authored schema 11,
        /// extending schema 7). No C001 import or official semantics are implied.
        #[arg(long, requires = "lifecycle")]
        modifiers: bool,
        #[arg(long)]
        data_dir: PathBuf,
        /// Validate and report counts without opening the destination store.
        #[arg(long)]
        dry_run: bool,
    },
    /// Apply one strict authored modifier request to an existing native world.
    /// Publication follows complete validation, runtime application and capture.
    ApplyNativeModifier {
        #[arg(long)]
        request: PathBuf,
        #[arg(long)]
        data_dir: PathBuf,
    },
    /// Materialize only the explicitly bound, authored C001 inert fixture
    /// contract. This does not enable ordinary ICARUS save import.
    CreateC001InertFixture {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        bindings: PathBuf,
        /// Independently authored links; these do not interpret source relationships.
        #[arg(long)]
        relationships: Option<PathBuf>,
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore an explicitly bound authored C001 fixture through native owner
    /// find, spawn and replacement. Ordinary ICARUS save import stays closed.
    CreateC001OwnerFixture {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        lifecycle: PathBuf,
        #[arg(long)]
        bindings: PathBuf,
        /// Independently authored links; source relationship state is unsupported.
        #[arg(long)]
        relationships: Option<PathBuf>,
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore an authored Bool/I32 owner contract with a native limited-use consumer.
    CreateC001ScalarOwnerFixture {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        lifecycle: PathBuf,
        #[arg(long)]
        contracts: PathBuf,
        #[arg(long)]
        bindings: PathBuf,
        #[arg(long)]
        relationships: Option<PathBuf>,
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Restore an authored inventory and scalar owner contract from a strict fixture.
    CreateC001InventoryOwnerFixture {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        catalog: PathBuf,
        #[arg(long)]
        lifecycle: PathBuf,
        #[arg(long)]
        scalar_contracts: PathBuf,
        #[arg(long)]
        inventory_contracts: PathBuf,
        #[arg(long)]
        bindings: PathBuf,
        #[arg(long)]
        relationships: Option<PathBuf>,
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    /// Inspect an experimental prospect envelope without importing or writing state.
    InspectSave {
        #[arg(long)]
        source: PathBuf,
        /// Also frame opaque root properties with the experimental memory-522 profile.
        #[arg(long)]
        inner_tags: bool,
        /// Also frame recorder fields, leaving component bytes opaque; implies root framing.
        #[arg(long)]
        recorders: bool,
        /// Stage and validate the `FProspectSaveState` root schema, version
        /// migration (`F-715`), `ProspectInfo` inner tags (`F-726`), and bounded
        /// base-component framing of every `StateRecorderBlob` (`F-403`, `F-733`)
        /// without importing or writing state.
        #[arg(
            long,
            conflicts_with_all = ["inner_tags", "recorders", "preserve_check"]
        )]
        stage_check: bool,
        /// Optional expected prospect name (normalizes an ASCII case-insensitive
        /// `.json` suffix via `F-715`) to compare against `ProspectInfo.ProspectID`
        /// (`F-717`) during `--stage-check`.
        #[arg(long, requires = "stage_check")]
        expect_prospect_name: Option<String>,
        /// Preserve the payload losslessly, then save and reload it through a
        /// temporary native schema-3 store; reports counts, sizes, timings and
        /// the resident set with and without the decoded tree (Linux only).
        #[arg(
            long,
            conflicts_with_all = ["inner_tags", "recorders", "stage_check"]
        )]
        preserve_check: bool,
        /// Stage the prospect, then type every recorder body's values and
        /// require each to re-encode exactly; reports typed and uninterpreted
        /// leaf counts per class pseudonym without importing or writing state.
        #[arg(
            long,
            conflicts_with_all = ["inner_tags", "recorders", "stage_check", "preserve_check"]
        )]
        typed_check: bool,
        /// Decode one import candidate under default allocation allowances;
        /// report counts and remaining allowance, not RSS or import readiness.
        #[arg(
            long,
            conflicts_with_all = ["inner_tags", "recorders", "stage_check", "preserve_check", "typed_check", "expect_prospect_name"]
        )]
        candidate_check: bool,
    },
    /// Seed a NEW native world whose only content is the lossless, value-opaque
    /// preservation of a supplied prospect. Nothing is interpreted or imported;
    /// an existing world is never overwritten.
    PreserveSave {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        data_dir: PathBuf,
    },
    /// Plan a native import of a supplied prospect. `--dry-run` prints the
    /// plan (counts only); without it the import is refused while the plan has
    /// blockers. `--catalog` resolves references against a native content
    /// catalog file (empty without it); `--data-pak` also resolves data-row
    /// references against the official server's data tables (F-840).
    /// Catalog format 2 scopes paths/tiles to maps and variables to classes.
    /// Nothing is ever written; the destination is not touched.
    ImportSave {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        destination: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        catalog: Option<PathBuf>,
        #[arg(long)]
        data_pak: Option<PathBuf>,
    },
}

fn main() -> std::process::ExitCode {
    match execute(Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "{}",
                json!({"event": "fatal", "error": format!("{error:#}")})
            );
            std::process::ExitCode::FAILURE
        }
    }
}

fn execute(cli: Cli) -> Result<()> {
    match cli.command {
        Command::InstallOfficialData { data_dir, branch } => {
            println!("{}", install_official_data(&data_dir, &branch)?);
            Ok(())
        }
        Command::CheckConfig { options } => {
            println!("{}", toml::to_string_pretty(&options.resolve()?)?);
            Ok(())
        }
        Command::CreateNativeWorld {
            catalog,
            map,
            relationships,
            lifecycle,
            restore,
            modifiers,
            data_dir,
            dry_run,
        } => {
            use icarus_bevy::native_world::{CompiledNativeCatalog, prepare_authored_world};
            // All semantic rejection precedes the destination lock/directory.
            let catalog = Arc::new(CompiledNativeCatalog::load(&catalog)?);
            let prepared = prepare_authored_world(Arc::clone(&catalog), &map)?;
            let base = prepared.initial_snapshot();
            let related = relationships
                .map(|path| {
                    use icarus_bevy::native_relationships::{
                        CompiledNativeRelationships, prepare_relationship_world,
                    };
                    let declarations = CompiledNativeRelationships::load(&path, &catalog)?;
                    prepare_relationship_world(base.clone(), &declarations)
                })
                .transpose()?;
            let snapshot = related
                .as_ref()
                .map_or(&base, |prepared| prepared.snapshot());
            let with_lifecycle = lifecycle
                .map(|path| {
                    use icarus_bevy::native_lifecycle::{
                        CompiledNativeLifecycle, prepare_lifecycle_world,
                    };
                    let declarations = CompiledNativeLifecycle::load(&path, &catalog)?;
                    prepare_lifecycle_world(snapshot.clone(), &declarations)
                })
                .transpose()?;
            let snapshot = with_lifecycle
                .as_ref()
                .map_or(snapshot, |prepared| prepared.snapshot());
            let restored = restore
                .map(|path| {
                    use icarus_bevy::native_lifecycle::{RestoreBatch, prepare_owner_restore};
                    prepare_owner_restore(snapshot, &RestoreBatch::load(&path)?)
                })
                .transpose()?;
            let snapshot = restored
                .as_ref()
                .map_or(snapshot, |prepared| prepared.snapshot());
            let with_modifiers = modifiers
                .then(|| icarus_bevy::native_modifiers::enable_modifier_owners(snapshot.clone()))
                .transpose()?;
            let snapshot = with_modifiers.as_ref().unwrap_or(snapshot);
            SnapshotStore::preflight(snapshot)?;
            let actors = prepared.snapshot().actors.len();
            let live_actors = snapshot.native_lifecycle.as_ref().map(|lifecycle| {
                lifecycle
                    .slots
                    .iter()
                    .filter(|slot| slot.presence == icarus_bevy::native_lifecycle::Presence::Live)
                    .count()
            });
            if dry_run {
                let mut report = json!({"event": "native_world_prepared", "actors": actors,
                    "schema_version": snapshot.schema_version, "dry_run": true,
                    "world_imported": false, "gameplay_ready": false});
                if let Some(live_actors) = live_actors {
                    report["live_actors"] = json!(live_actors);
                }
                println!("{report}");
            } else {
                let mut store = SnapshotStore::open(&data_dir)?;
                let receipt = store.save_new(snapshot)?;
                let mut report = json!({"event": "native_world_created", "actors": actors,
                    "schema_version": snapshot.schema_version, "receipt": receipt,
                    "world_imported": false, "gameplay_ready": false});
                if let Some(live_actors) = live_actors {
                    report["live_actors"] = json!(live_actors);
                }
                println!("{report}");
            }
            Ok(())
        }
        Command::ApplyNativeModifier { request, data_dir } => {
            let request = read_native_modifier_request(&request)?;
            ensure!(
                data_dir.is_dir(),
                "modifier command requires an existing store"
            );
            let mut store = SnapshotStore::open(&data_dir)
                .map_err(|_| anyhow::anyhow!("open native modifier store failed"))?;
            let loaded = store
                .load()
                .map_err(|_| anyhow::anyhow!("load native modifier world failed"))?
                .context("modifier command requires an existing committed world")?;
            let mut app = build_app(
                &icarus_bevy::config::ServerConfig::default(),
                &loaded.snapshot,
            )
            .map_err(|_| anyhow::anyhow!("activate native modifier world failed"))?;
            icarus_bevy::native_lifecycle_runtime::apply_native_modifier(app.world_mut(), &request)
                .map_err(|_| anyhow::anyhow!("native modifier request rejected"))?;
            let snapshot = capture(app.world_mut())
                .map_err(|_| anyhow::anyhow!("capture native modifier world failed"))?;
            let receipt = store
                .save(&snapshot)
                .map_err(|_| anyhow::anyhow!("save native modifier world failed"))?;
            println!(
                "{}",
                json!({"event": "native_modifier_applied", "receipt": receipt,
                    "world_imported": false, "gameplay_ready": false})
            );
            Ok(())
        }
        Command::CreateC001InertFixture {
            source,
            catalog,
            bindings,
            relationships,
            data_dir,
            dry_run,
        } => {
            use icarus_bevy::{
                import_candidate::DecodedImportCandidate,
                native_import::{
                    CompiledC001InertBindings, prepare_c001_inert_import,
                    publish_new_c001_inert_import,
                },
                native_world::CompiledNativeCatalog,
            };
            let catalog = Arc::new(CompiledNativeCatalog::load(&catalog)?);
            let bindings = CompiledC001InertBindings::load(&bindings)?;
            let candidate = DecodedImportCandidate::decode(open_regular_source(&source)?)?;
            let prepared = prepare_c001_inert_import(&candidate, Arc::clone(&catalog), &bindings)?;
            let related = relationships
                .map(|path| {
                    use icarus_bevy::native_relationships::{
                        CompiledNativeRelationships, prepare_relationship_world,
                    };
                    let declarations = CompiledNativeRelationships::load(&path, &catalog)?;
                    prepare_relationship_world(prepared.snapshot().clone(), &declarations)
                })
                .transpose()?;
            let schema_version = related
                .as_ref()
                .map_or(prepared.snapshot().schema_version, |prepared| {
                    prepared.snapshot().schema_version
                });
            let inspection = prepared.inspection();
            if dry_run {
                println!(
                    "{}",
                    json!({"event": "c001_inert_fixture_prepared", "dry_run": true,
                    "inspection": inspection, "schema_version": schema_version,
                    "world_imported": false, "gameplay_ready": false})
                );
            } else {
                let receipt = if let Some(related) = related {
                    icarus_bevy::native_relationships::publish_new_relationship_world(
                        related, &data_dir,
                    )?
                } else {
                    publish_new_c001_inert_import(prepared, &data_dir)?
                };
                println!(
                    "{}",
                    json!({"event": "c001_inert_fixture_materialized", "inspection": inspection,
                    "receipt": receipt, "schema_version": schema_version,
                    "world_imported": false, "gameplay_ready": false})
                );
            }
            Ok(())
        }
        Command::CreateC001OwnerFixture {
            source,
            catalog,
            lifecycle,
            bindings,
            relationships,
            data_dir,
            dry_run,
        } => {
            use icarus_bevy::{
                import_candidate::DecodedImportCandidate,
                native_c001_owners::{
                    CompiledC001OwnerBindings, prepare_c001_owner_import,
                    publish_new_c001_owner_import,
                },
                native_lifecycle::{CompiledNativeLifecycle, prepare_lifecycle_world},
                native_relationships::{CompiledNativeRelationships, prepare_relationship_world},
                native_world::{CompiledNativeCatalog, prepare_authored_world},
            };
            let catalog = Arc::new(CompiledNativeCatalog::load(&catalog)?);
            let bindings = CompiledC001OwnerBindings::load(&bindings)?;
            let mut base =
                prepare_authored_world(Arc::clone(&catalog), &bindings.definition().native_map)?
                    .initial_snapshot();
            if let Some(path) = relationships {
                let declarations = CompiledNativeRelationships::load(&path, &catalog)?;
                base = prepare_relationship_world(base, &declarations)?
                    .snapshot()
                    .clone();
            }
            let declarations = CompiledNativeLifecycle::load(&lifecycle, &catalog)?;
            let base = prepare_lifecycle_world(base, &declarations)?.into_snapshot();
            let candidate = DecodedImportCandidate::decode(open_regular_source(&source)?)?;
            let prepared = prepare_c001_owner_import(&candidate, base, &bindings)?;
            let inspection = prepared.inspection();
            let schema_version = prepared.snapshot().schema_version;
            if dry_run {
                println!(
                    "{}",
                    json!({"event": "c001_owner_fixture_prepared", "dry_run": true,
                    "inspection": inspection, "schema_version": schema_version,
                    "world_imported": false, "gameplay_ready": false})
                );
            } else {
                let receipt = publish_new_c001_owner_import(prepared, &data_dir)?;
                println!(
                    "{}",
                    json!({"event": "c001_owner_fixture_materialized",
                    "inspection": inspection, "receipt": receipt, "schema_version": schema_version,
                    "world_imported": false, "gameplay_ready": false})
                );
            }
            Ok(())
        }
        Command::CreateC001ScalarOwnerFixture {
            source,
            catalog,
            lifecycle,
            contracts,
            bindings,
            relationships,
            data_dir,
            dry_run,
        } => {
            use icarus_bevy::{
                import_candidate::DecodedImportCandidate,
                native_c001_scalar_owners::{
                    CompiledC001ScalarOwnerBindings, prepare_c001_scalar_owner_import,
                    publish_new_c001_scalar_owner_import,
                },
                native_lifecycle::{CompiledNativeLifecycle, prepare_lifecycle_world},
                native_relationships::{CompiledNativeRelationships, prepare_relationship_world},
                native_scalar_owners::CompiledScalarOwnerContracts,
                native_world::{CompiledNativeCatalog, prepare_authored_world},
            };
            let catalog = Arc::new(CompiledNativeCatalog::load(&catalog)?);
            let bindings = CompiledC001ScalarOwnerBindings::load(&bindings)?;
            let contracts = CompiledScalarOwnerContracts::load(&contracts, &catalog)?;
            let mut base =
                prepare_authored_world(Arc::clone(&catalog), &bindings.definition().native_map)?
                    .initial_snapshot();
            if let Some(path) = relationships {
                let declarations = CompiledNativeRelationships::load(&path, &catalog)?;
                base = prepare_relationship_world(base, &declarations)?
                    .snapshot()
                    .clone();
            }
            let declarations = CompiledNativeLifecycle::load(&lifecycle, &catalog)?;
            let base = prepare_lifecycle_world(base, &declarations)?.into_snapshot();
            let candidate = DecodedImportCandidate::decode(open_regular_source(&source)?)?;
            let prepared =
                prepare_c001_scalar_owner_import(&candidate, base, &bindings, &contracts)?;
            let inspection = serde_json::to_value(prepared.inspection())?;
            let schema_version = prepared.snapshot().schema_version;
            if dry_run {
                println!(
                    "{}",
                    json!({"event":"c001_scalar_owner_fixture_prepared", "dry_run":true,
                    "inspection":inspection,"schema_version":schema_version,"world_imported":false,"gameplay_ready":false})
                );
            } else {
                let receipt = publish_new_c001_scalar_owner_import(prepared, &data_dir)?;
                println!(
                    "{}",
                    json!({"event":"c001_scalar_owner_fixture_materialized", "receipt":receipt,
                    "inspection":inspection,"schema_version":schema_version,"world_imported":false,"gameplay_ready":false})
                );
            }
            Ok(())
        }
        Command::CreateC001InventoryOwnerFixture {
            source,
            catalog,
            lifecycle,
            scalar_contracts,
            inventory_contracts,
            bindings,
            relationships,
            data_dir,
            dry_run,
        } => {
            use icarus_bevy::{
                import_candidate::DecodedImportCandidate,
                native_c001_inventory_owners::{
                    CompiledC001InventoryOwnerBindings, prepare_c001_inventory_owner_import,
                    publish_new_c001_inventory_owner_import,
                },
                native_inventory::CompiledInventoryContractsV1,
                native_lifecycle::{CompiledNativeLifecycle, prepare_lifecycle_world},
                native_relationships::{CompiledNativeRelationships, prepare_relationship_world},
                native_scalar_owners::CompiledScalarOwnerContracts,
                native_world::{CompiledNativeCatalog, prepare_authored_world},
            };
            let catalog = Arc::new(CompiledNativeCatalog::load(&catalog)?);
            let bindings = CompiledC001InventoryOwnerBindings::load(&bindings)?;
            let scalar = CompiledScalarOwnerContracts::load(&scalar_contracts, &catalog)?;
            let inventory = CompiledInventoryContractsV1::load(&inventory_contracts, &catalog)?;
            let mut base =
                prepare_authored_world(Arc::clone(&catalog), &bindings.definition().native_map)?
                    .initial_snapshot();
            if let Some(path) = relationships {
                let declarations = CompiledNativeRelationships::load(&path, &catalog)?;
                base = prepare_relationship_world(base, &declarations)?
                    .snapshot()
                    .clone();
            }
            let lifecycle = CompiledNativeLifecycle::load(&lifecycle, &catalog)?;
            let base = prepare_lifecycle_world(base, &lifecycle)?.into_snapshot();
            let candidate = DecodedImportCandidate::decode(open_regular_source(&source)?)?;
            let prepared = prepare_c001_inventory_owner_import(
                &candidate, base, &bindings, &scalar, &inventory,
            )?;
            let inspection = serde_json::to_value(prepared.inspection())?;
            let schema_version = prepared.snapshot().schema_version;
            if dry_run {
                println!(
                    "{}",
                    json!({"event":"c001_inventory_owner_fixture_prepared", "dry_run":true,
                    "inspection":inspection,"schema_version":schema_version,"world_imported":false,"gameplay_ready":false})
                );
            } else {
                let receipt = publish_new_c001_inventory_owner_import(prepared, &data_dir)?;
                println!(
                    "{}",
                    json!({"event":"c001_inventory_owner_fixture_materialized", "receipt":receipt,
                    "inspection":inspection,"schema_version":schema_version,"world_imported":false,"gameplay_ready":false})
                );
            }
            Ok(())
        }
        Command::PreserveSave { source, data_dir } => {
            let file = open_regular_source(&source)?;
            println!("{}", preserve_save(file, &data_dir)?);
            Ok(())
        }
        Command::ImportSave {
            source,
            dry_run,
            catalog,
            data_pak,
            ..
        } => {
            use icarus_bevy::import_plan::{ContentCatalog, plan_import};
            let mut catalog = match catalog {
                Some(path) => ContentCatalog::load(&path)?,
                None => ContentCatalog::empty(),
            };
            if let Some(path) = data_pak {
                let mut pak = icarus_bevy::data_pak::DataPak::open(&path)
                    .with_context(|| format!("open --data-pak {}", path.display()))?;
                let (with_rows, tables, skipped) = catalog
                    .with_data_tables(&mut pak)
                    .with_context(|| format!("read --data-pak {}", path.display()))?;
                catalog = with_rows;
                eprintln!(
                    "{}",
                    json!({"event": "data_tables_loaded", "tables": tables, "skipped_members": skipped})
                );
            }
            let file = open_regular_source(&source)?;
            let plan = plan_import(file, &catalog)?;
            if dry_run {
                println!("{}", serde_json::to_string(&plan)?);
                return Ok(());
            }
            ensure!(
                plan.importable,
                "import blocked by {}; run with --dry-run for the plan; no files were written",
                serde_json::to_string(&plan.blockers)?
            );
            bail!("native world import writing is not implemented; no files were written")
        }
        Command::InspectSave {
            source,
            inner_tags,
            recorders,
            stage_check,
            expect_prospect_name,
            preserve_check,
            typed_check,
            candidate_check,
        } => {
            // Check the opened handle: a path can change to a FIFO after a
            // regular-file precheck. Normal regular-file symlinks still work.
            let file = open_regular_source(&source)?;
            let output = if candidate_check {
                let candidate =
                    icarus_bevy::import_candidate::DecodedImportCandidate::decode(file)?;
                serde_json::to_string(&candidate.inspection())?
            } else if typed_check {
                serde_json::to_string(&icarus_bevy::prospect::typed_check(file)?)?
            } else if preserve_check {
                serde_json::to_string(&preserve_check_report(file)?)?
            } else if stage_check {
                serde_json::to_string(&icarus_bevy::prospect::stage_prospect(
                    file,
                    icarus_bevy::experimental::component::MAX_BOUNDED_COMPONENT_BYTES,
                    expect_prospect_name.as_deref(),
                )?)?
            } else if recorders {
                serde_json::to_string(&icarus_bevy::prospect::inspect_with_recorders(file)?)?
            } else if inner_tags {
                serde_json::to_string(&icarus_bevy::prospect::inspect_with_inner_tags(file)?)?
            } else {
                serde_json::to_string(&icarus_bevy::prospect::inspect(file)?)?
            };
            println!("{output}");
            Ok(())
        }
        Command::Run {
            options,
            max_ticks,
            compressed,
        } => run(options, max_ticks, compressed),
    }
}

/// Store files after a save: current generation bytes and every sidecar's
/// (name, bytes, inode, modification time in nanoseconds).
type StoreFiles = (u64, Vec<(String, u64, u64, i128)>);

fn store_files(root: &std::path::Path, generation: u64) -> Result<StoreFiles> {
    use std::os::unix::fs::MetadataExt;

    let generation_bytes =
        std::fs::metadata(root.join(format!("snapshot-{generation}.json")))?.len();
    let mut sidecars = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if icarus_bevy::preservation::is_sidecar_name(&name) {
            let metadata = entry.metadata()?;
            let modified =
                i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
            sidecars.push((name, metadata.len(), metadata.ino(), modified));
        }
    }
    Ok((generation_bytes, sidecars))
}

/// Open a supplied envelope nonblocking and admit only an opened regular file.
fn open_regular_source(source: &std::path::Path) -> Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(icarus_bevy::nonblocking_flag())
        .open(source)
        .context("open prospect envelope")?;
    ensure!(file.metadata()?.is_file(), "source must be a regular file");
    Ok(file)
}

/// Request inputs are bounded independently of the saved world. Resolve each
/// component through an opened directory so intermediate symlinks are refused
/// too. NONBLOCK prevents a FIFO from stalling before the handle's type check.
fn read_native_modifier_request(
    source: &std::path::Path,
) -> Result<icarus_bevy::native_modifiers::ModifierRequest> {
    use rustix::fs::{Mode, OFlags, open, openat};
    use std::io::Read;

    const MAX_REQUEST_BYTES: u64 = 64 * 1024;
    let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let mut directory = open(
        if source.is_absolute() { "/" } else { "." },
        flags | OFlags::DIRECTORY,
        Mode::empty(),
    )
    .map_err(|_| anyhow::anyhow!("open modifier request directory failed"))?;
    let mut components = source
        .components()
        .filter(|component| {
            !matches!(
                component,
                std::path::Component::RootDir | std::path::Component::CurDir
            )
        })
        .peekable();
    let mut file = loop {
        let component = components
            .next()
            .context("modifier request path is empty")?;
        let last = components.peek().is_none();
        let handle = openat(
            &directory,
            component.as_os_str(),
            if last {
                flags
            } else {
                flags | OFlags::DIRECTORY
            },
            Mode::empty(),
        )
        .map_err(|_| anyhow::anyhow!("open native modifier request failed"))?;
        if last {
            break std::fs::File::from(handle);
        }
        directory = handle;
    };
    let metadata = file.metadata().context("inspect native modifier request")?;
    ensure!(
        metadata.is_file(),
        "modifier request must be a regular file"
    );
    ensure!(
        metadata.len() <= MAX_REQUEST_BYTES,
        "modifier request exceeds 64 KiB limit"
    );
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_REQUEST_BYTES)
        .read_to_end(&mut bytes)
        .context("read native modifier request")?;
    // Recheck growth without ever reading more than the 64 KiB allowance.
    ensure!(
        file.metadata()
            .context("reinspect native modifier request")?
            .len()
            <= MAX_REQUEST_BYTES,
        "modifier request exceeds 64 KiB limit"
    );
    // Serde diagnostics can contain caller-controlled values and field names.
    serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid native modifier request JSON"))
}

/// Preserve a supplied envelope into a fresh native world at tick 0. The
/// directory may exist but must hold no committed world.
fn preserve_save(file: std::fs::File, data_dir: &std::path::Path) -> Result<serde_json::Value> {
    use icarus_bevy::{
        preservation::PreserveLimits,
        simulation::{SNAPSHOT_SCHEMA_REFERENCED, WorldSnapshot},
    };

    let preserved = icarus_bevy::prospect::preserve(file, PreserveLimits::default())?;
    let summary = serde_json::to_value(preserved.preserved.summary())?;
    let mut store = SnapshotStore::open(data_dir)?;
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_REFERENCED,
        preserved_prospect: Some(std::sync::Arc::new(preserved.preserved)),
        ..WorldSnapshot::default()
    };
    let receipt = store.save_new(&snapshot)?;
    Ok(json!({
        "event": "preserved_world_created",
        "validation": "experimental_lossless_preservation",
        "envelope": preserved.envelope,
        "preservation": summary,
        "native_schema": SNAPSHOT_SCHEMA_REFERENCED,
        "generation": receipt.generation,
        "world_imported": false,
        "gameplay_ready": false,
    }))
}

/// Resident set size of this process from `/proc/self/status` (`VmRSS`), or
/// `None` where that interface is unavailable.
fn resident_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?;
    let kibibytes = line.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()?;
    kibibytes.checked_mul(1024)
}

/// Preserve a supplied envelope and prove the native persistence round trip in
/// a private temporary store that is removed afterwards. Nothing is imported.
/// A second save shows the write-once sidecar is reused rather than rewritten.
/// The reloaded compact section is then decoded on demand to its full tree to
/// report the resident set with and without that tree.
fn preserve_check_report(file: std::fs::File) -> Result<serde_json::Value> {
    use icarus_bevy::{
        persistence::SnapshotStore,
        preservation::PreserveLimits,
        simulation::{SNAPSHOT_SCHEMA_REFERENCED, WorldSnapshot},
    };
    use std::{sync::Arc, time::Instant};

    let started = Instant::now();
    let preserved = icarus_bevy::prospect::preserve(file, PreserveLimits::default())?;
    let preserve_ms = started.elapsed().as_millis();
    let payload_sha256 = preserved.preserved.payload_sha256();
    let summary = serde_json::to_value(preserved.preserved.summary())?;
    let mut snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_REFERENCED,
        preserved_prospect: Some(Arc::new(preserved.preserved)),
        ..WorldSnapshot::default()
    };
    let original = snapshot
        .preserved_prospect
        .clone()
        .context("preserved section missing before save")?;
    let directory = tempfile::tempdir().context("create temporary native store")?;
    let mut store = SnapshotStore::open(directory.path())?;
    let saved = Instant::now();
    let first = store.save(&snapshot)?;
    let save_ms = saved.elapsed().as_millis();
    let (generation_bytes, sidecars) = store_files(directory.path(), first.generation)?;
    ensure!(
        sidecars.len() == 1,
        "expected exactly one preserved sidecar"
    );
    snapshot.tick += 1;
    let saved = Instant::now();
    let second = store.save(&snapshot)?;
    let second_save_ms = saved.elapsed().as_millis();
    let (second_generation_bytes, after) = store_files(directory.path(), second.generation)?;
    ensure!(
        after == sidecars,
        "second save rewrote the preserved sidecar"
    );
    let loaded_at = Instant::now();
    let loaded = store
        .load()?
        .context("saved native generation is missing")?;
    let load_ms = loaded_at.elapsed().as_millis();
    let reloaded = loaded
        .snapshot
        .preserved_prospect
        .context("reloaded generation lost its preserved section")?;
    ensure!(*reloaded == *original, "reloaded preserved section differs");
    drop((snapshot, original));
    let section_only = resident_bytes();
    let decoded_at = Instant::now();
    // Verified again: re-hashed, bounded decode and byte-identical reconstruction.
    let tree = reloaded.decode()?;
    let decode_ms = decoded_at.elapsed().as_millis();
    let with_tree = resident_bytes();
    ensure!(
        tree.payload_sha256() == payload_sha256 && tree.payload_bytes() == reloaded.payload_bytes(),
        "decoded preserved section differs"
    );
    drop(tree);
    let after_tree_dropped = resident_bytes();
    let held_section_bytes = reloaded.held_bytes();
    drop(store);
    directory.close().context("remove temporary native store")?;
    Ok(json!({
        "validation": "experimental_lossless_preservation",
        "envelope": preserved.envelope,
        "preservation": summary,
        "native_schema": SNAPSHOT_SCHEMA_REFERENCED,
        "native_generation_bytes": generation_bytes,
        "native_second_generation_bytes": second_generation_bytes,
        "native_sidecar_bytes": sidecars[0].1,
        "native_sidecar_rewritten": false,
        "native_round_trip": true,
        "preserve_ms": preserve_ms, "save_ms": save_ms,
        "second_save_ms": second_save_ms, "load_ms": load_ms, "decode_ms": decode_ms,
        "held_section_bytes": held_section_bytes,
        // Linux VmRSS; null elsewhere. Freed memory may stay with the allocator.
        "resident_bytes": {
            "section_only": section_only,
            "with_decoded_tree": with_tree,
            "after_tree_dropped": after_tree_dropped,
        },
        "world_imported": false,
    }))
}

/// `submitted` is when the committed snapshot was handed to the save worker;
/// the latency covers queueing, serialization and durable publication.
fn saved(receipt: SaveReceipt, submitted: Option<Instant>) {
    eprintln!(
        "{}",
        json!({"event": "save_committed", "generation": receipt.generation, "tick": receipt.tick,
        "save_latency_micros": submitted.map(|time| time.elapsed().as_micros())})
    );
}

/// Autosave bookkeeping shared by the loop and a compressed run's wait.
struct Autosave {
    submitted: Option<Instant>,
    last_commit: Option<Duration>,
    failures: u64,
}

impl Autosave {
    /// Reports one poll result; only a lost worker is fatal.
    fn observe(&mut self, polled: Result<Option<SaveReceipt>>, now: Duration) -> Result<bool> {
        match polled {
            Ok(Some(receipt)) => {
                self.last_commit = Some(now);
                saved(receipt, self.submitted.take());
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(error) => {
                if error.is::<SaveWorkerDisconnected>() {
                    return Err(error.context("stop simulation after terminal save-worker failure"));
                }
                self.submitted = None;
                self.failures += 1;
                eprintln!(
                    "{}",
                    json!({"event": "save_failed", "error": format!("{error:#}")})
                );
                Ok(true)
            }
        }
    }
}

/// Fetch the official server's data tables into `<data_dir>/official/` and
/// record what was fetched beside them.
fn install_official_data(data_dir: &std::path::Path, branch: &str) -> Result<serde_json::Value> {
    use icarus_bevy::steam_fetch::{FetchRequest, hex, resolve};
    let official = data_dir.join(network::OFFICIAL_DIR);
    std::fs::create_dir_all(&official).context("create the official data directory")?;
    let request = FetchRequest::new(
        network::OFFICIAL_SERVER_APP_ID,
        branch,
        Some(network::OFFICIAL_DATA_PAK),
    );
    let progress =
        |message: &str| eprintln!("{}", json!({"event": "fetch_progress", "message": message}));
    let resolved = resolve(&request, &progress)?;
    let file = resolved
        .file(&request)?
        .context("the official server lists no data.pak")?;
    let sha1 = hex(&file.sha_content);
    let written = resolved.download(&request, &official)?;
    let record = json!({
        "event": "official_data_installed",
        "app": network::OFFICIAL_SERVER_APP_ID,
        "branch": branch,
        "build_id": resolved.build_id,
        "depot": resolved.depot_id,
        "manifest_gid": resolved.manifest_gid.to_string(),
        "file": network::OFFICIAL_DATA_PAK,
        "size": written.size,
        "sha1": sha1,
        "sha256": written.sha256,
    });
    network::write_install_record(&official, &record)?;
    Ok(record)
}

fn with_native_replication_counts(
    mut record: serde_json::Value,
    world: &bevy::prelude::World,
) -> serde_json::Value {
    if let Some(counts) =
        world.get_resource::<icarus_bevy::native_replication::NativeReplicationCounts>()
    {
        record["native_replication"] = json!(counts);
    }
    record
}

fn run(options: ConfigOptions, max_ticks: Option<u64>, compressed: bool) -> Result<()> {
    let mut config = options.resolve()?;
    // The official data tables an install step fetched into the data
    // directory serve when no data pak is configured.
    let mut data_pak_source = config.network.data_pak.as_ref().map(|_| "configured");
    if config.network.enabled && config.network.data_pak.is_none() {
        let installed = config
            .data_dir
            .join(network::OFFICIAL_DIR)
            .join(network::OFFICIAL_DATA_PAK);
        if installed.is_file() {
            config.network.data_pak = Some(installed);
            data_pak_source = Some("installed");
        }
    }
    ensure!(max_ticks != Some(0), "max_ticks must be positive");
    ensure!(
        !compressed || max_ticks.is_some(),
        "a compressed run needs max_ticks"
    );
    // Load the release profile and bind before any world state opens: either
    // failure exits without creating, locking or loading the data directory.
    let bound = network::bind(&config.network)?;
    let pinned_build = bound
        .as_ref()
        .and_then(|bound| bound.release.as_ref())
        .map(|pin| pin.build_id);
    let store = SnapshotStore::open(&config.data_dir)?;
    let loaded = store.load()?;
    if loaded
        .as_ref()
        .is_some_and(|loaded| loaded.recovered_previous)
    {
        eprintln!(
            "{}",
            json!({"event": "recovered_previous_generation", "warning": "current snapshot was unusable; restored preceding committed state"})
        );
    }
    let snapshot = loaded.map(|loaded| loaded.snapshot).unwrap_or_default();
    let target_tick = max_ticks
        .map(|ticks| {
            snapshot
                .tick
                .checked_add(ticks)
                .context("max_ticks overflows simulation clock")
        })
        .transpose()?;
    let mut app = build_app(&config, &snapshot)?;
    if compressed {
        app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    }
    let mut bound = bound;
    if let Some(bound) = &mut bound
        && bound.lab.has_game_state_open()
    {
        network::apply_server_settings(&mut bound.lab, config.autosave_seconds)?;
    }
    if let (Some(bound), Some(section)) = (&mut bound, &snapshot.preserved_prospect)
        && bound.lab.has_game_state_open()
    {
        // A loaded world's prospect and game mode record replicate in the
        // GameState open; a prospect that cannot supply them leaves the
        // lab's values alone.
        match network::apply_saved_game_state(
            &mut bound.lab,
            section,
            bound.tables.as_mut(),
            std::mem::take(&mut bound.saved_managers),
        ) {
            Ok(applied) => {
                let channel = |wanted| {
                    applied
                        .saved_managers
                        .iter()
                        .find_map(|(slot, channel)| (*slot == wanted).then_some(*channel))
                };
                eprintln!(
                    "{}",
                    json!({"event": "active_prospect_replicated", "unmapped_session_flags": applied.unmapped_session_flags, "locked_missions": applied.locked_missions, "quest_channels": applied.quest_channels, "skipped_quests": applied.skipped_quests.as_ref().map(|(count, reason)| json!({"count": count, "reason": reason})), "quest_manager_channel": channel(icarus_bevy::experimental::manager_open::ManagerSlot::Quest), "player_history_channel": channel(icarus_bevy::experimental::manager_open::ManagerSlot::PlayerHistory), "container_manager_channel": channel(icarus_bevy::experimental::manager_open::ManagerSlot::InventoryContainer), "flod_channel": channel(icarus_bevy::experimental::manager_open::ManagerSlot::Flod), "skipped_managers": applied.skipped_managers.iter().map(|(slot, reason)| json!({"manager": format!("{slot:?}"), "reason": reason})).collect::<Vec<_>>()})
                );
            }
            // Reader errors name fields and kinds, never saved values.
            Err(error) => eprintln!(
                "{}",
                json!({"event": "active_prospect_unavailable", "warning": "the preserved prospect supplies no replicated active prospect", "error": format!("{error:#}")})
            ),
        }
    }
    let network_address = bound
        .map(|bound| network::install_configured(&mut app, bound, &config.network))
        .transpose()?;
    app.finish();
    app.cleanup();
    let mut writer = SaveWorker::spawn(store)?;
    let stopping = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stopping);
    ctrlc::set_handler(move || signal.store(true, Ordering::SeqCst))
        .context("install SIGINT/SIGTERM handler")?;

    let clock = RunClock {
        compressed,
        start: Instant::now(),
        start_tick: snapshot.tick,
        tick_hz: config.tick_hz,
    };
    let mut last_save_request = Duration::ZERO;
    let mut last_status = Duration::ZERO;
    let mut autosave = Autosave {
        submitted: None,
        last_commit: None,
        failures: 0,
    };
    let mut clamped_frames = 0_u64;
    let mut previous_frame = Instant::now();
    let interval = config.tick_interval();
    eprintln!(
        "{}",
        json!({"event": "started", "tick": snapshot.tick, "tick_hz": config.tick_hz,
        "icarus_compatible": false, "gameplay_ready": false,
        "native_schema": snapshot.schema_version, "compressed": compressed,
        "network_enabled": network_address.is_some()})
    );
    if let Some(address) = network_address {
        // The server's own bound address, configured limits and pinned build;
        // never a client-supplied value.
        eprintln!(
            "{}",
            with_native_replication_counts(
                json!({"event": "network_listening", "experimental": true,
            "address": address.to_string(), "max_players": config.network.max_players,
            "session_timeout_seconds": config.network.session_timeout_seconds,
            "version_check": pinned_build.is_some(), "release_build_id": pinned_build,
            "data_pak": config.network.data_pak.as_ref().map(|path| path.display().to_string()),
            "data_pak_source": data_pak_source,
            "icarus_compatible": false, "gameplay_ready": false}),
                app.world()
            )
        );
    }

    loop {
        let frame = Instant::now();
        if !compressed
            && frame.duration_since(previous_frame) > interval * config.max_catch_up_ticks
        {
            clamped_frames += 1;
        }
        previous_frame = frame;
        // Manual runner still executes Bevy's complete app schedules and clocks.
        app.update();
        icarus_bevy::native_lifecycle_runtime::modifier_runtime_health(app.world())?;
        let tick = app.world().resource::<SimulationClock>().tick;
        let now = clock.elapsed(tick);
        autosave.observe(writer.poll(), now)?;
        if stopping.load(Ordering::SeqCst) || target_tick.is_some_and(|target| tick >= target) {
            // Admission stops before any drain or final-save work.
            network::stop_admission(app.world_mut());
            break;
        }
        if now.saturating_sub(last_save_request) >= Duration::from_secs(config.autosave_seconds) {
            // A compressed run keeps every autosave by awaiting the one in
            // flight; a stop request leaves the wait for the bounded shutdown.
            while compressed && writer.is_busy() && !autosave.observe(writer.poll(), now)? {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(1));
            }
            if stopping.load(Ordering::SeqCst) {
                network::stop_admission(app.world_mut());
                break;
            }
            if !writer.is_busy() {
                writer.submit(capture(app.world_mut())?)?;
                // The interval runs from submission, after capture.
                last_save_request = clock.elapsed(tick);
                autosave.submitted = Some(Instant::now());
            }
        }
        if now.saturating_sub(last_status) >= Duration::from_secs(config.status_interval_seconds) {
            let metrics = app.world().resource::<TickMetrics>();
            eprintln!(
                "{}",
                with_native_replication_counts(
                    json!({"event": "status", "tick": tick,
                "gameplay_ready": false, "save_in_flight": writer.is_busy(),
                "save_failures": autosave.failures,
                "seconds_since_save": autosave.last_commit.map(|time| now.saturating_sub(time).as_secs()),
                "clamped_frames": (!compressed).then_some(clamped_frames),
                "tick_deadline_misses": metrics.deadline_misses,
                "last_tick_micros": metrics.last_tick_micros,
                "max_tick_micros": metrics.max_tick_micros,
                "tick_p50_micros": metrics.tick_micros.quantile_micros(50),
                "tick_p99_micros": metrics.tick_micros.quantile_micros(99),
                "sessions": network::session_counts(app.world())}),
                    app.world()
                )
            );
            last_status = now;
        }
        if !compressed {
            thread::sleep(interval.saturating_sub(frame.elapsed()));
        }
    }

    // Idempotent: admission is already stopped; close every session and drop
    // the socket before the bounded drain and final save.
    network::stop_admission(app.world_mut());
    let sessions = network::teardown(app.world_mut());
    let deadline = Instant::now() + Duration::from_secs(config.shutdown_timeout_seconds);
    // A failed in-flight autosave is reported as in the loop; the final save still runs.
    match writer.settle_before(deadline)? {
        Some(Ok(receipt)) => saved(receipt, autosave.submitted.take()),
        Some(Err(error)) => eprintln!(
            "{}",
            json!({"event": "save_failed", "error": format!("{error:#}")})
        ),
        None => {}
    }
    ensure!(
        Instant::now() < deadline,
        "snapshot shutdown deadline exceeded before final capture"
    );
    let final_submitted = Instant::now();
    writer.submit_before(capture(app.world_mut())?, deadline)?;
    if let Some(receipt) = writer.finish_before(deadline)? {
        saved(receipt, Some(final_submitted));
    }
    eprintln!(
        "{}",
        with_native_replication_counts(
            json!({"event": "stopped", "tick": app.world().resource::<SimulationClock>().tick,
        "gameplay_ready": false, "sessions": sessions}),
            app.world()
        )
    );
    Ok(())
}
