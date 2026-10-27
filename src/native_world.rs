//! Independently authored spatial worlds. This contract describes no official
//! content, recorder adapter, compatibility claim, or ECS publication operation.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

use crate::simulation::DomainId;

pub const NATIVE_FORMAT: u32 = 1;
pub const MAX_NATIVE_CATALOG_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_NATIVE_KEY_BYTES: usize = 64;
pub const MAX_NATIVE_STRING_UNITS: usize = 1024;
pub const MAX_NATIVE_MAPS: usize = 64;
pub const MAX_NATIVE_CLASSES: usize = 1024;
/// Aggregate bounds across all maps, also enforced for programmatic catalogs.
pub const MAX_NATIVE_PLACEMENTS: usize = 16_384;
pub const MAX_NATIVE_TILES: usize = 4096;
pub const MAX_NATIVE_VARIABLES_PER_CLASS: usize = 64;
pub const MAX_NATIVE_VARIABLE_VALUES: usize = 1_048_576;
/// Bound default expansion before cloning per-placement state. Catalog bytes
/// alone cannot bound a large string default repeated across many objects.
pub const MAX_NATIVE_VARIABLE_BYTES: usize = 32 * 1024 * 1024;
pub const NATIVE_QUATERNION_TOLERANCE: f64 = 1e-5;

// A derived struct accepts positional arrays. Route even direct DTO decoding
// through the project's named-object visitor, for JSON and nested persisted DTOs.
macro_rules! object_dto {
    ($(#[$meta:meta])* pub struct $name:ident {
        $($(#[$field_meta:meta])* pub $field:ident: $ty:ty),* $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Clone, PartialEq, Serialize)]
        pub struct $name {
            $($(#[$field_meta])* pub $field: $ty),*
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($(#[$field_meta])* $field: $ty),* }
                let fields: Fields = crate::json::object(deserializer)?;
                Ok(Self { $($field: fields.$field),* })
            }
        }
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeFrame {
    RightHandedZUpMeters,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeClassKind {
    SpatialObjectV1,
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum NativeValue {
    Bool(bool),
    I32(i32),
    F32(f32),
    String(String),
}

impl<'de> Deserialize<'de> for NativeValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(
            tag = "type",
            content = "value",
            rename_all = "snake_case",
            deny_unknown_fields
        )]
        enum TaggedValue {
            Bool(bool),
            I32(i32),
            F32(f32),
            String(String),
        }
        Ok(match crate::json::object(deserializer)? {
            TaggedValue::Bool(value) => Self::Bool(value),
            TaggedValue::I32(value) => Self::I32(value),
            TaggedValue::F32(value) => Self::F32(value),
            TaggedValue::String(value) => Self::String(value),
        })
    }
}

object_dto! {
    pub struct NativeCatalogDefinition {
        pub native_format: u32,
        pub frame: NativeFrame,
        pub classes: Vec<NativeClassDefinition>,
        pub maps: Vec<NativeMapDefinition>,
    }
}
object_dto! {
    pub struct NativeClassDefinition {
        pub key: String,
        pub kind: NativeClassKind,
        pub local_bounds: Aabb3,
        pub variables: Vec<VariableDefinition>,
    }
}
object_dto! {
    pub struct VariableDefinition {
        pub key: String,
        pub default: NativeValue,
    }
}
object_dto! {
    pub struct NativeVariableValue {
        pub key: String,
        pub value: NativeValue,
    }
}
object_dto! {
    pub struct NativeMapDefinition {
        pub key: String,
        pub tiles: Vec<NativeTileDefinition>,
        pub placements: Vec<NativePlacement>,
    }
}
object_dto! {
    pub struct NativeTileDefinition {
        pub key: String,
        pub bounds: Aabb3,
    }
}
object_dto! {
    pub struct NativePlacement {
        pub key: String,
        pub class: String,
        pub tile: String,
        pub transform: NativeTransform,
        #[serde(default)]
        pub initial_values: Vec<NativeVariableValue>,
    }
}
object_dto! {
    pub struct Aabb3 {
        pub min: [f32; 3],
        pub max: [f32; 3],
    }
}
object_dto! {
    pub struct NativeTransform {
        pub translation: [f32; 3],
        /// Quaternion components in x, y, z, w order.
        pub rotation: [f32; 4],
        pub scale: [f32; 3],
    }
}
object_dto! {
    pub struct NativeWorldSnapshot {
        pub catalog_id: String,
        pub definitions: NativeCatalogDefinition,
        pub map: String,
        pub actors: Vec<NativeActorRecord>,
    }
}
object_dto! {
    pub struct NativeActorRecord {
        pub id: DomainId,
        pub placement: String,
        pub class: String,
        pub tile: String,
        pub transform: NativeTransform,
        pub variables: Vec<NativeVariableValue>,
    }
}

/// Borrow mutable records without copying their names or variable payloads.
/// The small pose is owned so ECS components can supply it directly.
#[derive(Clone)]
pub(crate) struct NativeActorView<'a> {
    pub id: DomainId,
    pub placement: &'a str,
    pub class: &'a str,
    pub tile: &'a str,
    pub transform: NativeTransform,
    pub variables: &'a [NativeVariableValue],
}

impl<'a> From<&'a NativeActorRecord> for NativeActorView<'a> {
    fn from(actor: &'a NativeActorRecord) -> Self {
        Self {
            id: actor.id,
            placement: &actor.placement,
            class: &actor.class,
            tile: &actor.tile,
            transform: actor.transform.clone(),
            variables: &actor.variables,
        }
    }
}

impl Default for NativeTransform {
    fn default() -> Self {
        Self {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        }
    }
}

impl Aabb3 {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (0..3).all(|i| self.min[i].is_finite()
                && self.max[i].is_finite()
                && self.min[i] <= self.max[i]),
            "native bounds invalid"
        );
        Ok(())
    }

    fn contains(&self, other: &Self) -> bool {
        (0..3).all(|i| self.min[i] <= other.min[i] && self.max[i] >= other.max[i])
    }
}

impl NativeTransform {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.translation.iter().all(|v| v.is_finite())
                && self.rotation.iter().all(|v| v.is_finite())
                && self.scale.iter().all(|v| v.is_finite() && *v > 0.0),
            "native transform nonfinite or invalid scale"
        );
        let norm = self
            .rotation
            .iter()
            .map(|v| f64::from(*v).powi(2))
            .sum::<f64>()
            .sqrt();
        ensure!(
            (norm - 1.0).abs() <= NATIVE_QUATERNION_TOLERANCE,
            "native transform quaternion is not unit length"
        );
        Ok(())
    }

    /// Transform every corner in f64, then round bounds outward to f32. This
    /// prevents rounding from admitting a box that extends beyond its tile.
    pub fn transformed_bounds(&self, local: &Aabb3) -> Result<Aabb3> {
        self.validate()?;
        local.validate()?;
        let (min, max) = if self.rotation == [0.0, 0.0, 0.0, 1.0] {
            self.identity_extremes(local)?
        } else {
            self.corner_extremes(local)?
        };
        let result = Aabb3 {
            min: min.map(|v| {
                let f = v as f32;
                if f64::from(f) > v { f.next_down() } else { f }
            }),
            max: max.map(|v| {
                let f = v as f32;
                if f64::from(f) < v { f.next_up() } else { f }
            }),
        };
        result.validate().context("native transformed bounds")?;
        Ok(result)
    }

    /// Identity rotation: every corner coordinate is translation plus local
    /// times scale (an exact f64 product) under one rounding, as in
    /// [`Self::corner_extremes`], and rounding is monotonic, so per-axis
    /// extremes need no corner loop. Only the sign of an exact zero may differ,
    /// which no bounds comparison observes.
    fn identity_extremes(&self, local: &Aabb3) -> Result<([f64; 3], [f64; 3])> {
        let mut min = [0.0; 3];
        let mut max = [0.0; 3];
        for i in 0..3 {
            let translation = f64::from(self.translation[i]);
            let scale = f64::from(self.scale[i]);
            let low = f64::from(local.min[i]) * scale;
            let high = f64::from(local.max[i]) * scale;
            min[i] = translation + low.min(high);
            max[i] = translation + low.max(high);
            ensure!(
                min[i].is_finite() && max[i].is_finite(),
                "native transformed bounds nonfinite"
            );
        }
        Ok((min, max))
    }

    /// Transform all eight corners by the rotation matrix.
    fn corner_extremes(&self, local: &Aabb3) -> Result<([f64; 3], [f64; 3])> {
        let [x, y, z, w] = self.rotation.map(f64::from);
        let matrix = [
            [
                1.0 - 2.0 * (y * y + z * z),
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
            ],
            [
                2.0 * (x * y + z * w),
                1.0 - 2.0 * (x * x + z * z),
                2.0 * (y * z - x * w),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                1.0 - 2.0 * (x * x + y * y),
            ],
        ];
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for corner in 0..8 {
            let point: [f64; 3] = std::array::from_fn(|i| {
                f64::from(if corner & (1 << i) == 0 {
                    local.min[i]
                } else {
                    local.max[i]
                }) * f64::from(self.scale[i])
            });
            for i in 0..3 {
                let value = f64::from(self.translation[i])
                    + (0..3).map(|j| matrix[i][j] * point[j]).sum::<f64>();
                ensure!(value.is_finite(), "native transformed bounds nonfinite");
                min[i] = min[i].min(value);
                max[i] = max[i].max(value);
            }
        }
        Ok((min, max))
    }
}

impl NativeValue {
    fn validate(&self) -> Result<()> {
        match self {
            Self::F32(value) => ensure!(value.is_finite(), "native variable nonfinite"),
            Self::String(value) => ensure!(
                value
                    .encode_utf16()
                    .take(MAX_NATIVE_STRING_UNITS + 1)
                    .count()
                    <= MAX_NATIVE_STRING_UNITS,
                "native variable string limit"
            ),
            _ => {}
        }
        Ok(())
    }

    fn same_type(&self, other: &Self) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

fn validate_key(key: &str) -> Result<()> {
    ensure!(
        !key.is_empty()
            && key.len() <= MAX_NATIVE_KEY_BYTES
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "native identifier invalid"
    );
    Ok(())
}

fn validate_keys<T>(values: &[T], limit: usize, key: impl Fn(&T) -> &str) -> Result<()> {
    ensure!(values.len() <= limit, "native array limit");
    let mut seen = BTreeSet::new();
    for (index, value) in values.iter().enumerate() {
        let key = key(value);
        validate_key(key).with_context(|| format!("native identifier index {index}"))?;
        ensure!(
            seen.insert(key),
            "native duplicate identifier index {index}"
        );
    }
    Ok(())
}

fn add_count(total: &mut usize, count: usize, limit: usize) -> Result<()> {
    *total = total
        .checked_add(count)
        .context("native aggregate overflow")?;
    ensure!(*total <= limit, "native aggregate limit");
    Ok(())
}

fn variable_bytes(key: &str, value: &NativeValue) -> Result<usize> {
    #[derive(Serialize)]
    struct BorrowedValue<'a> {
        key: &'a str,
        value: &'a NativeValue,
    }
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .filter(|n| *n <= MAX_NATIVE_VARIABLE_BYTES)
                .ok_or_else(|| std::io::Error::other("native variable byte limit"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, &BorrowedValue { key, value })
        .map_err(|_| anyhow::anyhow!("native variable serialization or byte limit"))?;
    Ok(count.0)
}

fn validate_values(
    values: &[NativeVariableValue],
    class: &NativeClassDefinition,
    complete: bool,
) -> Result<()> {
    validate_keys(values, MAX_NATIVE_VARIABLES_PER_CLASS, |v| &v.key)?;
    if complete {
        ensure!(
            values.len() == class.variables.len(),
            "native variable schema incomplete"
        );
    }
    for (index, value) in values.iter().enumerate() {
        value
            .value
            .validate()
            .with_context(|| format!("native variable index {index}"))?;
        let definition = class
            .variables
            .iter()
            .find(|v| v.key == value.key)
            .with_context(|| format!("native variable binding index {index}"))?;
        ensure!(
            definition.default.same_type(&value.value),
            "native variable type index {index}"
        );
    }
    Ok(())
}

/// Validated and canonicalized authored definitions, with no external aliases.
#[derive(Clone)]
pub struct CompiledNativeCatalog {
    identity: String,
    definition: NativeCatalogDefinition,
}

impl CompiledNativeCatalog {
    pub fn compile(mut definition: NativeCatalogDefinition) -> Result<Self> {
        ensure!(
            definition.native_format == NATIVE_FORMAT,
            "unsupported native catalog format"
        );
        validate_keys(&definition.classes, MAX_NATIVE_CLASSES, |v| &v.key)
            .context("native classes")?;
        validate_keys(&definition.maps, MAX_NATIVE_MAPS, |v| &v.key).context("native maps")?;
        let mut explicit_values = 0;
        let mut default_bytes = BTreeMap::new();
        for (index, class) in definition.classes.iter().enumerate() {
            class
                .local_bounds
                .validate()
                .with_context(|| format!("native class bounds index {index}"))?;
            validate_keys(&class.variables, MAX_NATIVE_VARIABLES_PER_CLASS, |v| &v.key)
                .with_context(|| format!("native class variables index {index}"))?;
            add_count(
                &mut explicit_values,
                class.variables.len(),
                MAX_NATIVE_VARIABLE_VALUES,
            )?;
            for (variable_index, variable) in class.variables.iter().enumerate() {
                variable.default.validate().with_context(|| {
                    format!("native class index {index} variable index {variable_index}")
                })?;
                default_bytes.insert(
                    (class.key.as_str(), variable.key.as_str()),
                    variable_bytes(&variable.key, &variable.default)?,
                );
            }
        }
        let classes: BTreeMap<_, _> = definition
            .classes
            .iter()
            .map(|v| (v.key.as_str(), v))
            .collect();
        let (mut tiles, mut placements, mut resolved_values, mut resolved_bytes) = (0, 0, 0, 0);
        for (index, map) in definition.maps.iter().enumerate() {
            let validate_map = || -> Result<()> {
                validate_keys(&map.tiles, MAX_NATIVE_TILES, |v| &v.key)?;
                validate_keys(&map.placements, MAX_NATIVE_PLACEMENTS, |v| &v.key)?;
                ensure!(!map.placements.is_empty(), "native map requires placements");
                for (tile_index, tile) in map.tiles.iter().enumerate() {
                    tile.bounds
                        .validate()
                        .with_context(|| format!("native tile index {tile_index}"))?;
                }
                Ok(())
            };
            validate_map().with_context(|| format!("native map index {index}"))?;
            add_count(&mut tiles, map.tiles.len(), MAX_NATIVE_TILES)?;
            add_count(&mut placements, map.placements.len(), MAX_NATIVE_PLACEMENTS)?;
            let tiles: BTreeMap<_, _> = map.tiles.iter().map(|v| (v.key.as_str(), v)).collect();
            for (placement_index, placement) in map.placements.iter().enumerate() {
                let validate_placement = || -> Result<()> {
                    validate_key(&placement.class)?;
                    validate_key(&placement.tile)?;
                    let class = classes
                        .get(placement.class.as_str())
                        .context("native placement class binding")?;
                    let tile = tiles
                        .get(placement.tile.as_str())
                        .context("native placement tile binding")?;
                    validate_values(&placement.initial_values, class, false)?;
                    ensure!(
                        tile.bounds.contains(
                            &placement
                                .transform
                                .transformed_bounds(&class.local_bounds)?
                        ),
                        "native placement outside tile"
                    );
                    Ok(())
                };
                validate_placement().with_context(|| {
                    format!("native map index {index} placement index {placement_index}")
                })?;
                add_count(
                    &mut explicit_values,
                    placement.initial_values.len(),
                    MAX_NATIVE_VARIABLE_VALUES,
                )?;
                add_count(
                    &mut resolved_values,
                    classes[placement.class.as_str()].variables.len(),
                    MAX_NATIVE_VARIABLE_VALUES,
                )?;
                for variable in &classes[placement.class.as_str()].variables {
                    let bytes = if let Some(value) = placement
                        .initial_values
                        .iter()
                        .find(|v| v.key == variable.key)
                    {
                        variable_bytes(&value.key, &value.value)?
                    } else {
                        default_bytes[&(placement.class.as_str(), variable.key.as_str())]
                    };
                    add_count(&mut resolved_bytes, bytes, MAX_NATIVE_VARIABLE_BYTES)?;
                }
            }
        }
        // Duplicate checks above precede every canonicalization operation.
        for class in &mut definition.classes {
            class.variables.sort_by(|a, b| a.key.cmp(&b.key));
        }
        definition.classes.sort_by(|a, b| a.key.cmp(&b.key));
        for map in &mut definition.maps {
            map.tiles.sort_by(|a, b| a.key.cmp(&b.key));
            map.placements.sort_by(|a, b| a.key.cmp(&b.key));
            for placement in &mut map.placements {
                placement.initial_values.sort_by(|a, b| a.key.cmp(&b.key));
            }
        }
        definition.maps.sort_by(|a, b| a.key.cmp(&b.key));
        let bytes = canonical_bytes(&definition)?;
        let identity = crate::hex_lower(&Sha256::digest(bytes));
        Ok(Self {
            identity,
            definition,
        })
    }

    /// Accept JSON objects or TOML tables, under the same strict authored schema.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_NATIVE_CATALOG_BYTES,
            "native catalog byte limit"
        );
        let definition = if matches!(text.trim_start().as_bytes().first(), Some(b'{' | b'[')) {
            serde_json::from_str(text)
                .map_err(|error| crate::json_parse_error("native catalog JSON", error))?
        } else {
            toml::from_str(text).map_err(|_| anyhow::anyhow!("native catalog TOML invalid"))?
        };
        Self::compile(definition)
    }

    /// Open only regular files, nonblocking, then bound the actual read as well
    /// as metadata. Diagnostics never retain input paths or supplied text.
    pub fn load(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .map_err(|error| anyhow::anyhow!("native catalog open: {:?}", error.kind()))?;
        let metadata = file
            .metadata()
            .map_err(|error| anyhow::anyhow!("native catalog metadata: {:?}", error.kind()))?;
        ensure!(metadata.is_file(), "native catalog requires regular file");
        ensure!(
            metadata.len() <= MAX_NATIVE_CATALOG_BYTES as u64,
            "native catalog byte limit"
        );
        let mut bytes = Vec::new();
        file.take(MAX_NATIVE_CATALOG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| anyhow::anyhow!("native catalog read: {:?}", error.kind()))?;
        ensure!(
            bytes.len() <= MAX_NATIVE_CATALOG_BYTES,
            "native catalog byte limit"
        );
        let text = std::str::from_utf8(&bytes)
            .map_err(|_| anyhow::anyhow!("native catalog encoding invalid"))?;
        Self::parse(text)
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn definition(&self) -> &NativeCatalogDefinition {
        &self.definition
    }
    pub fn class(&self, key: &str) -> Option<&NativeClassDefinition> {
        self.definition
            .classes
            .binary_search_by(|v| v.key.as_str().cmp(key))
            .ok()
            .map(|i| &self.definition.classes[i])
    }
    pub fn map(&self, key: &str) -> Option<&NativeMapDefinition> {
        self.definition
            .maps
            .binary_search_by(|v| v.key.as_str().cmp(key))
            .ok()
            .map(|i| &self.definition.maps[i])
    }
}

fn canonical_bytes(definition: &NativeCatalogDefinition) -> Result<Vec<u8>> {
    struct BoundedBytes(Vec<u8>);
    impl Write for BoundedBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_NATIVE_CATALOG_BYTES.saturating_sub(self.0.len()) {
                return Err(std::io::Error::other("native catalog byte limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = BoundedBytes(Vec::new());
    serde_json::to_writer(&mut writer, definition)
        .map_err(|_| anyhow::anyhow!("native catalog serialization or byte limit"))?;
    Ok(writer.0)
}

impl NativeWorldSnapshot {
    /// Validate a complete fixed topology, permitting mutable values and
    /// transforms only within each placement's original class and tile.
    pub fn validate(&self) -> Result<()> {
        let catalog = CompiledNativeCatalog::compile(self.definitions.clone())?;
        ensure!(
            self.catalog_id == catalog.identity(),
            "native snapshot catalog identity mismatch"
        );
        catalog.validate_records(&self.map, self.actors.iter().map(NativeActorView::from))
    }
}

impl CompiledNativeCatalog {
    /// Validate the complete mutable state against already verified, immutable
    /// definitions. Persisted snapshots must also verify their embedded catalog.
    pub(crate) fn validate_records<'a>(
        &self,
        map: &str,
        actors: impl ExactSizeIterator<Item = NativeActorView<'a>>,
    ) -> Result<()> {
        validate_key(map).context("native snapshot map")?;
        let map = self.map(map).context("native snapshot map binding")?;
        ensure!(
            !map.placements.is_empty() && actors.len() == map.placements.len(),
            "native snapshot placement set incomplete"
        );
        let mut seen = vec![false; map.placements.len()];
        let mut values = 0;
        let mut value_bytes = 0;
        for (index, actor) in actors.enumerate() {
            let validate_actor = || -> Result<usize> {
                // Exact fast path: compiled keys are valid, unique and sorted,
                // so a key equal to the identity's rank (ID - 1) passes
                // validate_key and is exactly what the search would return.
                let rank = match usize::try_from(actor.id.0)
                    .ok()
                    .and_then(|id| id.checked_sub(1))
                    .filter(|rank| {
                        map.placements
                            .get(*rank)
                            .is_some_and(|v| v.key == actor.placement)
                    }) {
                    Some(rank) => rank,
                    None => {
                        validate_key(actor.placement)?;
                        map.placements
                            .binary_search_by(|v| v.key.as_str().cmp(actor.placement))
                            .ok()
                            .context("native actor placement binding")?
                    }
                };
                let placement = &map.placements[rank];
                ensure!(
                    actor.id.0 == rank as u64 + 1,
                    "native actor domain identity mismatch"
                );
                ensure!(
                    actor.class == placement.class && actor.tile == placement.tile,
                    "native actor immutable binding mismatch"
                );
                let class = self
                    .class(&placement.class)
                    .context("native actor class binding")?;
                let tile = map
                    .tiles
                    .binary_search_by(|v| v.key.cmp(&placement.tile))
                    .ok()
                    .map(|i| &map.tiles[i])
                    .context("native actor tile binding")?;
                validate_values(actor.variables, class, true)?;
                ensure!(
                    tile.bounds
                        .contains(&actor.transform.transformed_bounds(&class.local_bounds)?),
                    "native actor outside tile"
                );
                Ok(rank)
            };
            let rank = validate_actor().with_context(|| format!("native actor index {index}"))?;
            ensure!(
                !std::mem::replace(&mut seen[rank], true),
                "native duplicate placement index {index}"
            );
            add_count(
                &mut values,
                actor.variables.len(),
                MAX_NATIVE_VARIABLE_VALUES,
            )?;
            for variable in actor.variables {
                add_count(
                    &mut value_bytes,
                    variable_bytes(&variable.key, &variable.value)?,
                    MAX_NATIVE_VARIABLE_BYTES,
                )?;
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct PreparedNativeWorld {
    snapshot: NativeWorldSnapshot,
}

impl PreparedNativeWorld {
    pub fn snapshot(&self) -> &NativeWorldSnapshot {
        &self.snapshot
    }
}

/// Resolve every placement before exposing a prepared world. No ECS or store
/// writes occur; the catalog remains immutable and the caller owns publication.
pub fn prepare_authored_world(
    catalog: Arc<CompiledNativeCatalog>,
    map: &str,
) -> Result<PreparedNativeWorld> {
    validate_key(map).context("native preparation map")?;
    let definition = catalog.map(map).context("native preparation map binding")?;
    let mut actors = Vec::with_capacity(definition.placements.len());
    for (rank, placement) in definition.placements.iter().enumerate() {
        let class = catalog
            .class(&placement.class)
            .context("native preparation class binding")?;
        let variables = class
            .variables
            .iter()
            .map(|variable| NativeVariableValue {
                key: variable.key.clone(),
                value: placement
                    .initial_values
                    .iter()
                    .find(|v| v.key == variable.key)
                    .map_or_else(|| variable.default.clone(), |v| v.value.clone()),
            })
            .collect();
        actors.push(NativeActorRecord {
            id: DomainId(rank as u64 + 1),
            placement: placement.key.clone(),
            class: placement.class.clone(),
            tile: placement.tile.clone(),
            transform: placement.transform.clone(),
            variables,
        });
    }
    let snapshot = NativeWorldSnapshot {
        catalog_id: catalog.identity().to_owned(),
        definitions: catalog.definition().clone(),
        map: map.to_owned(),
        actors,
    };
    snapshot.validate()?;
    Ok(PreparedNativeWorld { snapshot })
}

// Every diagnostic representation is structural. Never expose authored names,
// identities, transforms or supplied variable contents through Debug.
macro_rules! redacted_debug {
    ($name:ident, $($label:literal => $value:expr),* $(,)?) => {
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                let mut debug = f.debug_struct(stringify!($name));
                $(debug.field($label, &($value)(self));)*
                debug.finish_non_exhaustive()
            }
        }
    };
}
redacted_debug!(NativeCatalogDefinition, "classes" => |v: &NativeCatalogDefinition| v.classes.len(), "maps" => |v: &NativeCatalogDefinition| v.maps.len());
redacted_debug!(NativeClassDefinition, "variables" => |v: &NativeClassDefinition| v.variables.len());
redacted_debug!(NativeMapDefinition, "tiles" => |v: &NativeMapDefinition| v.tiles.len(), "placements" => |v: &NativeMapDefinition| v.placements.len());
redacted_debug!(NativePlacement, "initial_values" => |v: &NativePlacement| v.initial_values.len());
redacted_debug!(NativeActorRecord, "variables" => |v: &NativeActorRecord| v.variables.len());
redacted_debug!(NativeWorldSnapshot, "actors" => |v: &NativeWorldSnapshot| v.actors.len());
redacted_debug!(CompiledNativeCatalog, "classes" => |v: &CompiledNativeCatalog| v.definition.classes.len(), "maps" => |v: &CompiledNativeCatalog| v.definition.maps.len());
redacted_debug!(PreparedNativeWorld, "actors" => |v: &PreparedNativeWorld| v.snapshot.actors.len());
redacted_debug!(VariableDefinition,);
redacted_debug!(NativeVariableValue,);
redacted_debug!(NativeTileDefinition,);
redacted_debug!(NativeTransform,);
redacted_debug!(Aabb3,);
redacted_debug!(NativeValue,);

#[cfg(test)]
mod tests {
    use super::{Aabb3, NativeTransform};

    /// Identity extremes equal the general corner transform, up to the sign of
    /// zero, across signs, zeros, mixed magnitudes and extreme finite values.
    #[test]
    fn identity_extremes_match_the_corner_transform() {
        let values = [
            0.0_f32,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -2.75,
            1.0e-30,
            -3.0e7,
            1.0e30,
            -f32::MAX,
            f32::MAX,
            f32::MIN_POSITIVE,
        ];
        let mut checked = 0;
        for (index, translation) in values.iter().enumerate() {
            for scale in [1.0_f32, 2.5, 1.0e-20, 3.0e20, f32::MIN_POSITIVE, f32::MAX] {
                for (low, high) in [(-0.5_f32, 0.5), (0.0, 0.0), (-0.0, 0.0), (-1.0e30, 7.0)] {
                    let other = values[(index * 7 + 3) % values.len()];
                    let transform = NativeTransform {
                        translation: [*translation, other, -*translation],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [scale, scale.min(0.75), 1.0],
                    };
                    let local = Aabb3 {
                        min: [low, low, -high],
                        max: [high, high, -low],
                    };
                    if transform.validate().is_err() || local.validate().is_err() {
                        continue;
                    }
                    let fast = transform.identity_extremes(&local).unwrap();
                    let corners = transform.corner_extremes(&local).unwrap();
                    assert_eq!(fast, corners, "{transform:?} {local:?}");
                    checked += 1;
                }
            }
        }
        assert!(checked > 100, "{checked}");
    }
}
