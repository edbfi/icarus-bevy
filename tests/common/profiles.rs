//! Authored profile values, deliberately unrelated to login/save identities.
#![allow(dead_code)]
use icarus_bevy::experimental::{
    control_messages::ArchiveText,
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

pub fn user() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.value(text("authored-profile-user"));
    fixture.array(vec![
        vec![text("Ore"), RepValue::Int(17)],
        vec![text(""), RepValue::Int(-8)],
    ]);
    fixture.array(vec![vec![RepValue::Int(42)], vec![RepValue::Int(-1)]]);
    fixture.array(vec![vec![text("Talent"), RepValue::Int(3)]]);
    fixture.value(RepValue::Int(8));
    fixture.value(RepValue::Int(19));
    fixture
}

pub fn character(slot: i32) -> Fixture {
    let mut fixture = Fixture::new();
    fixture.value(text("authored-character"));
    fixture.value(RepValue::Int(slot));
    fixture.value(RepValue::Int(41));
    fixture.value(RepValue::Int(-99));
    fixture.value(RepValue::Bool(true));
    fixture.value(RepValue::Bool(false));
    fixture.value(text("authored-prospect"));
    fixture.value(RepValue::Enum(1));
    fixture.array(vec![vec![RepValue::Int(9)], vec![RepValue::Int(-7)]]);
    fixture.array(vec![vec![text("Skill"), RepValue::Int(13)]]);
    for value in -10..0 {
        fixture.value(RepValue::Int(value));
    }
    fixture.value(RepValue::Bool(true));
    for value in 21..26 {
        fixture.value(RepValue::Int(value));
    }
    fixture.value(RepValue::Bool(false));
    fixture.value(RepValue::Int(27));
    for value in [1.5_f32, -3.25, 0.0] {
        fixture.value(RepValue::Float(value.to_bits()));
    }
    for value in 31..38 {
        fixture.value(RepValue::Int(value));
    }
    for value in [0.25_f32, -0.0, 100.5, 1.0] {
        fixture.value(RepValue::Float(value.to_bits()));
    }
    fixture.array(vec![
        vec![text("Effect"), RepValue::Int(-12)],
        vec![text("Other"), RepValue::Int(6)],
    ]);
    fixture.value(RepValue::Int64(-9_876_543_210));
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
