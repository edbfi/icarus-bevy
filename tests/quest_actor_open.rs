//! Native `C019` quest actor stitching, pure `Check()` completion evaluation,
//! and `AQuest` open composition (`F-934`, `F-935`, `U-074`, `U-076`), using authored
//! records and captured `M-707` bit-length / SHA-256 vectors (no supplied save
//! files).
mod pak_writer;

use std::collections::BTreeMap;

use icarus_bevy::{
    config::NetworkConfig,
    data_pak::DataTables,
    experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        control_messages::ArchiveText,
        manager_open::{ManagerSlot, compose_manager_open, quest_handle},
        packet::Payload,
        peer_lab::DEFAULT_MAX_CHANNELS,
        prospect_info::{
            RelevantQuestActor, SavedQuestActor, SavedQuestManager, SavedQuestVariable,
            SavedSubQuest,
        },
        quest_open::{
            AQUEST_CLASS_CHECKSUM, QUEST_CLASSES, QuestCheckRule, QuestTableRow,
            compose_quest_open, evaluate_quest_check, fname_network_name, quest_actor_handle,
            quest_class_spec, stitch_saved_quests,
        },
        release::ReleaseProfile,
        rep_layout::{
            MovementLevel, NetworkName, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
            quest_rep_layout, read_rep_properties,
        },
        session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
    },
    network::{apply_saved_game_state, bind, saved_game_state},
    preservation::{PreserveLimits, PreservedSection},
};
use sha2::{Digest, Sha256};

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn sha256(payload: &Payload) -> String {
    Sha256::digest(payload.bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn burn_static_to(cache: &mut NetGuidCache, next: u32) {
    for index in 0.. {
        let probe = cache.assign_static(&[PathSegment::new(&format!("burn{index}"), 0, false)]);
        if probe + 2 >= next {
            assert_eq!(probe + 2, next, "static GUID {next} unreachable");
            return;
        }
    }
}

fn var(name: &str, bool_value: bool, float_value: f32, int_value: i32) -> SavedQuestVariable {
    SavedQuestVariable {
        name: units(name),
        bool_value,
        float_value,
        int_value,
    }
}

fn authored_quest(
    icarus_guid: i32,
    quest_name: &str,
    class_name: &str,
    subquests: Vec<(&str, &str, i32)>,
    variables: Vec<SavedQuestVariable>,
) -> SavedQuestActor {
    SavedQuestActor {
        icarus_guid,
        class_name: units(class_name),
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale_3d: [1.0, 1.0, 1.0],
        base_extras: 0,
        quest_name: units(quest_name),
        relevant_actors: Vec::new(),
        relevant_characters: Vec::new(),
        subquests: subquests
            .into_iter()
            .map(|(row, cls, uid)| SavedSubQuest {
                row_name: units(row),
                class_name: units(cls),
                icarus_uid: uid,
            })
            .collect(),
        variables,
    }
}

#[test]
fn quest_class_table_is_sorted_and_covers_all_2044_d_quests_classes() {
    assert_eq!(QUEST_CLASSES.len(), 2_044);
    assert!(
        QUEST_CLASSES
            .windows(2)
            .all(|pair| pair[0].class_name < pair[1].class_name),
        "QUEST_CLASSES must be strictly sorted by class_name"
    );
    assert_eq!(
        QUEST_CLASSES
            .iter()
            .filter(|spec| spec.adds_net_props)
            .count(),
        270
    );
    let base = quest_class_spec("Quest").expect("native AQuest entry");
    assert_eq!(base.checksum, AQUEST_CLASS_CHECKSUM);
    assert_eq!(base.checksum, 4_033_596_864);
    assert!(!base.adds_net_props);
    assert_eq!(base.check, QuestCheckRule::AlwaysFalse);

    let travel = quest_class_spec("BPQ_Travel_C").expect("BPQ_Travel_C");
    assert_eq!(travel.checksum, AQUEST_CLASS_CHECKSUM);
    assert!(!travel.adds_net_props);
    assert_eq!(travel.check, QuestCheckRule::BoolVar("LocationReached"));

    // CDO `AllowChecksAfterCompletion == false` compiles to pure `BoolVar`.
    let deploy = quest_class_spec("BPQ_OLY_Riverlands_Construction_Beds_C")
        .expect("BPQ_OLY_Riverlands_Construction_Beds_C");
    assert_eq!(deploy.check, QuestCheckRule::BoolVar("QuestComplete"));

    // CDO `AllowChecksAfterCompletion == true` compiles to `BoolVarFalseOnly`.
    let rechecked_deploy =
        quest_class_spec("BPQ_GH_Ape_O2_Deploy_C").expect("BPQ_GH_Ape_O2_Deploy_C");
    assert_eq!(
        rechecked_deploy.check,
        QuestCheckRule::BoolVarFalseOnly("QuestComplete")
    );
}

#[test]
fn fname_network_name_splits_canonical_decimal_suffixes_only() {
    assert_eq!(
        fname_network_name(&units("OLY_Riverlands_Construction_0")).unwrap(),
        NetworkName::String {
            text: ArchiveText::text("OLY_Riverlands_Construction").unwrap(),
            number: 1,
        }
    );
    assert_eq!(
        fname_network_name(&units("Stage_12")).unwrap(),
        NetworkName::String {
            text: ArchiveText::text("Stage").unwrap(),
            number: 13,
        }
    );
    assert_eq!(
        fname_network_name(&units("Stage_2147483646")).unwrap(),
        NetworkName::String {
            text: ArchiveText::text("Stage").unwrap(),
            number: 2_147_483_647,
        }
    );
    for plain in [
        "PlainName",
        "_0",
        "Stage_",
        "Stage_00",
        "Stage_01",
        "Stage_2147483647",
        "Stage_999999999999",
    ] {
        assert_eq!(
            fname_network_name(&units(plain)).unwrap(),
            NetworkName::String {
                text: ArchiveText::text(plain).unwrap(),
                number: 0,
            },
            "{plain}"
        );
    }
}

#[test]
fn evaluate_quest_check_covers_every_rule_variant_and_fails_closed() {
    let vars = vec![
        var("Complete", true, 0.0, 0),
        var("Partial", false, 0.0, 0),
        var("Progress", false, 2.5, 4),
        var("Target", false, 2.0, 4),
    ];
    let flags = vec![units(" boss_defeated ")];
    let exact_flags = vec![units("Boss_Defeated")];

    assert!(!evaluate_quest_check(QuestCheckRule::AlwaysFalse, &vars, &exact_flags).unwrap());
    assert!(evaluate_quest_check(QuestCheckRule::AlwaysTrue, &vars, &exact_flags).unwrap());
    assert!(
        evaluate_quest_check(QuestCheckRule::BoolVar("Complete"), &vars, &exact_flags).unwrap()
    );
    // QuestVariables use exact case-sensitive FString comparison.
    assert!(
        !evaluate_quest_check(QuestCheckRule::BoolVar("complete"), &vars, &exact_flags).unwrap()
    );
    // SessionFlags use ASCII case-insensitive FName comparison.
    assert!(
        evaluate_quest_check(
            QuestCheckRule::QuestFlag("boss_defeated"),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        !evaluate_quest_check(QuestCheckRule::QuestFlag("boss_defeated"), &vars, &flags).unwrap()
    );

    assert!(
        !evaluate_quest_check(
            QuestCheckRule::BoolVarFalseOnly("Partial"),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::BoolVarFalseOnly("Complete"),
            &vars,
            &exact_flags
        )
        .is_err()
    );

    assert!(
        evaluate_quest_check(
            QuestCheckRule::AllBoolVars(&["Complete"]),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        !evaluate_quest_check(
            QuestCheckRule::AllBoolVars(&["Complete", "Partial"]),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::AnyBoolVars(&["Missing", "Complete"]),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::AnyQuestFlags(&["Absent", "BOSS_DEFEATED"]),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::BoolVarOrQuestFlag("Partial", "BOSS_DEFEATED"),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::IntVarAtLeast("Progress", 4),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::IntVarAtLeastVar("Progress", "Target"),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::IntVarAtMost("Progress", 4),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::FloatVarAtLeastVar("Progress", "Target"),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(
        evaluate_quest_check(
            QuestCheckRule::FloatVarGreater("Progress", 2.0),
            &vars,
            &exact_flags
        )
        .unwrap()
    );
    assert!(evaluate_quest_check(QuestCheckRule::Impure, &vars, &exact_flags).is_err());
}

struct CapturedQuestExport {
    channel: u32,
    next_static: Option<u32>,
    next_dynamic: Option<u32>,
    expected_actor_guid: u32,
    exports_bits: usize,
    exports_sha256: &'static str,
}

const CAPTURED_QUEST_EXPORTS: [CapturedQuestExport; 8] = [
    CapturedQuestExport {
        channel: 36,
        next_static: Some(267),
        next_dynamic: Some(552),
        expected_actor_guid: 552,
        exports_bits: 1_793,
        exports_sha256: "b2d0b55e90427fa206e8d0d0274df23ba3f989ebf7ad41c29c3e4b96c2d8a773",
    },
    CapturedQuestExport {
        channel: 37,
        next_static: None,
        next_dynamic: None,
        expected_actor_guid: 560,
        exports_bits: 1_889,
        exports_sha256: "9f82d54cee034ad6e2ff28e31f3ff95b3af714f7438fa02c2d8708d455ef809f",
    },
    CapturedQuestExport {
        channel: 42,
        next_static: Some(285),
        next_dynamic: Some(618),
        expected_actor_guid: 618,
        exports_bits: 1_713,
        exports_sha256: "2eb88f3b1a26b4273856ca3cafc43d4ba23a9ad4c59f4f2cafcad4d17890fcf2",
    },
    CapturedQuestExport {
        channel: 43,
        next_static: None,
        next_dynamic: None,
        expected_actor_guid: 558,
        exports_bits: 1_889,
        exports_sha256: "cccd0857c415323db165d5e9acd48d1f5b554b0ae73764e89f7b0686e742f36a",
    },
    CapturedQuestExport {
        channel: 44,
        next_static: None,
        next_dynamic: None,
        expected_actor_guid: 630,
        exports_bits: 1_857,
        exports_sha256: "5a9d2a22736912e515b5679d1d67c91c744158aa9e2184b67ad8b3ee95cb8d3e",
    },
    CapturedQuestExport {
        channel: 45,
        next_static: None,
        next_dynamic: None,
        expected_actor_guid: 628,
        exports_bits: 1_857,
        exports_sha256: "8768f356c427e10a09b733a177550de4812ceb21d3007d98f86d150ecb6cd2c1",
    },
    CapturedQuestExport {
        channel: 46,
        next_static: None,
        next_dynamic: None,
        expected_actor_guid: 632,
        exports_bits: 1_633,
        exports_sha256: "e1b83b15da233199de7769b669f7be1bfc29d437bd5466cbf9fba5f139a615ad",
    },
    CapturedQuestExport {
        channel: 47,
        next_static: None,
        next_dynamic: None,
        expected_actor_guid: 646,
        exports_bits: 1_097,
        exports_sha256: "7cc969be27d02551ff626d8078dca71a55ec3ac926eeed648c4fe843a6911f1d",
    },
];

#[test]
fn quest_actor_opens_match_m707_captured_export_digests_and_net_guids() {
    let rows = vec![
        QuestTableRow {
            name: "OLY_Riverlands_Construction_0".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction.BPQ_OLY_Riverlands_Construction_C".to_owned(),
            ),
        },
        QuestTableRow {
            name: "OLY_Riverlands_Construction_Head_To_Location".to_owned(),
            class_path: Some("/Game/BP/Quests/Common/BPQ_Travel.BPQ_Travel_C".to_owned()),
        },
        QuestTableRow {
            name: "OLY_Riverlands_Construction_Medical_Outpost".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction_Medical_Outpost.BPQ_OLY_Riverlands_Construction_Medical_Outpost_C".to_owned(),
            ),
        },
        QuestTableRow {
            name: "OLY_Riverlands_Construction_Kitchen_Bench".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction_Kitchen_Bench.BPQ_OLY_Riverlands_Construction_Kitchen_Bench_C".to_owned(),
            ),
        },
        QuestTableRow {
            name: "OLY_Riverlands_Construction_Kitchen_Stove".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction_Kitchen_Stove.BPQ_OLY_Riverlands_Construction_Kitchen_Stove_C".to_owned(),
            ),
        },
        QuestTableRow {
            name: "OLY_Riverlands_Construction_Beds".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction_Beds.BPQ_OLY_Riverlands_Construction_Beds_C".to_owned(),
            ),
        },
        QuestTableRow {
            name: "OLY_Riverlands_Construction_Herbalism_Bench".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction_Herbalism_Bench.BPQ_OLY_Riverlands_Construction_Herbalism_Bench_C".to_owned(),
            ),
        },
        QuestTableRow {
            name: "OLY_Riverlands_Construction_Crop_Plot".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction_Crop_Plot.BPQ_OLY_Riverlands_Construction_Crop_Plot_C".to_owned(),
            ),
        },
    ];

    // Saved C019 order (reverse of channel open order):
    // 0: BPQ_Travel_C (LocationReached = true)
    // 1: Construction_C -> [BPQ_Travel_C, Medical_Outpost_C]
    // 2: Kitchen_Bench_C (QuestComplete = false)
    // 3: Kitchen_Stove_C (QuestComplete = false)
    // 4: Medical_Outpost_C -> [Kitchen_Bench_C, Kitchen_Stove_C, Beds_C, Herbalism_Bench_C, Crop_Plot_C]
    // 5: Beds_C (QuestComplete = true)
    // 6: Herbalism_Bench_C (QuestComplete = true)
    // 7: Crop_Plot_C (QuestComplete = true)
    let saved_quests = vec![
        authored_quest(
            101,
            "OLY_Riverlands_Construction_Head_To_Location",
            "BPQ_Travel_C",
            Vec::new(),
            vec![var("LocationReached", true, 0.0, 0)],
        ),
        authored_quest(
            102,
            "OLY_Riverlands_Construction_0",
            "BPQ_OLY_Riverlands_Construction_C",
            vec![
                (
                    "OLY_Riverlands_Construction_Head_To_Location",
                    "BPQ_Travel_C",
                    101,
                ),
                (
                    "OLY_Riverlands_Construction_Medical_Outpost",
                    "BPQ_OLY_Riverlands_Construction_Medical_Outpost_C",
                    105,
                ),
            ],
            Vec::new(),
        ),
        authored_quest(
            103,
            "OLY_Riverlands_Construction_Kitchen_Bench",
            "BPQ_OLY_Riverlands_Construction_Kitchen_Bench_C",
            Vec::new(),
            vec![
                var("Count", false, 0.0, 0),
                var("QuestComplete", false, 0.0, 0),
            ],
        ),
        authored_quest(
            104,
            "OLY_Riverlands_Construction_Kitchen_Stove",
            "BPQ_OLY_Riverlands_Construction_Kitchen_Stove_C",
            Vec::new(),
            vec![
                var("Count", false, 0.0, 0),
                var("QuestComplete", false, 0.0, 0),
            ],
        ),
        authored_quest(
            105,
            "OLY_Riverlands_Construction_Medical_Outpost",
            "BPQ_OLY_Riverlands_Construction_Medical_Outpost_C",
            vec![
                (
                    "OLY_Riverlands_Construction_Kitchen_Bench",
                    "BPQ_OLY_Riverlands_Construction_Kitchen_Bench_C",
                    103,
                ),
                (
                    "OLY_Riverlands_Construction_Kitchen_Stove",
                    "BPQ_OLY_Riverlands_Construction_Kitchen_Stove_C",
                    104,
                ),
                (
                    "OLY_Riverlands_Construction_Beds",
                    "BPQ_OLY_Riverlands_Construction_Beds_C",
                    106,
                ),
                (
                    "OLY_Riverlands_Construction_Herbalism_Bench",
                    "BPQ_OLY_Riverlands_Construction_Herbalism_Bench_C",
                    107,
                ),
                (
                    "OLY_Riverlands_Construction_Crop_Plot",
                    "BPQ_OLY_Riverlands_Construction_Crop_Plot_C",
                    108,
                ),
            ],
            Vec::new(),
        ),
        authored_quest(
            106,
            "OLY_Riverlands_Construction_Beds",
            "BPQ_OLY_Riverlands_Construction_Beds_C",
            Vec::new(),
            vec![
                var("Count", false, 0.0, 1),
                var("QuestComplete", true, 0.0, 0),
            ],
        ),
        authored_quest(
            107,
            "OLY_Riverlands_Construction_Herbalism_Bench",
            "BPQ_OLY_Riverlands_Construction_Herbalism_Bench_C",
            Vec::new(),
            vec![
                var("Count", false, 0.0, 1),
                var("QuestComplete", true, 0.0, 0),
            ],
        ),
        authored_quest(
            108,
            "OLY_Riverlands_Construction_Crop_Plot",
            "BPQ_OLY_Riverlands_Construction_Crop_Plot_C",
            Vec::new(),
            vec![
                var("Count", false, 0.0, 2),
                var("QuestComplete", true, 0.0, 0),
            ],
        ),
    ];

    let manager = SavedQuestManager {
        faction_mission: units("OLY_Riverlands_Construction"),
        initial_quest: Some(102),
        initial_quest_class: units("BPQ_OLY_Riverlands_Construction_C"),
        mission_complete: false,
        run_quests: true,
        dynamic_quest_difficulty: 2,
        dynamic_quest_delay: 0.0,
        persistent_actors: Vec::new(),
        persistent_characters: Vec::new(),
    };

    let stitched = stitch_saved_quests(&saved_quests, Some(&manager), &[], &rows).unwrap();
    assert_eq!(stitched.quests.len(), 8);
    // Medical_Outpost_C (`quests[3]`) has only the two incomplete children in ActiveQuests.
    assert_eq!(
        stitched.quests[3].active_quests,
        vec![ActorKey::Saved(103), ActorKey::Saved(104)]
    );
    // Construction_C (`quests[6]`) excludes completed BPQ_Travel_C (`101`) from ActiveQuests.
    assert_eq!(stitched.quests[6].active_quests, vec![ActorKey::Saved(105)]);

    let profile = ReleaseProfile::release_3_0_31();
    let map = "/Game/Maps/Terrain_016_OLY/Terrain_016";
    let base = profile.quest_open_base_plan(map).unwrap().unwrap();
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let layout = quest_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
        profile.tags.tag_encoding(),
    );

    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    cache.export_static(&mut exports, &base.level).unwrap();
    // Pre-export /Script/Icarus at static GUID 23 as in M-707.
    burn_static_to(&mut cache, 23);
    cache
        .export_static(
            &mut exports,
            &[PathSegment::new("/Script/Icarus", 0, false)],
        )
        .unwrap();
    // Reserve AQuestManager's actor GUID 74 at GameState open, as in M-707.
    while cache.assign_dynamic().unwrap() + 2 < 74 {}
    assert_eq!(cache.actor_guid(ManagerSlot::Quest.key()).unwrap(), 74);
    exports.commit_bunch(1);
    exports.received_ack(1);

    for (quest, captured) in stitched.quests.iter().zip(&CAPTURED_QUEST_EXPORTS) {
        if let Some(next_stat) = captured.next_static {
            burn_static_to(&mut cache, next_stat);
        }
        if let Some(next_dyn) = captured.next_dynamic {
            while cache.assign_dynamic().unwrap() + 2 < next_dyn {}
        }
        let plan = base.quest_plan(quest).unwrap();
        let open = compose_quest_open(
            &mut cache,
            &mut exports,
            &plan,
            captured.channel,
            ByteOrder::Little,
        )
        .unwrap();
        assert_eq!(
            cache.actor_guid(quest.key).unwrap(),
            captured.expected_actor_guid
        );
        let open_exports = open.exports.expect("quest open exports");
        assert_eq!(
            open_exports.bit_len(),
            captured.exports_bits,
            "channel {}",
            captured.channel
        );
        assert_eq!(
            sha256(&open_exports),
            captured.exports_sha256,
            "channel {}",
            captured.channel
        );
        exports.commit_bunch(captured.channel);
        exports.received_ack(captured.channel);

        // Verify QuestRow is encoded and handle 26 (`bQuestComplete`) is never emitted,
        // even on completed quests.
        let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec).unwrap();
        let decoded: BTreeMap<u32, RepValue> = read_rep_properties(
            &mut body.blocks[0].payload.reader(),
            &layout,
            &RepLayoutCodec {
                actor: codec,
                checksums: false,
                limits: RepLayoutLimits::default(),
            },
        )
        .unwrap()
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
        assert!(
            decoded.contains_key(&quest_actor_handle::QUEST_ROW),
            "channel {}",
            captured.channel
        );
        assert!(!decoded.contains_key(&26), "channel {}", captured.channel);
    }

    // Channel 126 (`AQuestManager`) exports match M-707 bit-for-bit and reuse
    // Construction_C's assigned NetGUID `632` for `InitialQuest`!
    burn_static_to(&mut cache, 821);
    while cache.assign_dynamic().unwrap() + 2 < 3086 {}
    let qm_plan = profile
        .manager_open_plan(ManagerSlot::Quest, map)
        .unwrap()
        .unwrap()
        .with_quest_state(stitched.quest_manager.as_ref().unwrap(), 14)
        .unwrap();
    let qm_open = compose_manager_open(
        &mut cache,
        &mut exports,
        &qm_plan,
        ManagerSlot::Quest.key(),
        126,
        ByteOrder::Little,
    )
    .unwrap();
    let qm_exports = qm_open.exports.expect("QuestManager exports");
    assert_eq!(qm_exports.bit_len(), 729);
    assert_eq!(
        sha256(&qm_exports),
        "e77873b5b3ea2ee4603d2981edd454894dc535cab7a1a4aed8cd5c796c2b358a"
    );
    let qm_body = read_actor_bunch_body(&mut qm_open.bunch.payload.reader(), true, &codec).unwrap();
    let qm_values: BTreeMap<u32, RepValue> = read_rep_properties(
        &mut qm_body.blocks[0].payload.reader(),
        &icarus_bevy::experimental::rep_layout::quest_manager_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
            profile.tags.tag_encoding(),
        ),
        &RepLayoutCodec {
            actor: codec,
            checksums: false,
            limits: RepLayoutLimits::default(),
        },
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|entry| (entry.handle, entry.value))
    .collect();
    let RepValue::Object(initial_ref) = &qm_values[&quest_handle::INITIAL_QUEST] else {
        panic!("InitialQuest object reference");
    };
    assert_eq!(initial_ref.guid, 632);
}

#[test]
fn completed_mission_with_zero_c019_records_clears_stale_initial_quest() {
    let rows = vec![QuestTableRow {
        name: "Quest_Root".to_owned(),
        class_path: Some("/Game/BP/Quests/Common/BPQ_Travel.BPQ_Travel_C".to_owned()),
    }];
    let stale_manager = SavedQuestManager {
        faction_mission: units("Mission_A"),
        initial_quest: Some(42_424),
        initial_quest_class: units("BPQ_Travel_C"),
        mission_complete: true,
        run_quests: false,
        dynamic_quest_difficulty: 1,
        dynamic_quest_delay: 0.0,
        persistent_actors: Vec::new(),
        persistent_characters: Vec::new(),
    };
    let stitched = stitch_saved_quests(&[], Some(&stale_manager), &[], &rows).unwrap();
    assert!(stitched.quests.is_empty());
    assert_eq!(stitched.quest_manager.unwrap().initial_quest, None);
}

#[test]
fn stitch_saved_quests_fails_closed_without_exposing_saved_values() {
    let rows = vec![
        QuestTableRow {
            name: "Row_NetProps".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Arctic/LongtermSurvey/BPQ_Arctic_LongtermSurvey.BPQ_Arctic_LongtermSurvey_C".to_owned(),
            ),
        },
        QuestTableRow {
            name: "Row_Impure".to_owned(),
            class_path: Some(
                "/Game/BP/Quests/Common/BPQ_Common_Clear_Area.BPQ_Common_Clear_Area_C".to_owned(),
            ),
        },
        QuestTableRow {
            name: "Row_Travel".to_owned(),
            class_path: Some("/Game/BP/Quests/Common/BPQ_Travel.BPQ_Travel_C".to_owned()),
        },
    ];
    let secret = "SECRET_QUEST_TOKEN";

    // 1. Class that adds Blueprint CPF_Net properties fails closed.
    let net_prop_quest = SavedQuestActor {
        relevant_actors: vec![RelevantQuestActor {
            key: units(secret),
            icarus_uid: 77_777,
        }],
        ..authored_quest(
            77_777,
            "Row_NetProps",
            "BPQ_Arctic_LongtermSurvey_C",
            Vec::new(),
            Vec::new(),
        )
    };
    let debug_text = format!("{net_prop_quest:?}");
    assert!(!debug_text.contains(secret) && !debug_text.contains("77777"));
    let err = format!(
        "{:#}",
        stitch_saved_quests(&[net_prop_quest], None, &[], &rows).unwrap_err()
    );
    assert!(err.contains("adds replicated Blueprint properties"));
    assert!(!err.contains(secret) && !err.contains("77777"));

    // 2. Parent with an Impure child Check() fails closed when quests are running.
    let impure_child = authored_quest(
        801,
        "Row_Impure",
        "BPQ_Common_Clear_Area_C",
        Vec::new(),
        Vec::new(),
    );
    let parent = authored_quest(
        802,
        "Row_Travel",
        "BPQ_Travel_C",
        vec![("Row_Impure", "BPQ_Common_Clear_Area_C", 801)],
        Vec::new(),
    );
    let err = format!(
        "{:#}",
        stitch_saved_quests(&[impure_child, parent], None, &[], &rows).unwrap_err()
    );
    assert!(err.contains("Check rule"));
    assert!(!err.contains("801") && !err.contains("802"));

    // 3. Cycle in SubQuestRecords fails closed.
    let cycle_a = authored_quest(
        901,
        "Row_Travel",
        "BPQ_Travel_C",
        vec![("Row_Travel", "BPQ_Travel_C", 902)],
        Vec::new(),
    );
    let cycle_b = authored_quest(
        902,
        "Row_Travel",
        "BPQ_Travel_C",
        vec![("Row_Travel", "BPQ_Travel_C", 901)],
        Vec::new(),
    );
    let err = format!(
        "{:#}",
        stitch_saved_quests(&[cycle_a, cycle_b], None, &[], &rows).unwrap_err()
    );
    assert!(err.contains("cycle"));

    // 4. Base-recorder extras or non-identity rotation fail closed.
    let mut extras = authored_quest(903, "Row_Travel", "BPQ_Travel_C", Vec::new(), Vec::new());
    extras.base_extras = 1;
    assert!(stitch_saved_quests(&[extras], None, &[], &rows).is_err());

    let mut rotated = authored_quest(904, "Row_Travel", "BPQ_Travel_C", Vec::new(), Vec::new());
    rotated.rotation = [0.0, 0.0, 0.6, 0.8];
    assert!(stitch_saved_quests(&[rotated], None, &[], &rows).is_err());
}

// --- End-to-end authored binary C019 + C018 + data.pak integration test ---

fn put_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_fstr(out: &mut Vec<u8>, text: &str) {
    if text.is_empty() {
        put_i32(out, 0);
    } else {
        put_i32(out, i32::try_from(text.len() + 1).unwrap());
        out.extend_from_slice(text.as_bytes());
        out.push(0);
    }
}

fn none_tag(out: &mut Vec<u8>) {
    put_fstr(out, "None");
}

fn prop_int(name: &str, value: i32) -> Vec<u8> {
    let mut out = Vec::new();
    put_fstr(&mut out, name);
    put_fstr(&mut out, "IntProperty");
    put_u64(&mut out, 4);
    out.push(0);
    put_i32(&mut out, value);
    out
}

fn prop_bool(name: &str, value: bool) -> Vec<u8> {
    let mut out = Vec::new();
    put_fstr(&mut out, name);
    put_fstr(&mut out, "BoolProperty");
    put_u64(&mut out, 0);
    out.push(u8::from(value));
    out.push(0);
    out
}

fn prop_float(name: &str, value: f32) -> Vec<u8> {
    let mut out = Vec::new();
    put_fstr(&mut out, name);
    put_fstr(&mut out, "FloatProperty");
    put_u64(&mut out, 4);
    out.push(0);
    out.extend_from_slice(&value.to_le_bytes());
    out
}

fn prop_str_or_name(name: &str, kind: &str, value: &str) -> Vec<u8> {
    let mut val = Vec::new();
    put_fstr(&mut val, value);
    let mut out = Vec::new();
    put_fstr(&mut out, name);
    put_fstr(&mut out, kind);
    put_u64(&mut out, val.len() as u64);
    out.push(0);
    out.extend_from_slice(&val);
    out
}

fn prop_struct(name: &str, struct_name: &str, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_fstr(&mut out, name);
    put_fstr(&mut out, "StructProperty");
    put_u64(&mut out, body.len() as u64);
    put_fstr(&mut out, struct_name);
    out.extend_from_slice(&[0_u8; 17]);
    out.extend_from_slice(body);
    out
}

fn prop_struct_array(
    name: &str,
    elem_name: &str,
    struct_name: &str,
    elements: &[Vec<u8>],
) -> Vec<u8> {
    let elems_bytes: usize = elements.iter().map(Vec::len).sum();
    let mut proto = Vec::new();
    put_fstr(&mut proto, elem_name);
    put_fstr(&mut proto, "StructProperty");
    put_u64(&mut proto, elems_bytes as u64);
    put_fstr(&mut proto, struct_name);
    proto.extend_from_slice(&[0_u8; 17]);
    for elem in elements {
        proto.extend_from_slice(elem);
    }

    let mut body = Vec::new();
    put_i32(&mut body, i32::try_from(elements.len()).unwrap());
    body.extend_from_slice(&proto);

    let mut out = Vec::new();
    put_fstr(&mut out, name);
    put_fstr(&mut out, "ArrayProperty");
    put_u64(&mut out, body.len() as u64);
    put_fstr(&mut out, "StructProperty");
    out.push(0);
    out.extend_from_slice(&body);
    out
}

fn prop_byte_array(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    put_i32(&mut body, i32::try_from(bytes.len()).unwrap());
    body.extend_from_slice(bytes);
    let mut out = Vec::new();
    put_fstr(&mut out, name);
    put_fstr(&mut out, "ArrayProperty");
    put_u64(&mut out, body.len() as u64);
    put_fstr(&mut out, "ByteProperty");
    out.push(0);
    out.extend_from_slice(&body);
    out
}

fn recorder_blob(component_class: &str, component_fields: &[u8]) -> Vec<u8> {
    let mut component = component_fields.to_vec();
    none_tag(&mut component);
    component.extend_from_slice(&[0_u8; 4]);

    let mut blob = Vec::new();
    blob.extend_from_slice(&prop_str_or_name(
        "ComponentClassName",
        "StrProperty",
        component_class,
    ));
    blob.extend_from_slice(&prop_byte_array("BinaryData", &component));
    none_tag(&mut blob);
    blob
}

fn identity_transform_prop() -> Vec<u8> {
    let mut rot = [0_u8; 16];
    rot[12..16].copy_from_slice(&1.0_f32.to_le_bytes());
    let mut scale = [0_u8; 12];
    for chunk in scale.as_chunks_mut::<4>().0 {
        chunk.copy_from_slice(&1.0_f32.to_le_bytes());
    }
    let mut body = Vec::new();
    body.extend_from_slice(&prop_struct("Rotation", "Quat", &rot));
    body.extend_from_slice(&prop_struct("Translation", "Vector", &[0_u8; 12]));
    body.extend_from_slice(&prop_struct("Scale3D", "Vector", &scale));
    none_tag(&mut body);
    prop_struct("ActorTransform", "Transform", &body)
}

fn c019_blob(
    uid: i32,
    quest_row: &str,
    class_name: &str,
    subquests: &[(&str, &str, i32)],
    location_reached: Option<bool>,
) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&prop_int("IcarusActorGUID", uid));
    body.extend_from_slice(&prop_str_or_name(
        "ActorClassName",
        "NameProperty",
        class_name,
    ));
    body.extend_from_slice(&identity_transform_prop());
    body.extend_from_slice(&prop_str_or_name("QuestName", "NameProperty", quest_row));
    if !subquests.is_empty() {
        let elems: Vec<Vec<u8>> = subquests
            .iter()
            .map(|(row, cls, child_uid)| {
                let mut el = Vec::new();
                el.extend_from_slice(&prop_str_or_name("SubQuestRowName", "NameProperty", row));
                el.extend_from_slice(&prop_str_or_name("SubQuestName", "StrProperty", cls));
                el.extend_from_slice(&prop_int("RelevantActorIcarusUID", *child_uid));
                none_tag(&mut el);
                el
            })
            .collect();
        body.extend_from_slice(&prop_struct_array(
            "SubQuestRecords",
            "SubQuestRecords",
            "SubQuestSaveRecord",
            &elems,
        ));
    }
    if let Some(done) = location_reached {
        let mut el = Vec::new();
        el.extend_from_slice(&prop_str_or_name(
            "VariableName",
            "StrProperty",
            "LocationReached",
        ));
        el.extend_from_slice(&prop_bool("bVariable", done));
        el.extend_from_slice(&prop_float("fVariable", 0.0));
        el.extend_from_slice(&prop_int("iVariable", 0));
        none_tag(&mut el);
        body.extend_from_slice(&prop_struct_array(
            "VariableRecords",
            "VariableRecords",
            "QuestVariableRecord",
            &[el],
        ));
    }
    recorder_blob("/Script/Icarus.IcarusQuestRecorderComponent", &body)
}

fn c018_blob(mission: &str, root_uid: i32, root_class: &str) -> Vec<u8> {
    let mut initial = Vec::new();
    initial.extend_from_slice(&prop_str_or_name(
        "QuestActorName",
        "StrProperty",
        root_class,
    ));
    initial.extend_from_slice(&prop_int("RelevantActorIcarusUID", root_uid));
    none_tag(&mut initial);

    let mut fields = Vec::new();
    fields.extend_from_slice(&prop_str_or_name(
        "FactionMissionName",
        "NameProperty",
        mission,
    ));
    fields.extend_from_slice(&prop_struct(
        "InitialQuestRecord",
        "InitialQuestRecord",
        &initial,
    ));
    fields.extend_from_slice(&prop_bool("bMissionComplete", false));
    fields.extend_from_slice(&prop_bool("bRunQuests", true));
    fields.extend_from_slice(&prop_int("DynamicQuestDifficulty", 1));
    fields.extend_from_slice(&prop_float("DynamicQuestDelay", 0.0));
    fields.extend_from_slice(&prop_int("IcarusActorGUID", 50));
    recorder_blob(
        "/Script/Icarus.IcarusQuestManagerRecorderComponent",
        &fields,
    )
}

fn prospect_payload(recorders: &[Vec<u8>]) -> Vec<u8> {
    let mut info = Vec::new();
    info.extend_from_slice(&prop_str_or_name("ProspectID", "StrProperty", "p1"));
    info.extend_from_slice(&prop_str_or_name("ProspectDTKey", "StrProperty", "Row1"));
    info.extend_from_slice(&prop_str_or_name(
        "FactionMissionDTKey",
        "StrProperty",
        "Mission_One",
    ));
    info.extend_from_slice(&prop_str_or_name("LobbyName", "StrProperty", "L"));
    none_tag(&mut info);

    let mut root = Vec::new();
    root.extend_from_slice(&prop_int("Version", 2));
    root.extend_from_slice(&prop_struct("ProspectInfo", "ProspectInfo", &info));
    root.extend_from_slice(&prop_struct_array(
        "StateRecorderBlobs",
        "StateRecorderBlobs",
        "StateRecorderBlob",
        recorders,
    ));
    none_tag(&mut root);
    root
}

fn synthetic_quest_pak(dir: &std::path::Path, quest_rows_json: &str) -> std::path::PathBuf {
    let pak = dir.join("quest_test.pak");
    let bytes = pak_writer::official_pak(&[
        (
            "Sessions/D_SessionFlags.json",
            br#"{"Rows":[{"Name":"Flag_A"}]}"#.to_vec(),
        ),
        (
            "Factions/D_FactionMissions.json",
            br#"{"Rows":[{"Name":"Mission_One","bUseOpenWorldRetryTimeout":false}]}"#.to_vec(),
        ),
        ("Quests/D_Quests.json", quest_rows_json.as_bytes().to_vec()),
    ]);
    std::fs::write(&pak, bytes).unwrap();
    pak
}

/// These authored quest scenarios use the qualified 3.0.32 class metadata,
/// independently of the repository's moving release pin.
fn qualified_quest_release(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("quest_release.toml");
    std::fs::write(
        &path,
        "[release]\napp_id = 2089300\nbuild_id = 25956137\nproject_version = \"3.0.32.158743-rel-DangerousHorizons\"\nnetwork_checksum = 243431684\n",
    )
    .unwrap();
    path
}

#[test]
fn apply_saved_game_state_opens_quests_before_quest_manager_and_skips_closed_on_failure() {
    let temp = tempfile::tempdir().unwrap();
    let pak_path = synthetic_quest_pak(
        temp.path(),
        r#"{"Rows":[
            {"Name":"Root_Quest","Class":"/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction.BPQ_OLY_Riverlands_Construction_C"},
            {"Name":"Child_Done","Class":"/Game/BP/Quests/Common/BPQ_Travel.BPQ_Travel_C"},
            {"Name":"Child_Active","Class":"/Game/BP/Quests/Common/BPQ_Travel.BPQ_Travel_C"},
            {"Name":"Unsupported_Scan","Class":"/Game/BP/Quests/Arctic/LongtermSurvey/BPQ_Arctic_LongtermSurvey.BPQ_Arctic_LongtermSurvey_C"}
        ]}"#,
    );

    let ok_payload = prospect_payload(&[
        c018_blob("Mission_One", 300, "BPQ_OLY_Riverlands_Construction_C"),
        c019_blob(301, "Child_Done", "BPQ_Travel_C", &[], Some(true)),
        c019_blob(302, "Child_Active", "BPQ_Travel_C", &[], Some(false)),
        c019_blob(
            300,
            "Root_Quest",
            "BPQ_OLY_Riverlands_Construction_C",
            &[
                ("Child_Done", "BPQ_Travel_C", 301),
                ("Child_Active", "BPQ_Travel_C", 302),
            ],
            None,
        ),
    ]);
    let ok_section =
        PreservedSection::from_inner_payload(&ok_payload, PreserveLimits::default()).unwrap();

    let net_cfg = NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: Some(qualified_quest_release(temp.path())),
        authored_gameplay: Some(pak_writer::authored_gameplay(&pak_path)),
        data_pak: Some(pak_path.clone()),
        ..NetworkConfig::default()
    };
    let mut bound = bind(&net_cfg).unwrap().unwrap();
    let applied = apply_saved_game_state(
        &mut bound.lab,
        &ok_section,
        bound.tables.as_mut(),
        std::mem::take(&mut bound.saved_managers),
    )
    .unwrap();
    // Baseline managers occupy 6, 7, 8; the 3 stitched C019 quests open on
    // 9, 10, 11 BEFORE AQuestManager opens on 12!
    assert_eq!(applied.quest_channels, vec![9, 10, 11]);
    assert!(applied.skipped_quests.is_none());
    assert_eq!(applied.saved_managers, vec![(ManagerSlot::Quest, 12)]);
    assert!(applied.skipped_managers.is_empty());

    // Without tables, non-empty C019 fails closed without opening quests or QuestManager.
    let saved_no_tables = saved_game_state(&ok_section, None).unwrap();
    assert!(saved_no_tables.quests.is_empty());
    assert_eq!(saved_no_tables.skipped_quests.as_ref().unwrap().0, 3);
    assert!(saved_no_tables.quest_manager.is_none());

    // With an unsupported adds_net_props quest class, both C019 quests and
    // ManagerSlot::Quest skip cleanly without aborting apply_saved_game_state.
    let bad_payload = prospect_payload(&[
        c018_blob("Mission_One", 400, "BPQ_Arctic_LongtermSurvey_C"),
        c019_blob(
            400,
            "Unsupported_Scan",
            "BPQ_Arctic_LongtermSurvey_C",
            &[],
            None,
        ),
    ]);
    let bad_section =
        PreservedSection::from_inner_payload(&bad_payload, PreserveLimits::default()).unwrap();
    let mut bound_bad = bind(&net_cfg).unwrap().unwrap();
    let applied_bad = apply_saved_game_state(
        &mut bound_bad.lab,
        &bad_section,
        bound_bad.tables.as_mut(),
        std::mem::take(&mut bound_bad.saved_managers),
    )
    .unwrap();
    assert!(applied_bad.quest_channels.is_empty());
    assert_eq!(applied_bad.skipped_quests.as_ref().unwrap().0, 1);
    assert!(applied_bad.saved_managers.is_empty());
    assert_eq!(
        applied_bad
            .skipped_managers
            .iter()
            .map(|(slot, _)| *slot)
            .collect::<Vec<_>>(),
        vec![ManagerSlot::Quest]
    );

    // Completed mission (C018 present, zero C019 records) opens QuestManager
    // with InitialQuest cleared to None.
    let done_payload = prospect_payload(&[c018_blob("Mission_One", 999, "BPQ_Travel_C")]);
    let done_section =
        PreservedSection::from_inner_payload(&done_payload, PreserveLimits::default()).unwrap();
    let mut tables = DataTables::open(&pak_path).unwrap();
    let done_state = saved_game_state(&done_section, Some(&mut tables)).unwrap();
    assert!(done_state.quests.is_empty());
    assert!(done_state.skipped_quests.is_none());
    assert_eq!(done_state.quest_manager.unwrap().0.initial_quest, None);
}

fn c024_blob() -> Vec<u8> {
    let mut entry = Vec::new();
    entry.extend_from_slice(&prop_str_or_name("UserID", "StrProperty", "user-1"));
    entry.extend_from_slice(&prop_int("ChrSlot", 0));
    entry.extend_from_slice(&prop_str_or_name(
        "CachedCharacterName",
        "StrProperty",
        "Runner",
    ));
    none_tag(&mut entry);

    let mut fields = Vec::new();
    fields.extend_from_slice(&prop_struct_array(
        "SavedHistoryData",
        "SavedHistoryData",
        "PlayerHistoryEntryRecord",
        &[entry],
    ));
    fields.extend_from_slice(&prop_int("IcarusActorGUID", 78));
    recorder_blob("/Script/Icarus.PlayerHistoryRecorderComponent", &fields)
}

#[test]
fn quest_opens_exceed_legacy_32_channel_bound_under_default_256_limit() {
    assert_eq!(DEFAULT_MAX_CHANNELS, 256);
    let temp = tempfile::tempdir().unwrap();
    let pak_path = synthetic_quest_pak(
        temp.path(),
        r#"{"Rows":[
            {"Name":"Root_Quest","Class":"/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction.BPQ_OLY_Riverlands_Construction_C"},
            {"Name":"Step_Travel","Class":"/Game/BP/Quests/Common/BPQ_Travel.BPQ_Travel_C"}
        ]}"#,
    );

    // 1 root quest + 29 subquests = 30 C019 quests on channels 9..=38, plus
    // AQuestManager on channel 39 (exceeding the legacy 32-channel cap).
    let mut recorders = vec![c018_blob(
        "Mission_One",
        1_000,
        "BPQ_OLY_Riverlands_Construction_C",
    )];
    let mut root_links = Vec::new();
    for step in 1..30 {
        let uid = 1_000 + step;
        recorders.push(c019_blob(
            uid,
            "Step_Travel",
            "BPQ_Travel_C",
            &[],
            Some(false),
        ));
        root_links.push(("Step_Travel", "BPQ_Travel_C", uid));
    }
    recorders.push(c019_blob(
        1_000,
        "Root_Quest",
        "BPQ_OLY_Riverlands_Construction_C",
        &root_links,
        None,
    ));

    let payload = prospect_payload(&recorders);
    let section =
        PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap();
    let net_cfg = NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: Some(qualified_quest_release(temp.path())),
        authored_gameplay: Some(pak_writer::authored_gameplay(&pak_path)),
        data_pak: Some(pak_path),
        ..NetworkConfig::default()
    };
    let mut bound = bind(&net_cfg).unwrap().unwrap();
    let applied = apply_saved_game_state(
        &mut bound.lab,
        &section,
        bound.tables.as_mut(),
        std::mem::take(&mut bound.saved_managers),
    )
    .unwrap();

    let expected_channels: Vec<u32> = (9..=38).collect();
    assert_eq!(applied.quest_channels, expected_channels);
    assert!(applied.skipped_quests.is_none());
    assert_eq!(applied.saved_managers, vec![(ManagerSlot::Quest, 39)]);
    assert!(applied.skipped_managers.is_empty());
    assert_eq!(bound.lab.quest_open_count(), 30);
    assert_eq!(bound.lab.manager_open_count(), 4);
}

#[test]
fn channel_exhaustion_skips_quests_and_quest_manager_atomically_without_dropping_other_managers() {
    let temp = tempfile::tempdir().unwrap();
    let pak_path = synthetic_quest_pak(
        temp.path(),
        r#"{"Rows":[
            {"Name":"Root_Quest","Class":"/Game/BP/Quests/Olympus/Riverlands/Construction/BPQ_OLY_Riverlands_Construction.BPQ_OLY_Riverlands_Construction_C"},
            {"Name":"Step_Travel","Class":"/Game/BP/Quests/Common/BPQ_Travel.BPQ_Travel_C"}
        ]}"#,
    );
    let net_cfg = NetworkConfig {
        enabled: true,
        port: 0,
        release_profile: Some(qualified_quest_release(temp.path())),
        authored_gameplay: Some(pak_writer::authored_gameplay(&pak_path)),
        data_pak: Some(pak_path),
        ..NetworkConfig::default()
    };

    // Baseline managers occupy 6..=8 (`first_channel = 9`) and `DEFAULT_MAX_CHANNELS = 256`:
    // - 247 quests occupy 9..=255, leaving no channel (256) for `ManagerSlot::Quest`.
    // - 250 quests overflow partway through the quest batch itself at channel 256.
    // Both cases must leave zero quests installed on `lab`, record `skipped_quests`
    // and `ManagerSlot::Quest` in `skipped_managers`, and still open `PlayerHistory`
    // cleanly on channel 9 without aborting `apply_saved_game_state`.
    for total_quests in [247_usize, 250_usize] {
        let mut recorders = vec![
            c018_blob("Mission_One", 2_000, "BPQ_OLY_Riverlands_Construction_C"),
            c024_blob(),
        ];
        let mut root_links = Vec::new();
        for step in 1..total_quests {
            let uid = 2_000 + i32::try_from(step).unwrap();
            recorders.push(c019_blob(
                uid,
                "Step_Travel",
                "BPQ_Travel_C",
                &[],
                Some(false),
            ));
            root_links.push(("Step_Travel", "BPQ_Travel_C", uid));
        }
        recorders.push(c019_blob(
            2_000,
            "Root_Quest",
            "BPQ_OLY_Riverlands_Construction_C",
            &root_links,
            None,
        ));

        let payload = prospect_payload(&recorders);
        let section =
            PreservedSection::from_inner_payload(&payload, PreserveLimits::default()).unwrap();
        let mut bound = bind(&net_cfg).unwrap().unwrap();
        let applied = apply_saved_game_state(
            &mut bound.lab,
            &section,
            bound.tables.as_mut(),
            std::mem::take(&mut bound.saved_managers),
        )
        .unwrap();

        assert!(applied.quest_channels.is_empty(), "{total_quests}");
        assert_eq!(bound.lab.quest_open_count(), 0, "{total_quests}");
        assert_eq!(
            applied.skipped_quests.as_ref().map(|(count, _)| *count),
            Some(total_quests),
            "{total_quests}"
        );
        assert_eq!(
            applied.saved_managers,
            vec![(ManagerSlot::PlayerHistory, 9)],
            "{total_quests}"
        );
        assert_eq!(
            applied
                .skipped_managers
                .iter()
                .map(|(slot, _)| *slot)
                .collect::<Vec<_>>(),
            vec![ManagerSlot::Quest],
            "{total_quests}"
        );
        assert_eq!(bound.lab.manager_open_count(), 4, "{total_quests}");
    }
}
