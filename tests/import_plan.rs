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
}

fn component(body: &Body) -> Vec<u8> {
    let transform = [
        struct_tag("Rotation", "Quat", &floats(&[0.0, 0.0, 0.0, 1.0])),
        struct_tag("Translation", "Vector", &floats(&body.translation)),
        struct_tag("Scale3D", "Vector", &floats(&[1.0, 1.0, 1.0])),
        end(),
    ]
    .concat();
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
        struct_tag("ActorTransform", "Transform", &transform),
        tag("Label", "StrProperty", &[0], &fstring("secret plain text")),
        tag("Nothing", "NameProperty", &[0], &fstring("None")),
        tag("ItemStaticData", "NameProperty", &[0], &fstring(ROW)),
        tag("TileName", "NameProperty", &[0], &fstring("SecretTile")),
    ];
    if body.gap {
        fields.push(struct_tag("Mystery", "SecretBlobType", &[1, 2, 3]));
    }
    fields.push(end());
    [fields.concat(), vec![0; 4]].concat()
}

fn recorder(class: &str, body: &Body) -> Vec<u8> {
    let bytes = component(body);
    let count = i32::try_from(bytes.len()).unwrap().to_le_bytes();
    [
        tag("ComponentClassName", "StrProperty", &[0], &fstring(class)),
        tag(
            "BinaryData",
            "ArrayProperty",
            &[fstring("ByteProperty"), vec![0]].concat(),
            &[count.as_slice(), &bytes].concat(),
        ),
        end(),
    ]
    .concat()
}

fn envelope(version: i32, recorders: &[Vec<u8>]) -> Vec<u8> {
    let info = [
        tag(
            "ProspectID",
            "StrProperty",
            &[0],
            &fstring("secret-prospect"),
        ),
        end(),
    ]
    .concat();
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &[fstring("StateRecorderBlob"), vec![0; 17]].concat(),
        &recorders.concat(),
    );
    let count = i32::try_from(recorders.len()).unwrap().to_le_bytes();
    let payload = [
        tag("Version", "IntProperty", &[0], &version.to_le_bytes()),
        struct_tag("ProspectInfo", "ProspectInfo", &info),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &[fstring("StructProperty"), vec![0]].concat(),
            &[count.as_slice(), &shared].concat(),
        ),
        end(),
    ]
    .concat();
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
    // Per retained recorder: class, row and tile names and one package path;
    // "None" and plain text are not references.
    assert_eq!(plan.content.name_references, 6);
    assert_eq!(plan.content.path_references, 2);
    assert_eq!(plan.content.distinct, 3);
    assert_eq!(plan.content.unmapped_distinct, 3);
    assert_eq!(plan.content.unmapped_references, 8);
    let role = |references, distinct| RoleCount {
        references,
        distinct,
        unmapped_distinct: distinct,
    };
    assert_eq!(plan.content.roles[&ContentRole::ActorClass], role(2, 1));
    assert_eq!(plan.content.roles[&ContentRole::ActorPath], role(2, 1));
    assert_eq!(plan.content.roles[&ContentRole::DataRow], role(2, 1));
    assert_eq!(plan.content.roles[&ContentRole::Unclassified], role(2, 1));
    assert!(!plan.content.roles.contains_key(&ContentRole::AssetPath));
    assert_eq!(plan.actors.owner_resolution["find_or_respawn"], 2);
    assert_eq!(plan.actors.owner_resolution.len(), 1);
    assert_eq!(
        plan.blockers,
        vec![
            ImportBlocker::UninterpretedValues { leaves: 1 },
            ImportBlocker::UnmappedContent {
                distinct: 3,
                references: 8
            },
        ]
    );
    assert!(!plan.importable);
    assert!(!plan.world_imported);
    let label = recorder_class_label(&units(ACTOR)).unwrap();
    assert_eq!(plan.classes[&label].retained, 2);
    assert_eq!(plan.classes[&label].unmapped_references, 8);
    let text = serde_json::to_string(&plan).unwrap();
    for private in ["Secret", "secret", "/Game", "ActorState", "NoRecorder"] {
        assert!(!text.contains(private), "{private} leaked");
    }
}

#[test]
fn a_catalog_covering_every_reference_makes_a_clean_save_importable() {
    let source = envelope(2, &[recorder(ACTOR, &CLEAN), recorder(ACTOR, &CLEAN)]);
    let catalog = ContentCatalog::from_references([ROW, PATH, "SecretTile"]);
    let plan = plan_import(source.as_slice(), &catalog).unwrap();
    assert_eq!(plan.catalog_references, 3);
    assert_eq!(plan.content.mapped_distinct, 3);
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
            distinct: 2,
            references: 4
        }]
    );
}

#[test]
fn version_zero_and_all_invalid_saves_would_import_as_empty_worlds() {
    let catalog = ContentCatalog::from_references([ROW, PATH, "SecretTile"]);
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
fn non_finite_transforms_block_import() {
    let catalog = ContentCatalog::from_references([ROW, PATH, "SecretTile"]);
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
    assert_eq!(
        plan.blockers,
        vec![ImportBlocker::NonFiniteTransforms { recorders: 1 }]
    );
}

#[test]
fn import_cli_prints_the_plan_on_dry_run_and_refuses_otherwise_without_writing() {
    let root = TempDir::new().unwrap();
    let path = root.path().join("authored.json");
    let destination = root.path().join("world");
    let source = envelope(2, &[recorder(ACTOR, &CLEAN)]);
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
    let catalog = ContentCatalog::from_references([ROW, PATH, "SecretTile"]);
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
