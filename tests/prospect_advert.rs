use base64::{Engine as _, engine::general_purpose::STANDARD};
use icarus_bevy::{
    experimental::prospect_info::{AssociatedMember, CustomGameSetting, ProspectInfo},
    steam_presence::encode_prospect_info,
};

fn bytes(info: &ProspectInfo) -> Vec<u8> {
    STANDARD
        .decode(encode_prospect_info(info).unwrap())
        .unwrap()
}

#[test]
fn authored_archive_has_exact_order_widths_and_member_tail() {
    let info = ProspectInfo {
        prospect_id: vec![80],
        claimed_account_id: vec![65],
        claimed_account_character: -2,
        prospect_dt_key: vec![68],
        faction_mission_dt_key: vec![70],
        lobby_name: vec![76],
        expire_time: 0x0807_0605_0403_0201,
        prospect_state: 2,
        cost: -3,
        reward: 0x1413_1211,
        difficulty: 3,
        insurance: true,
        no_respawns: false,
        elapsed_time: 0x2423_2221,
        associated_members: vec![AssociatedMember {
            account_name: vec![97],
            character_name: vec![99],
            user_id: vec![117],
            chr_slot: -4,
            experience: 0x3433_3231,
            status: 2,
            settled: false,
            is_currently_playing: true,
        }],
        ..Default::default()
    };
    let expected = vec![
        2, 0, 0, 0, 80, 0, // ProspectID
        2, 0, 0, 0, 65, 0, // ClaimedAccountID
        254, 255, 255, 255, // ClaimedAccountCharacter
        2, 0, 0, 0, 68, 0, // ProspectDTKey
        2, 0, 0, 0, 70, 0, // FactionMissionDTKey
        2, 0, 0, 0, 76, 0, // LobbyName
        1, 2, 3, 4, 5, 6, 7, 8, // ExpireTime
        2, // ProspectState
        253, 255, 255, 255, // Cost
        17, 18, 19, 20, // Reward
        3,  // Difficulty
        1, 0, 0, 0, // Insurance BOOL32
        0, 0, 0, 0, // NoRespawns BOOL32
        33, 34, 35, 36, // ElapsedTime
        1, 0, 0, 0, // AssociatedMembers count
        2, 0, 0, 0, 97, 0, // AccountName
        2, 0, 0, 0, 99, 0, // CharacterName
        2, 0, 0, 0, 117, 0, // UserID
        252, 255, 255, 255, // ChrSlot
        49, 50, 51, 52, // Experience
        2,  // Status
        0, 0, 0, 0, // Settled BOOL32
        1, 0, 0, 0, // IsCurrentlyPlaying BOOL32; EOF
    ];
    assert_eq!(bytes(&info), expected);
}

#[test]
fn empty_and_unicode_strings_and_empty_member_arrays_use_archive_encoding() {
    assert_eq!(bytes(&ProspectInfo::default()), vec![0; 58]);
    let info = ProspectInfo {
        prospect_id: vec![0x03a9],
        ..Default::default()
    };
    let mut expected = vec![254, 255, 255, 255, 0xa9, 0x03, 0, 0];
    expected.extend_from_slice(&[0; 54]);
    assert_eq!(bytes(&info), expected);
}

#[test]
fn drop_point_and_custom_settings_are_not_part_of_the_advert_archive() {
    let info = ProspectInfo {
        selected_drop_point: 42,
        custom_settings: vec![CustomGameSetting {
            setting_row_name: "authored-setting".encode_utf16().collect(),
            setting_value: -9,
        }],
        ..Default::default()
    };
    assert_eq!(bytes(&info), vec![0; 58]);
}

#[test]
fn multiple_members_preserve_unicode_empty_strings_and_order() {
    let info = ProspectInfo {
        associated_members: vec![
            AssociatedMember {
                account_name: vec![0x03a9],
                settled: true,
                ..Default::default()
            },
            AssociatedMember {
                is_currently_playing: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let mut expected = vec![0; 54];
    expected.extend_from_slice(&[
        2, 0, 0, 0, // Two members
        254, 255, 255, 255, 0xa9, 3, 0, 0, // Wide AccountName
        0, 0, 0, 0, // Empty CharacterName
        0, 0, 0, 0, // Empty UserID
        0, 0, 0, 0, // ChrSlot
        0, 0, 0, 0, // Experience
        0, // Status
        1, 0, 0, 0, // Settled
        0, 0, 0, 0, // IsCurrentlyPlaying
        0, 0, 0, 0, // Second member: empty AccountName
        0, 0, 0, 0, // Empty CharacterName
        0, 0, 0, 0, // Empty UserID
        0, 0, 0, 0, // ChrSlot
        0, 0, 0, 0, // Experience
        0, // Status
        0, 0, 0, 0, // Settled
        1, 0, 0, 0, // IsCurrentlyPlaying
    ]);
    assert_eq!(bytes(&info), expected);
}

#[test]
fn byte_and_member_limits_bound_base64_output() {
    let mut info = ProspectInfo {
        prospect_id: vec![65; 3013],
        ..Default::default()
    };
    assert_eq!(encode_prospect_info(&info).unwrap().len(), 4096);
    info.prospect_id.push(65);
    assert!(encode_prospect_info(&info).is_err());
    info.prospect_id = vec![0x03a9; 1506];
    assert_eq!(encode_prospect_info(&info).unwrap().len(), 4096);
    info.prospect_id.push(0x03a9);
    assert!(encode_prospect_info(&info).is_err());
    info.prospect_id = vec![65; 4097];
    assert!(encode_prospect_info(&info).is_err());
    info.prospect_id.clear();
    info.associated_members = vec![AssociatedMember::default(); 103];
    assert!(encode_prospect_info(&info).is_ok());
    info.associated_members.push(AssociatedMember::default());
    assert!(encode_prospect_info(&info).is_err());
}

#[test]
fn invalid_enums_and_archive_text_fail_without_echoing_values() {
    for info in [
        ProspectInfo {
            prospect_state: 255,
            ..Default::default()
        },
        ProspectInfo {
            difficulty: 255,
            ..Default::default()
        },
        ProspectInfo {
            associated_members: vec![AssociatedMember {
                status: 255,
                ..Default::default()
            }],
            ..Default::default()
        },
    ] {
        assert!(encode_prospect_info(&info).is_err());
    }
    for invalid in [0, 0xffff] {
        let mut secret: Vec<u16> = "authored-private-marker".encode_utf16().collect();
        secret.push(invalid);
        let info = ProspectInfo {
            associated_members: vec![AssociatedMember {
                account_name: secret,
                ..Default::default()
            }],
            ..Default::default()
        };
        let error = encode_prospect_info(&info).unwrap_err().to_string();
        assert!(!error.contains("authored-private-marker"));
    }
}
