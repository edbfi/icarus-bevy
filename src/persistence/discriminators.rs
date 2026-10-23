//! Streaming schema-9 upgrade probes. Visit every occurrence, including duplicate
//! members; typed decoding remains responsible for field admission afterward.
use serde::Deserialize;
use serde::Deserializer;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};

#[derive(Clone, Copy)]
enum Kind {
    Format(u32),
    Enum(&'static [&'static str]),
}

#[derive(Clone, Copy)]
struct Rule {
    path: &'static str,
    kind: Kind,
}

const VALUES: Kind = Kind::Enum(&["bool", "i32", "f32", "string"]);
const RECORDER: Kind = Kind::Enum(&["spatial_state_v1"]);
const PRESENCE: Kind = Kind::Enum(&["absent", "live"]);
const POLICY: Kind = Kind::Enum(&["find_only", "find_or_respawn", "respawn_only"]);
const SOURCE: Kind = Kind::Enum(&["bound", "detached"]);
const DISPOSITION: Kind = Kind::Enum(&["kept_bound", "attached", "spawned", "replaced"]);
const COMPONENT: Kind = Kind::Enum(&["existing", "created"]);

macro_rules! rules {
    ($($path:literal => $kind:expr),* $(,)?) => {
        &[$(Rule { path: $path, kind: $kind }),*]
    };
}

const RULES: &[Rule] = rules![
    "native_world/definitions/native_format" => Kind::Format(crate::native_world::NATIVE_FORMAT),
    "native_world/definitions/frame" => Kind::Enum(&["right_handed_z_up_meters"]),
    "native_world/definitions/classes/*/kind" => Kind::Enum(&["spatial_object_v1"]),
    "native_world/definitions/classes/*/variables/*/default/type" => VALUES,
    "native_world/definitions/maps/*/placements/*/initial_values/*/value/type" => VALUES,
    "native_world/actors/*/variables/*/value/type" => VALUES,
    "native_lifecycle/definition/lifecycle_format" => Kind::Format(crate::native_lifecycle::NATIVE_LIFECYCLE_FORMAT),
    "native_lifecycle/definition/slots/*/recorder_kind" => RECORDER,
    "native_lifecycle/definition/slots/*/initial_presence" => PRESENCE,
    "native_lifecycle/definition/slots/*/spawn_component" => Kind::Enum(&["missing", "declared_kind"]),
    "native_lifecycle/definition/slots/*/initial_component/kind" => RECORDER,
    "native_lifecycle/slots/*/presence" => PRESENCE,
    "native_lifecycle/slots/*/component/kind" => RECORDER,
    "native_lifecycle/bindings/*/kind" => RECORDER,
    "native_lifecycle/completed/outcomes/*/policy" => POLICY,
    "native_lifecycle/completed/outcomes/*/source/type" => SOURCE,
    "native_lifecycle/completed/outcomes/*/before_presence" => PRESENCE,
    "native_lifecycle/completed/outcomes/*/disposition" => DISPOSITION,
    "native_lifecycle/completed/outcomes/*/component_disposition" => COMPONENT,
    "native_relationships/definition/relationship_format" => Kind::Format(crate::native_relationships::NATIVE_RELATIONSHIP_FORMAT),
    "native_relationships/definition/actors/*/role" => Kind::Enum(&["spatial", "deployable", "foundation"]),
    "native_scalar_contracts/definition/format" => Kind::Format(1),
    "native_scalar_contracts/definition/classes/*/callback" => Kind::Enum(&["recompute_use_availability_v1"]),
    "native_scalar_owner_import/format" => Kind::Format(1),
    "native_scalar_owner_import/bindings/format" => Kind::Format(1),
    "native_scalar_owner_import/bindings/adapter" => Kind::Enum(&["c001_authored_scalar_owners_v1"]),
    "native_scalar_owner_import/bindings/coordinates" => Kind::Enum(&["right_handed_z_up_meters_quat_xyzw_world_space"]),
    "native_scalar_owner_import/bindings/owners/*/owner/expected_presence" => PRESENCE,
    "native_scalar_owner_import/resolved/*/source_policy" => POLICY,
    "native_scalar_owner_import/initial_completion/outcomes/*/policy" => POLICY,
    "native_scalar_owner_import/initial_completion/outcomes/*/source/type" => SOURCE,
    "native_scalar_owner_import/initial_completion/outcomes/*/before_presence" => PRESENCE,
    "native_scalar_owner_import/initial_completion/outcomes/*/disposition" => DISPOSITION,
    "native_scalar_owner_import/initial_completion/outcomes/*/component_disposition" => COMPONENT,
    "preserved_prospect/encoding" => Kind::Format(crate::preservation::PRESERVED_ENCODING_VERSION),
];

struct Probe<'a> {
    rules: &'a [Rule],
    future: &'a mut bool,
}

#[derive(Default)]
struct HeaderProbe {
    scalar_owners: bool,
    future: bool,
}

impl<'de> Visitor<'de> for &mut HeaderProbe {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("snapshot object")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if key == "schema_version" {
                let value = map.next_value::<serde_json::Value>()?;
                if let Some(schema) = value.as_u64() {
                    let current = u64::from(crate::simulation::SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS);
                    self.scalar_owners |= schema == current;
                    self.future |= schema > current;
                }
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

/// Inspect all header occurrences without choosing a duplicate's winner.
/// Strict header admission still follows this upgrade-only pass.
pub(super) fn header(bytes: &[u8]) -> anyhow::Result<bool> {
    let mut probe = HeaderProbe::default();
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let result = decoder
        .deserialize_map(&mut probe)
        .and_then(|()| decoder.end());
    if probe.future {
        return Err(super::UnsupportedSchema.into());
    }
    result.map_err(|error| crate::json_parse_error("decode native snapshot JSON", error))?;
    Ok(probe.scalar_owners)
}

impl Probe<'_> {
    fn children(&self, key: &str) -> Vec<Rule> {
        self.rules
            .iter()
            .filter_map(|rule| {
                let (head, tail) = rule.path.split_once('/').unwrap_or((rule.path, ""));
                (head == key && !head.is_empty()).then_some(Rule {
                    path: tail,
                    kind: rule.kind,
                })
            })
            .collect()
    }
}

impl<'de> DeserializeSeed<'de> for Probe<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        if self.rules.is_empty() {
            IgnoredAny::deserialize(deserializer).map(|_| ())
        } else {
            deserializer.deserialize_any(self)
        }
    }
}

impl<'de> Visitor<'de> for Probe<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("snapshot discriminator value")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            // '*' selects array elements only, never a supplied object member.
            let children = if key == "*" {
                Vec::new()
            } else {
                self.children(&key)
            };
            map.next_value_seed(Probe {
                rules: &children,
                future: self.future,
            })?;
        }
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let children = self.children("*");
        while seq
            .next_element_seed(Probe {
                rules: &children,
                future: self.future,
            })?
            .is_some()
        {}
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<(), E> {
        *self.future |= self.rules.iter().any(|r| {
            r.path.is_empty()
                && matches!(r.kind, Kind::Format(current) if value > u64::from(current))
        });
        Ok(())
    }
    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        *self.future |= self.rules.iter().any(|r| {
            r.path.is_empty()
                && matches!(r.kind, Kind::Enum(accepted) if !accepted.contains(&value))
        });
        Ok(())
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Ok(())
    }
}

pub(super) fn scalar_owners(bytes: &[u8]) -> anyhow::Result<()> {
    let mut future = false;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let result = Probe {
        rules: RULES,
        future: &mut future,
    }
    .deserialize(&mut decoder)
    .and_then(|()| decoder.end());
    // A detected upgrade requirement wins even if a later member is malformed.
    if future {
        return Err(super::UnsupportedSchema.into());
    }
    result.map_err(|error| {
        crate::json_parse_error("decode native snapshot discriminators JSON", error)
    })
}
