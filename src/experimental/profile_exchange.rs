//! Native profile exchange for the pinned BackendProxy subobject. Authored
//! loopback coverage only; this does not qualify stock-client compatibility.
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
        encode_rep_properties,
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

/// Private values deliberately have no Debug/status representation.
#[derive(Default)]
pub(crate) struct ProfileExchange {
    pub outstanding: Option<ProfileKind>,
    pub user_failures: u8,
    pub character_failures: u8,
    pub user: Vec<RepValue>,
    pub character: Vec<RepValue>,
    pub character_slot: Option<i32>,
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

/// One reliable PlayerState update carries both profiles and PlayerCharacterID.
pub(crate) fn player_state_update(
    user: &[RepValue],
    character: &[RepValue],
    login_user: &[u16],
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<Payload> {
    ensure!(
        user.len() == 6 && character.len() == 44,
        "profile command count"
    );
    let slot = character_slot(character)?;
    let player_id = ArchiveText::new(login_user).context("login user ID exceeds codec limits")?;
    let values = user
        .iter()
        .chain(character)
        .cloned()
        .chain([RepValue::Str(player_id), RepValue::Int(slot)]);
    let stream = PropertyStream {
        entries: values
            .enumerate()
            .map(|(offset, value)| RepEntry {
                handle: 27 + offset as u32,
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
