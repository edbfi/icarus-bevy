//! Authored fixtures only. These tests establish no official-save compatibility.
#[path = "common/c001_scalar_owners.rs"]
mod wire;
use icarus_bevy::{
    import_candidate::{CandidateLimits, DecodedImportCandidate},
    native_import::c001_variables::*,
    native_world::{
        Aabb3, CompiledNativeCatalog, MAX_NATIVE_VARIABLES_PER_CLASS, NativeCatalogDefinition,
        NativeClassDefinition, NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement,
        NativeTileDefinition, NativeTransform, NativeValue, VariableDefinition,
    },
};
use wire::{Tag, envelope, stream, string, transform, units};

const C001: &str = "/Script/Icarus.ActorStateRecorderComponent";
const BOOL_NAMES: [&str; 2] = ["PrivateSentinelDoor", "PrivateSentinelLamp"];
const INT_NAMES: [&str; 3] = [
    "PrivateSentinelTemperature",
    "PrivateSentinelDebt",
    "PrivateSentinelAltitude",
];

struct Layout {
    b: [Vec<u16>; 5],
    i: [Vec<u16>; 5],
}
impl Layout {
    fn new() -> Self {
        Self {
            b: [
                "Flags",
                "FlagElement",
                "AuthoredFlagRecord",
                "FlagIdentity",
                "FlagPayload",
            ]
            .map(units),
            i: [
                "Counters",
                "CounterElement",
                "AuthoredCounterRecord",
                "CounterIdentity",
                "CounterPayload",
            ]
            .map(units),
        }
    }
    fn view(&self) -> C001VariableLayoutV1<'_> {
        fn shape(a: &[Vec<u16>; 5]) -> C001VariableArrayLayoutV1<'_> {
            C001VariableArrayLayoutV1 {
                array_name: &a[0],
                element_tag_name: &a[1],
                record_type: &a[2],
                property_name_field: &a[3],
                value_field: &a[4],
            }
        }
        C001VariableLayoutV1 {
            bools: shape(&self.b),
            i32s: shape(&self.i),
        }
    }
}
fn array(name: &'static str, element: &'static str, ty: &str, records: &[Vec<Tag>]) -> Tag {
    Tag::array(
        name,
        "StructProperty",
        [
            i32::try_from(records.len()).unwrap().to_le_bytes().to_vec(),
            Tag::structure(
                element,
                ty,
                records.iter().flat_map(|r| stream(r)).collect(),
            )
            .bytes(),
        ]
        .concat(),
    )
}
fn flag(name: &[u16], value: bool) -> Vec<Tag> {
    // Deliberately value-first; property fields are not positional.
    vec![
        Tag::boolean("FlagPayload", value),
        Tag::text("FlagIdentity", "NameProperty", name),
    ]
}
fn counter(name: &[u16], value: i32) -> Vec<Tag> {
    vec![
        Tag::text("CounterIdentity", "NameProperty", name),
        Tag::integer("CounterPayload", value),
    ]
}
fn flags(records: &[Vec<Tag>]) -> Tag {
    array("Flags", "FlagElement", "AuthoredFlagRecord", records)
}
fn counters(records: &[Vec<Tag>]) -> Tag {
    array(
        "Counters",
        "CounterElement",
        "AuthoredCounterRecord",
        records,
    )
}
fn fields(index: usize, many: bool) -> Vec<Tag> {
    vec![
        Tag::integer("RecordedVersion", 3),
        counters(&if many {
            vec![
                counter(&units(INT_NAMES[0]), i32::MIN),
                counter(&units(INT_NAMES[1]), -1),
                counter(&units(INT_NAMES[2]), i32::MAX),
            ]
        } else {
            vec![]
        }),
        Tag::text(
            "ActorPathName",
            "StrProperty",
            &units(&format!("/PrivateSentinel/Owner{index}")),
        ),
        Tag::text("ObjectFName", "NameProperty", &[0xd800, 70 + index as u16]),
        flags(&if many {
            vec![
                flag(&units(BOOL_NAMES[0]), true),
                flag(&units(BOOL_NAMES[1]), false),
            ]
        } else {
            vec![]
        }),
        transform(&NativeTransform::default()),
        Tag::boolean("bWasMovedToSubLevel", false),
        Tag::integer("IcarusActorGUID", 701 + index as i32),
        Tag::text(
            "ActorClassName",
            "NameProperty",
            &units("PrivateSentinelOwnerClass"),
        ),
    ]
}
fn catalog() -> CompiledNativeCatalog {
    let vars = [
        ("door", NativeValue::Bool(false)),
        ("lamp", NativeValue::Bool(true)),
        ("temperature", NativeValue::I32(4)),
        ("debt", NativeValue::I32(5)),
        ("altitude", NativeValue::I32(6)),
    ]
    .into_iter()
    .map(|(key, default)| VariableDefinition {
        key: key.into(),
        default,
    })
    .collect();
    CompiledNativeCatalog::compile(NativeCatalogDefinition {
        native_format: 1,
        frame: NativeFrame::RightHandedZUpMeters,
        classes: vec![NativeClassDefinition {
            key: "authored".into(),
            kind: NativeClassKind::SpatialObjectV1,
            local_bounds: Aabb3 {
                min: [-0.5; 3],
                max: [0.5; 3],
            },
            variables: vars,
        }],
        maps: vec![NativeMapDefinition {
            key: "map".into(),
            tiles: vec![NativeTileDefinition {
                key: "tile".into(),
                bounds: Aabb3 {
                    min: [-10.0; 3],
                    max: [10.0; 3],
                },
            }],
            placements: ["first", "last"]
                .into_iter()
                .map(|key| NativePlacement {
                    key: key.into(),
                    class: "authored".into(),
                    tile: "tile".into(),
                    transform: NativeTransform::default(),
                    initial_values: vec![],
                })
                .collect(),
        }],
    })
    .unwrap()
}
fn payload(owners: &[Vec<Tag>], version: i32, classes: &[&str]) -> Vec<u8> {
    let records: Vec<u8> = owners
        .iter()
        .zip(classes)
        .flat_map(|(fields, class)| {
            let component = [stream(fields), vec![0; 4]].concat();
            stream(&[
                Tag::text("ComponentClassName", "StrProperty", &units(class)),
                Tag::array(
                    "BinaryData",
                    "ByteProperty",
                    [
                        i32::try_from(component.len())
                            .unwrap()
                            .to_le_bytes()
                            .to_vec(),
                        component,
                    ]
                    .concat(),
                ),
            ])
        })
        .collect();
    stream(&[
        Tag::integer("Version", version),
        Tag::text(
            "ProspectMapName",
            "StrProperty",
            &units("PrivateSentinelMap"),
        ),
        Tag::structure(
            "ProspectInfo",
            "ProspectInfo",
            stream(&[
                Tag::text(
                    "ProspectID",
                    "StrProperty",
                    &units("PrivateSentinelProspect"),
                ),
                Tag::text("LobbyName", "StrProperty", &units("PrivateSentinelLobby")),
            ]),
        ),
        Tag::array(
            "StateRecorderBlobs",
            "StructProperty",
            [
                i32::try_from(owners.len()).unwrap().to_le_bytes().to_vec(),
                Tag::structure("StateRecorderBlobs", "StateRecorderBlob", records).bytes(),
            ]
            .concat(),
        ),
    ])
}
fn candidate(owners: &[Vec<Tag>]) -> DecodedImportCandidate {
    let payload = payload(owners, 2, &vec![C001; owners.len()]);
    DecodedImportCandidate::decode(envelope(&payload).as_slice()).unwrap()
}
struct Identities {
    class: Vec<u16>,
    paths: [Vec<u16>; 2],
    names: [Vec<u16>; 2],
    map: Vec<u16>,
    properties: Vec<Vec<u16>>,
}
impl Identities {
    fn new() -> Self {
        Self {
            class: units("PrivateSentinelOwnerClass"),
            paths: [
                units("/PrivateSentinel/Owner0"),
                units("/PrivateSentinel/Owner1"),
            ],
            names: [vec![0xd800, 70], vec![0xd800, 71]],
            map: units("PrivateSentinelMap"),
            properties: BOOL_NAMES.into_iter().chain(INT_NAMES).map(units).collect(),
        }
    }
    fn properties(&self) -> Vec<C001VariablePropertyBindingV1<'_>> {
        self.properties
            .iter()
            .zip(["door", "lamp", "temperature", "debt", "altitude"])
            .enumerate()
            .map(
                |(i, (source_property, native_variable))| C001VariablePropertyBindingV1 {
                    source_property,
                    native_variable,
                    kind: if i < 2 {
                        C001VariableKind::Bool
                    } else {
                        C001VariableKind::I32
                    },
                    save_game: true,
                },
            )
            .collect()
    }
    fn owner<'a>(
        &'a self,
        index: usize,
        properties: &'a [C001VariablePropertyBindingV1<'a>],
    ) -> C001VariableOwnerBindingV1<'a> {
        C001VariableOwnerBindingV1 {
            source_index: index,
            source: C001VariableOwnerIdentityV1 {
                actor_class: &self.class,
                actor_path: &self.paths[index],
                object_name: &self.names[index],
                guid: 701 + index as i32,
            },
            native_placement: ["first", "last"][index],
            native_class: "authored",
            properties,
        }
    }
    fn bindings<'a>(
        &'a self,
        c: &DecodedImportCandidate,
        catalog: &'a CompiledNativeCatalog,
        owners: &'a [C001VariableOwnerBindingV1<'a>],
    ) -> C001VariableBindingsV1<'a> {
        C001VariableBindingsV1 {
            expected_payload_sha256: c.preserved().payload_sha256(),
            native_catalog_id: catalog.identity(),
            source_map: &self.map,
            native_map: "map",
            owners,
        }
    }
}
fn replace(fields: &mut [Tag], name: &str, tag: Tag) {
    *fields.iter_mut().find(|p| p.name == name).unwrap() = tag;
}

fn forced_wide_ascii(text: &str) -> Vec<u8> {
    [
        (-i32::try_from(text.len() + 1).unwrap())
            .to_le_bytes()
            .to_vec(),
        text.encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect(),
    ]
    .concat()
}

#[test]
fn empty_arrays_and_empty_candidate_remain_assessment_only() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    for count in [0, 1, 2] {
        let c = candidate(&(0..count).map(|i| fields(i, false)).collect::<Vec<_>>());
        let owners: Vec<_> = (0..count).map(|i| ids.owner(i, &[])).collect();
        let p = prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default(),
        )
        .unwrap();
        assert!(p.assignments().is_empty());
        assert_eq!(p.inspection().recorders, count);
        assert_eq!(p.inspection().unresolved_callbacks, 0);
        assert!(!p.activation_ready());
        assert!(!p.inspection().world_imported);
        assert_eq!(p.bindings().owners.len(), count);
    }
}

#[test]
fn many_arbitrary_properties_restore_full_signed_range_and_borrow_provenance() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let c = candidate(&[fields(0, true), fields(1, true)]);
    let before = c.payload().to_vec();
    let catalog_before = serde_json::to_vec(catalog.definition()).unwrap();
    let properties = ids.properties();
    // Binding order and recorder declaration order carry no semantics.
    let mut reversed = properties.clone();
    reversed.reverse();
    let owners = [ids.owner(1, &reversed), ids.owner(0, &properties)];
    let p = prepare_c001_variables(
        &c,
        &catalog,
        layout.view(),
        ids.bindings(&c, &catalog, &owners),
        C001VariableLimits::default(),
    )
    .unwrap();
    assert_eq!(p.inspection().assignments, 10);
    assert_eq!(p.inspection().bool_assignments, 4);
    assert_eq!(p.inspection().i32_assignments, 6);
    assert_eq!(p.inspection().unresolved_callbacks, 10);
    let (owner_assignments, remainder) = p.assignments().as_chunks::<5>();
    assert!(remainder.is_empty());
    for assignments in owner_assignments {
        assert_eq!(
            assignments.iter().map(|a| a.value()).collect::<Vec<_>>(),
            vec![
                C001VariableValue::Bool(true),
                C001VariableValue::Bool(false),
                C001VariableValue::I32(i32::MIN),
                C001VariableValue::I32(-1),
                C001VariableValue::I32(i32::MAX),
            ]
        );
        for a in assignments {
            assert_eq!(
                a.callback_blocker(),
                C001VariableCallbackBlocker::UnqualifiedPropertyRestoreAndRepNotify
            );
            let (component, _) = c.recorders()[a.source_index()].typing().lossless().unwrap();
            let array = component
                .tree
                .properties
                .iter()
                .find(|v| v.name == a.source_array())
                .unwrap();
            let icarus_bevy::experimental::typed_value::Value::StructArray { elements, .. } =
                &array.value
            else {
                panic!()
            };
            assert!(
                elements[a.source_element()]
                    .properties
                    .iter()
                    .any(|v| v.header.as_ptr() == a.source_name_header().as_ptr())
            );
            assert!(
                elements[a.source_element()]
                    .properties
                    .iter()
                    .any(|v| v.header.as_ptr() == a.source_value_header().as_ptr())
            );
        }
    }
    assert!(std::ptr::eq(p.candidate(), &c));
    assert!(std::ptr::eq(p.catalog(), &catalog));
    assert_eq!(c.payload(), before);
    assert_eq!(
        serde_json::to_vec(catalog.definition()).unwrap(),
        catalog_before
    );
    assert_eq!(
        c.preserved().decode().unwrap().to_inner_payload().unwrap(),
        before
    );
    assert!(!p.activation_ready());
}

#[test]
fn exact_owner_property_and_native_type_bindings_cannot_be_substituted() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let c = candidate(&[fields(0, true)]);
    let wrong = units("PrivateSentinelDifferent");
    let normalized = [0xfffd, 70];
    for variant in 0..16 {
        let mut properties = ids.properties();
        let mut owner = ids.owner(0, &[]);
        match variant {
            0 => owner.source.actor_class = &wrong,
            1 => owner.source.actor_path = &wrong,
            2 => owner.source.object_name = &normalized,
            3 => owner.source.guid += 1,
            4 => owner.native_class = "missing",
            5 => owner.native_placement = "missing",
            6 => properties[0].source_property = &wrong,
            7 => properties[0].native_variable = "missing",
            8 => properties[0].native_variable = "debt",
            9 => properties[2].native_variable = "door",
            10 => properties[0].save_game = false,
            11 => properties[0].kind = C001VariableKind::I32,
            12 => properties[1].source_property = properties[0].source_property,
            13 => properties[1].native_variable = properties[0].native_variable,
            14 => {
                properties.pop();
            }
            15 => properties.push(properties[0]),
            _ => unreachable!(),
        }
        owner.properties = &properties;
        let owners = [owner];
        assert!(
            prepare_c001_variables(
                &c,
                &catalog,
                layout.view(),
                ids.bindings(&c, &catalog, &owners),
                C001VariableLimits::default()
            )
            .is_err(),
            "variant {variant}"
        );
    }
}

#[test]
fn duplicate_owner_bindings_paths_guids_and_destinations_reject() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let properties = ids.properties();
    for many in [false, true] {
        let properties = if many { properties.as_slice() } else { &[] };
        for variant in 0..5 {
            let mut last = fields(1, many);
            let mut owners = [ids.owner(0, properties), ids.owner(1, properties)];
            match variant {
                0 => owners[1].source_index = 0,
                1 => {
                    owners[1].source.actor_path = owners[0].source.actor_path;
                    replace(
                        &mut last,
                        "ActorPathName",
                        Tag::text("ActorPathName", "StrProperty", &ids.paths[0]),
                    );
                }
                2 => {
                    owners[1].source.guid = owners[0].source.guid;
                    replace(
                        &mut last,
                        "IcarusActorGUID",
                        Tag::integer("IcarusActorGUID", owners[0].source.guid),
                    );
                }
                3 => owners[1].native_placement = owners[0].native_placement,
                4 => owners[1].source_index = 2,
                _ => unreachable!(),
            }
            // Path/GUID mutations match the actual decoded terminal owner.
            // Only the cross-owner uniqueness check can reject those variants.
            let c = candidate(&[fields(0, many), last]);
            let err = prepare_c001_variables(
                &c,
                &catalog,
                layout.view(),
                ids.bindings(&c, &catalog, &owners),
                C001VariableLimits::default(),
            )
            .unwrap_err();
            assert_eq!(
                err.kind,
                C001VariableErrorKind::Binding,
                "variant {variant}"
            );
            assert_eq!(err.source_index, Some(1), "variant {variant}");
        }
    }
}

#[test]
fn combined_bool_i32_property_limit_accepts_64_and_rejects_65() {
    let (layout, ids) = (Layout::new(), Identities::new());
    assert_eq!(MAX_NATIVE_VARIABLES_PER_CLASS, 64);
    let source_names: Vec<_> = (0..65)
        .map(|i| units(&format!("AuthoredProperty{i}")))
        .collect();
    let native_names: Vec<_> = (0..64).map(|i| format!("value{i}")).collect();
    let mut definition = catalog().definition().clone();
    definition.classes[0].variables = native_names
        .iter()
        .enumerate()
        .map(|(i, key)| VariableDefinition {
            key: key.clone(),
            default: if i < 32 {
                NativeValue::Bool(false)
            } else {
                NativeValue::I32(0)
            },
        })
        .collect();
    let catalog = CompiledNativeCatalog::compile(definition).unwrap();
    let properties: Vec<_> = source_names[..64]
        .iter()
        .zip(&native_names)
        .enumerate()
        .map(
            |(i, (source_property, native_variable))| C001VariablePropertyBindingV1 {
                source_property,
                native_variable,
                kind: if i < 32 {
                    C001VariableKind::Bool
                } else {
                    C001VariableKind::I32
                },
                save_game: true,
            },
        )
        .collect();
    let owners = [ids.owner(0, &properties)];
    for count in [64, 65] {
        let mut source = fields(0, false);
        let bools: Vec<_> = source_names[..32].iter().map(|n| flag(n, true)).collect();
        let ints: Vec<_> = source_names[32..count]
            .iter()
            .map(|n| counter(n, -7))
            .collect();
        // Each array is independently below the per-class ceiling.
        assert!(bools.len() < MAX_NATIVE_VARIABLES_PER_CLASS);
        assert!(ints.len() < MAX_NATIVE_VARIABLES_PER_CLASS);
        replace(&mut source, "Flags", flags(&bools));
        replace(&mut source, "Counters", counters(&ints));
        let c = candidate(&[source]);
        let result = prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default(),
        );
        if count == 64 {
            let p = result.unwrap();
            assert_eq!(p.inspection().assignments, 64);
            assert_eq!(p.inspection().bool_assignments, 32);
            assert_eq!(p.inspection().i32_assignments, 32);
            assert_eq!(p.inspection().unresolved_callbacks, 64);
            assert!(
                p.assignments()[..32]
                    .iter()
                    .all(|a| a.value() == C001VariableValue::Bool(true))
            );
            assert!(
                p.assignments()[32..]
                    .iter()
                    .all(|a| a.value() == C001VariableValue::I32(-7))
            );
            assert!(!p.activation_ready());
        } else {
            let err = result.unwrap_err();
            assert_eq!(err.kind, C001VariableErrorKind::Limit);
            assert_eq!(err.source_index, Some(0));
        }
    }
}

#[test]
fn malformed_arrays_headers_and_last_records_never_return_partial_plans() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    for variant in 0..23 {
        let mut records = vec![
            flag(&units(BOOL_NAMES[0]), true),
            flag(&units(BOOL_NAMES[1]), false),
        ];
        let mut last = fields(1, true);
        match variant {
            0 => records[1] = flag(&units(BOOL_NAMES[0]), false),
            1 => records[1].push(Tag::integer("PrivateSentinelExtra", 99)),
            2 => records[1][1] = Tag::text("FlagIdentity", "StrProperty", &units(BOOL_NAMES[1])),
            3 => records[1][0] = Tag::integer("FlagPayload", 1),
            4 => records[1][1].index = 1,
            5 => records[1][1].property_guid(),
            6 => records[1][0] = Tag::boolean("WrongValueName", true),
            7 => records[1][1] = records[1][0].clone(),
            _ => {}
        }
        let mut a = flags(&records);
        match variant {
            8 => a = array("Flags", "WrongElement", "AuthoredFlagRecord", &records),
            9 => a = array("Flags", "FlagElement", "WrongRecord", &records),
            10 => a.index = 1,
            11 => a.property_guid(),
            12 => a.payload[..4].copy_from_slice(&(-1i32).to_le_bytes()),
            13 => a.payload[..4].copy_from_slice(&3i32.to_le_bytes()),
            14 => a.payload.push(0),
            15 => {
                let mut shared = Tag::structure(
                    "FlagElement",
                    "AuthoredFlagRecord",
                    records.iter().flat_map(|r| stream(r)).collect(),
                );
                shared.struct_guid();
                a.payload = [2i32.to_le_bytes().to_vec(), shared.bytes()].concat();
            }
            16 => records[1][0].payload.push(1),
            20 => a.extras = [forced_wide_ascii("StructProperty"), vec![0]].concat(),
            21 => records[1][1].payload = forced_wide_ascii(BOOL_NAMES[1]),
            22 => {
                let mut shared = Tag::structure(
                    "FlagElement",
                    "AuthoredFlagRecord",
                    records.iter().flat_map(|r| stream(r)).collect(),
                );
                shared.index = 1;
                a.payload = [2i32.to_le_bytes().to_vec(), shared.bytes()].concat();
            }
            _ => {}
        }
        if variant == 16 || variant == 21 {
            a = flags(&records);
        }
        replace(&mut last, "Flags", a);
        match variant {
            17 => last.push(Tag::integer(
                "PrivateSentinelUnsupportedTerminal",
                123456789,
            )),
            18 => last.push(Tag::integer("RecordedVersion", 3)),
            19 => {
                last.iter_mut()
                    .find(|p| p.name == "ActorClassName")
                    .unwrap()
                    .index = 1
            }
            _ => {}
        }
        let raw = payload(&[fields(0, true), last], 2, &[C001, C001]);
        // Some malformed inputs are rejected by the candidate itself. Both
        // boundaries must fail closed; a successfully decoded candidate must
        // still be refused by preparation, including its otherwise valid prefix.
        if let Ok(c) = DecodedImportCandidate::decode(envelope(&raw).as_slice()) {
            let properties = ids.properties();
            let owners = [ids.owner(0, &properties), ids.owner(1, &properties)];
            let err = prepare_c001_variables(
                &c,
                &catalog,
                layout.view(),
                ids.bindings(&c, &catalog, &owners),
                C001VariableLimits::default(),
            )
            .unwrap_err();
            assert_eq!(err.source_index, Some(1), "variant {variant}");
        }
    }
}

#[test]
fn decoded_valid_prefix_with_invalid_terminal_recorder_is_rejected_atomically() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let properties = ids.properties();
    let prefix = candidate(&[fields(0, true)]);
    let prefix_owners = [ids.owner(0, &properties)];
    assert_eq!(
        prepare_c001_variables(
            &prefix,
            &catalog,
            layout.view(),
            ids.bindings(&prefix, &catalog, &prefix_owners),
            C001VariableLimits::default(),
        )
        .unwrap()
        .inspection()
        .assignments,
        5
    );
    let owners = [ids.owner(0, &properties), ids.owner(1, &properties)];
    for variant in 0..5 {
        let mut last = fields(1, true);
        let mut classes = [C001, C001];
        let expected = match variant {
            0 => {
                replace(
                    &mut last,
                    "Flags",
                    flags(&[
                        flag(&units(BOOL_NAMES[0]), true),
                        flag(&units(BOOL_NAMES[0]), false),
                    ]),
                );
                C001VariableErrorKind::Binding
            }
            1 => {
                replace(
                    &mut last,
                    "Flags",
                    array(
                        "Flags",
                        "WrongElement",
                        "AuthoredFlagRecord",
                        &[
                            flag(&units(BOOL_NAMES[0]), true),
                            flag(&units(BOOL_NAMES[1]), false),
                        ],
                    ),
                );
                C001VariableErrorKind::Type
            }
            2 => {
                last.push(Tag::integer("PrivateSentinelUnsupportedTerminal", 7));
                C001VariableErrorKind::Unsupported
            }
            3 => {
                classes[1] = "/Script/Icarus.NoRecorderComponent";
                C001VariableErrorKind::WrongRecorderKind
            }
            4 => {
                last.push(Tag::boolean("bSkipDefaultOwnerResolution", true));
                C001VariableErrorKind::Unsupported
            }
            _ => unreachable!(),
        };
        let raw = payload(&[fields(0, true), last], 2, &classes);
        // Every variant must reach this planner with a retained, losslessly
        // typed valid prefix; candidate rejection cannot make the test pass.
        let c = DecodedImportCandidate::decode(envelope(&raw).as_slice()).unwrap();
        assert!(c.recorders()[0].decision().retained);
        assert!(c.recorders()[0].typing().lossless().is_some());
        let err = prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default(),
        )
        .unwrap_err();
        assert_eq!(err.kind, expected, "variant {variant}");
        assert_eq!(err.source_index, Some(1), "variant {variant}");
        assert_eq!(c.payload(), raw);
    }
}

#[test]
fn empty_array_still_requires_exact_shared_type_and_layout_names_are_independent() {
    let (mut layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let mut owner = fields(0, false);
    replace(
        &mut owner,
        "Flags",
        array("Flags", "FlagElement", "WrongEmptyType", &[]),
    );
    let c = candidate(&[owner]);
    let owners = [ids.owner(0, &[])];
    assert!(
        prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .is_err()
    );
    let c = candidate(&[fields(0, true)]);
    let properties = ids.properties();
    let owners = [ids.owner(0, &properties)];
    layout.b[3] = units("VariableName");
    assert!(
        prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .is_err()
    );
    layout.b[3] = layout.b[4].clone();
    assert_eq!(
        prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::Layout
    );
}

#[test]
fn dropped_and_wrong_kind_recorders_have_explicit_errors_and_gaps_are_not_unavailable() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let properties = ids.properties();
    let owners = [ids.owner(0, &properties)];
    let raw = payload(&[fields(0, true)], 0, &[C001]);
    let c = DecodedImportCandidate::decode(envelope(&raw).as_slice()).unwrap();
    assert!(!c.recorders()[0].decision().retained);
    assert_eq!(
        prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::DroppedRecorder
    );

    let mut unavailable = fields(0, true);
    let mut nested = Tag::integer("Leaf", 17);
    for _ in 0..70 {
        nested = Tag::structure("Nested", "AuthoredNested", stream(&[nested]));
    }
    unavailable.push(nested);
    let c = candidate(&[unavailable]);
    let (_, summary) = c.recorders()[0].typing().lossless().unwrap();
    assert!(summary.uninterpreted > 0);
    assert_eq!(
        prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::Unsupported
    );

    // Version zero permits an unknown class to stage as dropped. Exact class
    // identity is checked first, rather than trusting a CRC or dropped status.
    let raw = payload(&[fields(0, true)], 0, &["/Script/Authored.WrongRecorder"]);
    let c = DecodedImportCandidate::decode(envelope(&raw).as_slice()).unwrap();
    assert_eq!(
        prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::WrongRecorderKind
    );
}

#[test]
fn allocations_obey_requested_candidate_and_native_ceilings() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let c = candidate(&[fields(0, true)]);
    let properties = ids.properties();
    let owners = [ids.owner(0, &properties)];
    let bindings = ids.bindings(&c, &catalog, &owners);
    let required = prepare_c001_variables(
        &c,
        &catalog,
        layout.view(),
        bindings,
        C001VariableLimits::default(),
    )
    .unwrap()
    .inspection()
    .additional_owned_bytes;
    for limits in [
        C001VariableLimits {
            assignments: 4,
            ..C001VariableLimits::default()
        },
        C001VariableLimits {
            owned_bytes: required - 1,
            ..C001VariableLimits::default()
        },
        C001VariableLimits {
            owned_bytes: usize::MAX,
            ..C001VariableLimits::default()
        },
        C001VariableLimits {
            assignments: usize::MAX,
            ..C001VariableLimits::default()
        },
    ] {
        assert_eq!(
            prepare_c001_variables(&c, &catalog, layout.view(), bindings, limits)
                .unwrap_err()
                .kind,
            C001VariableErrorKind::Limit
        );
    }
    assert!(
        prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            bindings,
            C001VariableLimits {
                assignments: 5,
                owned_bytes: required
            }
        )
        .is_ok()
    );
    let raw = envelope(c.payload());
    let required_decode = c.required_decode_limits();
    let tight = DecodedImportCandidate::decode_with_limits(
        raw.as_slice(),
        CandidateLimits {
            typed_elements: required_decode.typed_elements,
            owned_bytes: required_decode.owned_bytes,
        },
    )
    .unwrap();
    assert_eq!(
        tight
            .inspection()
            .allocation
            .remaining_headroom
            .typed_elements,
        0
    );
    assert_eq!(
        prepare_c001_variables(
            &tight,
            &catalog,
            layout.view(),
            ids.bindings(&tight, &catalog, &owners),
            C001VariableLimits::default()
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::Limit
    );

    let over = vec![flag(&units(BOOL_NAMES[0]), true); MAX_NATIVE_VARIABLES_PER_CLASS + 1];
    let mut owner = fields(0, true);
    replace(&mut owner, "Flags", flags(&over));
    let c = candidate(&[owner]);
    assert_eq!(
        prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::Limit
    );
}

#[test]
fn indexed_owners_preserve_source_order_and_byte_headroom_is_independent_of_elements() {
    let (layout, ids) = (Layout::new(), Identities::new());
    let count = 256;
    let properties = ids.properties();
    let paths: Vec<_> = (0..count)
        .map(|i| units(&format!("/PrivateSentinel/Owner{i}")))
        .collect();
    let names: Vec<_> = (0..count).map(|i| vec![0xd800, 70 + i as u16]).collect();
    let placements: Vec<_> = (0..count).map(|i| format!("slot{i}")).collect();
    let mut definition = catalog().definition().clone();
    let template = definition.maps[0].placements[0].clone();
    definition.maps[0].placements = placements
        .iter()
        .map(|key| {
            let mut p = template.clone();
            p.key = key.clone();
            p
        })
        .collect();
    let catalog = CompiledNativeCatalog::compile(definition).unwrap();
    let mut owners: Vec<_> = (0..count)
        .map(|i| {
            let mut b = ids.owner(0, &properties);
            b.source_index = i;
            b.source.actor_path = &paths[i];
            b.source.object_name = &names[i];
            b.source.guid = 701 + i as i32;
            b.native_placement = &placements[i];
            b
        })
        .collect();
    owners.reverse();
    let c = candidate(&(0..count).map(|i| fields(i, true)).collect::<Vec<_>>());
    let p = prepare_c001_variables(
        &c,
        &catalog,
        layout.view(),
        ids.bindings(&c, &catalog, &owners),
        C001VariableLimits::default(),
    )
    .unwrap();
    let required = p.inspection().additional_owned_bytes;
    assert_eq!(p.inspection().assignments, count * 5);
    let (owner_assignments, remainder) = p.assignments().as_chunks::<5>();
    assert!(remainder.is_empty());
    for (i, assignments) in owner_assignments.iter().enumerate() {
        for a in assignments {
            assert_eq!(a.source_index(), i);
            assert_eq!(a.owner().native_placement, placements[i]);
            assert_eq!(a.owner().source.actor_path, paths[i]);
        }
        assert_eq!(assignments[2].value(), C001VariableValue::I32(i32::MIN));
    }
    let raw = envelope(c.payload());
    let tight = DecodedImportCandidate::decode_with_limits(
        raw.as_slice(),
        CandidateLimits {
            owned_bytes: c.required_decode_limits().owned_bytes,
            ..CandidateLimits::default()
        },
    )
    .unwrap();
    let headroom = tight.inspection().allocation.remaining_headroom;
    assert!(headroom.owned_bytes < required);
    assert!(headroom.typed_elements >= count * 5);
    assert_eq!(
        prepare_c001_variables(
            &tight,
            &catalog,
            layout.view(),
            ids.bindings(&tight, &catalog, &owners),
            C001VariableLimits::default(),
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::Limit
    );
}

#[test]
fn empty_assignment_owner_lookup_still_requires_a_byte_budget() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let c = candidate(&[fields(0, false), fields(1, false)]);
    let owners = [ids.owner(1, &[]), ids.owner(0, &[])];
    let bindings = ids.bindings(&c, &catalog, &owners);
    let required = prepare_c001_variables(
        &c,
        &catalog,
        layout.view(),
        bindings,
        C001VariableLimits::default(),
    )
    .unwrap()
    .inspection()
    .additional_owned_bytes;
    for (bytes, accepted) in [(required - 1, false), (required, true)] {
        let result = prepare_c001_variables(
            &c,
            &catalog,
            layout.view(),
            bindings,
            C001VariableLimits {
                assignments: 0,
                owned_bytes: bytes,
            },
        );
        if accepted {
            let p = result.unwrap();
            assert!(p.assignments().is_empty());
            assert!(!p.activation_ready());
        } else {
            assert_eq!(result.unwrap_err().kind, C001VariableErrorKind::Limit);
        }
    }
}

#[test]
fn redaction_is_transitive_across_debug_errors_and_serialization() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let c = candidate(&[fields(0, true)]);
    let properties = ids.properties();
    let owners = [ids.owner(0, &properties)];
    let bindings = ids.bindings(&c, &catalog, &owners);
    let p = prepare_c001_variables(
        &c,
        &catalog,
        layout.view(),
        bindings,
        C001VariableLimits::default(),
    )
    .unwrap();
    let mut output = format!(
        "{p:?} {bindings:?} {:?} {:?} {:?}",
        layout.view(),
        owners[0],
        properties[0]
    );
    output.push_str(&serde_json::to_string(&p).unwrap());
    output.push_str(&serde_json::to_string(&bindings).unwrap());
    for a in p.assignments() {
        output.push_str(&format!("{a:?} {:?} {:?}", a.value(), a.owner()));
        output.push_str(&serde_json::to_string(a).unwrap());
        output.push_str(&serde_json::to_string(&a.value()).unwrap());
    }
    let err = prepare_c001_variables(
        &c,
        &catalog,
        layout.view(),
        bindings,
        C001VariableLimits {
            assignments: 0,
            ..C001VariableLimits::default()
        },
    )
    .unwrap_err();
    output.push_str(&format!("{err} {err:?}"));
    output.push_str(&serde_json::to_string(&err).unwrap());
    for secret in [
        "PrivateSentinel",
        "-2147483648",
        "2147483647",
        "FlagPayload",
        "CounterIdentity",
        "/Script/",
    ] {
        assert!(!output.contains(secret), "diagnostic leaked {secret}");
    }
}

#[test]
fn payload_catalog_map_and_layout_identity_mismatches_reject_before_output() {
    let (layout, ids, catalog) = (Layout::new(), Identities::new(), catalog());
    let c = candidate(&[fields(0, true)]);
    let properties = ids.properties();
    let owners = [ids.owner(0, &properties)];
    let wrong = units("PrivateSentinelWrongMap");
    for variant in 0..5 {
        let mut b = ids.bindings(&c, &catalog, &owners);
        match variant {
            0 => b.expected_payload_sha256[0] ^= 1,
            1 => b.native_catalog_id = "wrong",
            2 => b.source_map = &wrong,
            3 => b.native_map = "missing",
            4 => b.native_map = "MAP",
            _ => unreachable!(),
        }
        assert_eq!(
            prepare_c001_variables(
                &c,
                &catalog,
                layout.view(),
                b,
                C001VariableLimits::default()
            )
            .unwrap_err()
            .kind,
            C001VariableErrorKind::Binding
        );
    }
    let mut l = layout.view();
    l.i32s.array_name = l.bools.array_name;
    assert_eq!(
        prepare_c001_variables(
            &c,
            &catalog,
            l,
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::Layout
    );
    // Do not admit arbitrary huge authored name slices as header declarations.
    let huge = vec![65; 1024];
    let mut l = layout.view();
    l.bools.record_type = &huge;
    assert_eq!(
        prepare_c001_variables(
            &c,
            &catalog,
            l,
            ids.bindings(&c, &catalog, &owners),
            C001VariableLimits::default()
        )
        .unwrap_err()
        .kind,
        C001VariableErrorKind::Layout
    );
    assert_eq!(string("None"), b"\x05\0\0\0None\0");
}
