use icarus_bevy::experimental::component::MAX_COMPONENT_BYTES;
use icarus_bevy::experimental::prospect_info::{AssociatedMember, CustomGameSetting, ProspectInfo};
use icarus_bevy::experimental::recorders::{
    MAX_RECORDER_FIELDS, MAX_RECORDERS, RecorderArray, SavedPlayerState, SavedStomachContent,
    StagedLegacyDisposition, StagedProspectSaveState, StagedRecorderDecision, StagedRootEnumField,
    StagedRootEnumValue, StagedRootMigration, StagedRootPresence, StagedRootValues,
    find_saved_player_character, lookup_recorder_ctor_defaults, read_recorders,
    saved_player_characters, stage_prospect_save_state, stage_prospect_save_state_with_decisions,
    stage_prospect_save_state_with_root_values,
};
use icarus_bevy::experimental::restore_rules::{
    LobbyPrivacy, non_blob_recorder_ctor_defaults, recorder_ctor_defaults,
};
use icarus_bevy::experimental::tagged::{MAX_INPUT_BYTES, RootTag, TagKind, TagProfile, read_root};

const END: &[u8] = b"\x05\0\0\0None\0";
const PROFILE: TagProfile = TagProfile::MemoryArchive522;
// Authored payloads deliberately contain a terminal-looking byte sequence and
// non-text bytes. They must never be parsed as additional properties.
const BODY: &[u8] = b"\xff\0\x05\0\0\0None\0private-body-marker";
const CLASS: &[u8] = b"\x10\0\0\0private-class-X\0";

// Independent manual fixture assembly, never using the production string encoder.
fn string(text: &str) -> Vec<u8> {
    assert!(text.is_ascii());
    [
        i32::try_from(text.len() + 1)
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        text.as_bytes(),
        &[0],
    ]
    .concat()
}

fn tag(name: &str, kind: &str, index: i32, extras: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        string(name).as_slice(),
        string(kind).as_slice(),
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &index.to_le_bytes(),
        extras,
        payload,
    ]
    .concat()
}

fn class(payload: &[u8]) -> Vec<u8> {
    tag("ComponentClassName", "StrProperty", 0, &[0], payload)
}

fn binary(payload: &[u8]) -> Vec<u8> {
    tag(
        "BinaryData",
        "ArrayProperty",
        0,
        &[string("ByteProperty"), vec![0]].concat(),
        payload,
    )
}

fn binary_value() -> Vec<u8> {
    [
        i32::try_from(BODY.len()).unwrap().to_le_bytes().as_slice(),
        BODY,
    ]
    .concat()
}

fn element() -> Vec<u8> {
    // 15 ASCII bytes plus terminator: the count includes that final unit.
    [
        class(b"\x10\0\0\0private-class-X\0").as_slice(),
        binary(&binary_value()).as_slice(),
        END,
    ]
    .concat()
}

fn root_with_shared(count: i32, shared: &[u8]) -> Vec<u8> {
    let payload = [&count.to_le_bytes(), shared].concat();
    [
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            0,
            &[string("StructProperty"), vec![0]].concat(),
            &payload,
        ),
        END.to_vec(),
    ]
    .concat()
}

fn shared(body: &[u8]) -> Vec<u8> {
    tag(
        "StateRecorderBlobs",
        "StructProperty",
        0,
        &[string("StateRecorderBlob"), vec![0; 16], vec![0]].concat(),
        body,
    )
}

fn fixture(count: i32, body: &[u8]) -> Vec<u8> {
    root_with_shared(count, &shared(body))
}

fn read(bytes: &[u8]) -> anyhow::Result<RecorderArray<'_>> {
    let roots = read_root(bytes, PROFILE)?;
    read_recorders(&roots.tags, PROFILE)
}

#[test]
fn known_and_unknown_fields_preserve_exact_borrowed_spans_and_summary_privacy() {
    let unknown = tag(
        "private-unknown-name",
        "IntProperty",
        7,
        &[1; 17],
        b"opaque-no-int-width-check",
    );
    let element = [
        class(CLASS),
        unknown.clone(),
        binary(&binary_value()),
        END.to_vec(),
    ]
    .concat();
    let bytes = fixture(2, &element.repeat(2));
    let parsed = read(&bytes).unwrap();
    assert_eq!(parsed.records().len(), 2);
    assert_eq!(parsed.records()[0].record, element);
    assert_eq!(parsed.records()[0].fields()[1].record, unknown);
    assert_eq!(parsed.records()[0].binary_data, BODY);
    assert_eq!(parsed.records()[0].class_value, CLASS);
    assert_eq!(parsed.records()[0].terminator, END);
    let mut offset =
        parsed.root_header.len() + parsed.count_prefix.len() + parsed.shared_header.len();
    for recorder in parsed.records() {
        assert_eq!(recorder.record.as_ptr(), bytes[offset..].as_ptr());
        let mut field_offset = offset;
        for field in recorder.fields() {
            assert_eq!(field.record.as_ptr(), bytes[field_offset..].as_ptr());
            assert_eq!(field.header.len() + field.payload.len(), field.record.len());
            field_offset += field.record.len();
        }
        offset += recorder.record.len();
    }
    assert_eq!(offset, parsed.root_record.len());
    let summary = parsed.summary();
    assert_eq!(summary.recorder_count, 2);
    assert_eq!(summary.field_tags, 6);
    assert_eq!(summary.unknown_fields, 2);
    assert_eq!(summary.opaque_binary_bytes, BODY.len() * 2);
    assert_eq!(summary.class_value_bytes, CLASS.len() * 2);
    assert!(!summary.world_imported);
    let text = serde_json::to_string(&summary).unwrap();
    assert!(!text.contains("private"));
    assert!(!format!("{summary:?}").contains("private"));
}

#[test]
fn exact_counts_boundaries_and_every_element_truncation_are_required() {
    let element = element();
    assert!(read(&fixture(1, &element)).is_ok());
    assert_eq!(read(&fixture(0, &[])).unwrap().summary().recorder_count, 0);
    for count in [-1, 0, 2, i32::MAX] {
        assert!(read(&fixture(count, &element)).is_err());
    }
    for length in 0..element.len() {
        assert!(
            read(&fixture(1, &element[..length])).is_err(),
            "element truncation {length}"
        );
    }
    assert!(read(&fixture(1, &[element.as_slice(), &[0]].concat())).is_err());
    assert!(read(&root_with_shared(1, &[shared(&element), vec![0]].concat())).is_err());
    assert!(read(&root_with_shared(1, END)).is_err());
    let shared = shared(&element);
    for length in 0..shared.len() {
        assert!(
            read(&root_with_shared(1, &shared[..length])).is_err(),
            "shared truncation {length}"
        );
    }
}

#[test]
fn missing_duplicate_and_wrongly_indexed_known_fields_reject_schema_ambiguity() {
    for fields in [
        vec![],
        vec![class(CLASS)],
        vec![binary(&binary_value())],
        vec![class(CLASS), class(CLASS), binary(&binary_value())],
        vec![
            class(CLASS),
            binary(&binary_value()),
            binary(&binary_value()),
        ],
    ] {
        let body = [fields.concat(), END.to_vec()].concat();
        assert!(read(&fixture(1, &body)).is_err());
    }
    for bad_class in [
        tag("ComponentClassName", "StrProperty", 1, &[0], CLASS),
        tag("ComponentClassName", "IntProperty", 0, &[0], CLASS),
    ] {
        let body = [bad_class, binary(&binary_value()), END.to_vec()].concat();
        assert!(read(&fixture(1, &body)).is_err());
    }
    for bad_binary in [
        tag(
            "BinaryData",
            "ArrayProperty",
            1,
            &[string("ByteProperty"), vec![0]].concat(),
            &binary_value(),
        ),
        tag(
            "BinaryData",
            "ArrayProperty",
            0,
            &[string("IntProperty"), vec![0]].concat(),
            &binary_value(),
        ),
        tag("BinaryData", "StrProperty", 0, &[0], &binary_value()),
    ] {
        let body = [class(CLASS), bad_binary, END.to_vec()].concat();
        assert!(read(&fixture(1, &body)).is_err());
    }
}

#[test]
fn root_and_shared_headers_are_exact_and_duplicates_are_rejected() {
    let one = fixture(1, &element());
    let roots = read_root(&one, PROFILE).unwrap();
    let duplicated = [roots.tags[0].record, roots.tags[0].record, END].concat();
    assert!(read(&duplicated).is_err());
    assert!(read(END).is_err());
    for (name, kind, structure, index) in [
        ("Other", "StructProperty", "StateRecorderBlob", 0),
        ("StateRecorderBlobs", "StructProperty", "Other", 0),
        (
            "StateRecorderBlobs",
            "StructProperty",
            "StateRecorderBlob",
            1,
        ),
    ] {
        let wrong = tag(
            name,
            kind,
            index,
            &[string(structure), vec![0; 17]].concat(),
            &element(),
        );
        assert!(read(&root_with_shared(1, &wrong)).is_err());
    }
    for (kind, inner, index) in [
        ("ArrayProperty", "ByteProperty", 0),
        ("ArrayProperty", "StructProperty", 1),
    ] {
        let root = tag(
            "StateRecorderBlobs",
            kind,
            index,
            &[string(inner), vec![0]].concat(),
            &[],
        );
        assert!(read(&[root, END.to_vec()].concat()).is_err());
    }
}

#[test]
fn class_and_binary_payloads_have_exact_bounded_framing() {
    for payload in [
        vec![],
        vec![0; 3],
        (-1_i32).to_le_bytes().to_vec(),
        1_i32.to_le_bytes().to_vec(),
        [vec![0; 4], vec![1]].concat(),
    ] {
        let body = [class(CLASS), binary(&payload), END.to_vec()].concat();
        assert!(read(&fixture(1, &body)).is_err());
    }
    for payload in [
        vec![],
        vec![0; 3],
        [CLASS, &[0]].concat(),
        1025_i32.to_le_bytes().to_vec(),
        (-1025_i32).to_le_bytes().to_vec(),
        i32::MIN.to_le_bytes().to_vec(),
        b"\x02\0\0\0Xz".to_vec(),
        b"\x03\0\0\0\xffX\0".to_vec(),
    ] {
        let body = [class(&payload), binary(&binary_value()), END.to_vec()].concat();
        let error = read(&fixture(1, &body)).err().unwrap();
        assert!(!format!("{error:#}").contains("private"));
    }
    // FString zero length and raw UTF-16 units remain framing, not class resolution.
    for payload in [vec![0; 4], vec![254, 255, 255, 255, 0, 216, 0, 0]] {
        let body = [class(&payload), binary(&[0; 4]), END.to_vec()].concat();
        let bytes = fixture(1, &body);
        let parsed = read(&bytes).unwrap();
        assert_eq!(parsed.records()[0].class_value, payload);
        assert!(parsed.records()[0].binary_data.is_empty());
    }
}

#[test]
fn flags_unknown_headers_and_negative_sizes_never_expose_values() {
    let ordinary = class(CLASS);
    let name_type_len = string("ComponentClassName").len() + string("StrProperty").len();
    for (offset, value) in [(name_type_len, -1_i32), (name_type_len + 4, -1)] {
        let mut bad = ordinary.clone();
        bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        assert!(
            read(&fixture(
                1,
                &[bad, binary(&binary_value()), END.to_vec()].concat()
            ))
            .is_err()
        );
    }
    let mut flag = ordinary;
    flag[name_type_len + 8] = 2;
    for bad in [
        flag,
        tag("private-name", "private-kind", 0, &[0], b"private-value"),
        tag("private-name", "BoolProperty", 0, &[2, 0], b""),
    ] {
        let bytes = fixture(1, &[bad, binary(&binary_value()), END.to_vec()].concat());
        let error = read(&bytes).err().unwrap();
        assert!(!format!("{error:#}").contains("private"));
    }
}

#[test]
fn recorder_field_string_and_input_limits_are_enforced() {
    assert!(read(&fixture(i32::try_from(MAX_RECORDERS + 1).unwrap(), &[])).is_err());
    let empty = [class(&[0; 4]), binary(&[0; 4]), END.to_vec()].concat();
    let bytes = fixture(
        i32::try_from(MAX_RECORDERS).unwrap(),
        &empty.repeat(MAX_RECORDERS),
    );
    assert_eq!(
        read(&bytes).unwrap().summary().recorder_count,
        MAX_RECORDERS
    );

    let unknown = tag("U", "IntProperty", 0, &[0], &[]);
    let body = [
        class(&[0; 4]),
        binary(&[0; 4]),
        unknown.repeat(MAX_RECORDER_FIELDS - 2),
        END.to_vec(),
    ]
    .concat();
    assert_eq!(
        read(&fixture(1, &body)).unwrap().summary().field_tags,
        MAX_RECORDER_FIELDS
    );
    let excess = [unknown, body].concat();
    assert!(read(&fixture(1, &excess)).is_err());

    let oversized = vec![0; MAX_INPUT_BYTES + 1];
    let root = RootTag {
        kind: TagKind::Array,
        record: &oversized,
        header: &[],
        payload: &[],
    };
    let error = read_recorders(&[root], PROFILE).err().unwrap();
    assert!(error.to_string().contains("32 MiB"));
    let long = [1024_i32.to_le_bytes().to_vec(), vec![b'X'; 1023], vec![0]].concat();
    let body = [class(&long), binary(&[0; 4]), END.to_vec()].concat();
    assert!(read(&fixture(1, &body)).is_ok());
}

fn component_binary_from_tags(tags: &[u8]) -> Vec<u8> {
    let component_bytes = [tags, END, &[0, 0, 0, 0]].concat();
    let payload = [
        i32::try_from(component_bytes.len())
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        component_bytes.as_slice(),
    ]
    .concat();
    binary(&payload)
}

fn valid_component_binary() -> Vec<u8> {
    let prop = tag(
        "RecordedVersion",
        "IntProperty",
        0,
        &[0],
        &3_i32.to_le_bytes(),
    );
    component_binary_from_tags(&prop)
}

fn valid_recorder_element() -> Vec<u8> {
    [
        class(&string("/Script/Icarus.ActorStateRecorderComponent")).as_slice(),
        valid_component_binary().as_slice(),
        END,
    ]
    .concat()
}

fn recorder_element(class_name: &str, component_tags: &[u8]) -> Vec<u8> {
    [
        class(&string(class_name)).as_slice(),
        component_binary_from_tags(component_tags).as_slice(),
        END,
    ]
    .concat()
}

fn associated_members_tag(member_count: i32) -> Vec<u8> {
    let member_body = [
        tag(
            "AccountName",
            "StrProperty",
            0,
            &[0],
            &string("private-member"),
        ),
        tag("ChrSlot", "IntProperty", 0, &[0], &1_i32.to_le_bytes()),
        tag(
            "Status",
            "EnumProperty",
            0,
            &[string("EProspectLocation"), vec![0]].concat(),
            &string("EProspectLocation::Prospect_Inhabit"),
        ),
        tag("IsCurrentlyPlaying", "BoolProperty", 0, &[1, 0], &[]),
        END.to_vec(),
    ]
    .concat();
    let repeated = member_body.repeat(usize::try_from(member_count).unwrap());
    let shared_member = tag(
        "AssociatedMembers",
        "StructProperty",
        0,
        &[string("AssociatedMemberInfo"), vec![0; 17]].concat(),
        &repeated,
    );
    let payload = [&member_count.to_le_bytes(), shared_member.as_slice()].concat();
    tag(
        "AssociatedMembers",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &payload,
    )
}

fn custom_settings_tag(setting_count: i32) -> Vec<u8> {
    let setting_body = [
        tag(
            "SettingRowName",
            "StrProperty",
            0,
            &[0],
            &string("private-setting"),
        ),
        tag(
            "SettingValue",
            "IntProperty",
            0,
            &[0],
            &100_i32.to_le_bytes(),
        ),
        END.to_vec(),
    ]
    .concat();
    let repeated = setting_body.repeat(usize::try_from(setting_count).unwrap());
    let shared_setting = tag(
        "CustomSettings",
        "StructProperty",
        0,
        &[string("CustomGameSetting"), vec![0; 17]].concat(),
        &repeated,
    );
    let payload = [&setting_count.to_le_bytes(), shared_setting.as_slice()].concat();
    tag(
        "CustomSettings",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &payload,
    )
}

fn prospect_info_tag(prospect_id: &str, dt_key: &str, member_count: i32) -> Vec<u8> {
    let inner = [
        tag("ProspectID", "StrProperty", 0, &[0], &string(prospect_id)),
        tag("ProspectDTKey", "StrProperty", 0, &[0], &string(dt_key)),
        tag(
            "ElapsedTime",
            "IntProperty",
            0,
            &[0],
            &420_i32.to_le_bytes(),
        ),
        tag(
            "ExpireTime",
            "Int64Property",
            0,
            &[0],
            &(-1_i64).to_le_bytes(),
        ),
        tag("Insurance", "BoolProperty", 0, &[1, 0], &[]),
        associated_members_tag(member_count),
        custom_settings_tag(2),
        END.to_vec(),
    ]
    .concat();
    tag(
        "ProspectInfo",
        "StructProperty",
        0,
        &[string("ProspectInfo"), vec![0; 17]].concat(),
        &inner,
    )
}

#[test]
fn staged_prospect_save_state_validates_schema_migrations_and_fails_closed() {
    let version_v2 = tag("Version", "IntProperty", 0, &[0], &2_i32.to_le_bytes());
    let privacy = tag(
        "LobbyPrivacy",
        "EnumProperty",
        0,
        &[string("ELobbyPrivacy"), vec![0]].concat(),
        &string("ELobbyPrivacy::Private"),
    );
    let info = prospect_info_tag("private-prospect-alpha", "Olympus_Terrain", 2);
    let map_name = tag(
        "ProspectMapName",
        "StrProperty",
        0,
        &[0],
        &string("Terrain_016"),
    );
    let saved_dt = tag(
        "LastSavedDateTime",
        "StructProperty",
        0,
        &[string("DateTime"), vec![0; 17]].concat(),
        &638_000_000_000_000_000_i64.to_le_bytes(),
    );
    let elem = valid_recorder_element();
    let blobs = tag(
        "StateRecorderBlobs",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &[&2_i32.to_le_bytes(), shared(&elem.repeat(2)).as_slice()].concat(),
    );

    // Out-of-order root tags are accepted via PropertyLink wrap-around (F-718).
    let stream_v2 = [
        blobs.clone(),
        saved_dt.clone(),
        map_name.clone(),
        info,
        privacy,
        version_v2.clone(),
        END.to_vec(),
    ]
    .concat();
    let parsed_v2 = read_root(&stream_v2, PROFILE).unwrap();
    let expected_id: Vec<u16> = "PRIVATE-PROSPECT-ALPHA".encode_utf16().collect();
    let staged_v2 = stage_prospect_save_state(
        &parsed_v2.tags,
        PROFILE,
        MAX_COMPONENT_BYTES,
        Some(&expected_id),
    )
    .unwrap();
    assert_eq!(staged_v2.loaded_version, 2);
    assert_eq!(staged_v2.migrated_version, 2);
    assert_eq!(staged_v2.lobby_privacy, LobbyPrivacy::Private);
    assert_eq!(staged_v2.root_tag_count, 6);
    assert_eq!(staged_v2.prospect_info_tag_count, 7);
    assert_eq!(staged_v2.associated_member_count, 2);
    assert!(staged_v2.has_prospect_id);
    assert!(staged_v2.has_prospect_dt_key);
    assert!(staged_v2.has_map_name);
    assert!(staged_v2.has_last_saved_date_time);
    assert!(staged_v2.has_expire_time);
    assert_eq!(staged_v2.parsed_recorder_count, 2);
    assert_eq!(staged_v2.invalid_recorder_count, 0);
    assert_eq!(staged_v2.retained_recorder_count, 2);
    assert_eq!(staged_v2.reset_rocket_spawn_recorder_count, 0);
    assert_eq!(staged_v2.physics_dynamic_tree_recorder_count, 0);
    assert_eq!(staged_v2.skip_default_owner_resolution_count, 0);
    assert_eq!(staged_v2.find_only_owner_count, 0);
    assert_eq!(staged_v2.respawn_only_owner_count, 0);
    assert_eq!(staged_v2.find_or_respawn_owner_count, 2);
    assert_eq!(staged_v2.manually_resolved_owner_count, 0);
    assert_eq!(staged_v2.requires_stitching_count, 0);
    assert_eq!(staged_v2.total_component_field_tags, 2);
    assert!(!staged_v2.world_imported);
    let json = serde_json::to_string(&staged_v2).unwrap();
    assert!(!json.contains("private"));
    assert!(!format!("{staged_v2:?}").contains("private"));

    // Mismatched expected ProspectID fails closed before publication (F-717, F-733).
    let wrong_id: Vec<u16> = "other-prospect".encode_utf16().collect();
    assert!(
        stage_prospect_save_state(
            &parsed_v2.tags,
            PROFILE,
            MAX_COMPONENT_BYTES,
            Some(&wrong_id),
        )
        .is_err()
    );

    // Version 1 copies legacy root ProspectID and ProspectDTKey into ProspectInfo (F-715).
    let stream_v1 = [
        tag("Version", "IntProperty", 0, &[0], &1_i32.to_le_bytes()),
        tag(
            "ProspectID",
            "StrProperty",
            0,
            &[0],
            &string("private-legacy-id"),
        ),
        tag(
            "ProspectDTKey",
            "StrProperty",
            0,
            &[0],
            &string("private-legacy-dt"),
        ),
        blobs.clone(),
        END.to_vec(),
    ]
    .concat();
    let parsed_v1 = read_root(&stream_v1, PROFILE).unwrap();
    let legacy_expected: Vec<u16> = "PRIVATE-LEGACY-ID".encode_utf16().collect();
    let staged_v1 = stage_prospect_save_state(
        &parsed_v1.tags,
        PROFILE,
        MAX_COMPONENT_BYTES,
        Some(&legacy_expected),
    )
    .unwrap();
    assert_eq!(staged_v1.loaded_version, 1);
    assert_eq!(staged_v1.migrated_version, 2);
    assert!(staged_v1.has_prospect_id);
    assert!(staged_v1.has_prospect_dt_key);
    assert_eq!(staged_v1.retained_recorder_count, 2);

    // Version 0 (or omitted Version) resets ProspectInfo, sets FriendsOnly, and clears blobs (F-715).
    let stream_v0 = [
        prospect_info_tag("private-v0-id", "Olympus", 1),
        blobs.clone(),
        END.to_vec(),
    ]
    .concat();
    let parsed_v0 = read_root(&stream_v0, PROFILE).unwrap();
    let staged_v0 =
        stage_prospect_save_state(&parsed_v0.tags, PROFILE, MAX_COMPONENT_BYTES, None).unwrap();
    assert_eq!(staged_v0.loaded_version, 0);
    assert_eq!(staged_v0.migrated_version, 2);
    assert_eq!(staged_v0.lobby_privacy, LobbyPrivacy::FriendsOnly);
    assert!(!staged_v0.has_prospect_id);
    assert!(!staged_v0.has_prospect_dt_key);
    assert_eq!(staged_v0.parsed_recorder_count, 2);
    assert_eq!(staged_v0.invalid_recorder_count, 0);
    assert_eq!(staged_v0.retained_recorder_count, 0);

    // Reject unmapped root tags, unsupported versions, and malformed recorder bodies (F-403).
    let unmapped_stream = [
        version_v2.clone(),
        tag(
            "UnmappedRootField",
            "IntProperty",
            0,
            &[0],
            &1_i32.to_le_bytes(),
        ),
        END.to_vec(),
    ]
    .concat();
    let parsed_unmapped = read_root(&unmapped_stream, PROFILE).unwrap();
    assert!(
        stage_prospect_save_state(&parsed_unmapped.tags, PROFILE, MAX_COMPONENT_BYTES, None,)
            .is_err()
    );

    let future_version_stream = [
        tag("Version", "IntProperty", 0, &[0], &3_i32.to_le_bytes()),
        END.to_vec(),
    ]
    .concat();
    let parsed_future = read_root(&future_version_stream, PROFILE).unwrap();
    assert!(
        stage_prospect_save_state(&parsed_future.tags, PROFILE, MAX_COMPONENT_BYTES, None,)
            .is_err()
    );

    // Recorder with non-framing BinaryData fails closed during staging.
    let bad_rec = fixture(1, &element());
    let parsed_bad_rec = read_root(&bad_rec, PROFILE).unwrap();
    assert!(
        stage_prospect_save_state(&parsed_bad_rec.tags, PROFILE, MAX_COMPONENT_BYTES, None,)
            .is_err()
    );

    // Reject unmapped fields inside ProspectInfo, AssociatedMemberInfo, and CustomGameSetting (F-741).
    let unmapped_info = tag(
        "ProspectInfo",
        "StructProperty",
        0,
        &[string("ProspectInfo"), vec![0; 17]].concat(),
        &[
            tag("InGame", "BoolProperty", 0, &[1, 0], &[]).as_slice(),
            END,
        ]
        .concat(),
    );
    let unmapped_info_stream = [version_v2.clone(), unmapped_info, END.to_vec()].concat();
    let parsed_unmapped_info = read_root(&unmapped_info_stream, PROFILE).unwrap();
    assert!(
        stage_prospect_save_state(
            &parsed_unmapped_info.tags,
            PROFILE,
            MAX_COMPONENT_BYTES,
            None,
        )
        .is_err()
    );

    let bad_member_shared = tag(
        "AssociatedMembers",
        "StructProperty",
        0,
        &[string("AssociatedMemberInfo"), vec![0; 17]].concat(),
        &[
            tag(
                "CharacterSlot",
                "IntProperty",
                0,
                &[0],
                &1_i32.to_le_bytes(),
            )
            .as_slice(),
            END,
        ]
        .concat(),
    );
    let bad_members_info = tag(
        "ProspectInfo",
        "StructProperty",
        0,
        &[string("ProspectInfo"), vec![0; 17]].concat(),
        &[
            tag(
                "AssociatedMembers",
                "ArrayProperty",
                0,
                &[string("StructProperty"), vec![0]].concat(),
                &[&1_i32.to_le_bytes(), bad_member_shared.as_slice()].concat(),
            )
            .as_slice(),
            END,
        ]
        .concat(),
    );
    let bad_member_stream = [version_v2.clone(), bad_members_info, END.to_vec()].concat();
    let parsed_bad_member = read_root(&bad_member_stream, PROFILE).unwrap();
    assert!(
        stage_prospect_save_state(&parsed_bad_member.tags, PROFILE, MAX_COMPONENT_BYTES, None)
            .is_err()
    );

    let bad_setting_shared = tag(
        "CustomSettings",
        "StructProperty",
        0,
        &[string("CustomGameSetting"), vec![0; 17]].concat(),
        &[
            tag(
                "SettingName",
                "StrProperty",
                0,
                &[0],
                &string("private-setting"),
            )
            .as_slice(),
            END,
        ]
        .concat(),
    );
    let bad_settings_info = tag(
        "ProspectInfo",
        "StructProperty",
        0,
        &[string("ProspectInfo"), vec![0; 17]].concat(),
        &[
            tag(
                "CustomSettings",
                "ArrayProperty",
                0,
                &[string("StructProperty"), vec![0]].concat(),
                &[&1_i32.to_le_bytes(), bad_setting_shared.as_slice()].concat(),
            )
            .as_slice(),
            END,
        ]
        .concat(),
    );
    let bad_setting_stream = [version_v2, bad_settings_info, END.to_vec()].concat();
    let parsed_bad_setting = read_root(&bad_setting_stream, PROFILE).unwrap();
    assert!(
        stage_prospect_save_state(&parsed_bad_setting.tags, PROFILE, MAX_COMPONENT_BYTES, None)
            .is_err()
    );
}

fn player_char_id_struct(field_name: &str, player_id: &str, chr_slot: i32) -> Vec<u8> {
    let body = [
        tag("PlayerID", "StrProperty", 0, &[0], &string(player_id)),
        tag("ChrSlot", "IntProperty", 0, &[0], &chr_slot.to_le_bytes()),
        END.to_vec(),
    ]
    .concat();
    tag(
        field_name,
        "StructProperty",
        0,
        &[string("PlayerCharacterID"), vec![0; 17]].concat(),
        &body,
    )
}

#[test]
fn staged_prospect_save_state_runs_recorder_validate_and_tracks_tree_dynamics() {
    // AssociatedMembers with ("user-alpha", 1) and ("user-beta", 0 default).
    let member_1 = [
        tag("UserID", "StrProperty", 0, &[0], &string("user-alpha")),
        tag("ChrSlot", "IntProperty", 0, &[0], &1_i32.to_le_bytes()),
        END.to_vec(),
    ]
    .concat();
    let member_2 = [
        tag("UserID", "StrProperty", 0, &[0], &string("user-beta")),
        END.to_vec(),
    ]
    .concat();
    let members_shared = tag(
        "AssociatedMembers",
        "StructProperty",
        0,
        &[string("AssociatedMemberInfo"), vec![0; 17]].concat(),
        &[member_1, member_2].concat(),
    );
    let members_array = tag(
        "AssociatedMembers",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &[&2_i32.to_le_bytes(), members_shared.as_slice()].concat(),
    );
    let info = tag(
        "ProspectInfo",
        "StructProperty",
        0,
        &[string("ProspectInfo"), vec![0; 17]].concat(),
        &[
            tag("ProspectID", "StrProperty", 0, &[0], &string("p-val")),
            members_array,
            END.to_vec(),
        ]
        .concat(),
    );

    // 1. C011 DynamicRocketSpawnRecorderComponent with missing player -> retained + reset_spawn_record.
    let spawn_record = tag(
        "SpawnRecord",
        "StructProperty",
        0,
        &[string("RocketSpawnStateRecord"), vec![0; 17]].concat(),
        &[
            tag(
                "AssignedPlayerID",
                "StrProperty",
                0,
                &[0],
                &string("user-missing"),
            ),
            tag("ChrSlot", "IntProperty", 0, &[0], &9_i32.to_le_bytes()),
            END.to_vec(),
        ]
        .concat(),
    );
    let rec_c011_reset = recorder_element(
        "/Script/Icarus.DynamicRocketSpawnRecorderComponent",
        &[
            tag(
                "RocketSpawnRecorderVersion",
                "IntProperty",
                0,
                &[0],
                &2_i32.to_le_bytes(),
            ),
            spawn_record,
        ]
        .concat(),
    );

    // 2. C025 PlayerRecorderComponent matching ("user-alpha", 1) -> retained.
    let rec_c025_valid = recorder_element(
        "/Script/Icarus.PlayerRecorderComponent",
        &[
            tag(
                "PlayerRecorderVersion",
                "IntProperty",
                0,
                &[0],
                &1_i32.to_le_bytes(),
            ),
            player_char_id_struct("PlayerCharacterID", "user-alpha", 1),
        ]
        .concat(),
    );

    // 3. C025 PlayerRecorderComponent missing player -> invalid.
    let rec_c025_invalid = recorder_element(
        "/Script/Icarus.PlayerRecorderComponent",
        &[
            tag(
                "PlayerRecorderVersion",
                "IntProperty",
                0,
                &[0],
                &1_i32.to_le_bytes(),
            ),
            player_char_id_struct("PlayerCharacterID", "user-alpha", 2),
        ]
        .concat(),
    );

    // 4. C026 PlayerStateRecorderComponent with version 1 and matching ("user-beta", 0) -> retained.
    let rec_c026_valid = recorder_element(
        "/Script/Icarus.PlayerStateRecorderComponent",
        &[
            tag(
                "PlayerStateRecorderVersion",
                "IntProperty",
                0,
                &[0],
                &1_i32.to_le_bytes(),
            ),
            player_char_id_struct("PlayerCharacterID", "user-beta", 0),
        ]
        .concat(),
    );

    // 5. C026 PlayerStateRecorderComponent with version 0 -> invalid (UPlayerStateRecorderComponent::Validate 0x1418f9e80 requires Version >= 1).
    let rec_c026_legacy_invalid = recorder_element(
        "/Script/Icarus.PlayerStateRecorderComponent",
        &player_char_id_struct("PlayerCharacterID", "user-beta", 0),
    );

    // 6. C028 ResourceDepositRecorderComponent with ResourceDTKey = "Super_Cooled_Ice" -> invalid (0x1418f9f20).
    let rec_c028_invalid = recorder_element(
        "/Script/Icarus.ResourceDepositRecorderComponent",
        &tag(
            "ResourceDTKey",
            "NameProperty",
            0,
            &[0],
            &string("Super_Cooled_Ice"),
        ),
    );

    // 7. C028 ResourceDepositRecorderComponent with non-ice ResourceDTKey -> retained.
    let rec_c028_valid = recorder_element(
        "/Script/Icarus.ResourceDepositRecorderComponent",
        &tag(
            "ResourceDTKey",
            "NameProperty",
            0,
            &[0],
            &string("Iron_DeepVein"),
        ),
    );

    // 8. C029 RocketRecorderComponent with version 2 and missing player -> invalid.
    let rec_c029_invalid = recorder_element(
        "/Script/Icarus.RocketRecorderComponent",
        &[
            tag(
                "RocketRecorderVersion",
                "IntProperty",
                0,
                &[0],
                &2_i32.to_le_bytes(),
            ),
            player_char_id_struct("AssignedPlayerCharacterID", "user-missing", 0),
        ]
        .concat(),
    );

    // 9. C031 SpawnedVoxelRecorderComponent (default OwnerResolvePolicy = RespawnOnly) + bIsVoxelFullyMined = true -> invalid.
    let voxel_mined = tag(
        "VoxelSaveData",
        "StructProperty",
        0,
        &[string("VoxelSaveData"), vec![0; 17]].concat(),
        &[
            tag("bIsVoxelFullyMined", "BoolProperty", 0, &[1, 0], &[]).as_slice(),
            END,
        ]
        .concat(),
    );
    let rec_c031_invalid =
        recorder_element("/Script/Icarus.SpawnedVoxelRecorderComponent", &voxel_mined);

    // 10. C034 VoxelRecorderComponent (default OwnerResolvePolicy = ManuallyResolved) + bIsVoxelFullyMined = true -> retained.
    let rec_c034_valid = recorder_element("/Script/Icarus.VoxelRecorderComponent", &voxel_mined);

    // 11. C034 VoxelRecorderComponent with explicit OwnerResolvePolicy = RespawnOnly + bIsVoxelFullyMined = true -> invalid.
    let rec_c034_invalid = recorder_element(
        "/Script/Icarus.VoxelRecorderComponent",
        &[
            tag(
                "OwnerResolvePolicy",
                "EnumProperty",
                0,
                &[string("EStateRecorderOwnerResolvePolicy"), vec![0]].concat(),
                &string("EStateRecorderOwnerResolvePolicy::RespawnOnly"),
            ),
            voxel_mined,
        ]
        .concat(),
    );

    // 12. C033 TreeRecorderComponent with bIsPhysicsDynamic = true (carrying a property GUID whose last bytes are 0x00) -> retained + dynamic tree count.
    let mut bool_true_with_guid_ending_zero = vec![1_u8, 1_u8];
    bool_true_with_guid_ending_zero.extend_from_slice(&[0xaa; 14]);
    bool_true_with_guid_ending_zero.extend_from_slice(&[0x00, 0x00]);
    let rec_c033_dynamic = recorder_element(
        "/Script/Icarus.TreeRecorderComponent",
        &tag(
            "bIsPhysicsDynamic",
            "BoolProperty",
            0,
            &bool_true_with_guid_ending_zero,
            &[],
        ),
    );

    // 13. C033 TreeRecorderComponent with bIsPhysicsDynamic = false -> retained, not dynamic.
    let rec_c033_static = recorder_element(
        "/Script/Icarus.TreeRecorderComponent",
        &tag("bIsPhysicsDynamic", "BoolProperty", 0, &[0, 0], &[]),
    );

    // 14. C011 DynamicRocketSpawnRecorderComponent with omitted ChrSlot (defaults to -1) and empty AssignedPlayerID -> retained, not counted as reset.
    let rec_c011_unassigned = recorder_element(
        "/Script/Icarus.DynamicRocketSpawnRecorderComponent",
        &tag(
            "SpawnRecord",
            "StructProperty",
            0,
            &[string("RocketSpawnStateRecord"), vec![0; 17]].concat(),
            END,
        ),
    );

    let all_records = [
        rec_c011_reset,
        rec_c025_valid,
        rec_c025_invalid,
        rec_c026_valid,
        rec_c026_legacy_invalid,
        rec_c028_invalid,
        rec_c028_valid,
        rec_c029_invalid,
        rec_c031_invalid,
        rec_c034_valid,
        rec_c034_invalid,
        rec_c033_dynamic,
        rec_c033_static,
        rec_c011_unassigned,
    ];
    let blobs = tag(
        "StateRecorderBlobs",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &[
            &i32::try_from(all_records.len()).unwrap().to_le_bytes(),
            shared(&all_records.concat()).as_slice(),
        ]
        .concat(),
    );
    let stream = [
        tag("Version", "IntProperty", 0, &[0], &2_i32.to_le_bytes()),
        info.clone(),
        blobs,
        END.to_vec(),
    ]
    .concat();
    let parsed = read_root(&stream, PROFILE).unwrap();
    let staged =
        stage_prospect_save_state(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES, None).unwrap();
    assert_eq!(staged.parsed_recorder_count, 14);
    assert_eq!(staged.invalid_recorder_count, 6);
    assert_eq!(staged.retained_recorder_count, 8);
    assert_eq!(staged.reset_rocket_spawn_recorder_count, 1);
    assert_eq!(staged.physics_dynamic_tree_recorder_count, 1);
    assert_eq!(staged.skip_default_owner_resolution_count, 3);
    assert_eq!(staged.find_only_owner_count, 0);
    assert_eq!(staged.respawn_only_owner_count, 4);
    assert_eq!(staged.find_or_respawn_owner_count, 1);
    assert_eq!(staged.manually_resolved_owner_count, 0);
    assert_eq!(staged.requires_stitching_count, 2);

    // Malformed RocketRecorderVersion (wrong tag kind), duplicate component properties,
    // nonzero-indexed component properties, or duplicate/unmapped nested struct fields fail closed.
    let dup_prop = tag(
        "RecordedVersion",
        "IntProperty",
        0,
        &[0],
        &3_i32.to_le_bytes(),
    );
    let bad_recorders = [
        recorder_element("/Script/Icarus.UnknownRecorderComponent", &[]),
        recorder_element(
            "/Script/Icarus.RocketRecorderComponent",
            &tag(
                "RocketRecorderVersion",
                "StrProperty",
                0,
                &[0],
                &string("bad"),
            ),
        ),
        recorder_element(
            "/Script/Icarus.ActorStateRecorderComponent",
            &[dup_prop.clone(), dup_prop].concat(),
        ),
        recorder_element(
            "/Script/Icarus.ActorStateRecorderComponent",
            &tag(
                "RecordedVersion",
                "IntProperty",
                1,
                &[0],
                &3_i32.to_le_bytes(),
            ),
        ),
        recorder_element(
            "/Script/Icarus.PlayerRecorderComponent",
            &tag(
                "PlayerCharacterID",
                "StructProperty",
                0,
                &[string("PlayerCharacterID"), vec![0; 17]].concat(),
                &[
                    tag("PlayerID", "StrProperty", 0, &[0], &string("user-alpha")),
                    tag("PlayerID", "StrProperty", 0, &[0], &string("user-alpha")),
                    END.to_vec(),
                ]
                .concat(),
            ),
        ),
        recorder_element(
            "/Script/Icarus.PlayerRecorderComponent",
            &tag(
                "PlayerCharacterID",
                "StructProperty",
                0,
                &[string("PlayerCharacterID"), vec![0; 17]].concat(),
                &[
                    tag(
                        "UnmappedField",
                        "IntProperty",
                        0,
                        &[0],
                        &1_i32.to_le_bytes(),
                    ),
                    END.to_vec(),
                ]
                .concat(),
            ),
        ),
        recorder_element(
            "/Script/Icarus.DynamicRocketSpawnRecorderComponent",
            &tag(
                "SpawnRecord",
                "StructProperty",
                0,
                &[string("RocketSpawnStateRecord"), vec![0; 17]].concat(),
                &[
                    tag("ChrSlot", "IntProperty", 0, &[0], &1_i32.to_le_bytes()),
                    tag("ChrSlot", "IntProperty", 0, &[0], &2_i32.to_le_bytes()),
                    END.to_vec(),
                ]
                .concat(),
            ),
        ),
        recorder_element(
            "/Script/Icarus.VoxelRecorderComponent",
            &tag(
                "VoxelSaveData",
                "StructProperty",
                0,
                &[string("VoxelSaveData"), vec![0; 17]].concat(),
                &[
                    tag("bIsVoxelFullyMined", "BoolProperty", 0, &[1, 0], &[]),
                    tag("bIsVoxelFullyMined", "BoolProperty", 0, &[0, 0], &[]),
                    END.to_vec(),
                ]
                .concat(),
            ),
        ),
    ];
    for bad_rec in bad_recorders {
        let bad_blobs = tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            0,
            &[string("StructProperty"), vec![0]].concat(),
            &[&1_i32.to_le_bytes(), shared(&bad_rec).as_slice()].concat(),
        );
        let bad_stream = [
            tag("Version", "IntProperty", 0, &[0], &2_i32.to_le_bytes()),
            info.clone(),
            bad_blobs,
            END.to_vec(),
        ]
        .concat();
        let parsed_bad = read_root(&bad_stream, PROFILE).unwrap();
        assert!(
            stage_prospect_save_state(&parsed_bad.tags, PROFILE, MAX_COMPONENT_BYTES, None)
                .is_err()
        );
    }
}

#[test]
fn staged_prospect_save_state_evaluates_non_blob_recorders_and_owner_resolution_overrides() {
    let info = prospect_info_tag("prospect-gamma", "Olympus_Terrain", 1);

    // 1. N018 UNoRecorderComponent unconditionally fails Validate (+0x408 = false).
    let rec_no_recorder = recorder_element("/Script/Icarus.NoRecorderComponent", &[]);

    // 2. N012 UIcarusJuvenileRecorderComponent (found in backup saves): default RespawnOnly,
    // CharacterParentUidPresent RequiresStitching -> true when ParentCharacterUID != -1.
    let rec_juvenile_stitched = recorder_element(
        "/Script/Icarus.IcarusJuvenileRecorderComponent",
        &tag(
            "ParentCharacterUID",
            "IntProperty",
            0,
            &[0],
            &42_i32.to_le_bytes(),
        ),
    );

    // 3. N012 UIcarusJuvenileRecorderComponent with default ParentCharacterUID (-1) -> RequiresStitching false,
    // and explicit OwnerResolvePolicy = FindOnly override.
    let rec_juvenile_unstitched_find_only = recorder_element(
        "/Script/Icarus.IcarusJuvenileRecorderComponent",
        &tag(
            "OwnerResolvePolicy",
            "ByteProperty",
            0,
            &[string("None"), vec![0]].concat(),
            &[0_u8],
        ),
    );

    // 4. N020 UResourceDepositIceRecorderComponent overrides Validate (+0x408) back to true,
    // so "Super_Cooled_Ice" validates on N020 (FindOrRespawn, RequiresStitching = false).
    let rec_ice_deposit = recorder_element(
        "/Script/Icarus.ResourceDepositIceRecorderComponent",
        &tag(
            "ResourceDTKey",
            "NameProperty",
            0,
            &[0],
            &string("Super_Cooled_Ice"),
        ),
    );

    // 5. N021 URockBaseRecorderComponent defaults bSkipDefaultOwnerResolution = true,
    // overridden here to false with ManuallyResolved policy.
    let rec_rock_base_overridden = recorder_element(
        "/Script/Icarus.RockBaseRecorderComponent",
        &[
            tag(
                "bSkipDefaultOwnerResolution",
                "BoolProperty",
                0,
                &[0, 0],
                &[],
            ),
            tag(
                "OwnerResolvePolicy",
                "ByteProperty",
                0,
                &[string("None"), vec![0]].concat(),
                &[3_u8],
            ),
        ]
        .concat(),
    );

    let records = [
        rec_no_recorder,
        rec_juvenile_stitched,
        rec_juvenile_unstitched_find_only,
        rec_ice_deposit,
        rec_rock_base_overridden,
    ];
    let blobs = tag(
        "StateRecorderBlobs",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &[
            &i32::try_from(records.len()).unwrap().to_le_bytes(),
            shared(&records.concat()).as_slice(),
        ]
        .concat(),
    );
    let stream = [
        tag("Version", "IntProperty", 0, &[0], &2_i32.to_le_bytes()),
        info,
        blobs,
        END.to_vec(),
    ]
    .concat();
    let parsed = read_root(&stream, PROFILE).unwrap();
    let staged =
        stage_prospect_save_state(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES, None).unwrap();
    assert_eq!(staged.parsed_recorder_count, 5);
    assert_eq!(staged.invalid_recorder_count, 1);
    assert_eq!(staged.retained_recorder_count, 4);
    assert_eq!(staged.skip_default_owner_resolution_count, 0);
    assert_eq!(staged.find_only_owner_count, 1);
    assert_eq!(staged.respawn_only_owner_count, 1);
    assert_eq!(staged.find_or_respawn_owner_count, 1);
    assert_eq!(staged.manually_resolved_owner_count, 1);
    assert_eq!(staged.requires_stitching_count, 1);
}

#[test]
fn lookup_recorder_ctor_defaults_pins_all_38_blob_and_documented_non_blob_ordinals() {
    let blob_class_paths: [(u8, &str); 38] = [
        (1, "/Script/Icarus.ActorStateRecorderComponent"),
        (2, "/Script/Icarus.BaseLevelTeleportRecorderComponent"),
        (3, "/Script/Icarus.BedRecorderComponent"),
        (4, "/Script/Icarus.BuildingGridRecorderComponent"),
        (5, "/Script/Icarus.CaveAIRecorderComponent"),
        (6, "/Script/Icarus.CaveEntranceRecorderComponent"),
        (7, "/Script/Icarus.CharacterTrapRecorderComponent"),
        (8, "/Script/Icarus.CropPlotRecorderComponent"),
        (9, "/Script/Icarus.DeployableRecorderComponent"),
        (10, "/Script/Icarus.DrillRecorderComponent"),
        (11, "/Script/Icarus.DynamicRocketSpawnRecorderComponent"),
        (12, "/Script/Icarus.EnzymeGeyserRecorderComponent"),
        (13, "/Script/Icarus.FLODRecorderComponent"),
        (14, "/Script/Icarus.FLODTileRecorderComponent"),
        (15, "/Script/Icarus.GameModeStateRecorderComponent"),
        (16, "/Script/Icarus.IcarusContainerManagerRecorderComponent"),
        (17, "/Script/Icarus.IcarusMountCharacterRecorderComponent"),
        (18, "/Script/Icarus.IcarusQuestManagerRecorderComponent"),
        (19, "/Script/Icarus.IcarusQuestRecorderComponent"),
        (20, "/Script/Icarus.InstancedLevelRecorderComponent"),
        (21, "/Script/Icarus.ItemStateRecorderComponent"),
        (22, "/Script/Icarus.MapManagerRecorderComponent"),
        (23, "/Script/Icarus.OilGeyserRecorderComponent"),
        (24, "/Script/Icarus.PlayerHistoryRecorderComponent"),
        (25, "/Script/Icarus.PlayerRecorderComponent"),
        (26, "/Script/Icarus.PlayerStateRecorderComponent"),
        (27, "/Script/Icarus.PrebuiltStructureRecorderComponent"),
        (28, "/Script/Icarus.ResourceDepositRecorderComponent"),
        (29, "/Script/Icarus.RocketRecorderComponent"),
        (30, "/Script/Icarus.SignRecorderComponent"),
        (31, "/Script/Icarus.SpawnedVoxelRecorderComponent"),
        (32, "/Script/Icarus.SplineRecorderComponent"),
        (33, "/Script/Icarus.TreeRecorderComponent"),
        (34, "/Script/Icarus.VoxelRecorderComponent"),
        (35, "/Script/Icarus.WeatherControllerRecorderComponent"),
        (36, "/Script/Icarus.WeatherForecastRecorderComponent"),
        (37, "/Script/Icarus.WorldBossManagerRecorderComponent"),
        (38, "/Script/Icarus.WorldTalentManagerRecorderComponent"),
    ];
    for (ordinal, path) in blob_class_paths {
        let units: Vec<u16> = path.encode_utf16().collect();
        assert_eq!(
            lookup_recorder_ctor_defaults(&units),
            recorder_ctor_defaults(ordinal),
            "blob class ordinal {ordinal} ({path})"
        );
    }

    let non_blob_class_paths: [(u8, &str); 8] = [
        (10, "/Script/Icarus.IcarusCharacterRecorderComponent"),
        (12, "/Script/Icarus.IcarusJuvenileRecorderComponent"),
        (14, "/Script/Icarus.IcarusNPCRecorderComponent"),
        (15, "/Script/Icarus.IcarusStateRecorderComponent"),
        (18, "/Script/Icarus.NoRecorderComponent"),
        (20, "/Script/Icarus.ResourceDepositIceRecorderComponent"),
        (21, "/Script/Icarus.RockBaseRecorderComponent"),
        (22, "/Script/Icarus.RocketSpawnRecorderComponent"),
    ];
    for (ordinal, path) in non_blob_class_paths {
        let units: Vec<u16> = path.encode_utf16().collect();
        assert_eq!(
            lookup_recorder_ctor_defaults(&units),
            non_blob_recorder_ctor_defaults(ordinal),
            "non-blob class ordinal {ordinal} ({path})"
        );
    }

    let unknown: Vec<u16> = "/Script/Icarus.UnknownRecorderComponent"
        .encode_utf16()
        .collect();
    assert_eq!(lookup_recorder_ctor_defaults(&unknown), None);
}

// Independent authored encodings for owned-root retention. No original inputs.
fn root_value_struct(name: &str, type_name: &str, fields: &[Vec<u8>]) -> Vec<u8> {
    let mut payload = fields.concat();
    payload.extend_from_slice(END);
    tag(
        name,
        "StructProperty",
        0,
        &[string(type_name), vec![0; 17]].concat(),
        &payload,
    )
}

fn root_value_struct_array(name: &str, type_name: &str, elements: &[Vec<Vec<u8>>]) -> Vec<u8> {
    let mut body = Vec::new();
    for fields in elements {
        body.extend(fields.concat());
        body.extend_from_slice(END);
    }
    let shared = tag(
        name,
        "StructProperty",
        0,
        &[string(type_name), vec![0; 17]].concat(),
        &body,
    );
    tag(
        name,
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &[
            i32::try_from(elements.len())
                .unwrap()
                .to_le_bytes()
                .to_vec(),
            shared,
        ]
        .concat(),
    )
}

fn root_value_wide_string(units: &[u16]) -> Vec<u8> {
    let mut bytes = (-i32::try_from(units.len() + 1).unwrap())
        .to_le_bytes()
        .to_vec();
    for unit in units {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes
}

fn root_value_version(version: i32) -> Vec<u8> {
    tag("Version", "IntProperty", 0, &[0], &version.to_le_bytes())
}

fn root_value_timestamp(value: i64) -> Vec<u8> {
    tag(
        "LastSavedDateTime",
        "StructProperty",
        0,
        &[string("DateTime"), vec![0; 17]].concat(),
        &value.to_le_bytes(),
    )
}

fn stage_owned_root(
    fields: &[Vec<u8>],
) -> (
    StagedProspectSaveState,
    StagedRootValues,
    Vec<StagedRecorderDecision>,
) {
    let mut bytes = fields.concat();
    bytes.extend_from_slice(END);
    let parsed = read_root(&bytes, PROFILE).unwrap();
    stage_prospect_save_state_with_root_values(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES, None)
        .unwrap()
    // The source allocation and all borrowed tags are gone on return.
}

fn complete_owned_prospect_info() -> Vec<u8> {
    root_value_struct(
        "ProspectInfo",
        "ProspectInfo",
        &[
            tag(
                "ProspectID",
                "StrProperty",
                0,
                &[0],
                &string("private-root-id"),
            ),
            tag(
                "ClaimedAccountID",
                "StrProperty",
                0,
                &[0],
                &string("private-claim"),
            ),
            tag(
                "ClaimedAccountCharacter",
                "IntProperty",
                0,
                &[0],
                &(-7_i32).to_le_bytes(),
            ),
            tag(
                "ProspectDTKey",
                "StrProperty",
                0,
                &[0],
                &string("private-root-key"),
            ),
            tag(
                "FactionMissionDTKey",
                "StrProperty",
                0,
                &[0],
                &string("private-mission"),
            ),
            tag(
                "LobbyName",
                "StrProperty",
                0,
                &[0],
                &string("private-lobby"),
            ),
            tag(
                "ExpireTime",
                "Int64Property",
                0,
                &[0],
                &i64::MIN.to_le_bytes(),
            ),
            tag(
                "ProspectState",
                "EnumProperty",
                0,
                &[string("EProspectState"), vec![0]].concat(),
                &string("EProspectState::Active"),
            ),
            root_value_struct_array(
                "AssociatedMembers",
                "AssociatedMemberInfo",
                &[
                    vec![
                        tag(
                            "AccountName",
                            "StrProperty",
                            0,
                            &[0],
                            &string("private-account"),
                        ),
                        tag(
                            "CharacterName",
                            "StrProperty",
                            0,
                            &[0],
                            &string("private-character"),
                        ),
                        tag("UserID", "StrProperty", 0, &[0], &string("private-user")),
                        tag("ChrSlot", "IntProperty", 0, &[0], &(-3_i32).to_le_bytes()),
                        tag(
                            "Experience",
                            "IntProperty",
                            0,
                            &[0],
                            &i32::MAX.to_le_bytes(),
                        ),
                        tag(
                            "Status",
                            "EnumProperty",
                            0,
                            &[string("EProspectLocation"), vec![0]].concat(),
                            &string("Prospect_Jungle"),
                        ),
                        tag("Settled", "BoolProperty", 0, &[1, 0], &[]),
                        tag("IsCurrentlyPlaying", "BoolProperty", 0, &[1, 0], &[]),
                    ],
                    vec![],
                ],
            ),
            tag("Cost", "IntProperty", 0, &[0], &i32::MIN.to_le_bytes()),
            tag("Reward", "IntProperty", 0, &[0], &31_i32.to_le_bytes()),
            tag(
                "Difficulty",
                "ByteProperty",
                0,
                &[string("EMissionDifficulty"), vec![0]].concat(),
                &[4],
            ),
            tag("Insurance", "BoolProperty", 0, &[1, 0], &[]),
            tag("NoRespawns", "BoolProperty", 0, &[1, 0], &[]),
            tag(
                "ElapsedTime",
                "IntProperty",
                0,
                &[0],
                &123456_i32.to_le_bytes(),
            ),
            tag(
                "SelectedDropPoint",
                "IntProperty",
                0,
                &[0],
                &(-22_i32).to_le_bytes(),
            ),
            root_value_struct_array(
                "CustomSettings",
                "CustomGameSetting",
                &[
                    vec![
                        tag(
                            "SettingRowName",
                            "StrProperty",
                            0,
                            &[0],
                            &string("private-setting-first"),
                        ),
                        tag(
                            "SettingValue",
                            "IntProperty",
                            0,
                            &[0],
                            &(-100_i32).to_le_bytes(),
                        ),
                    ],
                    vec![tag(
                        "SettingValue",
                        "IntProperty",
                        0,
                        &[0],
                        &77_i32.to_le_bytes(),
                    )],
                ],
            ),
        ],
    )
}

#[test]
fn staged_root_values_own_every_prospect_member_and_setting_value() {
    let fields = [
        complete_owned_prospect_info(),
        root_value_timestamp(i64::MAX),
        tag(
            "ProspectMapName",
            "StrProperty",
            0,
            &[0],
            &string("private-map"),
        ),
        tag(
            "LobbyPrivacy",
            "ByteProperty",
            0,
            &[string("ELobbyPrivacy"), vec![0]].concat(),
            &[2],
        ),
        root_value_version(2),
    ];
    let (summary, root, decisions) = stage_owned_root(&fields);
    let utf16 = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    assert_eq!(
        root.prospect_info(),
        &ProspectInfo {
            prospect_id: utf16("private-root-id"),
            claimed_account_id: utf16("private-claim"),
            claimed_account_character: -7,
            prospect_dt_key: utf16("private-root-key"),
            faction_mission_dt_key: utf16("private-mission"),
            lobby_name: utf16("private-lobby"),
            expire_time: i64::MIN,
            prospect_state: 2,
            associated_members: vec![
                AssociatedMember {
                    account_name: utf16("private-account"),
                    character_name: utf16("private-character"),
                    user_id: utf16("private-user"),
                    chr_slot: -3,
                    experience: i32::MAX,
                    status: 12,
                    settled: true,
                    is_currently_playing: true,
                },
                AssociatedMember::default()
            ],
            cost: i32::MIN,
            reward: 31,
            difficulty: 4,
            insurance: true,
            no_respawns: true,
            elapsed_time: 123456,
            selected_drop_point: -22,
            custom_settings: vec![
                CustomGameSetting {
                    setting_row_name: utf16("private-setting-first"),
                    setting_value: -100,
                },
                CustomGameSetting {
                    setting_row_name: vec![],
                    setting_value: 77,
                }
            ],
        }
    );
    assert_eq!(root.prospect_map_name(), utf16("private-map"));
    assert_eq!(root.last_saved_date_time(), Some(i64::MAX));
    assert_eq!(root.loaded_version(), Some(2));
    assert_eq!(root.lobby_privacy(), LobbyPrivacy::Private);
    assert_eq!(root.migration(), StagedRootMigration::CurrentVersion);
    assert_eq!(root.source_enum_values().len(), 3);
    assert_eq!(root.source_enum_values()[0].resolved_value, Some(2));
    assert!(matches!(&root.source_enum_values()[0].source_value,
        StagedRootEnumValue::Name(units) if *units == utf16("EProspectState::Active")));
    assert_eq!(root.source_enum_values()[1].resolved_value, Some(12));
    assert!(matches!(&root.source_enum_values()[1].source_value,
        StagedRootEnumValue::Name(units) if *units == utf16("Prospect_Jungle")));
    assert_eq!(root.source_enum_values()[2].resolved_value, Some(4));
    assert!(matches!(
        root.source_enum_values()[2].source_value,
        StagedRootEnumValue::Byte(4)
    ));
    assert_eq!(summary.prospect_info_tag_count, 17);
    assert_eq!(summary.associated_member_count, 2);
    assert!(decisions.is_empty());

    // Existing summary and decision entry points retain their exact contract.
    let bytes = [fields.concat(), END.to_vec()].concat();
    let parsed = read_root(&bytes, PROFILE).unwrap();
    assert_eq!(
        stage_prospect_save_state(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES, None).unwrap(),
        summary
    );
    assert_eq!(
        stage_prospect_save_state_with_decisions(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES, None)
            .unwrap(),
        (summary.clone(), decisions)
    );
    assert_eq!(
        format!("{root:?}"),
        "StagedRootValues { associated_members: 2, custom_settings: 2, source_enum_values: 3, .. }"
    );
    assert!(!serde_json::to_string(&summary).unwrap().contains("private"));
}

#[test]
fn staged_root_values_preserve_exact_utf16_in_all_owned_strings() {
    let units = [0xd800, 0x61, 0xdc00, 0xdfff];
    let wide = root_value_wide_string(&units);
    let info = root_value_struct(
        "ProspectInfo",
        "ProspectInfo",
        &[
            tag("ProspectID", "StrProperty", 0, &[0], &wide),
            tag("ProspectDTKey", "StrProperty", 0, &[0], &wide),
            tag("FactionMissionDTKey", "StrProperty", 0, &[0], &wide),
            tag("ClaimedAccountID", "StrProperty", 0, &[0], &wide),
            tag("LobbyName", "StrProperty", 0, &[0], &wide),
            root_value_struct_array(
                "AssociatedMembers",
                "AssociatedMemberInfo",
                &[vec![
                    tag("AccountName", "StrProperty", 0, &[0], &wide),
                    tag("CharacterName", "StrProperty", 0, &[0], &wide),
                    tag("UserID", "StrProperty", 0, &[0], &wide),
                ]],
            ),
            root_value_struct_array(
                "CustomSettings",
                "CustomGameSetting",
                &[vec![tag("SettingRowName", "StrProperty", 0, &[0], &wide)]],
            ),
        ],
    );
    let (_, root, _) = stage_owned_root(&[
        root_value_version(2),
        info,
        tag("ProspectMapName", "StrProperty", 0, &[0], &wide),
        tag("ProspectID", "StrProperty", 0, &[0], &wide),
        tag("ProspectDTKey", "StrProperty", 0, &[0], &wide),
    ]);
    assert_eq!(root.prospect_map_name(), units);
    for retained in [
        &root.prospect_info().prospect_id,
        &root.prospect_info().prospect_dt_key,
        &root.prospect_info().faction_mission_dt_key,
        &root.prospect_info().claimed_account_id,
        &root.prospect_info().lobby_name,
        &root.prospect_info().associated_members[0].account_name,
        &root.prospect_info().associated_members[0].character_name,
        &root.prospect_info().associated_members[0].user_id,
        &root.prospect_info().custom_settings[0].setting_row_name,
    ] {
        assert_eq!(retained.as_slice(), units);
    }
    assert_eq!(root.legacy_prospect_id(), Some(units.as_slice()));
    assert_eq!(root.legacy_prospect_dt_key(), Some(units.as_slice()));
}

#[test]
fn staged_root_values_distinguish_omitted_and_present_defaults() {
    let (omitted_summary, omitted, _) = stage_owned_root(&[root_value_version(2)]);
    let (present_summary, present, _) = stage_owned_root(&[
        root_value_version(2),
        tag(
            "LobbyPrivacy",
            "ByteProperty",
            0,
            &[string("ELobbyPrivacy"), vec![0]].concat(),
            &[0],
        ),
        root_value_struct("ProspectInfo", "ProspectInfo", &[]),
        tag("ProspectMapName", "StrProperty", 0, &[0], &string("")),
        root_value_timestamp(0),
        tag("ProspectID", "StrProperty", 0, &[0], &string("")),
        tag("ProspectDTKey", "StrProperty", 0, &[0], &string("")),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            0,
            &[string("StructProperty"), vec![0]].concat(),
            &[0_i32.to_le_bytes().to_vec(), shared(&[])].concat(),
        ),
    ]);
    assert_eq!(omitted.prospect_info(), present.prospect_info());
    assert_eq!(omitted.lobby_privacy(), present.lobby_privacy());
    assert_eq!(omitted.prospect_map_name(), present.prospect_map_name());
    assert_eq!(
        omitted.source_presence(),
        StagedRootPresence {
            version: true,
            ..Default::default()
        }
    );
    assert_eq!(
        present.source_presence(),
        StagedRootPresence {
            version: true,
            lobby_privacy: true,
            prospect_info: true,
            prospect_map_name: true,
            last_saved_date_time: true,
            legacy_prospect_id: true,
            legacy_prospect_dt_key: true,
            state_recorder_blobs: true,
        }
    );
    assert_eq!(omitted.last_saved_date_time(), None);
    assert_eq!(present.last_saved_date_time(), Some(0));
    assert_eq!(omitted.legacy_prospect_id(), None);
    assert_eq!(present.legacy_prospect_id(), Some([].as_slice()));
    assert_eq!(omitted.legacy_prospect_dt_key(), None);
    assert_eq!(present.legacy_prospect_dt_key(), Some([].as_slice()));
    assert!(!omitted_summary.has_last_saved_date_time);
    assert!(present_summary.has_last_saved_date_time);
    assert!(!omitted_summary.has_map_name && !present_summary.has_map_name);
    let (_, absent_version, _) = stage_owned_root(&[]);
    let (_, zero_version, _) = stage_owned_root(&[root_value_version(0)]);
    assert_eq!(absent_version.loaded_version(), None);
    assert_eq!(zero_version.loaded_version(), Some(0));
    assert!(!absent_version.source_presence().version);
    assert!(zero_version.source_presence().version);
    assert_eq!(
        absent_version.migration(),
        StagedRootMigration::Version0Reset
    );
    assert_eq!(zero_version.migration(), absent_version.migration());
}

#[test]
fn staged_root_values_version_zero_resets_info_but_keeps_map_timestamp_and_legacy_source() {
    let map = [0xd800, 0x78, 0xdc00];
    for explicit_version in [false, true] {
        let blobs = fixture(
            1,
            &recorder_element("/Script/Icarus.ActorStateRecorderComponent", &[]),
        );
        let mut fields = vec![
            complete_owned_prospect_info(),
            tag(
                "LobbyPrivacy",
                "ByteProperty",
                0,
                &[string("ELobbyPrivacy"), vec![0]].concat(),
                &[2],
            ),
            tag(
                "ProspectMapName",
                "StrProperty",
                0,
                &[0],
                &root_value_wide_string(&map),
            ),
            root_value_timestamp(i64::MIN),
            tag(
                "ProspectID",
                "StrProperty",
                0,
                &[0],
                &string("private-legacy-id"),
            ),
            tag("ProspectDTKey", "StrProperty", 0, &[0], &string("")),
            blobs[..blobs.len() - END.len()].to_vec(),
        ];
        if explicit_version {
            fields.push(root_value_version(0));
        }
        let (summary, root, decisions) = stage_owned_root(&fields);
        assert_eq!(root.loaded_version(), explicit_version.then_some(0));
        assert_eq!(root.prospect_info(), &ProspectInfo::default());
        assert_eq!(root.lobby_privacy(), LobbyPrivacy::FriendsOnly);
        assert_eq!(root.prospect_map_name(), map);
        assert_eq!(root.last_saved_date_time(), Some(i64::MIN));
        assert_eq!(
            root.legacy_prospect_id(),
            Some(
                "private-legacy-id"
                    .encode_utf16()
                    .collect::<Vec<_>>()
                    .as_slice()
            )
        );
        assert_eq!(root.legacy_prospect_dt_key(), Some([].as_slice()));
        assert_eq!(root.migration(), StagedRootMigration::Version0Reset);
        assert_eq!(
            root.legacy_prospect_id_disposition(),
            StagedLegacyDisposition::AppliedThenReset
        );
        assert_eq!(
            root.legacy_prospect_dt_key_disposition(),
            StagedLegacyDisposition::AppliedThenReset
        );
        assert!(
            root.source_presence().prospect_info && root.source_presence().state_recorder_blobs
        );
        assert_eq!(root.source_enum_values().len(), 3);
        assert_eq!(summary.prospect_info_tag_count, 0);
        assert!(!summary.has_expire_time);
        assert_eq!(summary.parsed_recorder_count, 1);
        assert_eq!(summary.retained_recorder_count, 0);
        assert_eq!(decisions.len(), 1);
        assert!(!decisions[0].retained);
    }
}

#[test]
fn staged_root_values_legacy_precedence_preserves_explicit_empty_and_v2_ignored_sources() {
    for version in [1, 2] {
        for legacy in [None, Some(""), Some("private-legacy")] {
            let mut fields = vec![root_value_version(version), complete_owned_prospect_info()];
            if let Some(value) = legacy {
                fields.push(tag("ProspectID", "StrProperty", 0, &[0], &string(value)));
                fields.push(tag("ProspectDTKey", "StrProperty", 0, &[0], &string(value)));
            }
            let (summary, root, _) = stage_owned_root(&fields);
            let expected_id = if version == 1 {
                legacy.unwrap_or("private-root-id")
            } else {
                "private-root-id"
            };
            let expected_key = if version == 1 {
                legacy.unwrap_or("private-root-key")
            } else {
                "private-root-key"
            };
            assert_eq!(
                root.prospect_info().prospect_id,
                expected_id.encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(
                root.prospect_info().prospect_dt_key,
                expected_key.encode_utf16().collect::<Vec<_>>()
            );
            assert_eq!(
                root.legacy_prospect_id(),
                legacy
                    .map(|value| value.encode_utf16().collect::<Vec<_>>())
                    .as_deref()
            );
            assert_eq!(root.legacy_prospect_dt_key(), root.legacy_prospect_id());
            let disposition = match (version, legacy) {
                (_, None) => StagedLegacyDisposition::Absent,
                (1, Some(_)) => StagedLegacyDisposition::AppliedToProspectInfo,
                _ => StagedLegacyDisposition::IgnoredAtCurrentVersion,
            };
            assert_eq!(root.legacy_prospect_id_disposition(), disposition);
            assert_eq!(root.legacy_prospect_dt_key_disposition(), disposition);
            assert_eq!(summary.has_prospect_id, !expected_id.is_empty());
            assert_eq!(summary.has_prospect_dt_key, !expected_key.is_empty());
            assert_eq!(root.prospect_info().associated_members.len(), 2);
            assert_eq!(root.prospect_info().custom_settings[0].setting_value, -100);
        }
    }
}

#[test]
fn staged_root_values_keep_unqualified_enum_names_and_out_of_range_bytes() {
    let unpaired = [0xd800, 0x71, 0xdc00];
    for kind in ["EnumProperty", "ByteProperty"] {
        for raw_byte in [false, true] {
            if kind == "EnumProperty" && raw_byte {
                continue;
            }
            let state = if raw_byte {
                vec![255]
            } else {
                string("private-unknown-state")
            };
            let difficulty = if raw_byte {
                vec![254]
            } else {
                root_value_wide_string(&unpaired)
            };
            let status = if raw_byte {
                vec![253]
            } else {
                string("EProspectLocation::Prospect_Inhabit")
            };
            let info = root_value_struct(
                "ProspectInfo",
                "ProspectInfo",
                &[
                    tag(
                        "ProspectState",
                        kind,
                        0,
                        &[string("EProspectState"), vec![0]].concat(),
                        &state,
                    ),
                    tag(
                        "Difficulty",
                        kind,
                        0,
                        &[string("EMissionDifficulty"), vec![0]].concat(),
                        &difficulty,
                    ),
                    root_value_struct_array(
                        "AssociatedMembers",
                        "AssociatedMemberInfo",
                        &[vec![tag(
                            "Status",
                            kind,
                            0,
                            &[string("EProspectLocation"), vec![0]].concat(),
                            &status,
                        )]],
                    ),
                ],
            );
            let (_, reset, _) = stage_owned_root(&[root_value_version(0), info.clone()]);
            assert!(reset.resolved_prospect_info().is_some());
            let (_, root, _) = stage_owned_root(&[root_value_version(2), info]);
            assert!(root.resolved_prospect_info().is_none());
            assert_eq!(root.source_enum_values().len(), 3);
            for (index, field) in [
                StagedRootEnumField::ProspectState,
                StagedRootEnumField::Difficulty,
                StagedRootEnumField::AssociatedMemberStatus(0),
            ]
            .into_iter()
            .enumerate()
            {
                let retained = &root.source_enum_values()[index];
                assert_eq!(retained.field, field);
                assert_eq!(
                    retained.source_kind,
                    if kind == "EnumProperty" {
                        TagKind::Enum
                    } else {
                        TagKind::Byte
                    }
                );
                assert_eq!(retained.resolved_value, None);
                if raw_byte {
                    assert!(
                        matches!(retained.source_value, StagedRootEnumValue::Byte(value) if value == 255 - u8::try_from(index).unwrap())
                    );
                } else {
                    let expected = match index {
                        0 => "private-unknown-state".encode_utf16().collect::<Vec<_>>(),
                        1 => unpaired.to_vec(),
                        _ => "EProspectLocation::Prospect_Inhabit"
                            .encode_utf16()
                            .collect::<Vec<_>>(),
                    };
                    assert!(
                        matches!(&retained.source_value, StagedRootEnumValue::Name(units) if *units == expected)
                    );
                }
            }
            if raw_byte {
                assert_eq!(root.prospect_info().prospect_state, 255);
                assert_eq!(root.prospect_info().difficulty, 254);
                assert_eq!(root.prospect_info().associated_members[0].status, 253);
            }
            assert!(!format!("{root:?}").contains("private"));
        }
    }
}

#[test]
fn staged_root_values_fail_closed_without_supplied_diagnostics() {
    let invalid_infos = [
        vec![tag(
            "private-unknown-field",
            "StrProperty",
            0,
            &[0],
            &string("private-value"),
        )],
        vec![tag(
            "Cost",
            "StrProperty",
            0,
            &[0],
            &string("private-value"),
        )],
        vec![
            tag("Cost", "IntProperty", 0, &[0], &1_i32.to_le_bytes()),
            tag("Cost", "IntProperty", 0, &[0], &2_i32.to_le_bytes()),
        ],
        vec![tag("Reward", "IntProperty", 1, &[0], &1_i32.to_le_bytes())],
        vec![tag(
            "ProspectState",
            "EnumProperty",
            0,
            &[string("private-enum-type"), vec![0]].concat(),
            &string("private-value"),
        )],
        vec![root_value_struct_array(
            "AssociatedMembers",
            "AssociatedMemberInfo",
            &[vec![tag(
                "private-unknown-member-field",
                "IntProperty",
                0,
                &[0],
                &7_i32.to_le_bytes(),
            )]],
        )],
        vec![root_value_struct_array(
            "CustomSettings",
            "CustomGameSetting",
            &[vec![tag(
                "SettingValue",
                "IntProperty",
                0,
                &[0],
                &[1, 2, 3],
            )]],
        )],
    ];
    for fields in invalid_infos {
        let bytes = [
            root_value_version(2),
            root_value_struct("ProspectInfo", "ProspectInfo", &fields),
            END.to_vec(),
        ]
        .concat();
        let parsed = read_root(&bytes, PROFILE).unwrap();
        let old_error = stage_prospect_save_state(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES, None)
            .unwrap_err();
        let error = stage_prospect_save_state_with_root_values(
            &parsed.tags,
            PROFILE,
            MAX_COMPONENT_BYTES,
            None,
        )
        .unwrap_err();
        assert_eq!(format!("{old_error:#}"), format!("{error:#}"));
        assert!(!format!("{error:?}").contains("private"));
    }
}

#[test]
fn owned_enum_staging_preserves_field_specific_static_diagnostics() {
    for (field, descriptor, expected) in [
        (
            "ProspectState",
            "EProspectState",
            [
                "ProspectInfo.ProspectState must use EProspectState",
                "invalid ProspectState enum",
                "invalid ProspectState byte enum",
                "ProspectInfo.ProspectState must be EnumProperty or ByteProperty",
            ],
        ),
        (
            "Difficulty",
            "EMissionDifficulty",
            [
                "ProspectInfo.Difficulty must use EMissionDifficulty",
                "invalid Difficulty enum",
                "invalid Difficulty byte enum",
                "ProspectInfo.Difficulty must be EnumProperty or ByteProperty",
            ],
        ),
        (
            "Status",
            "EProspectLocation",
            [
                "AssociatedMemberInfo.Status must use EProspectLocation",
                "invalid AssociatedMemberInfo.Status enum string",
                "invalid AssociatedMemberInfo.Status byte enum",
                "AssociatedMemberInfo.Status must be EnumProperty or ByteProperty",
            ],
        ),
    ] {
        let cases = [
            tag(
                field,
                "EnumProperty",
                0,
                &[string("private-wrong-type"), vec![0]].concat(),
                &string("private-value"),
            ),
            tag(
                field,
                "EnumProperty",
                0,
                &[string(descriptor), vec![0]].concat(),
                &[1, 2],
            ),
            tag(
                field,
                "ByteProperty",
                0,
                &[string(descriptor), vec![0]].concat(),
                &[1, 2],
            ),
            tag(field, "IntProperty", 0, &[0], &0_i32.to_le_bytes()),
        ];
        for (property, expected) in cases.into_iter().zip(expected) {
            let field = if field == "Status" {
                root_value_struct_array(
                    "AssociatedMembers",
                    "AssociatedMemberInfo",
                    &[vec![property]],
                )
            } else {
                property
            };
            let bytes = [
                root_value_version(2),
                root_value_struct("ProspectInfo", "ProspectInfo", &[field]),
                END.to_vec(),
            ]
            .concat();
            let roots = read_root(&bytes, PROFILE).unwrap();
            let error = stage_prospect_save_state_with_root_values(
                &roots.tags,
                PROFILE,
                MAX_COMPONENT_BYTES,
                None,
            )
            .unwrap_err();
            let message = format!("{error:#}");
            assert!(message.contains(expected), "{message}");
            assert!(!message.contains("private"));
        }
    }
}

fn player_character_save(records: &[Vec<u8>]) -> Vec<u8> {
    let member = |user: &str, slot: i32| {
        [
            tag("UserID", "StrProperty", 0, &[0], &string(user)),
            tag("ChrSlot", "IntProperty", 0, &[0], &slot.to_le_bytes()),
            END.to_vec(),
        ]
        .concat()
    };
    let members_shared = tag(
        "AssociatedMembers",
        "StructProperty",
        0,
        &[string("AssociatedMemberInfo"), vec![0; 17]].concat(),
        &[member("user-alpha", 1), member("user-beta", 0)].concat(),
    );
    let members_array = tag(
        "AssociatedMembers",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &[&2_i32.to_le_bytes(), members_shared.as_slice()].concat(),
    );
    let info = tag(
        "ProspectInfo",
        "StructProperty",
        0,
        &[string("ProspectInfo"), vec![0; 17]].concat(),
        &[
            tag("ProspectID", "StrProperty", 0, &[0], &string("p-val")),
            members_array,
            END.to_vec(),
        ]
        .concat(),
    );
    let blobs = tag(
        "StateRecorderBlobs",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &[
            &i32::try_from(records.len()).unwrap().to_le_bytes(),
            shared(&records.concat()).as_slice(),
        ]
        .concat(),
    );
    [
        tag("Version", "IntProperty", 0, &[0], &2_i32.to_le_bytes()),
        info,
        blobs,
        END.to_vec(),
    ]
    .concat()
}

fn player_state_record(version: Option<i32>, user: &str, slot: i32, uid: Option<i32>) -> Vec<u8> {
    let mut tags = Vec::new();
    if let Some(version) = version {
        tags.push(tag(
            "PlayerStateRecorderVersion",
            "IntProperty",
            0,
            &[0],
            &version.to_le_bytes(),
        ));
    }
    tags.push(player_char_id_struct("PlayerCharacterID", user, slot));
    if let Some(uid) = uid {
        tags.push(tag(
            "IcarusActorGUID",
            "IntProperty",
            0,
            &[0],
            &uid.to_le_bytes(),
        ));
    }
    recorder_element(
        "/Script/Icarus.PlayerStateRecorderComponent",
        &tags.concat(),
    )
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn saved_player_characters_keep_only_validated_c026_records_in_order() {
    let player_recorder = recorder_element(
        "/Script/Icarus.PlayerRecorderComponent",
        &[
            tag(
                "PlayerRecorderVersion",
                "IntProperty",
                0,
                &[0],
                &1_i32.to_le_bytes(),
            ),
            player_char_id_struct("PlayerCharacterID", "user-alpha", 1),
            tag(
                "IcarusActorGUID",
                "IntProperty",
                0,
                &[0],
                &70_009_i32.to_le_bytes(),
            ),
        ]
        .concat(),
    );
    let stream = player_character_save(&[
        player_state_record(Some(1), "user-beta", 0, Some(70_001)),
        // Version 0 fails Validate (F-904): not restorable.
        player_state_record(None, "user-beta", 0, Some(70_002)),
        player_recorder,
        player_state_record(Some(1), "user-alpha", 1, Some(70_003)),
        // Not an associated member of the active prospect: Validate rejects it.
        player_state_record(Some(1), "user-missing", 0, Some(70_004)),
        player_state_record(Some(2), "user-alpha", 1, None),
    ]);
    let parsed = read_root(&stream, PROFILE).unwrap();
    let characters = saved_player_characters(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES).unwrap();
    let summary: Vec<_> = characters
        .iter()
        .map(|c| {
            (
                String::from_utf16(&c.player_id).unwrap(),
                c.chr_slot,
                c.icarus_uid,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("user-beta".to_owned(), 0, 70_001),
            ("user-alpha".to_owned(), 1, 70_003),
            ("user-alpha".to_owned(), 1, -1),
        ]
    );
    assert!(!format!("{characters:?}").contains("user-"));

    // The original's Stricmp: ASCII case folds, the slot must be equal, and
    // the first unresolved match wins; restored ones are skipped.
    let found = find_saved_player_character(&characters, &units("USER-BETA"), 0, &[]);
    assert_eq!(found.map(|(i, c)| (i, c.icarus_uid)), Some((0, 70_001)));
    assert!(find_saved_player_character(&characters, &units("user-beta"), 1, &[]).is_none());
    assert!(find_saved_player_character(&characters, &units("user-beta"), 0, &[0]).is_none());
    let found = find_saved_player_character(&characters, &units("user-alpha"), 1, &[1]);
    assert_eq!(found.map(|(i, c)| (i, c.icarus_uid)), Some((2, -1)));
    assert!(find_saved_player_character(&characters, &units("user-betä"), 0, &[]).is_none());
}

#[test]
fn saved_player_characters_reject_malformed_uids_and_accept_empty_saves() {
    let bad_uid = recorder_element(
        "/Script/Icarus.PlayerStateRecorderComponent",
        &[
            tag(
                "PlayerStateRecorderVersion",
                "IntProperty",
                0,
                &[0],
                &1_i32.to_le_bytes(),
            ),
            player_char_id_struct("PlayerCharacterID", "user-beta", 0),
            tag("IcarusActorGUID", "StrProperty", 0, &[0], &string("70001")),
        ]
        .concat(),
    );
    let stream = player_character_save(&[bad_uid]);
    let parsed = read_root(&stream, PROFILE).unwrap();
    assert!(saved_player_characters(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES).is_err());

    let stream = player_character_save(&[]);
    let parsed = read_root(&stream, PROFILE).unwrap();
    assert!(
        saved_player_characters(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES)
            .unwrap()
            .is_empty()
    );
}

fn name_array_tag(field: &str, names: &[&str]) -> Vec<u8> {
    let payload = [
        i32::try_from(names.len()).unwrap().to_le_bytes().to_vec(),
        names.iter().flat_map(|name| string(name)).collect(),
    ]
    .concat();
    tag(
        field,
        "ArrayProperty",
        0,
        &[string("NameProperty"), vec![0]].concat(),
        &payload,
    )
}

fn float3_tag(field: &str, struct_name: &str, values: [f32; 3]) -> Vec<u8> {
    let payload: Vec<u8> = values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    tag(
        field,
        "StructProperty",
        0,
        &[string(struct_name), vec![0; 17]].concat(),
        &payload,
    )
}

fn stomach_tag(entries: &[Vec<u8>]) -> Vec<u8> {
    let shared = tag(
        "StomachContents",
        "StructProperty",
        0,
        &[string("StomachContentSaveData"), vec![0; 17]].concat(),
        &entries.concat(),
    );
    tag(
        "StomachContents",
        "ArrayProperty",
        0,
        &[string("StructProperty"), vec![0]].concat(),
        &[
            i32::try_from(entries.len())
                .unwrap()
                .to_le_bytes()
                .as_slice(),
            shared.as_slice(),
        ]
        .concat(),
    )
}

fn stomach_entry(fields: &[(&str, &str)]) -> Vec<u8> {
    let mut entry: Vec<u8> = fields
        .iter()
        .flat_map(|(field, value)| tag(field, "NameProperty", 0, &[0], &string(value)))
        .collect();
    entry.extend_from_slice(END);
    entry
}

fn int_tag(field: &str, value: i32) -> Vec<u8> {
    tag(field, "IntProperty", 0, &[0], &value.to_le_bytes())
}

fn bool_tag(field: &str, value: bool) -> Vec<u8> {
    tag(field, "BoolProperty", 0, &[u8::from(value), 0], &[])
}

fn restored_player_state_record(extra: &[Vec<u8>]) -> Vec<u8> {
    recorder_element(
        "/Script/Icarus.PlayerStateRecorderComponent",
        &[
            int_tag("PlayerStateRecorderVersion", 1),
            player_char_id_struct("PlayerCharacterID", "user-beta", 0),
            int_tag("IcarusActorGUID", 70_001),
            extra.concat(),
        ]
        .concat(),
    )
}

#[test]
fn saved_player_characters_read_every_restored_field() {
    let record = restored_player_state_record(&[
        float3_tag("Location", "Vector", [100.5, -2.0, 3.25]),
        float3_tag("Rotation", "Rotator", [-10.0, 90.0, 0.0]),
        bool_tag("bIsAlive", true),
        int_tag("Health", 480),
        int_tag("FoodLevel", 900),
        int_tag("WaterLevel", -1),
        int_tag("OxygenLevel", 700),
        int_tag("RadiationLevel", 3),
        stomach_tag(&[
            stomach_entry(&[
                ("FoodRowName", "private-food-a"),
                ("ModifierName", "private-mod-a"),
            ]),
            // Field order is free; an omitted name is empty.
            stomach_entry(&[("ModifierName", "private-mod-b")]),
        ]),
        int_tag("RespawnCount", 2),
        bool_tag("HasGrantedLoadout", true),
        name_array_tag(
            "CosmeticArmourOverrides",
            &["private-armour-a", "private-armour-b"],
        ),
    ]);
    let stream = player_character_save(&[record]);
    let parsed = read_root(&stream, PROFILE).unwrap();
    let characters = saved_player_characters(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES).unwrap();
    assert_eq!(characters.len(), 1);
    assert_eq!(characters[0].icarus_uid, 70_001);
    let state = &characters[0].state;
    assert_eq!(state.location, Some([100.5, -2.0, 3.25]));
    assert_eq!(state.rotation, Some([-10.0, 90.0, 0.0]));
    assert_eq!(state.is_alive, Some(true));
    assert_eq!(state.health, Some(480));
    assert_eq!(
        (
            state.food_level,
            state.water_level,
            state.oxygen_level,
            state.radiation_level
        ),
        (Some(900), Some(-1), Some(700), Some(3))
    );
    assert_eq!(
        state.stomach_contents,
        [
            SavedStomachContent {
                food_row_name: units("private-food-a"),
                modifier_name: units("private-mod-a"),
            },
            SavedStomachContent {
                food_row_name: Vec::new(),
                modifier_name: units("private-mod-b"),
            },
        ]
    );
    assert_eq!(state.respawn_count, Some(2));
    assert_eq!(state.has_granted_loadout, Some(true));
    assert_eq!(
        state.cosmetic_armour_overrides,
        [units("private-armour-a"), units("private-armour-b")]
    );
    let debug = format!("{characters:?}");
    for private in ["user-", "private", "100.5", "480", "900"] {
        assert!(!debug.contains(private), "{debug}");
    }

    // A record without them leaves scalars unset and arrays empty.
    let stream = player_character_save(&[restored_player_state_record(&[])]);
    let parsed = read_root(&stream, PROFILE).unwrap();
    let characters = saved_player_characters(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES).unwrap();
    assert_eq!(characters[0].state, SavedPlayerState::default());
}

#[test]
fn saved_player_characters_reject_malformed_restored_fields() {
    let truncated_names = tag(
        "CosmeticArmourOverrides",
        "ArrayProperty",
        0,
        &[string("NameProperty"), vec![0]].concat(),
        &[2_i32.to_le_bytes().to_vec(), string("private-armour-a")].concat(),
    );
    let malformed = [
        (
            tag("Health", "StrProperty", 0, &[0], &string("480")),
            "Health must be",
        ),
        (
            tag("FoodLevel", "IntProperty", 0, &[0], &[1, 0]),
            "FoodLevel must be",
        ),
        (
            float3_tag("Location", "Rotator", [0.0; 3]),
            "Location must be",
        ),
        (
            tag(
                "Rotation",
                "StructProperty",
                0,
                &[string("Rotator"), vec![0; 17]].concat(),
                &[0; 8],
            ),
            "Rotation must be",
        ),
        (
            tag("bIsAlive", "IntProperty", 0, &[0], &1_i32.to_le_bytes()),
            "invalid BoolProperty tag",
        ),
        (
            stomach_tag(&[stomach_entry(&[("Food", "private-food-a")])]),
            "unmapped StomachContentSaveData property",
        ),
        (
            stomach_tag(&[stomach_entry(&[
                ("FoodRowName", "private-food-a"),
                ("FoodRowName", "private-food-b"),
            ])]),
            "duplicate StomachContentSaveData property",
        ),
        (
            stomach_tag(&[[
                tag(
                    "FoodRowName",
                    "StrProperty",
                    0,
                    &[0],
                    &string("private-food-a"),
                ),
                END.to_vec(),
            ]
            .concat()]),
            "names must be NameProperty",
        ),
        (
            tag(
                "CosmeticArmourOverrides",
                "ArrayProperty",
                0,
                &[string("StrProperty"), vec![0]].concat(),
                &0_i32.to_le_bytes(),
            ),
            "CosmeticArmourOverrides must be",
        ),
        (truncated_names, "CosmeticArmourOverrides must be"),
    ];
    for (field, expected) in malformed {
        let stream = player_character_save(&[restored_player_state_record(&[field])]);
        let parsed = read_root(&stream, PROFILE).unwrap();
        let error =
            saved_player_characters(&parsed.tags, PROFILE, MAX_COMPONENT_BYTES).unwrap_err();
        let error = format!("{error:#}");
        assert!(error.contains(expected), "{error}");
        assert!(!error.contains("private"), "{error}");
    }
}
