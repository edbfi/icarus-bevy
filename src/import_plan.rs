//! Native import planning for a supplied prospect: decide, before anything is
//! written, whether the save could be imported into a native world, and if not,
//! report exactly which fields block it.
//!
//! The plan runs the importer acceptance conditions of the prospect-loading
//! spec in order: stage and validate the full schema (`F-403`, `F-715` ..
//! `F-738`), keep each recorder's `Validate` decision, type every recorder
//! value losslessly ([`crate::experimental::typed_value::TypedComponent`]), read the base recorder fields every
//! class shares, and resolve each content reference (row names, object names,
//! package paths) through a native [`ContentCatalog`]. Unmapped content is an
//! incompatibility, never silently dropped. The plan never writes, and reports
//! counts only: no class strings, names, paths or values.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::experimental::recorders::{
    StagedProspectSaveState, StagedRecorderDecision, recorder_class_label,
};
use crate::experimental::restore_rules::OwnerResolvePolicy;
use crate::experimental::tagged::TagKind;
use crate::experimental::typed_value::{Item, Scalar, TypedProperty, TypedTree, Value};
use crate::import_candidate::DecodedImportCandidate;
use crate::prospect::EnvelopeInspection;

mod world_catalog;
use world_catalog::{WorldCatalog, WorldCatalogFile};

/// Base recorder fields: the policy, class and path every class saves, the
/// sub-level flag, and the actor transform of `UActorStateRecorderComponent`
/// classes (`C001` and its subclasses).
const OWNER_POLICY: &str = "OwnerResolvePolicy";
const ACTOR_CLASS: &str = "ActorClassName";
const ACTOR_PATH: &str = "ActorPathName";
const MOVED_TO_SUBLEVEL: &str = "bWasMovedToSubLevel";
const ACTOR_TRANSFORM: &str = "ActorTransform";
/// `UActorStateRecorderComponent::ObjectFName` (recorder `+0x148`): the
/// owner's object name at record time (F-813).
const OBJECT_NAME: &str = "ObjectFName";
/// The name field of the actor variable records that `UpdateOwnerVariables`
/// applies to the owner class's `SaveGame` properties by name (F-817).
const VARIABLE_NAME: &str = "VariableName";
/// Those records' struct types (the client's public property statics).
const ACTOR_VARIABLE_RECORDS: [&str; 5] = [
    "ActorBoolVariableRecord",
    "ActorIntVariableRecord",
    "ActorNameVariableRecord",
    "ActorTextVariableRecord",
    "LinearColorVariableRecord",
];
/// Fields whose role a static read of their recorder's record and restore
/// established, as `(property, enclosing struct, role)`; `None` is a
/// recorder's top level.
const READ_FIELD_ROLES: [(&str, Option<&str>, ContentRole); 15] = [
    // Item stat records name stat rows (F-660).
    ("Stat", Some("ItemRecordStatData"), ContentRole::DataRow),
    // The container manager saves each inventory's info row name (F-651).
    (
        "InventoryInfo",
        Some("SavedInventoryContainerData"),
        ContentRole::DataRow,
    ),
    // The world boss recorder writes name i into boss i's row handle (F-654).
    ("RecordedWorldBossTableNames", None, ContentRole::DataRow),
    // The sign recorder saves the sign's icon row name (F-677).
    ("SignIconRecord", None, ContentRole::DataRow),
    // The tree recorder saves the tree's prefab class (F-656).
    ("TreePrefabClassName", None, ContentRole::ActorClass),
    // Restores that turn each name into a row handle with the table's
    // `FromName` (F-820): crop seeds, stomach modifiers, mount and juvenile
    // lineages and genetic values, quests, faction missions, dynamic
    // mission prospects, prebuilt structures and session flags.
    ("Seed", Some("CultivationSaveData"), ContentRole::DataRow),
    (
        "ModifierName",
        Some("StomachContentSaveData"),
        ContentRole::DataRow,
    ),
    ("Lineage", None, ContentRole::DataRow),
    (
        "GeneticValueName",
        Some("MountGeneticsSaveData"),
        ContentRole::DataRow,
    ),
    ("QuestName", None, ContentRole::DataRow),
    ("FactionMissionName", None, ContentRole::DataRow),
    ("DynamicMissionProspectName", None, ContentRole::DataRow),
    ("PrebuiltStructureName", None, ContentRole::DataRow),
    (
        "SessionFlagRecords",
        Some("GameModeRecord"),
        ContentRole::DataRow,
    ),
    // The FLOD tile recorder finds its tile among the level's tiles by this
    // name or respawns one under it (F-820).
    (
        "TileName",
        Some("FLODTileRecorderRecord"),
        ContentRole::WorldTile,
    ),
];

/// Largest accepted content catalog file.
pub const MAX_CATALOG_BYTES: u64 = 4 * 1024 * 1024;
/// Largest accepted catalog reference, in UTF-16 units.
pub const MAX_CATALOG_REFERENCE_UNITS: usize = 1024;

/// Independently authored native content the importer can resolve references
/// against, by role. Empty until native world data exists; every reference
/// it lacks in the role it is used in blocks import.
#[derive(Clone, Default)]
pub struct ContentCatalog {
    /// References that resolve in every role ([`Self::from_references`]).
    any: BTreeSet<Vec<u16>>,
    by_role: BTreeMap<ContentRole, BTreeSet<Vec<u16>>>,
    /// Row names of the official server's data tables, ASCII-lowercased:
    /// a [`ContentRole::DataRow`] reference resolves when a table names
    /// it, compared without case as row names are (F-840).
    data_rows: BTreeSet<Vec<u16>>,
    /// Pak rows from the unique D_ProspectList table only (F-911).
    prospect_rows: BTreeSet<Vec<u16>>,
    /// Format-2 declarations resolve world references in their owning context.
    world: Option<WorldCatalog>,
}

impl std::fmt::Debug for ContentCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // References may come from supplied saves; print only the count.
        f.debug_struct("ContentCatalog")
            .field("references", &self.len())
            .finish()
    }
}

/// Format 1 lists references by role. Format 2 adds scoped world declarations
/// instead of the flat world-role lists. Instance names are never resolved.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogFile {
    format: u32,
    #[serde(default)]
    references: CatalogReferences,
    world: Option<WorldCatalogFile>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogReferences {
    #[serde(default)]
    actor_class: Vec<String>,
    #[serde(default)]
    actor_path: Vec<String>,
    #[serde(default)]
    data_row: Vec<String>,
    #[serde(default)]
    asset_path: Vec<String>,
    #[serde(default)]
    world_tile: Vec<String>,
    #[serde(default)]
    class_variable: Vec<String>,
    #[serde(default)]
    unclassified: Vec<String>,
}

impl ContentCatalog {
    pub fn empty() -> Self {
        Self::default()
    }

    /// References that resolve in every role.
    pub fn from_references<'a>(references: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            any: references
                .into_iter()
                .map(|reference| reference.encode_utf16().collect())
                .collect(),
            by_role: BTreeMap::new(),
            data_rows: BTreeSet::new(),
            prospect_rows: BTreeSet::new(),
            world: None,
        }
    }

    /// Parse a catalog file's text (see [`Self::load`]). Errors name the
    /// role and position only, never a reference.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() as u64 <= MAX_CATALOG_BYTES,
            "content catalog exceeds 4 MiB limit"
        );
        let file: CatalogFile = toml::from_str(text).map_err(|error| {
            // The 1-based line holding the error's start (0 when unknown).
            let line = error.span().map_or(0, |span| {
                1 + text.as_bytes()[..span.start.min(text.len())]
                    .iter()
                    .filter(|byte| **byte == b'\n')
                    .count()
            });
            anyhow::anyhow!("invalid content catalog near line {line}")
        })?;
        ensure!(
            matches!(file.format, 1 | 2),
            "unsupported content catalog format"
        );
        ensure!(
            (file.format == 2) == file.world.is_some(),
            "content catalog format 2 requires world declarations; format 1 forbids them"
        );
        let lists = file.references;
        if file.format == 2 {
            ensure!(
                lists.actor_class.is_empty()
                    && lists.actor_path.is_empty()
                    && lists.world_tile.is_empty()
                    && lists.class_variable.is_empty(),
                "content catalog format 2 requires scoped world roles, not flat role lists"
            );
        }
        let mut catalog = Self {
            world: file.world.map(WorldCatalog::compile).transpose()?,
            ..Self::default()
        };
        for (role, list) in [
            (ContentRole::ActorClass, lists.actor_class),
            (ContentRole::ActorPath, lists.actor_path),
            (ContentRole::DataRow, lists.data_row),
            (ContentRole::AssetPath, lists.asset_path),
            (ContentRole::WorldTile, lists.world_tile),
            (ContentRole::ClassVariable, lists.class_variable),
            (ContentRole::Unclassified, lists.unclassified),
        ] {
            let entries = catalog.by_role.entry(role).or_default();
            for reference in list {
                let units: Vec<u16> = reference.encode_utf16().collect();
                ensure!(
                    !units.is_empty() && units.len() <= MAX_CATALOG_REFERENCE_UNITS,
                    "content catalog {role:?} reference must be 1..={MAX_CATALOG_REFERENCE_UNITS} UTF-16 units"
                );
                ensure!(
                    entries.insert(units),
                    "content catalog {role:?} lists a reference twice"
                );
            }
        }
        catalog.by_role.retain(|_, entries| !entries.is_empty());
        Ok(catalog)
    }

    /// Load a catalog file: a regular file of at most [`MAX_CATALOG_BYTES`],
    /// opened without blocking so a FIFO cannot stall the importer.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .context("open content catalog")?;
        let metadata = file.metadata()?;
        ensure!(metadata.is_file(), "content catalog must be a regular file");
        ensure!(
            metadata.len() <= MAX_CATALOG_BYTES,
            "content catalog exceeds 4 MiB limit"
        );
        let mut bytes = Vec::new();
        file.take(MAX_CATALOG_BYTES + 1).read_to_end(&mut bytes)?;
        // The metadata bound alone does not cover growth after the check.
        ensure!(
            bytes.len() as u64 <= MAX_CATALOG_BYTES,
            "content catalog exceeds 4 MiB limit"
        );
        let text = std::str::from_utf8(&bytes).context("content catalog is not UTF-8")?;
        Self::parse(text)
    }

    /// Distinct authored reference spellings across every role, plus table row
    /// names not already represented by an authored reference. Table row names
    /// are deduplicated across tables and compared to authored references with
    /// ASCII case folding. Authored spellings remain distinct because their
    /// resolution is case-sensitive; counting does not change role resolution.
    pub fn len(&self) -> usize {
        let mut all: BTreeSet<&[u16]> = self.any.iter().map(Vec::as_slice).collect();
        for entries in self.by_role.values() {
            all.extend(entries.iter().map(Vec::as_slice));
        }
        if let Some(world) = &self.world {
            all.extend(world.references());
        }
        let authored_rows: BTreeSet<Vec<u16>> = all.iter().map(|units| fold_case(units)).collect();
        all.len() + self.data_rows.difference(&authored_rows).count()
    }

    pub fn is_empty(&self) -> bool {
        self.any.is_empty()
            && self.by_role.is_empty()
            && self.data_rows.is_empty()
            && self
                .world
                .as_ref()
                .is_none_or(|world| world.references().next().is_none())
    }

    /// Also resolve data-row references against every row of every data
    /// table in `pak` (F-828, F-840); returns the catalog, the number of
    /// tables read and of JSON members skipped as not tables. A member that
    /// cannot be read or parsed fails.
    pub fn with_data_tables(
        mut self,
        pak: &mut crate::data_pak::DataPak,
    ) -> Result<(Self, usize, usize)> {
        let members: Vec<String> = pak
            .members()
            .filter(|member| member.ends_with(".json"))
            .map(str::to_owned)
            .collect();
        let (mut read, mut skipped) = (0, 0);
        for member in members {
            match pak.member_rows(&member)? {
                Some(rows) => {
                    read += 1;
                    self.data_rows.extend(
                        rows.iter()
                            .map(|row| fold_case(&row.encode_utf16().collect::<Vec<u16>>())),
                    );
                }
                None => skipped += 1,
            }
        }
        // Use the same suffix lookup and ambiguity rejection as DataTables.
        if pak.has("D_ProspectList.json") {
            self.prospect_rows.extend(
                pak.table_rows(crate::network::PROSPECT_LIST_TABLE)?
                    .iter()
                    .map(|row| fold_case(&row.encode_utf16().collect::<Vec<u16>>())),
            );
        }
        Ok((self, read, skipped))
    }

    fn contains_prospect(&self, units: &[u16]) -> bool {
        self.any.contains(units)
            || self
                .by_role
                .get(&ContentRole::DataRow)
                .is_some_and(|entries| entries.contains(units))
            || self.prospect_rows.contains(&fold_case(units))
    }

    /// Whether `units` resolves in `role`.
    fn contains(&self, role: ContentRole, units: &[u16]) -> bool {
        self.any.contains(units)
            || self
                .by_role
                .get(&role)
                .is_some_and(|entries| entries.contains(units))
            || (role == ContentRole::DataRow && self.data_rows.contains(&fold_case(units)))
    }

    /// A format-2 declaration is authoritative for world roles. No legacy
    /// list or table row can satisfy a reference from the wrong map/class.
    fn contains_in_world(
        &self,
        role: ContentRole,
        units: &[u16],
        map: &[u16],
        owner_class: Option<&[u16]>,
    ) -> bool {
        let Some(world) = &self.world else {
            return self.contains(role, units);
        };
        match role {
            ContentRole::WorldMap => world.contains_map(units),
            ContentRole::ActorClass => world.contains_class(units),
            ContentRole::ActorPath => {
                owner_class.is_some_and(|class| world.contains_actor(map, units, class))
            }
            ContentRole::WorldTile => world.contains_tile(map, units),
            ContentRole::ClassVariable => {
                owner_class.is_some_and(|class| world.contains_variable(class, units))
            }
            _ => self.contains(role, units),
        }
    }
}

/// `units` with ASCII letters lowercased.
fn fold_case(units: &[u16]) -> Vec<u16> {
    units
        .iter()
        .map(|unit| match u8::try_from(*unit) {
            Ok(byte) => u16::from(byte.to_ascii_lowercase()),
            Err(_) => *unit,
        })
        .collect()
}

/// Why the plan cannot import; each carries counts only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "blocker", rename_all = "snake_case")]
pub enum ImportBlocker {
    /// Scoped world declarations need an explicit saved map identity.
    MissingWorldMap,
    /// A prospect without any recorder that survives `Validate` would import
    /// as an empty world, which the acceptance conditions forbid.
    NoRetainedRecorders,
    /// Retained recorder bodies that did not frame, type or re-encode exactly.
    UntypedRecorders { recorders: usize },
    /// Leaves without a typed, exactly re-encoding value.
    UninterpretedValues { leaves: usize },
    /// Content references the native catalog does not define.
    UnmappedContent { distinct: usize, references: usize },
    /// Actor transforms with a non-finite component.
    NonFiniteTransforms { recorders: usize },
    /// Present actor transforms with an unexpected struct identity or shape.
    MalformedTransforms { recorders: usize },
    /// Base recorder fields under an unexpected wire type, whose references
    /// and policies the plan could not read.
    UnexpectedBaseFieldTypes { fields: usize },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RecorderPlan {
    pub parsed: usize,
    pub retained: usize,
    /// Dropped by the original's `Validate` on load; not imported.
    pub dropped_by_validate: usize,
    /// Retained `C033` physics-dynamic trees the original drops on its next save.
    pub physics_dynamic_trees: usize,
    pub typed: usize,
    /// Retained bodies that did not frame, type or re-encode exactly.
    pub untyped: usize,
    pub properties: usize,
    pub uninterpreted_leaves: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BaseFieldPlan {
    pub with_owner_policy: usize,
    pub with_actor_class: usize,
    pub with_actor_path: usize,
    pub with_sublevel_flag: usize,
    /// A base field present under an unexpected wire type.
    pub unexpected_types: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ActorPlan {
    /// Retained recorders of `UActorStateRecorderComponent` classes.
    pub actor_recorders: usize,
    /// Of those, classes whose constructor keeps `bRecordActorTransform` set.
    pub transform_restoring: usize,
    /// Finite transforms with the expected outer and child struct identities.
    pub recorded_transforms: usize,
    /// Actor recorders without an `ActorTransform` property; not a blocker.
    pub missing_transforms: usize,
    /// Present transforms with an unexpected struct identity or shape.
    pub malformed_transforms: usize,
    pub non_finite_transforms: usize,
    /// Retained recorders by how restore finds their owner (`F-741`, `F-752`):
    /// `find_only` owners must already exist in the level.
    pub owner_resolution: BTreeMap<&'static str, usize>,
}

/// What a content reference names, judged only by the property it sits under
/// and the struct enclosing that property.
/// The rules are authored and deliberately narrow; anything else is
/// `Unclassified` rather than guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentRole {
    /// Root map identity in a scoped native catalog (format 2).
    WorldMap,
    /// An actor or Blueprint class to spawn (`*ActorClassName`, a tree's
    /// prefab class).
    ActorClass,
    /// A level placement path of an existing actor (`ActorPathName`).
    ActorPath,
    /// A data-table row (`*RowName`, `*Row`, `*StaticData`, `*DTKey`, and
    /// the fields whose restores were read to name rows).
    DataRow,
    /// A packaged asset path (`*AssetPath`).
    AssetPath,
    /// A tile of the world's landscape FLOD grid, named by a FLOD tile
    /// recorder (F-820): resolved against native world data.
    WorldTile,
    /// The owner's object name at record time (`ObjectFName`). Owner
    /// resolution and restore never read it (F-813): it is a saved
    /// instance's identity, not content, so it is never resolved against
    /// the catalog and never blocks import.
    InstanceName,
    /// A `SaveGame` property of the owner's class, named by an actor
    /// variable record (F-817): resolved against the native class schema.
    ClassVariable,
    Unclassified,
}

impl ContentRole {
    /// Whether references in this role are resolved against the catalog
    /// (and so can block import); instance names are not.
    fn resolves_against_catalog(self) -> bool {
        self != Self::InstanceName
    }

    /// The role of a reference under `property`, at a recorder's top level
    /// (`within` is `None`) or inside a struct of type `within` (empty in map
    /// or set items).
    fn of(property: &[u16], within: Option<&[u16]>) -> Self {
        let name = String::from_utf16_lossy(property);
        let enclosing = |owner: Option<&str>| match (owner, within) {
            (None, None) => true,
            (Some(owner), Some(within)) => owner.encode_utf16().eq(within.iter().copied()),
            _ => false,
        };
        if let Some((_, _, role)) = READ_FIELD_ROLES
            .iter()
            .find(|(field, owner, _)| *field == name && enclosing(*owner))
        {
            return *role;
        }
        if name == VARIABLE_NAME
            && ACTOR_VARIABLE_RECORDS
                .iter()
                .any(|record| enclosing(Some(record)))
        {
            return Self::ClassVariable;
        }
        if name.ends_with("ActorClassName") {
            Self::ActorClass
        } else if name == ACTOR_PATH {
            Self::ActorPath
        } else if name == OBJECT_NAME {
            Self::InstanceName
        } else if ["RowName", "Row", "StaticData", "DTKey"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        {
            Self::DataRow
        } else if name.ends_with("AssetPath") {
            Self::AssetPath
        } else {
            Self::Unclassified
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RoleCount {
    pub references: usize,
    pub distinct: usize,
    pub unmapped_distinct: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ContentPlan {
    /// Non-empty name values other than `None`.
    pub name_references: usize,
    /// String values naming a `/Game/` or `/Script/` package path.
    /// Excludes classified string references without a package prefix.
    pub path_references: usize,
    /// Distinct references resolved against the catalog (every role but
    /// [`ContentRole::InstanceName`]); the `mapped`/`unmapped` counts split them.
    pub distinct: usize,
    pub mapped_distinct: usize,
    pub unmapped_distinct: usize,
    pub unmapped_references: usize,
    pub roles: BTreeMap<ContentRole, RoleCount>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ClassPlan {
    pub retained: usize,
    pub dropped: usize,
    pub untyped: usize,
    pub uninterpreted_leaves: usize,
    pub unmapped_references: usize,
}

#[derive(Debug, Serialize)]
pub struct ImportPlan {
    pub validation: &'static str,
    pub envelope: EnvelopeInspection,
    pub staged: StagedProspectSaveState,
    /// Catalog size under [`ContentCatalog::len`]'s counting rules.
    pub catalog_references: usize,
    pub recorders: RecorderPlan,
    pub base_fields: BaseFieldPlan,
    pub actors: ActorPlan,
    pub content: ContentPlan,
    /// Per class pseudonym.
    pub classes: BTreeMap<String, ClassPlan>,
    pub blockers: Vec<ImportBlocker>,
    /// This planning pass has no blockers. This is not semantic preparation
    /// or authorization to publish native world records; no writer exists.
    pub importable: bool,
    pub world_imported: bool,
}

/// Plan the import of one supplied prospect envelope against `catalog`.
/// Fails on any staging error; otherwise always returns a plan whose
/// `blockers` list every reason it is not importable.
pub fn plan_import(reader: impl Read, catalog: &ContentCatalog) -> Result<ImportPlan> {
    let candidate = DecodedImportCandidate::decode(reader)?;
    plan_decoded_import(&candidate, catalog)
}

/// Assess an already decoded immutable candidate without rereading its source.
/// Coverage success remains separate from semantic preparation/publication.
pub fn plan_decoded_import(
    candidate: &DecodedImportCandidate,
    catalog: &ContentCatalog,
) -> Result<ImportPlan> {
    let staged = candidate.staged().clone();
    let staged_import = candidate.root_references();
    let mut plan = ImportPlan {
        validation: "native_import_plan_only",
        envelope: candidate.envelope().clone(),
        staged,
        catalog_references: catalog.len(),
        recorders: RecorderPlan {
            parsed: candidate.recorders().len(),
            ..RecorderPlan::default()
        },
        base_fields: BaseFieldPlan::default(),
        actors: ActorPlan::default(),
        content: ContentPlan::default(),
        classes: BTreeMap::new(),
        blockers: Vec::new(),
        importable: false,
        world_imported: false,
    };
    let mut distinct: BTreeMap<Vec<u16>, bool> = BTreeMap::new();
    let mut role_distinct: BTreeMap<(ContentRole, Vec<u16>), bool> = BTreeMap::new();
    let map = staged_import.prospect_map_name.clone();
    if catalog.world.is_some() && map.is_empty() {
        plan.blockers.push(ImportBlocker::MissingWorldMap);
    }
    // Root content stays outside recorder name/class counts. Only the
    // prospect key has a qualified table identity (F-911); the other row
    // fields follow the same suffix convention as recorder references.
    // Legacy format-1 catalogs retain their unclassified map lookup. Format 2
    // makes the map explicit and scopes placements and tiles to it.
    let mut root_references = vec![
        (
            ContentRole::DataRow,
            staged_import.prospect_dt_key.clone(),
            true,
        ),
        (
            if catalog.world.is_some() {
                ContentRole::WorldMap
            } else {
                ContentRole::Unclassified
            },
            map.clone(),
            false,
        ),
        (
            ContentRole::DataRow,
            staged_import.faction_mission_dt_key.clone(),
            false,
        ),
    ];
    root_references.extend(
        staged_import
            .setting_row_names
            .iter()
            .cloned()
            .map(|name| (ContentRole::DataRow, name, false)),
    );
    for (role, reference, prospect) in root_references {
        if reference.is_empty() {
            continue;
        }
        let mapped = if prospect {
            catalog.contains_prospect(&reference)
        } else {
            catalog.contains_in_world(role, &reference, &map, None)
        };
        plan.content.roles.entry(role).or_default().references += 1;
        role_distinct
            .entry((role, reference.clone()))
            .and_modify(|all| *all &= mapped)
            .or_insert(mapped);
        distinct
            .entry(reference)
            .and_modify(|all| *all &= mapped)
            .or_insert(mapped);
        if !mapped {
            plan.content.unmapped_references += 1;
        }
    }
    for recorder in candidate.recorders() {
        let decision = recorder.decision();
        // A version-0 save drops every recorder unevaluated; its exact class
        // identity remains in the candidate even when it is unregistered.
        let label = recorder_class_label(recorder.class_units())
            .unwrap_or_else(|| "unregistered".to_owned());
        let class_plan = plan.classes.entry(label).or_default();
        if !decision.retained {
            plan.recorders.dropped_by_validate += 1;
            class_plan.dropped += 1;
            continue;
        }
        let defaults = recorder
            .defaults()
            .context("retained recorder class has no constructor defaults")?;
        plan.recorders.retained += 1;
        class_plan.retained += 1;
        *plan
            .actors
            .owner_resolution
            .entry(owner_resolution(decision))
            .or_default() += 1;
        if decision.physics_dynamic_tree {
            plan.recorders.physics_dynamic_trees += 1;
        }
        // Optional typing failures remain report blockers, not decode errors.
        let Some((typed, summary)) = recorder.typing().lossless() else {
            plan.recorders.untyped += 1;
            class_plan.untyped += 1;
            continue;
        };
        plan.recorders.typed += 1;
        plan.recorders.properties += summary.properties;
        plan.recorders.uninterpreted_leaves += summary.uninterpreted;
        class_plan.uninterpreted_leaves += summary.uninterpreted;

        base_fields(&typed.tree, &mut plan.base_fields);
        if defaults.is_actor_recorder {
            plan.actors.actor_recorders += 1;
            if defaults.b_record_actor_transform == Some(true) {
                plan.actors.transform_restoring += 1;
            }
            match actor_transform(&typed.tree) {
                None if typed.tree.get(ACTOR_TRANSFORM).is_some() => {
                    plan.actors.malformed_transforms += 1;
                }
                None => plan.actors.missing_transforms += 1,
                Some(parts) if parts.iter().all(|part| part.is_finite()) => {
                    plan.actors.recorded_transforms += 1;
                }
                Some(_) => plan.actors.non_finite_transforms += 1,
            }
        }

        let mut references = References::default();
        references.tree(&typed.tree, None);
        let owner_class = typed
            .tree
            .get(ACTOR_CLASS)
            .and_then(|property| match &property.value {
                Value::Scalar(Scalar::Name(units)) => Some(units.as_slice()),
                _ => None,
            });
        plan.content.name_references += references.names;
        plan.content.path_references += references.paths;
        for (role, reference) in references.values {
            plan.content.roles.entry(role).or_default().references += 1;
            let mapped = catalog.contains_in_world(role, &reference, &map, owner_class);
            role_distinct
                .entry((role, reference.clone()))
                .and_modify(|all| *all &= mapped)
                .or_insert(mapped);
            if !role.resolves_against_catalog() {
                continue;
            }
            // A reference maps only where the catalog defines it in every
            // role it is used in.
            distinct
                .entry(reference)
                .and_modify(|all| *all &= mapped)
                .or_insert(mapped);
            if !mapped {
                plan.content.unmapped_references += 1;
                class_plan.unmapped_references += 1;
            }
        }
    }
    for ((role, _), mapped) in &role_distinct {
        let count = plan.content.roles.entry(*role).or_default();
        count.distinct += 1;
        if role.resolves_against_catalog() && !mapped {
            count.unmapped_distinct += 1;
        }
    }
    plan.content.distinct = distinct.len();
    plan.content.mapped_distinct = distinct.values().filter(|mapped| **mapped).count();
    plan.content.unmapped_distinct = plan.content.distinct - plan.content.mapped_distinct;

    if plan.recorders.retained == 0 {
        plan.blockers.push(ImportBlocker::NoRetainedRecorders);
    }
    if plan.recorders.untyped > 0 {
        plan.blockers.push(ImportBlocker::UntypedRecorders {
            recorders: plan.recorders.untyped,
        });
    }
    if plan.recorders.uninterpreted_leaves > 0 {
        plan.blockers.push(ImportBlocker::UninterpretedValues {
            leaves: plan.recorders.uninterpreted_leaves,
        });
    }
    if plan.content.unmapped_distinct > 0 {
        plan.blockers.push(ImportBlocker::UnmappedContent {
            distinct: plan.content.unmapped_distinct,
            references: plan.content.unmapped_references,
        });
    }
    if plan.base_fields.unexpected_types > 0 {
        plan.blockers.push(ImportBlocker::UnexpectedBaseFieldTypes {
            fields: plan.base_fields.unexpected_types,
        });
    }
    if plan.actors.non_finite_transforms > 0 {
        plan.blockers.push(ImportBlocker::NonFiniteTransforms {
            recorders: plan.actors.non_finite_transforms,
        });
    }
    if plan.actors.malformed_transforms > 0 {
        plan.blockers.push(ImportBlocker::MalformedTransforms {
            recorders: plan.actors.malformed_transforms,
        });
    }
    plan.importable = plan.blockers.is_empty();
    Ok(plan)
}

fn owner_resolution(decision: &StagedRecorderDecision) -> &'static str {
    if decision.skip_default_owner_resolution {
        return "skip_default";
    }
    match decision.owner_resolve_policy {
        OwnerResolvePolicy::FindOnly => "find_only",
        OwnerResolvePolicy::RespawnOnly => "respawn_only",
        OwnerResolvePolicy::FindOrRespawn => "find_or_respawn",
        OwnerResolvePolicy::ManuallyResolved => "manually_resolved",
    }
}

fn base_fields(tree: &TypedTree, plan: &mut BaseFieldPlan) {
    let mut check = |name: &str, expected: fn(&TypedProperty) -> bool, count: &mut usize| {
        if let Some(property) = tree.get(name) {
            if expected(property) {
                *count += 1;
            } else {
                plan.unexpected_types += 1;
            }
        }
    };
    let mut owner = 0;
    let mut class = 0;
    let mut path = 0;
    let mut sublevel = 0;
    check(
        OWNER_POLICY,
        // Staging accepts the enum name or, as a ByteProperty, the raw value.
        |p| {
            matches!(
                p.value,
                Value::Scalar(Scalar::Enum(_)) | Value::Scalar(Scalar::Byte(_))
            )
        },
        &mut owner,
    );
    check(
        ACTOR_CLASS,
        |p| matches!(p.value, Value::Scalar(Scalar::Name(_))),
        &mut class,
    );
    check(
        ACTOR_PATH,
        |p| matches!(p.value, Value::Scalar(Scalar::Str(_))),
        &mut path,
    );
    check(
        MOVED_TO_SUBLEVEL,
        |p| matches!(p.value, Value::Scalar(Scalar::Bool(_))),
        &mut sublevel,
    );
    plan.with_owner_policy += owner;
    plan.with_actor_class += class;
    plan.with_actor_path += path;
    plan.with_sublevel_flag += sublevel;
}

/// The ten transform parts (translation, rotation quaternion, scale) when the
/// actor transform is a tagged `Transform` with `Vector` translation/scale and
/// `Quat` rotation. Returns `None` for absent or malformed transforms; finite
/// components are checked by callers.
pub fn actor_transform(tree: &TypedTree) -> Option<[f32; 10]> {
    let Value::TaggedStruct {
        type_name,
        tree: inner,
    } = &tree.get(ACTOR_TRANSFORM)?.value
    else {
        return None;
    };
    if !type_name.iter().copied().eq("Transform".encode_utf16()) {
        return None;
    }
    let part = |name: &str, expected: &str| match &inner.get(name)?.value {
        Value::FixedStruct { type_name, value }
            if type_name.iter().copied().eq(expected.encode_utf16()) =>
        {
            Some(value.clone())
        }
        _ => None,
    };
    let translation = part("Translation", "Vector")?.floats::<3>()?;
    let rotation = part("Rotation", "Quat")?.floats::<4>()?;
    let scale = part("Scale3D", "Vector")?.floats::<3>()?;
    let mut out = [0.0; 10];
    out[..3].copy_from_slice(&translation);
    out[3..7].copy_from_slice(&rotation);
    out[7..].copy_from_slice(&scale);
    Some(out)
}

/// References with the role of the nearest enclosing property.
#[derive(Default)]
struct References {
    names: usize,
    paths: usize,
    values: Vec<(ContentRole, Vec<u16>)>,
}

impl References {
    /// Walk `tree`: a recorder's top level (`within` is `None`) or a struct
    /// of type `within` (empty when unknown).
    fn tree(&mut self, tree: &TypedTree, within: Option<&[u16]>) {
        for property in &tree.properties {
            self.value(ContentRole::of(&property.name, within), &property.value);
        }
    }

    fn value(&mut self, role: ContentRole, value: &Value) {
        match value {
            Value::Scalar(scalar) => self.scalar(role, scalar),
            Value::ScalarArray { inner, items } => {
                if matches!(inner, TagKind::Name | TagKind::String) {
                    items.iter().for_each(|item| self.scalar(role, item));
                }
            }
            Value::TaggedStruct { type_name, tree } => self.tree(tree, Some(type_name)),
            Value::StructArray {
                type_name,
                elements,
                ..
            } => elements.iter().for_each(|e| self.tree(e, Some(type_name))),
            Value::Map { removed, entries } => {
                removed.iter().for_each(|item| self.item(role, item));
                for (key, value) in entries {
                    self.item(role, key);
                    self.item(role, value);
                }
            }
            Value::Set { removed, elements } => {
                removed
                    .iter()
                    .chain(elements)
                    .for_each(|item| self.item(role, item));
            }
            Value::Text(_)
            | Value::FixedStruct { .. }
            | Value::FixedStructArray { .. }
            | Value::Uninterpreted { .. } => {}
        }
    }

    fn item(&mut self, role: ContentRole, item: &Item) {
        match item {
            Item::Scalar(scalar) => self.scalar(role, scalar),
            Item::Struct(tree) => self.tree(tree, Some(&[])),
            Item::UnnamedStruct(_) => {}
        }
    }

    fn scalar(&mut self, role: ContentRole, scalar: &Scalar) {
        match scalar {
            Scalar::Name(units) if !units.is_empty() && !is_none(units) => {
                self.names += 1;
                self.values.push((role, units.clone()));
            }
            Scalar::Str(units) if !units.is_empty() && !is_none(units) => {
                let package_path = is_package_path(units);
                if package_path {
                    self.paths += 1;
                }
                if package_path || role != ContentRole::Unclassified {
                    self.values.push((role, units.clone()));
                }
            }
            _ => {}
        }
    }
}

fn is_none(units: &[u16]) -> bool {
    units.iter().copied().eq("None".encode_utf16())
}

fn is_package_path(units: &[u16]) -> bool {
    ["/Game/", "/Script/"].iter().any(|prefix| {
        let prefix: Vec<u16> = prefix.encode_utf16().collect();
        units.starts_with(&prefix)
    })
}

/// One retained actor recorder with a finite recorded transform. Holds private
/// supplied-save values, so it deliberately implements neither `Debug` nor
/// `Serialize`; callers report aggregates only.
pub struct PlannedActor {
    /// Class pseudonym (`C001..C038`, `N001..N027`).
    pub class_label: String,
    /// How restore finds the owner; see [`ActorPlan::owner_resolution`].
    pub owner_resolution: &'static str,
    pub translation: [f32; 3],
    /// Quaternion `x, y, z, w` in serialized order.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// The retained actor recorders of a supplied prospect with finite recorded
/// transforms, in recorder order, after the same staging, `Validate` and
/// lossless typing as [`plan_import`]. Positions are recorded values only; no
/// world frame or gameplay meaning is implied.
pub fn planned_actors(reader: impl Read) -> Result<Vec<PlannedActor>> {
    let candidate = DecodedImportCandidate::decode(reader)?;
    planned_actors_from_candidate(&candidate)
}

/// Recorded actor poses from the same decoded input used for assessment.
/// This remains a lab projection, not a prepared world or semantic acceptance.
pub fn planned_actors_from_candidate(
    candidate: &DecodedImportCandidate,
) -> Result<Vec<PlannedActor>> {
    let mut actors = Vec::new();
    for recorder in candidate.recorders() {
        let decision = recorder.decision();
        if !decision.retained {
            continue;
        }
        let defaults = recorder
            .defaults()
            .context("staged recorder class has no constructor defaults")?;
        if !defaults.is_actor_recorder {
            continue;
        }
        let (typed, _) = recorder
            .typing()
            .lossless()
            .context("retained actor recorder lacks lossless component typing")?;
        let Some(parts) = actor_transform(&typed.tree) else {
            continue;
        };
        if !parts.iter().all(|part| part.is_finite()) {
            continue;
        }
        let label = recorder_class_label(recorder.class_units())
            .context("staged recorder class is not registered")?;
        actors.push(PlannedActor {
            class_label: label,
            owner_resolution: owner_resolution(decision),
            translation: [parts[0], parts[1], parts[2]],
            rotation: [parts[3], parts[4], parts[5], parts[6]],
            scale: [parts[7], parts[8], parts[9]],
        });
    }
    Ok(actors)
}
