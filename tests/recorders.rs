use icarus_bevy::experimental::component::MAX_COMPONENT_BYTES;
use icarus_bevy::experimental::recorders::{
    MAX_RECORDER_FIELDS, MAX_RECORDERS, RecorderArray, lookup_recorder_ctor_defaults,
    read_recorders, stage_prospect_save_state,
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
