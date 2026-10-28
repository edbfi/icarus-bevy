//! Authored profile values, deliberately unrelated to login/save identities.
#![allow(dead_code)]
use icarus_bevy::experimental::{
    control_messages::ArchiveText,
    probe_initialise::{authored_character_profile, authored_user_profile},
    rep_layout::{PropertyStream, RepArray, RepEntry, RepValue},
    rep_rpc::RpcItem,
};

pub struct Fixture {
    pub rpc: Vec<RpcItem>,
    pub properties: Vec<RepValue>,
}

impl Fixture {
    fn new() -> Self {
        Self {
            rpc: Vec::new(),
            properties: Vec::new(),
        }
    }
    /// Authored RPC items with their property values; arrays hold values only.
    fn from_rpc(items: Vec<RpcItem>) -> Self {
        let mut fixture = Self::new();
        for item in items {
            match item {
                RpcItem::Value(value) => fixture.value(value),
                RpcItem::Array(elements) => fixture.array(
                    elements
                        .into_iter()
                        .map(|element| {
                            element
                                .into_iter()
                                .map(|item| match item {
                                    RpcItem::Value(value) => value,
                                    RpcItem::Array(_) => panic!("nested profile array"),
                                })
                                .collect()
                        })
                        .collect(),
                ),
            }
        }
        fixture
    }
    fn value(&mut self, value: RepValue) {
        self.rpc.push(RpcItem::Value(value.clone()));
        self.properties.push(value);
    }
    fn array(&mut self, elements: Vec<Vec<RepValue>>) {
        self.rpc.push(RpcItem::Array(
            elements
                .iter()
                .map(|element| element.iter().cloned().map(RpcItem::Value).collect())
                .collect(),
        ));
        self.properties.push(RepValue::DynamicArray(RepArray {
            count: elements.len() as u16,
            stream: PropertyStream {
                entries: elements
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, value)| RepEntry {
                        handle: index as u32 + 1,
                        checksum: None,
                        value,
                    })
                    .collect(),
                terminator_checksum: None,
            },
        }));
    }
}

fn text(value: &str) -> RepValue {
    RepValue::Str(ArchiveText::text(value).unwrap())
}

/// The probe's authored user profile.
pub fn user() -> Fixture {
    Fixture::from_rpc(authored_user_profile().unwrap())
}

/// The probe's authored character profile, `slot` its ChrSlot.
pub fn character(slot: i32) -> Fixture {
    let fixture = Fixture::from_rpc(authored_character_profile(slot).unwrap());
    assert_eq!(fixture.rpc.len(), 44);
    fixture
}

/// Fully populated prospect, including both array element layouts.
pub fn prospect() -> (
    icarus_bevy::experimental::prospect_info::ProspectInfo,
    Fixture,
) {
    use icarus_bevy::experimental::prospect_info::{
        AssociatedMember, CustomGameSetting, ProspectInfo,
    };
    let units = |value: &str| value.encode_utf16().collect();
    let info = ProspectInfo {
        prospect_id: units("prospect"),
        claimed_account_id: units("owner"),
        claimed_account_character: 3,
        prospect_dt_key: units("row"),
        faction_mission_dt_key: units("mission"),
        lobby_name: units("lobby"),
        expire_time: 123_456_789,
        prospect_state: 2,
        associated_members: vec![AssociatedMember {
            account_name: units("account"),
            character_name: units("character"),
            user_id: units("member"),
            chr_slot: 9,
            experience: -12,
            status: 3,
            settled: true,
            is_currently_playing: false,
        }],
        cost: -4,
        reward: 8,
        difficulty: 4,
        insurance: true,
        no_respawns: true,
        elapsed_time: 321,
        selected_drop_point: 2,
        custom_settings: vec![CustomGameSetting {
            setting_row_name: units("setting"),
            setting_value: -3,
        }],
    };
    let mut fixture = Fixture::new();
    fixture.value(text("prospect"));
    fixture.value(text("owner"));
    fixture.value(RepValue::Int(3));
    for value in ["row", "mission", "lobby"] {
        fixture.value(text(value));
    }
    fixture.value(RepValue::Int64(123_456_789));
    fixture.value(RepValue::Enum(2));
    fixture.array(vec![vec![
        text("account"),
        text("character"),
        text("member"),
        RepValue::Int(9),
        RepValue::Int(-12),
        RepValue::Enum(3),
        RepValue::Bool(true),
        RepValue::Bool(false),
    ]]);
    fixture.value(RepValue::Int(-4));
    fixture.value(RepValue::Int(8));
    fixture.value(RepValue::Enum(4));
    fixture.value(RepValue::Bool(true));
    fixture.value(RepValue::Bool(true));
    fixture.value(RepValue::Int(321));
    fixture.value(RepValue::Int(2));
    fixture.array(vec![vec![text("setting"), RepValue::Int(-3)]]);
    (info, fixture)
}
