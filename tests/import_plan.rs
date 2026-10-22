#[path = "pak_writer/mod.rs"]
mod pak_writer;

use std::{fs, io::Write, process::Command};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::experimental::recorders::{lookup_recorder_ctor_defaults, recorder_class_label};
use icarus_bevy::import_plan::{
    ContentCatalog, ContentRole, ImportBlocker, RoleCount, plan_import, planned_actors,
};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tempfile::TempDir;

const ACTOR: &str = "/Script/Icarus.ActorStateRecorderComponent";
const NO_RECORDER: &str = "/Script/Icarus.NoRecorderComponent";
const ROW: &str = "SecretRow_7";
const PATH: &str = "/Game/Secret/Path.Secret_C";

// Authored framing, independent of the readers and the archive encoder.
fn fstring(value: &str) -> Vec<u8> {
    if value.is_empty() {
        return 0_i32.to_le_bytes().to_vec();
    }
    [
        i32::try_from(value.len() + 1)
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        value.as_bytes(),
        &[0],
    ]
    .concat()
}

fn tag(name: &str, kind: &str, extras: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        fstring(name).as_slice(),
        &fstring(kind),
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &0_i32.to_le_bytes(),
        extras,
        payload,
    ]
    .concat()
}

fn struct_tag(name: &str, type_name: &str, payload: &[u8]) -> Vec<u8> {
    tag(
        name,
        "StructProperty",
        &[fstring(type_name), vec![0; 17]].concat(),
        payload,
    )
}

fn end() -> Vec<u8> {
    fstring("None")
}

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

struct Body {
    translation: [f32; 3],
    gap: bool,
    /// `OwnerResolvePolicy` as a raw ByteProperty value instead of an enum name.
    owner_byte: bool,
    /// `ActorClassName` under an unexpected wire type (IntProperty).
    class_as_int: bool,
    /// Add the fields whose roles static reads established, plus a loose
    /// `Stat` outside its record.
    read_fields: bool,
}

fn transform_children(body: &Body) -> Vec<Vec<u8>> {
    vec![
        struct_tag("Rotation", "Quat", &floats(&[0.0, 0.0, 0.0, 1.0])),
        struct_tag("Translation", "Vector", &floats(&body.translation)),
        struct_tag("Scale3D", "Vector", &floats(&[1.0, 1.0, 1.0])),
    ]
}

fn transform_tag(type_name: &str, mut children: Vec<Vec<u8>>) -> Vec<u8> {
    children.push(end());
    struct_tag("ActorTransform", type_name, &children.concat())
}

fn component(body: &Body, transform: Option<Vec<u8>>) -> Vec<u8> {
    let owner = if body.owner_byte {
        tag(
            "OwnerResolvePolicy",
            "ByteProperty",
            &[fstring("None"), vec![0]].concat(),
            &[2],
        )
    } else {
        tag(
            "OwnerResolvePolicy",
            "EnumProperty",
            &[fstring("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
            &fstring("EStateRecorderOwnerResolvePolicy::FindOrRespawn"),
        )
    };
    let class = if body.class_as_int {
        tag("ActorClassName", "IntProperty", &[0], &5_i32.to_le_bytes())
    } else {
        tag("ActorClassName", "NameProperty", &[0], &fstring(ROW))
    };
    let mut fields = vec![
        owner,
        class,
        tag("ActorPathName", "StrProperty", &[0], &fstring(PATH)),
        tag("bWasMovedToSubLevel", "BoolProperty", &[0, 0], &[]),
        tag("Label", "StrProperty", &[0], &fstring("secret plain text")),
        tag("Nothing", "NameProperty", &[0], &fstring("None")),
        tag("ItemStaticData", "NameProperty", &[0], &fstring(ROW)),
        tag("TileName", "NameProperty", &[0], &fstring("SecretTile")),
        tag(
            "ObjectFName",
            "NameProperty",
            &[0],
            &fstring("SecretObject_C_7"),
        ),
        // A class variable record and a quest variable record: only the
        // first names a class property.
        struct_tag(
            "BoolVariables",
            "ActorBoolVariableRecord",
            &[
                tag(
                    "VariableName",
                    "NameProperty",
                    &[0],
                    &fstring("bSecretFlag"),
                ),
                end(),
            ]
            .concat(),
        ),
        struct_tag(
            "QuestVariables",
            "QuestVariableRecord",
            &[
                tag(
                    "VariableName",
                    "NameProperty",
                    &[0],
                    &fstring("SecretQuestVar"),
                ),
                end(),
            ]
            .concat(),
        ),
    ];
    if let Some(transform) = transform {
        fields.insert(4, transform);
    }
    if body.gap {
        fields.push(struct_tag("Mystery", "SecretBlobType", &[1, 2, 3]));
    }
    if body.read_fields {
        let name = |property, value| tag(property, "NameProperty", &[0], &fstring(value));
        let bosses = [
            2_i32.to_le_bytes().to_vec(),
            fstring("SecretBossA"),
            fstring("SecretBossB"),
        ];
        fields.extend([
            struct_tag(
                "Stats",
                "ItemRecordStatData",
                &[name("Stat", "SecretStat"), end()].concat(),
            ),
            name("Stat", "SecretLooseStat"),
            struct_tag(
                "Inventories",
                "SavedInventoryContainerData",
                &[name("InventoryInfo", "SecretInventory"), end()].concat(),
            ),
            tag(
                "RecordedWorldBossTableNames",
                "ArrayProperty",
                &[fstring("NameProperty"), vec![0]].concat(),
                &bosses.concat(),
            ),
            name("SignIconRecord", "SecretIcon"),
            name("TreePrefabClassName", "SecretTree_C"),
            struct_tag(
                "Crops",
                "CultivationSaveData",
                &[name("Seed", "SecretSeed"), end()].concat(),
            ),
            name("QuestName", "SecretQuest"),
            struct_tag(
                "Record",
                "GameModeRecord",
                &[
                    tag(
                        "SessionFlagRecords",
                        "ArrayProperty",
                        &[fstring("NameProperty"), vec![0]].concat(),
                        &[1_i32.to_le_bytes().to_vec(), fstring("SecretFlag")].concat(),
                    ),
                    end(),
                ]
                .concat(),
            ),
            struct_tag(
                "Tile",
                "FLODTileRecorderRecord",
                &[name("TileName", "SecretWorldTile"), end()].concat(),
            ),
            // Top-level fields nested in a named struct or a map item
            // struct stay unclassified.
            struct_tag(
                "Holder",
                "SecretHolder",
                &[name("SignIconRecord", "SecretNestedIcon"), end()].concat(),
            ),
            tag(
                "Items",
                "MapProperty",
                &[fstring("NameProperty"), fstring("StructProperty"), vec![0]].concat(),
                &[
                    0_i32.to_le_bytes().to_vec(),
                    1_i32.to_le_bytes().to_vec(),
                    fstring("SecretKey"),
                    name("TreePrefabClassName", "SecretItemTree_C"),
                    end(),
                ]
                .concat(),
            ),
        ]);
    }
    fields.push(end());
    [fields.concat(), vec![0; 4]].concat()
}

fn recorder(class: &str, body: &Body) -> Vec<u8> {
    recorder_with_transform(
        class,
        body,
        Some(transform_tag("Transform", transform_children(body))),
    )
}

fn recorder_with_transform(class: &str, body: &Body, transform: Option<Vec<u8>>) -> Vec<u8> {
    let bytes = component(body, transform);
    recorder_bytes(class, &bytes)
}

fn recorder_bytes(class: &str, bytes: &[u8]) -> Vec<u8> {
    let count = i32::try_from(bytes.len()).unwrap().to_le_bytes();
    [
        tag("ComponentClassName", "StrProperty", &[0], &fstring(class)),
        tag(
            "BinaryData",
            "ArrayProperty",
            &[fstring("ByteProperty"), vec![0]].concat(),
            &[count.as_slice(), bytes].concat(),
        ),
        end(),
    ]
    .concat()
}

fn envelope(version: i32, recorders: &[Vec<u8>]) -> Vec<u8> {
    envelope_with_prospect_keys(version, recorders, None, None)
}

fn envelope_with_prospect_keys(
    version: i32,
    recorders: &[Vec<u8>],
    nested: Option<&str>,
    legacy: Option<&str>,
) -> Vec<u8> {
    let mut info = vec![tag(
        "ProspectID",
        "StrProperty",
        &[0],
        &fstring("secret-prospect"),
    )];
    if let Some(key) = nested {
        info.push(tag("ProspectDTKey", "StrProperty", &[0], &fstring(key)));
    }
    info.push(end());
    let info = info.concat();
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &[fstring("StateRecorderBlob"), vec![0; 17]].concat(),
        &recorders.concat(),
    );
    let count = i32::try_from(recorders.len()).unwrap().to_le_bytes();
    let mut roots = vec![
        tag("Version", "IntProperty", &[0], &version.to_le_bytes()),
        struct_tag("ProspectInfo", "ProspectInfo", &info),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &[fstring("StructProperty"), vec![0]].concat(),
            &[count.as_slice(), &shared].concat(),
        ),
    ];
    if let Some(key) = legacy {
        roots.push(tag("ProspectDTKey", "StrProperty", &[0], &fstring(key)));
    }
    roots.push(end());
    let payload = roots.concat();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&payload).unwrap();
    let compressed = encoder.finish().unwrap();
    let hash = Sha1::digest(&payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    serde_json::to_vec(&json!({"ProspectBlob": {
        "Hash": hash, "TotalLength": compressed.len(), "DataLength": 0,
        "UncompressedLength": payload.len(), "BinaryBlob": STANDARD.encode(compressed)
    }}))
    .unwrap()
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

const CLEAN: Body = Body {
    translation: [10.0, -20.0, 30.0],
    gap: false,
    owner_byte: false,
    class_as_int: false,
    read_fields: false,
};

#[test]
fn fixture_classes_are_a_registered_actor_recorder_and_an_always_invalid_one() {
    let actor = lookup_recorder_ctor_defaults(&units(ACTOR)).unwrap();
    assert!(actor.is_actor_recorder);
    assert_eq!(actor.b_record_actor_transform, Some(true));
    assert!(recorder_class_label(&units(NO_RECORDER)).is_some());
}

#[test]
fn empty_catalog_blocks_on_unmapped_content_and_reports_counts_only() {
    let source = envelope(
        2,
        &[
            recorder(ACTOR, &CLEAN),
            recorder(ACTOR, &Body { gap: true, ..CLEAN }),
            recorder(NO_RECORDER, &CLEAN),
        ],
    );
    let plan = plan_import(source.as_slice(), &ContentCatalog::empty()).unwrap();
    assert_eq!(plan.validation, "native_import_plan_only");
    assert_eq!(plan.recorders.parsed, 3);
    assert_eq!(plan.recorders.retained, 2);
    assert_eq!(plan.recorders.dropped_by_validate, 1);
    assert_eq!(plan.recorders.typed, 2);
    assert_eq!(plan.recorders.uninterpreted_leaves, 1);
    assert_eq!(plan.base_fields.with_owner_policy, 2);
    assert_eq!(plan.base_fields.with_actor_class, 2);
    assert_eq!(plan.base_fields.with_actor_path, 2);
    assert_eq!(plan.base_fields.with_sublevel_flag, 2);
    assert_eq!(plan.base_fields.unexpected_types, 0);
    assert_eq!(plan.actors.actor_recorders, 2);
    assert_eq!(plan.actors.transform_restoring, 2);
    assert_eq!(plan.actors.recorded_transforms, 2);
    // Per retained recorder: class, row, tile, object and two variable
    // names and one package path; "None" and plain text are not
    // references, and the object name is an instance identity the catalog
    // never resolves.
    assert_eq!(plan.content.name_references, 12);
    assert_eq!(plan.content.path_references, 2);
    assert_eq!(plan.content.distinct, 5);
    assert_eq!(plan.content.unmapped_distinct, 5);
    assert_eq!(plan.content.unmapped_references, 12);
    let role = |references, distinct| RoleCount {
        references,
        distinct,
        unmapped_distinct: distinct,
    };
    assert_eq!(plan.content.roles[&ContentRole::ActorClass], role(2, 1));
    assert_eq!(plan.content.roles[&ContentRole::ActorPath], role(2, 1));
    assert_eq!(plan.content.roles[&ContentRole::DataRow], role(2, 1));
    // The quest variable name stays unclassified beside the tile name.
    assert_eq!(plan.content.roles[&ContentRole::Unclassified], role(4, 2));
    assert_eq!(plan.content.roles[&ContentRole::ClassVariable], role(2, 1));
    assert_eq!(
        plan.content.roles[&ContentRole::InstanceName],
        RoleCount {
            references: 2,
            distinct: 1,
            unmapped_distinct: 0,
        }
    );
    assert!(!plan.content.roles.contains_key(&ContentRole::AssetPath));
    assert_eq!(plan.actors.owner_resolution["find_or_respawn"], 2);
    assert_eq!(plan.actors.owner_resolution.len(), 1);
    assert_eq!(
        plan.blockers,
        vec![
            ImportBlocker::UninterpretedValues { leaves: 1 },
            ImportBlocker::UnmappedContent {
                distinct: 5,
                references: 12
            },
        ]
    );
    assert!(!plan.importable);
    assert!(!plan.world_imported);
    let label = recorder_class_label(&units(ACTOR)).unwrap();
    assert_eq!(plan.classes[&label].retained, 2);
    assert_eq!(plan.classes[&label].unmapped_references, 12);
    let text = serde_json::to_string(&plan).unwrap();
    for private in ["Secret", "secret", "/Game", "ActorState", "NoRecorder"] {
        assert!(!text.contains(private), "{private} leaked");
    }
}

#[test]
fn read_row_and_class_fields_classify_only_inside_their_owners() {
    let body = Body {
        read_fields: true,
        ..CLEAN
    };
    let source = envelope(2, &[recorder(ACTOR, &body)]);
    let plan = plan_import(source.as_slice(), &ContentCatalog::empty()).unwrap();
    let role = |references, distinct| RoleCount {
        references,
        distinct,
        unmapped_distinct: distinct,
    };
    // The item static data, stat, inventory, both bosses, the icon, the
    // seed, the quest and the session flag.
    assert_eq!(plan.content.roles[&ContentRole::DataRow], role(9, 9));
    // Only the tile name inside a FLOD tile record names a world tile; the
    // loose top-level tile name stays unclassified.
    assert_eq!(plan.content.roles[&ContentRole::WorldTile], role(1, 1));
    // The actor class and the tree prefab class.
    assert_eq!(plan.content.roles[&ContentRole::ActorClass], role(2, 2));
    // The tile, the quest variable, the stat outside its record, the nested
    // icon, the map key and the map item's prefab class.
    assert_eq!(plan.content.roles[&ContentRole::Unclassified], role(6, 6));
    let text = serde_json::to_string(&plan).unwrap();
    assert!(!text.contains("Secret"), "a reference leaked");
}

#[test]
fn a_catalog_covering_every_reference_makes_a_clean_save_importable() {
    let source = envelope(2, &[recorder(ACTOR, &CLEAN), recorder(ACTOR, &CLEAN)]);
    let catalog =
        ContentCatalog::from_references([ROW, PATH, "SecretTile", "bSecretFlag", "SecretQuestVar"]);
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert_eq!(plan.catalog_references, 5);
    assert_eq!(plan.content.mapped_distinct, 5);
    assert!(
        plan.content
            .roles
            .values()
            .all(|count| count.unmapped_distinct == 0)
    );
    assert_eq!(plan.content.unmapped_references, 0);
    assert!(plan.blockers.is_empty());
    assert!(plan.importable);
    assert!(!plan.world_imported);

    let partial = ContentCatalog::from_references([ROW]);
    let plan = plan_import(source.as_slice(), &partial).unwrap();
    assert_eq!(
        plan.blockers,
        vec![ImportBlocker::UnmappedContent {
            distinct: 4,
            references: 8
        }]
    );
}

#[test]
fn version_zero_and_all_invalid_saves_would_import_as_empty_worlds() {
    let catalog =
        ContentCatalog::from_references([ROW, PATH, "SecretTile", "bSecretFlag", "SecretQuestVar"]);
    for source in [
        envelope(0, &[recorder(ACTOR, &CLEAN)]),
        envelope(2, &[recorder(NO_RECORDER, &CLEAN)]),
    ] {
        let plan = plan_import(source.as_slice(), &catalog).unwrap();
        assert_eq!(plan.recorders.retained, 0);
        assert_eq!(plan.blockers, vec![ImportBlocker::NoRetainedRecorders]);
        assert!(!plan.importable);
    }
}

#[test]
fn malformed_present_transforms_block_import_and_are_not_planned_actors() {
    let catalog =
        ContentCatalog::from_references([ROW, PATH, "SecretTile", "bSecretFlag", "SecretQuestVar"]);
    let mut cases = vec![
        (
            "wrong outer struct identity".to_owned(),
            transform_tag("AuthoredImpostor", transform_children(&CLEAN)),
        ),
        (
            "wrong outer wire type".to_owned(),
            tag("ActorTransform", "IntProperty", &[0], &0_i32.to_le_bytes()),
        ),
    ];
    for (index, name, impostor, values) in [
        (0, "Rotation", "LinearColor", vec![0.0, 0.0, 0.0, 1.0]),
        (1, "Translation", "Rotator", CLEAN.translation.to_vec()),
        (2, "Scale3D", "Rotator", vec![1.0, 1.0, 1.0]),
    ] {
        // These impostors type losslessly with the same number of floats.
        let mut children = transform_children(&CLEAN);
        children[index] = struct_tag(name, impostor, &floats(&values));
        cases.push((
            format!("{name} same-width impostor"),
            transform_tag("Transform", children),
        ));

        let mut children = transform_children(&CLEAN);
        children.remove(index);
        cases.push((
            format!("{name} absent"),
            transform_tag("Transform", children),
        ));

        let mut children = transform_children(&CLEAN);
        children[index] = tag(name, "FloatProperty", &[0], &1.0_f32.to_le_bytes());
        cases.push((
            format!("{name} wrong wire type"),
            transform_tag("Transform", children),
        ));
    }
    for (case, transform) in cases {
        let source = envelope(
            2,
            &[recorder_with_transform(ACTOR, &CLEAN, Some(transform))],
        );
        let plan = plan_import(source.as_slice(), &catalog).unwrap();
        assert_eq!(plan.recorders.typed, 1, "{case}");
        assert_eq!(plan.recorders.uninterpreted_leaves, 0, "{case}");
        assert_eq!(plan.actors.malformed_transforms, 1, "{case}");
        assert_eq!(plan.actors.missing_transforms, 0, "{case}");
        assert_eq!(plan.actors.recorded_transforms, 0, "{case}");
        assert_eq!(plan.actors.non_finite_transforms, 0, "{case}");
        assert_eq!(
            plan.blockers,
            vec![ImportBlocker::MalformedTransforms { recorders: 1 }],
            "{case}"
        );
        assert!(!plan.importable, "{case}");
        assert!(
            planned_actors(source.as_slice()).unwrap().is_empty(),
            "{case}"
        );
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(json["actors"]["malformed_transforms"], 1, "{case}");
        assert_eq!(
            json["blockers"][0]["blocker"], "malformed_transforms",
            "{case}"
        );
    }
}

#[test]
fn valid_and_absent_transforms_remain_importable_without_inventing_defaults() {
    let catalog =
        ContentCatalog::from_references([ROW, PATH, "SecretTile", "bSecretFlag", "SecretQuestVar"]);
    let absent = recorder_with_transform(ACTOR, &CLEAN, None);
    for recorders in [vec![absent.clone()], vec![recorder(ACTOR, &CLEAN), absent]] {
        let source = envelope(2, &recorders);
        let plan = plan_import(source.as_slice(), &catalog).unwrap();
        assert_eq!(plan.actors.recorded_transforms, recorders.len() - 1);
        assert_eq!(plan.actors.missing_transforms, 1);
        assert_eq!(plan.actors.malformed_transforms, 0);
        assert_eq!(plan.actors.non_finite_transforms, 0);
        assert!(plan.blockers.is_empty());
        assert!(plan.importable);
        let actors = planned_actors(source.as_slice()).unwrap();
        assert_eq!(actors.len(), recorders.len() - 1);
        if let Some(actor) = actors.first() {
            assert_eq!(actor.translation, CLEAN.translation);
            assert_eq!(actor.rotation, [0.0, 0.0, 0.0, 1.0]);
            assert_eq!(actor.scale, [1.0, 1.0, 1.0]);
        }
    }
}

#[test]
fn non_finite_transforms_block_import() {
    let catalog =
        ContentCatalog::from_references([ROW, PATH, "SecretTile", "bSecretFlag", "SecretQuestVar"]);
    let source = envelope(
        2,
        &[recorder(
            ACTOR,
            &Body {
                translation: [f32::NAN, 0.0, f32::INFINITY],
                gap: false,
                ..CLEAN
            },
        )],
    );
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert_eq!(plan.actors.non_finite_transforms, 1);
    assert_eq!(plan.actors.recorded_transforms, 0);
    assert_eq!(plan.actors.missing_transforms, 0);
    assert_eq!(plan.actors.malformed_transforms, 0);
    assert_eq!(
        plan.blockers,
        vec![ImportBlocker::NonFiniteTransforms { recorders: 1 }]
    );
    assert!(!plan.importable);
    assert!(planned_actors(source.as_slice()).unwrap().is_empty());
}

#[test]
fn import_cli_prints_the_plan_on_dry_run_and_refuses_otherwise_without_writing() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("authored.json");
    let destination = root.path().join("world");
    let source = envelope_with_prospect_keys(
        2,
        &[recorder(ACTOR, &CLEAN)],
        Some("SecretProspectRow"),
        None,
    );
    fs::write(&path, &source).unwrap();
    let run = |dry_run: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_icarus-server"));
        command
            .current_dir(root.path())
            .args(["import-save", "--source"])
            .arg(&path)
            .arg("--destination")
            .arg(&destination);
        if dry_run {
            command.arg("--dry-run");
        }
        command.output().unwrap()
    };
    let dry = run(true);
    assert!(dry.status.success(), "{:?}", dry.stderr);
    assert!(dry.stderr.is_empty());
    let plan: Value = serde_json::from_slice(&dry.stdout).unwrap();
    assert_eq!(plan["validation"], "native_import_plan_only");
    assert_eq!(plan["importable"], false);
    assert_eq!(plan["blockers"][0]["blocker"], "unmapped_content");
    assert!(!String::from_utf8_lossy(&dry.stdout).contains("Secret"));

    let refused = run(false);
    assert!(!refused.status.success());
    assert!(refused.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(stderr.contains("import blocked by"));
    assert!(stderr.contains("no files were written"));
    assert!(!stderr.contains("Secret"));
    assert!(!destination.exists());
    assert_eq!(fs::read(&path).unwrap(), source);
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn planned_actors_keep_retained_finite_transforms_in_order() {
    let source = envelope(
        2,
        &[
            recorder(ACTOR, &CLEAN),
            recorder(NO_RECORDER, &CLEAN),
            recorder(
                ACTOR,
                &Body {
                    translation: [f32::NAN, 0.0, 0.0],
                    gap: false,
                    ..CLEAN
                },
            ),
            recorder(
                ACTOR,
                &Body {
                    translation: [1.5, 2.5, -3.5],
                    gap: true,
                    ..CLEAN
                },
            ),
        ],
    );
    let actors = planned_actors(source.as_slice()).unwrap();
    assert_eq!(actors.len(), 2);
    assert_eq!(actors[0].translation, [10.0, -20.0, 30.0]);
    assert_eq!(actors[1].translation, [1.5, 2.5, -3.5]);
    assert_eq!(actors[0].rotation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(actors[0].scale, [1.0, 1.0, 1.0]);
    assert_eq!(actors[0].owner_resolution, "find_or_respawn");
    assert_eq!(
        actors[0].class_label,
        recorder_class_label(&units(ACTOR)).unwrap()
    );
}

#[test]
fn byte_owner_policies_count_and_unexpected_base_types_block() {
    let catalog =
        ContentCatalog::from_references([ROW, PATH, "SecretTile", "bSecretFlag", "SecretQuestVar"]);
    let bytes = envelope(
        2,
        &[recorder(
            ACTOR,
            &Body {
                owner_byte: true,
                ..CLEAN
            },
        )],
    );
    let plan = plan_import(bytes.as_slice(), &catalog).unwrap();
    assert_eq!(plan.base_fields.with_owner_policy, 1);
    assert_eq!(plan.base_fields.unexpected_types, 0);
    assert_eq!(plan.actors.owner_resolution["find_or_respawn"], 1);
    assert!(plan.importable);

    let odd = envelope(
        2,
        &[recorder(
            ACTOR,
            &Body {
                class_as_int: true,
                ..CLEAN
            },
        )],
    );
    let plan = plan_import(odd.as_slice(), &catalog).unwrap();
    assert_eq!(plan.base_fields.unexpected_types, 1);
    assert_eq!(
        plan.blockers,
        vec![ImportBlocker::UnexpectedBaseFieldTypes { fields: 1 }]
    );
    assert!(!plan.importable);
}

#[test]
fn catalog_debug_prints_only_its_size() {
    let printed = format!("{:?}", ContentCatalog::from_references([ROW, PATH]));
    assert!(!printed.contains("Secret") && !printed.contains("83"));
    assert!(printed.contains("references: 2"));
}

#[test]
fn version_zero_saves_with_unregistered_classes_still_plan() {
    let source = envelope(
        0,
        &[
            recorder("/Script/Icarus.Unregistered", &CLEAN),
            recorder(ACTOR, &CLEAN),
        ],
    );
    let plan = plan_import(source.as_slice(), &ContentCatalog::empty()).unwrap();
    assert_eq!(plan.recorders.parsed, 2);
    assert_eq!(plan.recorders.dropped_by_validate, 2);
    assert_eq!(plan.classes["unregistered"].dropped, 1);
    assert_eq!(plan.blockers, vec![ImportBlocker::NoRetainedRecorders]);
    assert!(planned_actors(source.as_slice()).unwrap().is_empty());
}

/// A catalog with every reference of [`CLEAN`] in its role except the item
/// row; `extra` lists more TOML lines under `[references]`.
fn clean_catalog(extra: &str) -> String {
    format!(
        "format = 1\n[references]\nactor_class = [\"{ROW}\"]\nactor_path = [\"{PATH}\"]\nclass_variable = [\"bSecretFlag\"]\nunclassified = [\"SecretTile\", \"SecretQuestVar\"]\n{extra}"
    )
}

// Minimal authored recorder framing for planning strings; these fixtures make
// no claim about the wire types used by the original game.
fn string_reference_source(fields: &[Vec<u8>]) -> Vec<u8> {
    let bytes = [fields.concat(), end(), vec![0; 4]].concat();
    envelope(2, &[recorder_bytes(ACTOR, &bytes)])
}

#[test]
fn effective_prospect_key_resolves_only_as_a_data_row_without_leaking() {
    let key = "SecretProspectRow";
    let body = [end(), vec![0; 4]].concat();
    let source = envelope_with_prospect_keys(2, &[recorder_bytes(ACTOR, &body)], Some(key), None);
    for role in [None, Some("actor_class"), Some("data_row")] {
        let catalog = role.map_or_else(ContentCatalog::empty, |role| {
            ContentCatalog::parse(&format!("format = 1\n[references]\n{role} = [\"{key}\"]\n"))
                .unwrap()
        });
        let plan = plan_import(source.as_slice(), &catalog).unwrap();
        let mapped = role == Some("data_row");
        assert_eq!(plan.content.name_references, 0);
        assert_eq!(plan.content.path_references, 0);
        assert_eq!(plan.content.distinct, 1);
        assert_eq!(plan.content.mapped_distinct, usize::from(mapped));
        assert_eq!(
            plan.content.roles[&ContentRole::DataRow],
            RoleCount {
                references: 1,
                distinct: 1,
                unmapped_distinct: usize::from(!mapped),
            }
        );
        assert_eq!(
            plan.classes
                .values()
                .map(|class| class.unmapped_references)
                .sum::<usize>(),
            0
        );
        assert_eq!(plan.importable, mapped);
        assert!(!plan.world_imported);
        assert_eq!(
            plan.blockers,
            if mapped {
                vec![]
            } else {
                vec![ImportBlocker::UnmappedContent {
                    distinct: 1,
                    references: 1,
                }]
            }
        );
        assert!(!serde_json::to_string(&plan).unwrap().contains(key));
        assert!(!format!("{plan:?}").contains(key));
    }
}

#[test]
fn prospect_key_resolution_uses_staged_migration_precedence() {
    let body = [end(), vec![0; 4]].concat();
    for (version, nested, legacy, effective) in [
        (
            1,
            Some("SecretNested"),
            Some("SecretLegacy"),
            Some("SecretLegacy"),
        ),
        (1, Some("SecretNested"), None, Some("SecretNested")),
        (1, Some("SecretNested"), Some(""), None),
        (
            2,
            Some("SecretNested"),
            Some("SecretLegacy"),
            Some("SecretNested"),
        ),
        (2, None, Some("SecretLegacy"), None),
        (0, Some("SecretNested"), Some("SecretLegacy"), None),
    ] {
        let source =
            envelope_with_prospect_keys(version, &[recorder_bytes(ACTOR, &body)], nested, legacy);
        let plan = plan_import(source.as_slice(), &ContentCatalog::empty()).unwrap();
        assert_eq!(plan.staged.has_prospect_dt_key, effective.is_some());
        assert_eq!(plan.content.distinct, usize::from(effective.is_some()));
        assert_eq!(plan.content.name_references, 0);
        if version == 0 {
            assert_eq!(plan.blockers, vec![ImportBlocker::NoRetainedRecorders]);
        } else {
            let catalog = ContentCatalog::from_references(effective);
            let resolved = plan_import(source.as_slice(), &catalog).unwrap();
            assert!(resolved.blockers.is_empty(), "{version}: {resolved:?}");
            assert!(resolved.importable);
        }
    }
}

#[test]
fn root_and_recorder_rows_deduplicate_but_keep_occurrence_counts() {
    let body = [
        tag("ItemStaticData", "NameProperty", &[0], &fstring(ROW)),
        end(),
        vec![0; 4],
    ]
    .concat();
    let source = envelope_with_prospect_keys(2, &[recorder_bytes(ACTOR, &body)], Some(ROW), None);
    let plan = plan_import(source.as_slice(), &ContentCatalog::empty()).unwrap();
    assert_eq!(plan.content.name_references, 1);
    assert_eq!(plan.content.distinct, 1);
    assert_eq!(plan.content.unmapped_references, 2);
    assert_eq!(
        plan.content.roles[&ContentRole::DataRow],
        RoleCount {
            references: 2,
            distinct: 1,
            unmapped_distinct: 1,
        }
    );
    assert_eq!(
        plan.classes
            .values()
            .map(|class| class.unmapped_references)
            .sum::<usize>(),
        1
    );
    assert_eq!(
        plan.blockers,
        vec![ImportBlocker::UnmappedContent {
            distinct: 1,
            references: 2,
        }]
    );
    let catalog = ContentCatalog::parse(&format!(
        "format = 1\n[references]\ndata_row = [\"{ROW}\"]\n"
    ))
    .unwrap();
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert_eq!(plan.content.mapped_distinct, 1);
    assert_eq!(plan.content.roles[&ContentRole::DataRow].references, 2);
    assert!(plan.importable);
}

#[test]
fn classified_strings_without_package_prefixes_resolve_only_in_their_role() {
    for (property, role, catalog_role, wrong_role) in [
        (
            "ItemStaticData",
            ContentRole::DataRow,
            "data_row",
            "actor_path",
        ),
        (
            "ActorPathName",
            ContentRole::ActorPath,
            "actor_path",
            "data_row",
        ),
    ] {
        let source = string_reference_source(&[tag(property, "StrProperty", &[0], &fstring(ROW))]);
        for catalog in [
            ContentCatalog::empty(),
            ContentCatalog::parse(&format!(
                "format = 1\n[references]\n{wrong_role} = [\"{ROW}\"]\n"
            ))
            .unwrap(),
        ] {
            let plan = plan_import(source.as_slice(), &catalog).unwrap();
            assert_eq!(plan.recorders.typed, 1);
            assert_eq!(plan.content.name_references, 0);
            assert_eq!(plan.content.path_references, 0);
            assert_eq!(plan.content.roles[&role].references, 1);
            assert_eq!(plan.content.roles[&role].unmapped_distinct, 1);
            assert_eq!(
                plan.blockers,
                vec![ImportBlocker::UnmappedContent {
                    distinct: 1,
                    references: 1,
                }]
            );
            assert!(!plan.importable);
        }
        let catalog = ContentCatalog::parse(&format!(
            "format = 1\n[references]\n{catalog_role} = [\"{ROW}\"]\n"
        ))
        .unwrap();
        let plan = plan_import(source.as_slice(), &catalog).unwrap();
        assert_eq!(plan.content.mapped_distinct, 1);
        assert_eq!(plan.content.roles[&role].unmapped_distinct, 0);
        assert!(plan.blockers.is_empty());
        assert!(plan.importable);
        assert!(!plan.world_imported);
    }
}

#[test]
fn free_text_and_empty_or_none_strings_are_not_references() {
    let mut fields = vec![("Label", ROW)];
    for property in ["ItemStaticData", "ActorPathName", "Label"] {
        for value in ["", "None"] {
            fields.push((property, value));
        }
    }
    for (property, value) in fields {
        let source =
            string_reference_source(&[tag(property, "StrProperty", &[0], &fstring(value))]);
        let plan = plan_import(source.as_slice(), &ContentCatalog::empty()).unwrap();
        assert_eq!(plan.recorders.typed, 1);
        assert_eq!(plan.content.name_references, 0);
        assert_eq!(plan.content.path_references, 0);
        assert_eq!(plan.content.distinct, 0);
        assert!(plan.content.roles.is_empty());
        assert!(plan.blockers.is_empty(), "{property}={value:?}: {plan:?}");
        assert!(plan.importable);
    }
}

#[test]
fn nested_classified_string_arrays_collect_references_and_skip_sentinels() {
    let source = string_reference_source(&[struct_tag(
        "Record",
        "GameModeRecord",
        &[
            tag(
                "SessionFlagRecords",
                "ArrayProperty",
                &[fstring("StrProperty"), vec![0]].concat(),
                &[
                    4_i32.to_le_bytes().to_vec(),
                    fstring(ROW),
                    fstring(ROW),
                    fstring(""),
                    fstring("None"),
                ]
                .concat(),
            ),
            end(),
        ]
        .concat(),
    )]);
    let plan = plan_import(source.as_slice(), &ContentCatalog::empty()).unwrap();
    assert_eq!(plan.recorders.typed, 1);
    assert_eq!(plan.recorders.uninterpreted_leaves, 0, "{plan:?}");
    assert_eq!(plan.content.path_references, 0);
    assert_eq!(
        plan.content.roles[&ContentRole::DataRow],
        RoleCount {
            references: 2,
            distinct: 1,
            unmapped_distinct: 1,
        }
    );
    assert_eq!(
        plan.blockers,
        vec![ImportBlocker::UnmappedContent {
            distinct: 1,
            references: 2,
        }]
    );
    let catalog = ContentCatalog::parse(&format!(
        "format = 1\n[references]\ndata_row = [\"{ROW}\"]\n"
    ))
    .unwrap();
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert!(plan.blockers.is_empty());
    assert!(plan.importable);
}

#[test]
fn a_catalog_resolves_each_reference_only_in_the_roles_it_lists() {
    let source = envelope(2, &[recorder(ACTOR, &CLEAN)]);
    // The row name is the actor class and the item's static data row; the
    // catalog lists it as an actor class only.
    let catalog = ContentCatalog::parse(&clean_catalog("")).unwrap();
    assert_eq!(catalog.len(), 5);
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert_eq!(plan.catalog_references, 5);
    assert_eq!(
        plan.content.roles[&ContentRole::ActorClass].unmapped_distinct,
        0
    );
    assert_eq!(
        plan.content.roles[&ContentRole::DataRow].unmapped_distinct,
        1
    );
    assert_eq!(
        plan.blockers,
        vec![ImportBlocker::UnmappedContent {
            distinct: 1,
            references: 1
        }]
    );
    let catalog =
        ContentCatalog::parse(&clean_catalog(&format!("data_row = [\"{ROW}\"]\n"))).unwrap();
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(plan.importable);
}

#[test]
fn malformed_catalogs_fail_without_naming_their_references() {
    for (text, expected) in [
        ("format = 2\n", "unsupported content catalog format"),
        (
            "format = 1\n[references]\ninstance_name = [\"SecretObject\"]\n",
            "invalid content catalog",
        ),
        (
            "format = 1\n[references]\ndata_row = [\"SecretRow\", \"SecretRow\"]\n",
            "lists a reference twice",
        ),
        (
            "format = 1\n[references]\ndata_row = [\"\"]\n",
            "reference must be",
        ),
        (
            "format = 1\n[references]\ndata_row = [\"SecretRow\"\n",
            "invalid content catalog",
        ),
        (
            "format = 1\nSecretKey = 1\n",
            "invalid content catalog near line 2",
        ),
    ] {
        let error = format!("{:#}", ContentCatalog::parse(text).unwrap_err());
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("Secret"), "{error}");
    }
    let long = "x".repeat(1_025);
    let error = ContentCatalog::parse(&format!(
        "format = 1\n[references]\ndata_row = [\"{long}\"]\n"
    ))
    .unwrap_err();
    assert!(format!("{error:#}").contains("reference must be"));
}

#[test]
fn catalog_files_must_be_small_regular_files() {
    let root = TempDir::new().unwrap();
    let error = ContentCatalog::load(root.path()).unwrap_err();
    assert!(format!("{error:#}").contains("regular file"), "{error:#}");
    let fifo = root.path().join("catalog.fifo");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let error = ContentCatalog::load(&fifo).unwrap_err();
    assert!(format!("{error:#}").contains("regular file"), "{error:#}");
    let large = root.path().join("large.toml");
    let file = fs::File::create(&large).unwrap();
    file.set_len(icarus_bevy::import_plan::MAX_CATALOG_BYTES + 1)
        .unwrap();
    let error = ContentCatalog::load(&large).unwrap_err();
    assert!(format!("{error:#}").contains("4 MiB"), "{error:#}");
}

#[test]
fn import_cli_resolves_against_a_catalog_file() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("authored.json");
    fs::write(&path, envelope(2, &[recorder(ACTOR, &CLEAN)])).unwrap();
    let catalog = root.path().join("catalog.toml");
    fs::write(
        &catalog,
        clean_catalog(&format!("data_row = [\"{ROW}\"]\n")),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["import-save", "--dry-run", "--source"])
        .arg(&path)
        .arg("--destination")
        .arg(root.path().join("world"))
        .arg("--catalog")
        .arg(&catalog)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    // The row name counts once though listed in two roles.
    assert_eq!(plan["catalog_references"], 5);
    assert_eq!(plan["importable"], true);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Secret"));

    fs::write(
        &catalog,
        "format = 1\n[references]\ndata_row = [\"SecretRow\"\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["import-save", "--dry-run", "--source"])
        .arg(&path)
        .arg("--destination")
        .arg(root.path().join("world"))
        .arg("--catalog")
        .arg(&catalog)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid content catalog"), "{stderr}");
    assert!(!stderr.contains("Secret"), "{stderr}");
}

/// A pak whose tables name the item row in another case (F-840).
fn data_pak(root: &TempDir) -> std::path::PathBuf {
    let path = root.path().join("data.pak");
    fs::write(
        &path,
        pak_writer::official_pak(&[
            (
                "Items/D_SecretItems.json",
                pak_writer::table_json(&["Other", &ROW.to_ascii_lowercase()]),
            ),
            ("DataTableMetadata.json", b"{\"Tables\": []}".to_vec()),
        ]),
    )
    .unwrap();
    path
}

#[test]
fn table_catalog_counts_distinct_rows_and_authored_overlap() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("counts.pak");
    fs::write(
        &path,
        pak_writer::write_pak(&[
            (
                "A/D_Rows.json",
                pak_writer::table_json(&[ROW, "Other", "CASE"]).as_slice(),
                false,
            ),
            (
                "B/D_Rows.json",
                pak_writer::table_json(&[&ROW.to_ascii_lowercase(), "Other", "case"]).as_slice(),
                true,
            ),
        ]),
    )
    .unwrap();
    let mut pak = icarus_bevy::data_pak::DataPak::open(&path).unwrap();
    let empty = ContentCatalog::empty();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    let (catalog, tables, skipped) = empty.with_data_tables(&mut pak).unwrap();
    assert_eq!((tables, skipped), (2, 0));
    assert!(!catalog.is_empty());
    assert_eq!(catalog.len(), 3);
    let source =
        string_reference_source(&[tag("ItemStaticData", "StrProperty", &[0], &fstring(ROW))]);
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert_eq!(plan.catalog_references, 3);
    assert!(plan.importable);
    assert!(format!("{catalog:?}").contains("references: 3"));

    // Repeated attachment and rows shared across tables add no references.
    let (catalog, ..) = catalog.with_data_tables(&mut pak).unwrap();
    assert_eq!(catalog.len(), 3);

    for authored in [
        ContentCatalog::from_references([ROW, "AuthoredOnly"]),
        ContentCatalog::parse(&format!(
            "format = 1\n[references]\nactor_class = [\"{ROW}\", \"AuthoredOnly\"]\ndata_row = [\"{ROW}\"]\n"
        ))
        .unwrap(),
    ] {
        let (catalog, ..) = authored.with_data_tables(&mut pak).unwrap();
        assert_eq!(catalog.len(), 4);
        assert_eq!(plan_import(source.as_slice(), &catalog).unwrap().catalog_references, 4);
    }

    // Case-sensitive authored spellings still count separately. A table row
    // covered by either spelling does not add a third entry.
    let (catalog, ..) = ContentCatalog::from_references(["Case", "CASE"])
        .with_data_tables(&mut pak)
        .unwrap();
    assert_eq!(catalog.len(), 4);
    let source =
        string_reference_source(&[tag("ActorPathName", "StrProperty", &[0], &fstring("case"))]);
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert_eq!(
        plan.content.roles[&ContentRole::ActorPath].unmapped_distinct,
        1
    );
    assert!(!plan.importable);
}

/// Data-row references resolve against the official server's data tables
/// without case; other roles do not, and non-table members are skipped.
#[test]
fn data_rows_resolve_against_the_official_data_tables() {
    let root = TempDir::new().unwrap();
    let mut pak = icarus_bevy::data_pak::DataPak::open(&data_pak(&root)).unwrap();
    let (catalog, tables, skipped) = ContentCatalog::parse(&clean_catalog(""))
        .unwrap()
        .with_data_tables(&mut pak)
        .unwrap();
    // The authored player tables plus the item table; the metadata member
    // is skipped as not a table.
    assert_eq!(tables, pak_writer::player_tables().len() + 1);
    assert_eq!(skipped, 1);
    let source = envelope(2, &[recorder(ACTOR, &CLEAN)]);
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(plan.importable);
    // Without the actor class listed, the table's row does not resolve it.
    let (catalog, ..) = ContentCatalog::empty().with_data_tables(&mut pak).unwrap();
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert_eq!(
        plan.content.roles[&ContentRole::DataRow].unmapped_distinct,
        0
    );
    assert_eq!(
        plan.content.roles[&ContentRole::ActorClass].unmapped_distinct,
        1
    );
}

#[test]
fn import_cli_resolves_data_rows_against_a_data_pak() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("authored.json");
    fs::write(&path, envelope(2, &[recorder(ACTOR, &CLEAN)])).unwrap();
    let catalog = root.path().join("catalog.toml");
    fs::write(&catalog, clean_catalog("")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["import-save", "--dry-run", "--source"])
        .arg(&path)
        .arg("--destination")
        .arg(root.path().join("world"))
        .arg("--catalog")
        .arg(&catalog)
        .arg("--data-pak")
        .arg(data_pak(&root))
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.stderr);
    let plan: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(plan["importable"], true);
    let record: Value = serde_json::from_slice(
        output
            .stderr
            .split(|byte| *byte == b'\n')
            .find(|line| !line.is_empty())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(record["event"], "data_tables_loaded");
    assert_eq!(record["skipped_members"], 1);
    // A missing pak fails before planning.
    let output = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .args(["import-save", "--dry-run", "--source"])
        .arg(&path)
        .arg("--destination")
        .arg(root.path().join("world"))
        .arg("--data-pak")
        .arg(root.path().join("missing.pak"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--data-pak"));
}

/// A JSON member that does not parse, or two tables sharing a file name,
/// fail instead of being skipped: every member is read by its full path.
#[test]
fn unreadable_data_table_members_fail_and_shared_names_both_load() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("broken.pak");
    fs::write(
        &path,
        pak_writer::write_pak(&[("Items/D_Broken.json", b"{\"Rows\": [".as_slice(), false)]),
    )
    .unwrap();
    let mut pak = icarus_bevy::data_pak::DataPak::open(&path).unwrap();
    assert!(ContentCatalog::empty().with_data_tables(&mut pak).is_err());
    let path = root.path().join("shared.pak");
    fs::write(
        &path,
        pak_writer::write_pak(&[
            (
                "A/D_Same.json",
                pak_writer::table_json(&["Other"]).as_slice(),
                false,
            ),
            (
                "B/D_Same.json",
                pak_writer::table_json(&[ROW]).as_slice(),
                true,
            ),
        ]),
    )
    .unwrap();
    let mut pak = icarus_bevy::data_pak::DataPak::open(&path).unwrap();
    let (catalog, tables, skipped) = ContentCatalog::empty().with_data_tables(&mut pak).unwrap();
    assert_eq!((tables, skipped), (2, 0));
    let plan = plan_import(envelope(2, &[recorder(ACTOR, &CLEAN)]).as_slice(), &catalog).unwrap();
    assert_eq!(
        plan.content.roles[&ContentRole::DataRow].unmapped_distinct,
        0
    );
}

#[test]
fn prospect_pak_resolution_requires_the_prospect_table_for_every_occurrence() {
    let root = TempDir::new().unwrap();
    let body = [
        tag("ItemStaticData", "NameProperty", &[0], &fstring(ROW)),
        end(),
        vec![0; 4],
    ]
    .concat();
    for prospect_table in [false, true] {
        let path = root.path().join("prospect.pak");
        let mut members = vec![("Items/D_Unrelated.json", pak_writer::table_json(&[ROW]))];
        if prospect_table {
            members.push((
                "Prospects/D_ProspectList.json",
                pak_writer::table_json(&[&ROW.to_ascii_lowercase()]),
            ));
        }
        fs::write(&path, pak_writer::official_pak(&members)).unwrap();
        let mut pak = icarus_bevy::data_pak::DataPak::open(&path).unwrap();
        let (catalog, ..) = ContentCatalog::empty().with_data_tables(&mut pak).unwrap();
        for version in [1, 2] {
            let source = envelope_with_prospect_keys(
                version,
                &[recorder_bytes(ACTOR, &body)],
                Some(if version == 1 {
                    "SecretOverwritten"
                } else {
                    ROW
                }),
                (version == 1).then_some(ROW),
            );
            let plan = plan_import(source.as_slice(), &catalog).unwrap();
            assert_eq!(plan.importable, prospect_table);
            assert_eq!(plan.content.distinct, 1);
            assert_eq!(
                plan.content.unmapped_references,
                usize::from(!prospect_table)
            );
            assert_eq!(
                plan.content.roles[&ContentRole::DataRow],
                RoleCount {
                    references: 2,
                    distinct: 1,
                    unmapped_distinct: usize::from(!prospect_table),
                }
            );
            assert_eq!(
                plan.classes
                    .values()
                    .map(|class| class.unmapped_references)
                    .sum::<usize>(),
                0
            );
            assert_eq!(
                plan.blockers,
                if prospect_table {
                    vec![]
                } else {
                    vec![ImportBlocker::UnmappedContent {
                        distinct: 1,
                        references: 1,
                    }]
                }
            );
        }
        for role in ["data_row", "actor_class"] {
            let explicit =
                ContentCatalog::parse(&format!("format = 1\n[references]\n{role} = [\"{ROW}\"]\n"))
                    .unwrap();
            let (explicit, ..) = explicit.with_data_tables(&mut pak).unwrap();
            let source =
                envelope_with_prospect_keys(2, &[recorder_bytes(ACTOR, &body)], Some(ROW), None);
            assert_eq!(
                plan_import(source.as_slice(), &explicit)
                    .unwrap()
                    .importable,
                prospect_table || role == "data_row"
            );
        }
        let (any, ..) = ContentCatalog::from_references([ROW])
            .with_data_tables(&mut pak)
            .unwrap();
        let source =
            envelope_with_prospect_keys(2, &[recorder_bytes(ACTOR, &body)], Some(ROW), None);
        assert!(plan_import(source.as_slice(), &any).unwrap().importable);
    }
}

#[test]
fn ambiguous_prospect_tables_fail_instead_of_unioning_their_rows() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("ambiguous.pak");
    fs::write(
        &path,
        pak_writer::official_pak(&[
            ("A/D_ProspectList.json", pak_writer::table_json(&[ROW])),
            ("B/D_ProspectList.json", pak_writer::table_json(&["Other"])),
        ]),
    )
    .unwrap();
    let mut pak = icarus_bevy::data_pak::DataPak::open(&path).unwrap();
    assert!(ContentCatalog::empty().with_data_tables(&mut pak).is_err());
}
