//! Independently authored limited-use scalar owner contracts and native results.
use crate::{
    native_lifecycle::{OwnerHandle, validate_native_bounds},
    native_world::{
        CompiledNativeCatalog, MAX_NATIVE_CLASSES, MAX_NATIVE_KEY_BYTES, NativeClassKind,
        NativeValue, NativeVariableValue, NativeWorldSnapshot,
    },
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

pub const SCALAR_OWNER_CONTRACT_FORMAT: u32 = 1;
pub const MAX_SCALAR_OWNER_CONTRACT_BYTES: usize = 4 * 1024 * 1024;
macro_rules! object_dto {
    (pub struct $name:ident { $(pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Serialize)] pub struct $name { $(pub $field: $ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self,D::Error> {
                #[derive(Deserialize)] #[serde(deny_unknown_fields)] struct Fields { $($field: $ty),* }
                let fields: Fields = crate::json::object(deserializer).map_err(|_| <D::Error as serde::de::Error>::custom("scalar owner object invalid"))?;
                Ok(Self { $($field: fields.$field),* })
            }
        }
        impl fmt::Debug for $name { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.debug_struct(stringify!($name)).finish_non_exhaustive() } }
    };
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Debug)]
pub enum ScalarOwnerCallback {
    #[serde(rename = "recompute_use_availability_v1")]
    RecomputeUseAvailabilityV1,
}
impl<'de> Deserialize<'de> for ScalarOwnerCallback {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = ScalarOwnerCallback;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("scalar owner callback string")
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                match v {
                    "recompute_use_availability_v1" => {
                        Ok(ScalarOwnerCallback::RecomputeUseAvailabilityV1)
                    }
                    _ => Err(E::custom("scalar owner callback invalid")),
                }
            }
        }
        d.deserialize_str(Visitor)
            .map_err(|_| <D::Error as serde::de::Error>::custom("scalar owner callback invalid"))
    }
}
object_dto! { pub struct ScalarOwnerClassV1 { pub native_class: String, pub authored_max: i32, pub callback: ScalarOwnerCallback } }
object_dto! { pub struct ScalarOwnerContractsV1 { pub format: u32, pub native_catalog_id: String, pub native_map: String, pub classes: Vec<ScalarOwnerClassV1> } }
object_dto! { pub struct ScalarOwnerContractsSnapshotV1 { pub contract_id: String, pub definition: ScalarOwnerContractsV1 } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarValues {
    pub enabled: bool,
    pub remaining_uses: i32,
    pub available: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalarOwnerState {
    pub owner: OwnerHandle,
    pub enabled: bool,
    pub remaining_uses: i32,
    pub available: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarUseOutcome {
    Used(ScalarOwnerState),
    Disabled(ScalarOwnerState),
    Exhausted(ScalarOwnerState),
}

pub(crate) fn encoded_size(value: &impl Serialize, limit: usize) -> Result<usize> {
    struct Counter {
        size: usize,
        limit: usize,
    }
    impl Write for Counter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.size = self
                .size
                .checked_add(b.len())
                .filter(|n| *n <= self.limit)
                .ok_or_else(|| std::io::Error::other("scalar owner byte limit"))?;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut w = Counter { size: 0, limit };
    serde_json::to_writer(&mut w, value)
        .map_err(|_| anyhow::anyhow!("scalar owner serialization or byte limit"))?;
    Ok(w.size)
}
pub(crate) fn key(value: &String) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= MAX_NATIVE_KEY_BYTES
            && value.capacity() <= MAX_NATIVE_KEY_BYTES
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
        "scalar owner key invalid"
    );
    Ok(())
}
pub(crate) fn digest(value: &String) -> Result<()> {
    ensure!(
        value.len() == 64
            && value.capacity() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "scalar owner digest invalid"
    );
    Ok(())
}
pub(crate) fn canonical_digest(prefix: &[u8], value: &impl Serialize) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(prefix);
    let bytes = serde_json::to_vec(value)
        .map_err(|_| anyhow::anyhow!("scalar owner canonical encoding invalid"))?;
    hash.update(bytes);
    Ok(crate::hex_lower(&hash.finalize()))
}
pub(crate) fn load_text(path: &Path, limit: usize) -> Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(crate::nonblocking_flag())
        .open(path)
        .map_err(|e| anyhow::anyhow!("scalar owner file open: {:?}", e.kind()))?;
    let m = file
        .metadata()
        .map_err(|e| anyhow::anyhow!("scalar owner file metadata: {:?}", e.kind()))?;
    ensure!(
        m.is_file() && m.len() <= limit as u64,
        "scalar owner regular file or size invalid"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| anyhow::anyhow!("scalar owner file read: {:?}", e.kind()))?;
    ensure!(bytes.len() <= limit, "scalar owner file size limit");
    String::from_utf8(bytes).map_err(|_| anyhow::anyhow!("scalar owner file encoding invalid"))
}
impl ScalarOwnerContractsV1 {
    pub(crate) fn validate_bounds(&self) -> Result<()> {
        ensure!(
            self.format == 1
                && !self.classes.is_empty()
                && self.classes.capacity() <= MAX_NATIVE_CLASSES,
            "scalar owner contract format or class count invalid"
        );
        digest(&self.native_catalog_id)?;
        key(&self.native_map)?;
        for c in &self.classes {
            key(&c.native_class)?;
            ensure!(c.authored_max >= 1, "scalar owner maximum invalid");
        }
        encoded_size(self, MAX_SCALAR_OWNER_CONTRACT_BYTES)?;
        Ok(())
    }
}
impl ScalarOwnerContractsSnapshotV1 {
    pub(crate) fn validate_bounds(&self) -> Result<()> {
        digest(&self.contract_id)?;
        self.definition.validate_bounds()?;
        encoded_size(self, MAX_SCALAR_OWNER_CONTRACT_BYTES)?;
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct CompiledScalarOwnerContracts {
    identity: String,
    definition: ScalarOwnerContractsV1,
    maxima: BTreeMap<String, i32>,
}
impl CompiledScalarOwnerContracts {
    pub fn compile(
        mut definition: ScalarOwnerContractsV1,
        catalog: &CompiledNativeCatalog,
    ) -> Result<Self> {
        definition.validate_bounds()?;
        ensure!(
            definition.native_catalog_id == catalog.identity(),
            "scalar owner catalog identity mismatch"
        );
        let map = catalog
            .map(&definition.native_map)
            .context("scalar owner map missing")?;
        let used: BTreeSet<_> = map.placements.iter().map(|p| p.class.as_str()).collect();
        let mut maxima = BTreeMap::new();
        for contract in &definition.classes {
            ensure!(
                used.contains(contract.native_class.as_str())
                    && maxima
                        .insert(contract.native_class.clone(), contract.authored_max)
                        .is_none(),
                "scalar owner unused or duplicate contract"
            );
            let class = catalog
                .class(&contract.native_class)
                .context("scalar owner class missing")?;
            ensure!(
                class.kind == NativeClassKind::SpatialObjectV1 && class.variables.len() == 2,
                "scalar owner class declaration invalid"
            );
            let enabled = class
                .variables
                .iter()
                .find(|v| v.key == "enabled")
                .context("scalar owner enabled declaration missing")?;
            let count = class
                .variables
                .iter()
                .find(|v| v.key == "remaining_uses")
                .context("scalar owner count declaration missing")?;
            ensure!(
                matches!(enabled.default, NativeValue::Bool(_))
                    && matches!(count.default,NativeValue::I32(n) if n>=0 && n<=contract.authored_max),
                "scalar owner defaults invalid"
            );
            for placement in map
                .placements
                .iter()
                .filter(|p| p.class == contract.native_class)
            {
                ensure!(
                    placement.initial_values.capacity() <= 4,
                    "scalar owner override capacity limit"
                );
                for v in &placement.initial_values {
                    match (&*v.key, &v.value) {
                        ("enabled", NativeValue::Bool(_)) => {}
                        ("remaining_uses", NativeValue::I32(n))
                            if *n >= 0 && *n <= contract.authored_max => {}
                        _ => anyhow::bail!("scalar owner placement override invalid"),
                    }
                }
            }
        }
        ensure!(
            used.len() == maxima.len(),
            "scalar owner contract coverage invalid"
        );
        definition
            .classes
            .sort_by(|a, b| a.native_class.cmp(&b.native_class));
        let identity = canonical_digest(b"icarus/native_scalar_contracts/v1\0", &definition)?;
        Ok(Self {
            identity,
            definition,
            maxima,
        })
    }
    pub fn parse(text: &str, catalog: &CompiledNativeCatalog) -> Result<Self> {
        ensure!(
            text.len() <= MAX_SCALAR_OWNER_CONTRACT_BYTES,
            "scalar owner contract input limit"
        );
        let definition = if text.trim_start().starts_with('{') {
            serde_json::from_str(text)
                .map_err(|_| anyhow::anyhow!("scalar owner contract JSON invalid"))?
        } else {
            toml::from_str(text)
                .map_err(|_| anyhow::anyhow!("scalar owner contract TOML invalid"))?
        };
        Self::compile(definition, catalog)
    }
    pub fn load(path: &Path, catalog: &CompiledNativeCatalog) -> Result<Self> {
        Self::parse(&load_text(path, MAX_SCALAR_OWNER_CONTRACT_BYTES)?, catalog)
    }
    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn definition(&self) -> &ScalarOwnerContractsV1 {
        &self.definition
    }
    pub fn snapshot(&self) -> ScalarOwnerContractsSnapshotV1 {
        ScalarOwnerContractsSnapshotV1 {
            contract_id: self.identity.clone(),
            definition: self.definition.clone(),
        }
    }
    pub fn from_snapshot(
        snapshot: &ScalarOwnerContractsSnapshotV1,
        native: &NativeWorldSnapshot,
    ) -> Result<Self> {
        snapshot.validate_bounds()?;
        validate_native_bounds(native)?;
        ensure!(
            native.actors.iter().all(|a| a.variables.capacity() <= 4),
            "scalar owner variable capacity limit"
        );
        let catalog = CompiledNativeCatalog::compile(native.definitions.clone())?;
        ensure!(
            catalog.identity() == native.catalog_id,
            "scalar owner catalog identity mismatch"
        );
        let result = Self::compile(snapshot.definition.clone(), &catalog)?;
        ensure!(
            result.identity == snapshot.contract_id,
            "scalar owner contract identity mismatch"
        );
        result.validate_native(native)?;
        Ok(result)
    }
    pub fn evaluate(&self, class: &str, variables: &[NativeVariableValue]) -> Result<ScalarValues> {
        let max = *self
            .maxima
            .get(class)
            .context("scalar owner contract missing")?;
        evaluate_values(max, variables)
    }

    pub fn validate_native(&self, native: &NativeWorldSnapshot) -> Result<()> {
        validate_native_bounds(native)?;
        ensure!(
            native.catalog_id == self.definition.native_catalog_id
                && native.map == self.definition.native_map,
            "scalar owner current identity mismatch"
        );
        let map = native
            .definitions
            .maps
            .iter()
            .find(|m| m.key == native.map)
            .context("scalar owner current map missing")?;
        ensure!(
            map.placements.len() == native.actors.len(),
            "scalar owner current coverage invalid"
        );
        let mut ids = BTreeSet::new();
        for actor in &native.actors {
            ensure!(
                actor.variables.capacity() <= 4 && ids.insert(actor.id),
                "scalar owner current allocation or identity invalid"
            );
            let placement = map
                .placements
                .iter()
                .find(|p| p.key == actor.placement)
                .context("scalar owner current placement missing")?;
            ensure!(
                actor.class == placement.class,
                "scalar owner current class mismatch"
            );
            self.evaluate(&actor.class, &actor.variables)?;
        }
        Ok(())
    }
}

pub(crate) fn evaluate_values(max: i32, variables: &[NativeVariableValue]) -> Result<ScalarValues> {
    ensure!(
        variables.len() == 2,
        "scalar owner requires complete scalar pair"
    );
    let mut enabled = None;
    let mut count = None;
    for variable in variables {
        key(&variable.key)?;
        match (variable.key.as_str(), &variable.value) {
            ("enabled", NativeValue::Bool(v)) if enabled.is_none() => enabled = Some(*v),
            ("remaining_uses", NativeValue::I32(v)) if count.is_none() && *v >= 0 && *v <= max => {
                count = Some(*v)
            }
            _ => anyhow::bail!("scalar owner variable invalid"),
        }
    }
    let enabled = enabled.context("scalar owner enabled missing")?;
    let remaining_uses = count.context("scalar owner count missing")?;
    Ok(ScalarValues {
        enabled,
        remaining_uses,
        available: enabled && remaining_uses > 0,
    })
}
