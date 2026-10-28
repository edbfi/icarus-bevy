//! Native profile exchange and initialisation for pinned controller subobjects.
//! Authored loopback coverage only; no stock-client compatibility qualification.
use anyhow::{Context, Result, ensure};

use super::{
    actor_channel::{
        ActorBunchBody, BlockTarget, ContentBlock, NetGuidRef, SubObjectClass,
        encode_actor_bunch_body,
    },
    control_messages::ArchiveText,
    packet::{BitWriter, Payload},
    rep_layout::{
        PropertyStream, RepArray, RepCommand, RepEntry, RepLayoutCodec, RepValue,
        encode_rep_properties, prospect_info_commands,
    },
    rep_rpc::{
        BACKEND_PROXY_FIELD_INDEX_BOUND, CLIENT_REQUEST_GET_CHARACTER_PROFILE,
        CLIENT_REQUEST_GET_USER_PROFILE, FieldHeader, RpcArgument, RpcItem, RpcParameter,
        backend_profile_response_parameters, encode_rpc_payload, parse_rpc_payload,
        write_field_header,
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProfileKind {
    User,
    Character,
}

#[derive(Clone, Copy)]
pub(crate) enum InitialisationAnswer {
    Loadout,
    Bestiary,
}

/// Private values deliberately have no Debug/status representation.
#[derive(Default)]
pub(crate) struct ProfileExchange {
    pub outstanding: Option<ProfileKind>,
    pub user_failures: u8,
    pub character_failures: u8,
    pub character_slot: Option<i32>,
    pub character_name: Option<ArchiveText>,
    pub initialisation_requested: bool,
    pub has_loadout: bool,
    pub has_bestiary: bool,
    pub completed: bool,
    /// The controller's `ServerFriendsListUpdated` arrived (F-973).
    pub friend_ids_ready: bool,
    /// Finalised: restored or added as a new player (F-973).
    pub finalised: bool,
}

pub(crate) struct ProfileResponse {
    pub kind: ProfileKind,
    pub success: bool,
    pub values: Vec<RepValue>,
}

/// RPC arrays contain complete elements; property streams flatten their
/// elements under sequential handles. Preserve all values, including empties.
fn property_values(items: Vec<RpcItem>) -> Result<Vec<RepValue>> {
    items
        .into_iter()
        .map(|item| match item {
            RpcItem::Value(value) => Ok(value),
            RpcItem::Array(elements) => {
                let count = u16::try_from(elements.len())?;
                let mut entries = Vec::new();
                for element in elements {
                    for value in property_values(element)? {
                        entries.push(RepEntry {
                            handle: u32::try_from(entries.len() + 1)?,
                            checksum: None,
                            value,
                        });
                    }
                }
                Ok(RepValue::DynamicArray(RepArray {
                    count,
                    stream: PropertyStream {
                        entries,
                        terminator_checksum: None,
                    },
                }))
            }
        })
        .collect()
}

pub(crate) fn parse_response(
    payload: &Payload,
    kind: ProfileKind,
    codec: &RepLayoutCodec,
) -> Result<ProfileResponse> {
    let arguments = parse_rpc_payload(
        payload,
        &backend_profile_response_parameters(kind == ProfileKind::Character),
        codec,
    )?;
    let mut arguments = arguments.into_iter();
    let (success, values) = match arguments.next() {
        // An omitted struct retains its initialized Success=false.
        Some(RpcArgument::NotSent) => (false, Vec::new()),
        Some(RpcArgument::Sent(mut items)) => {
            let RpcItem::Value(RepValue::Bool(success)) = items.remove(0) else {
                anyhow::bail!("profile Success is not a bool")
            };
            (success, property_values(items)?)
        }
        _ => anyhow::bail!("profile response needs one struct parameter"),
    };
    Ok(ProfileResponse {
        kind,
        success,
        values,
    })
}

pub(crate) fn request(
    proxy: u32,
    kind: ProfileKind,
    slot: i32,
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    let (index, parameters, arguments) = match kind {
        ProfileKind::User => (CLIENT_REQUEST_GET_USER_PROFILE, vec![], vec![]),
        ProfileKind::Character => (
            CLIENT_REQUEST_GET_CHARACTER_PROFILE,
            vec![RpcParameter::Value(vec![RepCommand::Int])],
            vec![RpcArgument::Sent(vec![RpcItem::Value(RepValue::Int(slot))])],
        ),
    };
    let field = FieldHeader {
        index,
        payload: encode_rpc_payload(&parameters, &arguments, codec)?,
    };
    let mut writer = BitWriter::new();
    write_field_header(&mut writer, &field, BACKEND_PROXY_FIELD_INDEX_BOUND)?;
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::SubObject {
                    object: NetGuidRef::plain(proxy),
                    class: SubObjectClass::StablyNamed,
                },
                payload: Payload::from_writer(writer),
            }],
        },
        &codec.actor,
    )
}

pub(crate) fn character_slot(values: &[RepValue]) -> Result<i32> {
    let Some(RepValue::Int(slot)) = values.get(1) else {
        anyhow::bail!("character profile needs ChrSlot")
    };
    Ok(*slot)
}

/// A reliable PlayerState update for one phase's contiguous handles.
pub(crate) fn player_state_update(
    first_handle: u32,
    values: &[RepValue],
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    let stream = PropertyStream {
        entries: values
            .iter()
            .cloned()
            .enumerate()
            .map(|(offset, value)| RepEntry {
                handle: first_handle + offset as u32,
                checksum: None,
                value,
            })
            .collect(),
        terminator_checksum: None,
    };
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![ContentBlock {
                has_rep_layout: true,
                target: BlockTarget::Actor,
                payload: encode_rep_properties(&stream, layout, codec)?,
            }],
        },
        &codec.actor,
    )
}

pub(crate) fn player_character_id(login_user: &[u16], slot: i32) -> Result<Vec<RepValue>> {
    Ok(vec![
        RepValue::Str(ArchiveText::new(login_user).context("login user ID exceeds codec limits")?),
        RepValue::Int(slot),
    ])
}

/// Convert complete property-array elements into the RPC's flattened items.
fn rpc_items(commands: &[RepCommand], values: &[RepValue]) -> Result<Vec<RpcItem>> {
    ensure!(commands.len() == values.len(), "RPC command count");
    commands
        .iter()
        .zip(values)
        .map(|(command, value)| match (command, value) {
            (RepCommand::DynamicArray { element }, RepValue::DynamicArray(array)) => {
                ensure!(
                    !element.is_empty()
                        && array.stream.entries.len() == usize::from(array.count) * element.len(),
                    "RPC needs complete array elements"
                );
                let elements = array
                    .stream
                    .entries
                    .chunks(element.len())
                    .map(|entries| {
                        let values: Vec<_> =
                            entries.iter().map(|entry| entry.value.clone()).collect();
                        rpc_items(element, &values)
                    })
                    .collect::<Result<Vec<_>>>()?;
                Ok(RpcItem::Array(elements))
            }
            _ => Ok(RpcItem::Value(value.clone())),
        })
        .collect()
}

/// One reliable controller step, in original order. PlayerData's two fields
/// surround the Bestiary block, so they deliberately use separate blocks.
pub(crate) fn initialisation_requests(
    player_data: u32,
    bestiary: u32,
    slot: i32,
    prospect: &[RepValue],
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    use super::rep_rpc::{
        ADD_ASSOCIATED_PROSPECT, BESTIARY_MANAGER_FIELD_INDEX_BOUND, CLIENT_PREPARE_LOADOUT,
        CLIENT_REQUEST_BESTIARY_DATA, PLAYER_DATA_FIELD_INDEX_BOUND,
        add_associated_prospect_parameters, client_prepare_loadout_parameters,
    };
    let host = || -> Result<Vec<RpcItem>> {
        Ok(vec![
            RpcItem::Value(RepValue::Byte(2)),
            RpcItem::Value(RepValue::Str(ArchiveText::new(&[])?)),
            RpcItem::Value(RepValue::Str(ArchiveText::new(&[])?)),
            RpcItem::Value(RepValue::Str(ArchiveText::new(&[])?)),
        ])
    };
    let slot = || RpcArgument::Sent(vec![RpcItem::Value(RepValue::Int(slot))]);
    let mut associated = rpc_items(&prospect_info_commands(), prospect)?;
    associated.extend(host()?);
    let fields = [
        (
            player_data,
            PLAYER_DATA_FIELD_INDEX_BOUND,
            CLIENT_PREPARE_LOADOUT,
            encode_rpc_payload(
                &client_prepare_loadout_parameters(),
                &[slot(), RpcArgument::Sent(host()?)],
                codec,
            )?,
        ),
        (
            bestiary,
            BESTIARY_MANAGER_FIELD_INDEX_BOUND,
            CLIENT_REQUEST_BESTIARY_DATA,
            encode_rpc_payload(&[], &[], codec)?,
        ),
        (
            player_data,
            PLAYER_DATA_FIELD_INDEX_BOUND,
            ADD_ASSOCIATED_PROSPECT,
            encode_rpc_payload(
                &add_associated_prospect_parameters(),
                &[slot(), RpcArgument::Sent(associated)],
                codec,
            )?,
        ),
    ];
    let blocks = fields
        .into_iter()
        .map(|(object, bound, index, payload)| {
            let mut writer = BitWriter::new();
            write_field_header(&mut writer, &FieldHeader { index, payload }, bound)?;
            Ok(ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::SubObject {
                    object: NetGuidRef::plain(object),
                    class: SubObjectClass::StablyNamed,
                },
                payload: Payload::from_writer(writer),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks,
        },
        &codec.actor,
    )
}
