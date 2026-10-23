//! Pure, independently authored capabilities and deployable/foundation links.
//! This is not a recorder adapter and makes no original-game behavior claim.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    experimental::restore_rules::{ActorLink, evaluate_deployable_stitch, names_match},
    native_world::{
        CompiledNativeCatalog, MAX_NATIVE_KEY_BYTES, MAX_NATIVE_PLACEMENTS, NativeWorldSnapshot,
    },
    persistence::{SaveReceipt, SnapshotStore},
    simulation::{
        DomainId, SNAPSHOT_SCHEMA_C001_INERT, SNAPSHOT_SCHEMA_NATIVE,
        SNAPSHOT_SCHEMA_RELATIONSHIPS, WorldSnapshot,
    },
};

pub const NATIVE_RELATIONSHIP_FORMAT: u32 = 1;
pub const MAX_NATIVE_RELATIONSHIP_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_NATIVE_LINK_NAME_UNITS: usize = 1024;
pub const MAX_NATIVE_RELATIONSHIP_LINK_UNITS: usize = 1 << 20;

/// Complete, immutable, preflighted authored relationship world. Optional
/// C001 metadata remains source provenance, not a relationship interpretation.
pub struct PreparedRelationshipWorld {
    snapshot: WorldSnapshot,
}

impl PreparedRelationshipWorld {
    pub fn snapshot(&self) -> &WorldSnapshot {
        &self.snapshot
    }
}

/// Compose only a complete schema-4/5 world; validate before destination I/O.
pub fn prepare_relationship_world(
    mut base: WorldSnapshot,
    relationships: &CompiledNativeRelationships,
) -> Result<PreparedRelationshipWorld> {
    ensure!(
        matches!(
            base.schema_version,
            SNAPSHOT_SCHEMA_NATIVE | SNAPSHOT_SCHEMA_C001_INERT
        ),
        "relationship preparation requires a native base world"
    );
    base.validate()?;
    base.schema_version = SNAPSHOT_SCHEMA_RELATIONSHIPS;
    base.native_relationships = Some(relationships.initial_snapshot());
    SnapshotStore::preflight(&base)?;
    Ok(PreparedRelationshipWorld { snapshot: base })
}

/// Publish only after preparation, refusing every existing or recovered world.
pub fn publish_new_relationship_world(
    prepared: PreparedRelationshipWorld,
    destination: &Path,
) -> Result<SaveReceipt> {
    SnapshotStore::open(destination)?.save_new(prepared.snapshot())
}

// Deserialization errors are deliberately replaced at every named boundary:
// neither serde's unknown-key errors nor invalid values may expose names.
macro_rules! object_dto {
    (pub struct $name:ident { $(pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Serialize)]
        pub struct $name { $(pub $field: $ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($field: $ty),* }
                let fields: Fields = crate::json::object(deserializer)
                    .map_err(|_| <D::Error as serde::de::Error>::custom("native relationship object invalid"))?;
                Ok(Self { $($field: fields.$field),* })
            }
        }
    };
}

/// Independently declared, mutually exclusive V1 capabilities. Native class
/// keys never imply these roles or substitute for UTF-16 link names.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRelationshipRole {
    Spatial,
    Deployable,
    Foundation,
}

impl<'de> Deserialize<'de> for NativeRelationshipRole {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct RoleVisitor;
        impl serde::de::Visitor<'_> for RoleVisitor {
            type Value = NativeRelationshipRole;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a native relationship role string")
            }

            fn visit_str<E: serde::de::Error>(
                self,
                role: &str,
            ) -> std::result::Result<Self::Value, E> {
                match role {
                    "spatial" => Ok(NativeRelationshipRole::Spatial),
                    "deployable" => Ok(NativeRelationshipRole::Deployable),
                    "foundation" => Ok(NativeRelationshipRole::Foundation),
                    _ => Err(E::custom("native relationship role invalid")),
                }
            }
        }
        deserializer
            .deserialize_str(RoleVisitor)
            .map_err(|_| <D::Error as serde::de::Error>::custom("native relationship role invalid"))
    }
}

object_dto! {
    pub struct NativeActorLinkV1 {
        pub name: Vec<u16>,
        pub guid: i32,
    }
}
object_dto! {
    pub struct NativeRelationshipActorV1 {
        pub placement: String,
        pub identity: NativeActorLinkV1,
        pub role: NativeRelationshipRole,
        pub foundation: Option<NativeActorLinkV1>,
    }
}
object_dto! {
    pub struct NativeRelationshipsDefinition {
        pub relationship_format: u32,
        pub native_catalog_id: String,
        pub map: String,
        pub actors: Vec<NativeRelationshipActorV1>,
    }
}
object_dto! {
    pub struct NativeFoundationRecord {
        pub owner: DomainId,
        pub foundation: Option<DomainId>,
    }
}
object_dto! {
    pub struct NativeRelationshipsSnapshot {
        pub relationship_id: String,
        pub definition: NativeRelationshipsDefinition,
        pub foundations: Vec<NativeFoundationRecord>,
    }
}

macro_rules! counts_debug {
    ($name:ident,) => {
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
    ($name:ident, $($field:ident),* $(,)?) => {
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut debug = f.debug_struct(stringify!($name));
                $(debug.field(stringify!($field), &self.$field.len());)*
                debug.finish_non_exhaustive()
            }
        }
    };
}
counts_debug!(NativeRelationshipRole,);
counts_debug!(NativeActorLinkV1, name);
counts_debug!(NativeRelationshipActorV1,);
counts_debug!(NativeRelationshipsDefinition, actors);
counts_debug!(NativeFoundationRecord,);
counts_debug!(NativeRelationshipsSnapshot, foundations);

/// Validated immutable declarations and their initial edges. Mutable edge state
/// belongs exclusively to snapshots, never to this definition or its digest.
#[derive(Clone)]
pub struct CompiledNativeRelationships {
    identity: String,
    definition: NativeRelationshipsDefinition,
    roles: BTreeMap<DomainId, NativeRelationshipRole>,
    initial_foundations: Vec<NativeFoundationRecord>,
}

impl fmt::Debug for CompiledNativeRelationships {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledNativeRelationships")
            .field("actors", &self.roles.len())
            .field("foundations", &self.initial_foundations.len())
            .finish_non_exhaustive()
    }
}

fn key_valid(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_NATIVE_KEY_BYTES
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

fn digest_valid(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn add_units(total: &mut usize, count: usize) -> Result<()> {
    *total = total
        .checked_add(count)
        .context("native relationship aggregate overflow")?;
    ensure!(
        *total <= MAX_NATIVE_RELATIONSHIP_LINK_UNITS,
        "native relationship aggregate limit"
    );
    Ok(())
}

fn validate_link(link: &NativeActorLinkV1, total: &mut usize) -> Result<()> {
    ensure!(link.guid > 0, "native relationship GUID invalid");
    ensure!(
        !link.name.is_empty() && link.name.len() <= MAX_NATIVE_LINK_NAME_UNITS,
        "native relationship name length invalid"
    );
    add_units(total, link.name.len())
}

/// Preflight programmatic DTOs before cloning, indexing or hashing them.
fn validate_definition(definition: &NativeRelationshipsDefinition) -> Result<()> {
    ensure!(
        definition.relationship_format == NATIVE_RELATIONSHIP_FORMAT,
        "native relationship format unsupported"
    );
    ensure!(
        digest_valid(&definition.native_catalog_id),
        "native relationship catalog digest invalid"
    );
    ensure!(
        key_valid(&definition.map),
        "native relationship map invalid"
    );
    ensure!(
        !definition.actors.is_empty() && definition.actors.len() <= MAX_NATIVE_PLACEMENTS,
        "native relationship actor count invalid"
    );
    let mut total = 0;
    for actor in &definition.actors {
        ensure!(
            key_valid(&actor.placement),
            "native relationship placement invalid"
        );
        validate_link(&actor.identity, &mut total)?;
        if let Some(link) = &actor.foundation {
            ensure!(
                actor.role == NativeRelationshipRole::Deployable,
                "native relationship link owner role invalid"
            );
            validate_link(link, &mut total)?;
        }
    }
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n <= MAX_NATIVE_RELATIONSHIP_BYTES)
                .ok_or_else(|| std::io::Error::other("native relationship byte limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(Counter(0), definition)
        .map_err(|_| anyhow::anyhow!("native relationship serialization or byte limit"))
}

/// Exactly the equivalence relation of `names_match`: ASCII letters fold;
/// every other UTF-16 unit, including unpaired surrogates, remains unchanged.
fn link_key(link: &NativeActorLinkV1) -> (i32, Vec<u16>) {
    (
        link.guid,
        link.name
            .iter()
            .map(|&unit| {
                if (u16::from(b'A')..=u16::from(b'Z')).contains(&unit) {
                    unit + 32
                } else {
                    unit
                }
            })
            .collect(),
    )
}

fn actor_link(link: &NativeActorLinkV1) -> ActorLink<'_> {
    ActorLink {
        name: &link.name,
        icarus_id: link.guid,
    }
}

fn domain_id(rank: usize) -> Result<DomainId> {
    Ok(DomainId(
        u64::try_from(rank)
            .ok()
            .and_then(|n| n.checked_add(1))
            .context("native relationship domain ID overflow")?,
    ))
}

impl CompiledNativeRelationships {
    pub fn compile(
        definition: NativeRelationshipsDefinition,
        catalog: &CompiledNativeCatalog,
    ) -> Result<Self> {
        validate_definition(&definition)?;
        ensure!(
            definition.native_catalog_id == catalog.identity(),
            "native relationship catalog mismatch"
        );
        let map = catalog
            .map(&definition.map)
            .context("native relationship map binding missing")?;
        // Compiled catalogs already have canonical placement order.
        let bindings = map
            .placements
            .iter()
            .enumerate()
            .map(|(rank, placement)| Ok((placement.key.as_str(), domain_id(rank)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        Self::compile_bound(definition, &bindings)
    }

    // Both callers preflight the complete owned definition first. Bindings
    // carry canonical native IDs, never declaration order or runtime entities.
    fn compile_bound(
        mut definition: NativeRelationshipsDefinition,
        bindings: &BTreeMap<&str, DomainId>,
    ) -> Result<Self> {
        ensure!(
            definition.actors.len() == bindings.len(),
            "native relationship placement coverage incomplete"
        );
        definition
            .actors
            .sort_by(|a, b| a.placement.cmp(&b.placement));
        let mut identities = BTreeMap::new();
        let mut roles = BTreeMap::new();
        let mut eligible_foundations = BTreeSet::new();
        // All identities are checked before any edge is resolved. The original
        // evaluator's first-match behavior must never hide authored ambiguity.
        for (index, actor) in definition.actors.iter().enumerate() {
            let id = *bindings
                .get(actor.placement.as_str())
                .context("native relationship placement binding missing")?;
            ensure!(
                roles.insert(id, actor.role).is_none(),
                "native relationship duplicate placement"
            );
            ensure!(
                identities
                    .insert(link_key(&actor.identity), index)
                    .is_none(),
                "native relationship ambiguous authored identity"
            );
            if actor.role == NativeRelationshipRole::Foundation {
                eligible_foundations.insert(id);
            }
        }
        let mut initial_foundations = Vec::new();
        for actor in &definition.actors {
            if actor.role != NativeRelationshipRole::Deployable {
                continue;
            }
            let foundation = if let Some(link) = &actor.foundation {
                let candidate = &definition.actors[*identities
                    .get(&link_key(link))
                    .context("native relationship initial link unresolved")?];
                let target = bindings[candidate.placement.as_str()];
                ensure!(
                    eligible_foundations.contains(&target),
                    "native relationship target role invalid"
                );
                ensure!(
                    names_match(&link.name, &candidate.identity.name),
                    "native relationship name mismatch"
                );
                let plan = evaluate_deployable_stitch(
                    true,
                    false,
                    actor_link(link),
                    &[actor_link(&candidate.identity)],
                );
                ensure!(
                    plan.foundation_actor_index == Some(0),
                    "native relationship qualified link unresolved"
                );
                Some(target)
            } else {
                None
            };
            initial_foundations.push(NativeFoundationRecord {
                owner: bindings[actor.placement.as_str()],
                foundation,
            });
        }
        initial_foundations.sort_by_key(|row| row.owner);
        // V1 canonical encoding is serde_json's compact named-object encoding
        // of the validated definition, with actors sorted by placement. The
        // relationship_format field versions this hash input. Preserve exact
        // original name units and spelling; normalization is for matching only.
        struct HashWriter(Sha256);
        impl Write for HashWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut writer = HashWriter(Sha256::new());
        serde_json::to_writer(&mut writer, &definition)
            .map_err(|_| anyhow::anyhow!("native relationship serialization invalid"))?;
        let identity = crate::hex_lower(&writer.0.finalize());
        Ok(Self {
            identity,
            definition,
            roles,
            initial_foundations,
        })
    }

    pub fn parse(text: &str, catalog: &CompiledNativeCatalog) -> Result<Self> {
        ensure!(
            text.len() <= MAX_NATIVE_RELATIONSHIP_BYTES,
            "native relationship byte limit"
        );
        let definition = if matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'[')) {
            serde_json::from_str(text)
                .map_err(|_| anyhow::anyhow!("native relationship JSON invalid"))?
        } else {
            toml::from_str(text).map_err(|_| anyhow::anyhow!("native relationship TOML invalid"))?
        };
        Self::compile(definition, catalog)
    }

    /// Admit the opened regular file, allowing regular symlinks. The open and
    /// actual read are bounded; diagnostics retain neither paths nor payloads.
    pub fn load(path: &Path, catalog: &CompiledNativeCatalog) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .map_err(|e| anyhow::anyhow!("native relationship open: {:?}", e.kind()))?;
        let metadata = file
            .metadata()
            .map_err(|e| anyhow::anyhow!("native relationship metadata: {:?}", e.kind()))?;
        ensure!(
            metadata.is_file(),
            "native relationship requires regular file"
        );
        ensure!(
            metadata.len() <= MAX_NATIVE_RELATIONSHIP_BYTES as u64,
            "native relationship byte limit"
        );
        let mut bytes = Vec::new();
        file.take(MAX_NATIVE_RELATIONSHIP_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| anyhow::anyhow!("native relationship read: {:?}", e.kind()))?;
        ensure!(
            bytes.len() <= MAX_NATIVE_RELATIONSHIP_BYTES,
            "native relationship byte limit"
        );
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| anyhow::anyhow!("native relationship encoding invalid"))?;
        Self::parse(text, catalog)
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn definition(&self) -> &NativeRelationshipsDefinition {
        &self.definition
    }
    pub fn initial_snapshot(&self) -> NativeRelationshipsSnapshot {
        NativeRelationshipsSnapshot {
            relationship_id: self.identity.clone(),
            definition: self.definition.clone(),
            foundations: self.initial_foundations.clone(),
        }
    }

    /// The enclosing native snapshot must already be independently validated.
    /// Verify bindings and saved edges without recompiling geometry, resetting
    /// edited links or making the initial links part of mutable state.
    pub fn from_snapshot(
        snapshot: &NativeRelationshipsSnapshot,
        native: &NativeWorldSnapshot,
    ) -> Result<Self> {
        validate_definition(&snapshot.definition)?;
        ensure!(
            snapshot.foundations.len() <= snapshot.definition.actors.len(),
            "native relationship state count invalid"
        );
        ensure!(
            snapshot.definition.native_catalog_id == native.catalog_id,
            "native relationship snapshot catalog mismatch"
        );
        ensure!(
            snapshot.definition.map == native.map,
            "native relationship snapshot map mismatch"
        );
        let map = native
            .definitions
            .maps
            .iter()
            .find(|map| map.key == native.map)
            .context("native relationship native map binding missing")?;
        ensure!(
            !map.placements.is_empty()
                && map.placements.len() <= MAX_NATIVE_PLACEMENTS
                && native.actors.len() == map.placements.len(),
            "native relationship native actor count invalid"
        );
        let mut placements = BTreeMap::new();
        for placement in &map.placements {
            ensure!(
                placements
                    .insert(placement.key.as_str(), placement)
                    .is_none(),
                "native relationship native duplicate placement"
            );
        }
        let bindings = placements
            .keys()
            .enumerate()
            .map(|(rank, key)| Ok((*key, domain_id(rank)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut seen = BTreeSet::new();
        for actor in &native.actors {
            let placement = placements
                .get(actor.placement.as_str())
                .context("native relationship native placement missing")?;
            ensure!(
                bindings.get(actor.placement.as_str()) == Some(&actor.id)
                    && actor.class == placement.class
                    && actor.tile == placement.tile,
                "native relationship native actor binding invalid"
            );
            ensure!(
                seen.insert(actor.id),
                "native relationship native duplicate actor"
            );
        }
        let compiled = Self::compile_bound(snapshot.definition.clone(), &bindings)?;
        compiled.validate_snapshot(snapshot)?;
        Ok(compiled)
    }

    pub fn validate_snapshot(&self, snapshot: &NativeRelationshipsSnapshot) -> Result<()> {
        ensure!(
            snapshot.relationship_id == self.identity,
            "native relationship snapshot identity mismatch"
        );
        validate_definition(&snapshot.definition)?;
        ensure!(
            snapshot.definition.relationship_format == self.definition.relationship_format
                && snapshot.definition.native_catalog_id == self.definition.native_catalog_id
                && snapshot.definition.map == self.definition.map
                && snapshot.definition.actors.len() == self.definition.actors.len(),
            "native relationship immutable definition mismatch"
        );
        let mut declarations = BTreeSet::new();
        for actor in &snapshot.definition.actors {
            let rank = self
                .definition
                .actors
                .binary_search_by(|expected| expected.placement.cmp(&actor.placement))
                .ok()
                .context("native relationship snapshot declaration missing")?;
            ensure!(
                declarations.insert(rank) && *actor == self.definition.actors[rank],
                "native relationship immutable actor mismatch"
            );
        }
        self.validate_foundations(&snapshot.foundations)
    }

    /// Validate mutable edges against the already verified declarations.
    pub(crate) fn validate_foundations(&self, rows: &[NativeFoundationRecord]) -> Result<()> {
        ensure!(
            rows.len() == self.initial_foundations.len(),
            "native relationship state coverage incomplete"
        );
        let mut owners = BTreeSet::new();
        for row in rows {
            self.require_role(row.owner, NativeRelationshipRole::Deployable)?;
            ensure!(
                owners.insert(row.owner),
                "native relationship duplicate state owner"
            );
            if let Some(target) = row.foundation {
                self.require_role(target, NativeRelationshipRole::Foundation)?;
            }
        }
        Ok(())
    }

    fn require_role(&self, id: DomainId, role: NativeRelationshipRole) -> Result<()> {
        ensure!(
            self.roles.get(&id) == Some(&role),
            "native relationship domain ID missing or wrong role"
        );
        Ok(())
    }

    /// Validate existing state and the entire edit before the single write.
    pub fn reassign_foundation(
        &self,
        snapshot: &mut NativeRelationshipsSnapshot,
        owner: DomainId,
        target: Option<DomainId>,
    ) -> Result<()> {
        self.validate_snapshot(snapshot)?;
        self.require_role(owner, NativeRelationshipRole::Deployable)?;
        if let Some(target) = target {
            self.require_role(target, NativeRelationshipRole::Foundation)?;
        }
        let row = snapshot
            .foundations
            .iter_mut()
            .find(|row| row.owner == owner)
            .context("native relationship owner state missing")?;
        row.foundation = target;
        Ok(())
    }

    pub fn foundation_of(
        &self,
        snapshot: &NativeRelationshipsSnapshot,
        owner: DomainId,
    ) -> Result<Option<DomainId>> {
        self.validate_snapshot(snapshot)?;
        self.require_role(owner, NativeRelationshipRole::Deployable)?;
        Ok(snapshot
            .foundations
            .iter()
            .find(|row| row.owner == owner)
            .context("native relationship owner state missing")?
            .foundation)
    }

    pub fn dependents_of(
        &self,
        snapshot: &NativeRelationshipsSnapshot,
        target: DomainId,
    ) -> Result<Vec<DomainId>> {
        self.validate_snapshot(snapshot)?;
        self.require_role(target, NativeRelationshipRole::Foundation)?;
        let mut owners: Vec<_> = snapshot
            .foundations
            .iter()
            .filter(|row| row.foundation == Some(target))
            .map(|row| row.owner)
            .collect();
        owners.sort_unstable();
        Ok(owners)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn aggregate_arithmetic_cannot_wrap() {
        let mut count = usize::MAX;
        assert!(super::add_units(&mut count, 1).is_err());
    }
}
