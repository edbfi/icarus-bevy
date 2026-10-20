//! Native import planning for a supplied prospect: decide, before anything is
//! written, whether the save could be imported into a native world, and if not,
//! report exactly which fields block it.
//!
//! The plan runs the importer acceptance conditions of the prospect-loading
//! spec in order: stage and validate the full schema (`F-403`, `F-715` ..
//! `F-738`), keep each recorder's `Validate` decision, type every recorder
//! value losslessly ([`TypedComponent`]), read the base recorder fields every
//! class shares, and resolve each content reference (row names, object names,
//! package paths) through a native [`ContentCatalog`]. Unmapped content is an
//! incompatibility, never silently dropped. The plan never writes, and reports
//! counts only: no class strings, names, paths or values.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use crate::experimental::archive::{ByteOrder, decode_string};
use crate::experimental::component::MAX_BOUNDED_COMPONENT_BYTES;
use crate::experimental::property_tree::{
    BaseComponentTree, MAX_TREE_DEPTH, MAX_TREE_NODES, TreeLimits,
};
use crate::experimental::recorders::{
    StagedProspectSaveState, StagedRecorderDecision, lookup_recorder_ctor_defaults, read_recorders,
    recorder_class_label, stage_prospect_save_state_with_decisions,
};
use crate::experimental::restore_rules::OwnerResolvePolicy;
use crate::experimental::struct_array::MAX_BOUNDED_STRUCT_ARRAY_BYTES;
use crate::experimental::tagged::{TagKind, TagProfile, read_root};
use crate::experimental::typed_value::{
    Item, Scalar, TypedComponent, TypedProperty, TypedTree, Value,
};
use crate::prospect::{EnvelopeInspection, decode_envelope};

const PROFILE: TagProfile = TagProfile::MemoryArchive522;

/// The hard tree ceilings, so large construction bodies frame fully.
const PLAN_TREE_LIMITS: TreeLimits = TreeLimits {
    max_depth: MAX_TREE_DEPTH,
    max_nodes: MAX_TREE_NODES,
    struct_array_bytes: MAX_BOUNDED_STRUCT_ARRAY_BYTES,
    container_struct_bytes: None,
};

/// Base recorder fields: the policy, class and path every class saves, the
/// sub-level flag, and the actor transform of `UActorStateRecorderComponent`
/// classes (`C001` and its subclasses).
const OWNER_POLICY: &str = "OwnerResolvePolicy";
const ACTOR_CLASS: &str = "ActorClassName";
const ACTOR_PATH: &str = "ActorPathName";
const MOVED_TO_SUBLEVEL: &str = "bWasMovedToSubLevel";
const ACTOR_TRANSFORM: &str = "ActorTransform";

/// Independently authored native content the importer can resolve references
/// against. Empty until native world data exists; every reference it lacks
/// blocks import.
#[derive(Clone, Default)]
pub struct ContentCatalog {
    references: BTreeSet<Vec<u16>>,
}

impl std::fmt::Debug for ContentCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // References come from supplied saves; print only the count.
        f.debug_struct("ContentCatalog")
            .field("references", &self.references.len())
            .finish()
    }
}

impl ContentCatalog {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_references<'a>(references: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            references: references
                .into_iter()
                .map(|reference| reference.encode_utf16().collect())
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.references.len()
    }

    pub fn is_empty(&self) -> bool {
        self.references.is_empty()
    }

    fn contains(&self, units: &[u16]) -> bool {
        self.references.contains(units)
    }
}

/// Why the plan cannot import; each carries counts only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "blocker", rename_all = "snake_case")]
pub enum ImportBlocker {
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
    /// Transforms with a translation, rotation and scale that all type.
    pub recorded_transforms: usize,
    pub missing_transforms: usize,
    pub non_finite_transforms: usize,
    /// Retained recorders by how restore finds their owner (`F-741`, `F-752`):
    /// `find_only` owners must already exist in the level.
    pub owner_resolution: BTreeMap<&'static str, usize>,
}

/// What a content reference names, judged only by the property it sits under.
/// The rules are authored and deliberately narrow; anything else is
/// `Unclassified` rather than guessed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentRole {
    /// An actor or Blueprint class to spawn (`*ActorClassName`).
    ActorClass,
    /// A level placement path of an existing actor (`ActorPathName`).
    ActorPath,
    /// A data-table row (`*RowName`, `*Row`, `*StaticData`, `*DTKey`).
    DataRow,
    /// A packaged asset path (`*AssetPath`).
    AssetPath,
    Unclassified,
}

impl ContentRole {
    fn of(property: &[u16]) -> Self {
        let name = String::from_utf16_lossy(property);
        if name.ends_with("ActorClassName") {
            Self::ActorClass
        } else if name == ACTOR_PATH {
            Self::ActorPath
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
    pub path_references: usize,
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
    pub catalog_references: usize,
    pub recorders: RecorderPlan,
    pub base_fields: BaseFieldPlan,
    pub actors: ActorPlan,
    pub content: ContentPlan,
    /// Per class pseudonym.
    pub classes: BTreeMap<String, ClassPlan>,
    pub blockers: Vec<ImportBlocker>,
    pub importable: bool,
    pub world_imported: bool,
}

/// Plan the import of one supplied prospect envelope against `catalog`.
/// Fails on any staging error; otherwise always returns a plan whose
/// `blockers` list every reason it is not importable.
pub fn plan_import(reader: impl Read, catalog: &ContentCatalog) -> Result<ImportPlan> {
    let validated = decode_envelope(reader)?;
    let tags = read_root(&validated.payload, PROFILE).context("invalid experimental root tags")?;
    let (staged, decisions) = stage_prospect_save_state_with_decisions(
        &tags.tags,
        PROFILE,
        MAX_BOUNDED_COMPONENT_BYTES,
        None,
    )
    .context("invalid staged FProspectSaveState")?;
    let recorders =
        read_recorders(&tags.tags, PROFILE).context("invalid experimental recorder framing")?;
    ensure!(
        decisions.len() == recorders.records().len(),
        "staging decisions do not match the recorder array"
    );
    let mut plan = ImportPlan {
        validation: "native_import_plan_only",
        envelope: validated.inspection,
        staged,
        catalog_references: catalog.len(),
        recorders: RecorderPlan {
            parsed: decisions.len(),
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
    let mut role_distinct: BTreeSet<(ContentRole, Vec<u16>)> = BTreeSet::new();
    for (recorder, decision) in recorders.records().iter().zip(&decisions) {
        // read_recorders and staging validated this exact FString; it is never reported.
        let class = decode_string(recorder.class_value, ByteOrder::Little)
            .context("invalid experimental recorder class string")?;
        // A version-0 save drops every recorder unevaluated, so a dropped
        // class may be unregistered; retained ones always resolve.
        let label = recorder_class_label(&class.units).unwrap_or_else(|| "unregistered".to_owned());
        let class_plan = plan.classes.entry(label).or_default();
        if !decision.retained {
            plan.recorders.dropped_by_validate += 1;
            class_plan.dropped += 1;
            continue;
        }
        let defaults = lookup_recorder_ctor_defaults(&class.units)
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
        // A body that does not frame, type or re-encode exactly is a blocker,
        // so the plan stays total.
        let typed = BaseComponentTree::parse(
            recorder.binary_data,
            PROFILE,
            MAX_BOUNDED_COMPONENT_BYTES,
            PLAN_TREE_LIMITS,
        )
        .and_then(|tree| TypedComponent::from_component(&tree))
        .ok()
        .filter(|(typed, _)| {
            typed
                .to_bytes()
                .is_ok_and(|bytes| bytes == recorder.binary_data)
        });
        let Some((typed, summary)) = typed else {
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
                None => plan.actors.missing_transforms += 1,
                Some(parts) if parts.iter().all(|part| part.is_finite()) => {
                    plan.actors.recorded_transforms += 1;
                }
                Some(_) => plan.actors.non_finite_transforms += 1,
            }
        }

        let mut references = References::default();
        references.tree(&typed.tree);
        plan.content.name_references += references.names;
        plan.content.path_references += references.paths;
        for (role, reference) in references.values {
            plan.content.roles.entry(role).or_default().references += 1;
            role_distinct.insert((role, reference.clone()));
            let mapped = *distinct
                .entry(reference)
                .or_insert_with_key(|key| catalog.contains(key));
            if !mapped {
                plan.content.unmapped_references += 1;
                class_plan.unmapped_references += 1;
            }
        }
    }
    for (role, reference) in &role_distinct {
        let count = plan.content.roles.entry(*role).or_default();
        count.distinct += 1;
        if !distinct.get(reference).copied().unwrap_or(false) {
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
/// actor transform is a tagged struct whose three parts all type as floats.
pub fn actor_transform(tree: &TypedTree) -> Option<[f32; 10]> {
    let Value::TaggedStruct { tree: inner, .. } = &tree.get(ACTOR_TRANSFORM)?.value else {
        return None;
    };
    let part = |name: &str| match &inner.get(name)?.value {
        Value::FixedStruct { value, .. } => Some(value.clone()),
        _ => None,
    };
    let translation = part("Translation")?.floats::<3>()?;
    let rotation = part("Rotation")?.floats::<4>()?;
    let scale = part("Scale3D")?.floats::<3>()?;
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
    fn tree(&mut self, tree: &TypedTree) {
        for property in &tree.properties {
            self.value(ContentRole::of(&property.name), &property.value);
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
            Value::TaggedStruct { tree, .. } => self.tree(tree),
            Value::StructArray { elements, .. } => elements.iter().for_each(|e| self.tree(e)),
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
            Item::Struct(tree) => self.tree(tree),
            Item::UnnamedStruct(_) => {}
        }
    }

    fn scalar(&mut self, role: ContentRole, scalar: &Scalar) {
        match scalar {
            Scalar::Name(units) if !units.is_empty() && !is_none(units) => {
                self.names += 1;
                self.values.push((role, units.clone()));
            }
            Scalar::Str(units) if is_package_path(units) => {
                self.paths += 1;
                self.values.push((role, units.clone()));
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
    let validated = decode_envelope(reader)?;
    let tags = read_root(&validated.payload, PROFILE).context("invalid experimental root tags")?;
    let (_, decisions) = stage_prospect_save_state_with_decisions(
        &tags.tags,
        PROFILE,
        MAX_BOUNDED_COMPONENT_BYTES,
        None,
    )
    .context("invalid staged FProspectSaveState")?;
    let recorders =
        read_recorders(&tags.tags, PROFILE).context("invalid experimental recorder framing")?;
    ensure!(
        decisions.len() == recorders.records().len(),
        "staging decisions do not match the recorder array"
    );
    let mut actors = Vec::new();
    for (recorder, decision) in recorders.records().iter().zip(&decisions) {
        if !decision.retained {
            continue;
        }
        let class = decode_string(recorder.class_value, ByteOrder::Little)
            .context("invalid experimental recorder class string")?;
        let defaults = lookup_recorder_ctor_defaults(&class.units)
            .context("staged recorder class has no constructor defaults")?;
        if !defaults.is_actor_recorder {
            continue;
        }
        let tree = BaseComponentTree::parse(
            recorder.binary_data,
            PROFILE,
            MAX_BOUNDED_COMPONENT_BYTES,
            PLAN_TREE_LIMITS,
        )
        .context("retained recorder body does not frame under the plan's tree limits")?;
        let (typed, _) = TypedComponent::from_component(&tree)?;
        let Some(parts) = actor_transform(&typed.tree) else {
            continue;
        };
        if !parts.iter().all(|part| part.is_finite()) {
            continue;
        }
        let label = recorder_class_label(&class.units)
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
