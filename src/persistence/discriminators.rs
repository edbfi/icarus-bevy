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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_lifecycle::{
        ComponentDisposition, Presence, RecorderKind, RecorderSource, ResolvePolicy,
        RestoreDisposition, SpawnComponent,
    };
    use crate::native_world::{NativeClassKind, NativeFrame, NativeValue};
    use serde::{Serialize, de::DeserializeOwned};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;

    struct Legacy {
        path: &'static str,
        probe: fn(&Value) -> bool,
    }

    struct Case {
        path: &'static str,
        accepted: Vec<Value>,
        legacy: Option<Legacy>,
    }

    impl Case {
        fn specific(path: &'static str, accepted: &[Value]) -> Self {
            Self {
                path,
                accepted: accepted.to_vec(),
                legacy: None,
            }
        }

        fn shared(path: &'static str, probe: fn(&Value) -> bool, accepted: &[Value]) -> Self {
            Self::remapped(path, path.split_once('/').unwrap().1, probe, accepted)
        }

        fn remapped(
            path: &'static str,
            legacy_path: &'static str,
            probe: fn(&Value) -> bool,
            accepted: &[Value],
        ) -> Self {
            Self {
                legacy: Some(Legacy {
                    path: legacy_path,
                    probe,
                }),
                ..Self::specific(path, accepted)
            }
        }
    }

    // Obtain spellings from real serializers, and check their deserializers too.
    // Neither the fixtures nor their expected classifications come from RULES.
    fn wire<T: Serialize + DeserializeOwned>(values: impl IntoIterator<Item = T>) -> Vec<Value> {
        values
            .into_iter()
            .map(|value| {
                let encoded = serde_json::to_value(value).unwrap();
                let decoded: T = serde_json::from_value(encoded.clone()).unwrap();
                assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
                encoded
            })
            .collect()
    }

    fn cases() -> Vec<Case> {
        use super::super::{
            future_completion_discriminator, future_lifecycle_discriminator,
            future_native_discriminator, future_owner_import_discriminator,
            future_relationship_discriminator,
        };
        use crate::native_relationships::NativeRelationshipRole;

        let values: Vec<_> = wire([
            NativeValue::Bool(false),
            NativeValue::I32(0),
            NativeValue::F32(0.0),
            NativeValue::String(String::new()),
        ])
        .into_iter()
        .map(|value| value["type"].clone())
        .collect();
        let recorder = wire([RecorderKind::SpatialStateV1]);
        let presence = wire([Presence::Absent, Presence::Live]);
        let policy = wire([
            ResolvePolicy::FindOnly,
            ResolvePolicy::FindOrRespawn,
            ResolvePolicy::RespawnOnly,
        ]);
        let source: Vec<_> = wire([
            RecorderSource::Bound,
            RecorderSource::Detached {
                expected_prior_binding: None,
            },
        ])
        .into_iter()
        .map(|value| value["type"].clone())
        .collect();
        let disposition = wire([
            RestoreDisposition::KeptBound,
            RestoreDisposition::Attached,
            RestoreDisposition::Spawned,
            RestoreDisposition::Replaced,
        ]);
        let component = wire([
            ComponentDisposition::Existing,
            ComponentDisposition::Created,
        ]);

        vec![
            Case::shared(
                "native_world/definitions/native_format",
                future_native_discriminator,
                &[json!(crate::native_world::NATIVE_FORMAT)],
            ),
            Case::shared(
                "native_world/definitions/frame",
                future_native_discriminator,
                &wire([NativeFrame::RightHandedZUpMeters]),
            ),
            Case::shared(
                "native_world/definitions/classes/*/kind",
                future_native_discriminator,
                &wire([NativeClassKind::SpatialObjectV1]),
            ),
            Case::shared(
                "native_world/definitions/classes/*/variables/*/default/type",
                future_native_discriminator,
                &values,
            ),
            Case::shared(
                "native_world/definitions/maps/*/placements/*/initial_values/*/value/type",
                future_native_discriminator,
                &values,
            ),
            Case::shared(
                "native_world/actors/*/variables/*/value/type",
                future_native_discriminator,
                &values,
            ),
            Case::shared(
                "native_lifecycle/definition/lifecycle_format",
                future_lifecycle_discriminator,
                &[json!(crate::native_lifecycle::NATIVE_LIFECYCLE_FORMAT)],
            ),
            Case::shared(
                "native_lifecycle/definition/slots/*/recorder_kind",
                future_lifecycle_discriminator,
                &recorder,
            ),
            Case::shared(
                "native_lifecycle/definition/slots/*/initial_presence",
                future_lifecycle_discriminator,
                &presence,
            ),
            Case::shared(
                "native_lifecycle/definition/slots/*/spawn_component",
                future_lifecycle_discriminator,
                &wire([SpawnComponent::Missing, SpawnComponent::DeclaredKind]),
            ),
            Case::shared(
                "native_lifecycle/definition/slots/*/initial_component/kind",
                future_lifecycle_discriminator,
                &recorder,
            ),
            Case::shared(
                "native_lifecycle/slots/*/presence",
                future_lifecycle_discriminator,
                &presence,
            ),
            Case::shared(
                "native_lifecycle/slots/*/component/kind",
                future_lifecycle_discriminator,
                &recorder,
            ),
            Case::shared(
                "native_lifecycle/bindings/*/kind",
                future_lifecycle_discriminator,
                &recorder,
            ),
            Case::shared(
                "native_lifecycle/completed/outcomes/*/policy",
                future_lifecycle_discriminator,
                &policy,
            ),
            Case::shared(
                "native_lifecycle/completed/outcomes/*/source/type",
                future_lifecycle_discriminator,
                &source,
            ),
            Case::shared(
                "native_lifecycle/completed/outcomes/*/before_presence",
                future_lifecycle_discriminator,
                &presence,
            ),
            Case::shared(
                "native_lifecycle/completed/outcomes/*/disposition",
                future_lifecycle_discriminator,
                &disposition,
            ),
            Case::shared(
                "native_lifecycle/completed/outcomes/*/component_disposition",
                future_lifecycle_discriminator,
                &component,
            ),
            Case::shared(
                "native_relationships/definition/relationship_format",
                future_relationship_discriminator,
                &[json!(crate::native_relationships::NATIVE_RELATIONSHIP_FORMAT)],
            ),
            Case::shared(
                "native_relationships/definition/actors/*/role",
                future_relationship_discriminator,
                &wire([
                    NativeRelationshipRole::Spatial,
                    NativeRelationshipRole::Deployable,
                    NativeRelationshipRole::Foundation,
                ]),
            ),
            // These five schema-9-only rules have no legacy aggregate predicate.
            Case::specific(
                "native_scalar_contracts/definition/format",
                &[json!(crate::native_scalar_owners::SCALAR_OWNER_CONTRACT_FORMAT)],
            ),
            Case::specific(
                "native_scalar_contracts/definition/classes/*/callback",
                &wire([crate::native_scalar_owners::ScalarOwnerCallback::RecomputeUseAvailabilityV1]),
            ),
            Case::specific(
                "native_scalar_owner_import/format",
                &[json!(crate::native_c001_scalar_owners::C001_SCALAR_OWNER_IMPORT_FORMAT)],
            ),
            Case::specific(
                "native_scalar_owner_import/bindings/format",
                &[json!(crate::native_c001_scalar_owners::C001_SCALAR_OWNER_IMPORT_FORMAT)],
            ),
            Case::specific(
                "native_scalar_owner_import/bindings/adapter",
                &wire([crate::native_c001_scalar_owners::C001ScalarOwnerAdapter::C001AuthoredScalarOwnersV1]),
            ),
            // Schema 9 moved these fields; compare with the actual schema-8
            // owner/completion predicates at their original relative paths.
            Case::remapped(
                "native_scalar_owner_import/bindings/coordinates",
                "coordinates",
                future_owner_import_discriminator,
                &wire([crate::native_import::AuthoredCoordinatesV1::RightHandedZUpMetersQuatXyzwWorldSpace]),
            ),
            Case::remapped(
                "native_scalar_owner_import/bindings/owners/*/owner/expected_presence",
                "owners/*/binding/expected_presence",
                future_owner_import_discriminator,
                &presence,
            ),
            Case::remapped(
                "native_scalar_owner_import/resolved/*/source_policy",
                "owners/*/source_policy",
                future_owner_import_discriminator,
                &policy,
            ),
            Case::remapped(
                "native_scalar_owner_import/initial_completion/outcomes/*/policy",
                "outcomes/*/policy",
                future_completion_discriminator,
                &policy,
            ),
            Case::remapped(
                "native_scalar_owner_import/initial_completion/outcomes/*/source/type",
                "outcomes/*/source/type",
                future_completion_discriminator,
                &source,
            ),
            Case::remapped(
                "native_scalar_owner_import/initial_completion/outcomes/*/before_presence",
                "outcomes/*/before_presence",
                future_completion_discriminator,
                &presence,
            ),
            Case::remapped(
                "native_scalar_owner_import/initial_completion/outcomes/*/disposition",
                "outcomes/*/disposition",
                future_completion_discriminator,
                &disposition,
            ),
            Case::remapped(
                "native_scalar_owner_import/initial_completion/outcomes/*/component_disposition",
                "outcomes/*/component_disposition",
                future_completion_discriminator,
                &component,
            ),
            Case::shared(
                "preserved_prospect/encoding",
                |value| super::super::future_format(&value["encoding"], crate::preservation::PRESERVED_ENCODING_VERSION),
                &[json!(crate::preservation::PRESERVED_ENCODING_VERSION)],
            ),
        ]
    }

    // Isolate one discriminator so a different future field cannot mask a
    // missing path. Put it after a malformed entry at every array depth.
    fn document(path: &str, value: Value) -> Value {
        path.rsplit('/').fold(value, |value, member| {
            if member == "*" {
                json!([null, value])
            } else {
                json!({ (member): value })
            }
        })
    }

    #[test]
    fn every_streaming_rule_matches_independent_variants_and_constants() {
        let cases = cases();
        let paths: BTreeSet<_> = cases.iter().map(|case| case.path).collect();
        let rules: BTreeSet<_> = RULES.iter().map(|rule| rule.path).collect();
        assert_eq!(paths.len(), cases.len(), "duplicate fixture path");
        assert_eq!(rules.len(), RULES.len(), "duplicate streaming rule");
        assert_eq!(paths, rules, "each rule needs an independent fixture");

        for case in cases {
            let rule = RULES.iter().find(|rule| rule.path == case.path).unwrap();
            match rule.kind {
                Kind::Format(current) => {
                    assert_eq!(case.accepted, [json!(current)], "{}", case.path);
                }
                Kind::Enum(accepted) => {
                    let variants: BTreeSet<_> = case
                        .accepted
                        .iter()
                        .map(|value| value.as_str().unwrap())
                        .collect();
                    assert_eq!(variants.len(), case.accepted.len(), "{}", case.path);
                    assert_eq!(
                        variants,
                        accepted.iter().copied().collect::<BTreeSet<_>>(),
                        "{}",
                        case.path
                    );
                    assert_eq!(accepted.len(), variants.len(), "{}", case.path);
                }
            }
        }
    }

    #[test]
    fn streaming_rules_match_legacy_predicates_for_accepted_future_and_malformed_values() {
        for case in cases() {
            let mut samples: Vec<_> = case
                .accepted
                .iter()
                .cloned()
                .map(|value| (value, false))
                .collect();
            if let Some(current) = case.accepted[0].as_u64() {
                samples.extend([
                    (json!(0), false),
                    (json!(current.saturating_sub(1)), false),
                    (json!(current + 1), true),
                    (json!(u64::MAX), true),
                    (json!("future_format"), false),
                ]);
            } else {
                samples.extend([
                    (json!("future_discriminator_v999"), true),
                    (json!(""), true),
                    (json!(u64::MAX), false),
                ]);
            }
            // Malformed values are not upgrades; typed admission is separate.
            samples.extend(
                [
                    Value::Null,
                    json!(true),
                    json!(-1),
                    json!(1.5),
                    json!([]),
                    json!({}),
                ]
                .map(|value| (value, false)),
            );
            for (value, expected) in samples {
                if let Some(legacy) = &case.legacy {
                    assert_eq!(
                        (legacy.probe)(&document(legacy.path, value.clone())),
                        expected,
                        "legacy {}: {value}",
                        case.path
                    );
                }
                let bytes = serde_json::to_vec(&document(case.path, value.clone())).unwrap();
                let result = scalar_owners(&bytes);
                if expected {
                    let error = result.expect_err(case.path);
                    assert!(
                        error.is::<super::super::UnsupportedSchema>(),
                        "{}: {value}: {error}",
                        case.path
                    );
                } else {
                    assert!(result.is_ok(), "{}: {value}: {result:?}", case.path);
                }
            }
        }
    }
}
