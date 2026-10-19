//! EXPERIMENTAL login, connection-state admission, and `SpawnPlayActor` checks
//! mirroring the client and server builds' engine logic.
//!
//! The ICARUS game mode and session add no `PreLogin`, `ApproveLogin`, `Login`,
//! or `InitNewPlayer` checks of their own; the engine's run instead (`F-712`,
//! `F-713`). Exhaustive `.text` and `.rdata`/`.data` cross-reference scans
//! across both `Icarus-Win64-Shipping.exe` (`25793767`) and
//! `IcarusServer-Win64-Shipping.exe` (`25621760`) confirm that neither the
//! engine nor ICARUS game code binds any subscriber or callback to
//! `FGameModeEvents::GameModePreLoginEvent` (`0x146056cf8` / `0x145f404b8`),
//! `FNetworkVersion::GetLocalNetworkVersionOverride` (`0x1463251b0` /
//! `0x1462116a0`), or `FNetworkVersion::IsNetworkCompatibleOverride`
//! (`0x1463251c0` / `0x1462116b0`; `U-037` resolved).
//!
//! This module reproduces the parts a dedicated server decides from its own
//! state and the control-channel `Hello` / `Login` / `Netspeed` / `Join` /
//! `PCSwap` / `ActorChannelFailure` fields:
//! - `UNetConnection::IsClientMsgTypeValid` (`0x143991c70`),
//!   `SendChallengeControlMessage` (`0x143998730`), `WelcomePlayer`
//!   (`0x143cc9170`), and `SetClientLoginState` (`0x14399add0`) gate control
//!   message IDs by `ClientLoginState` (`Connection + 0x1c8`) and
//!   `ExpectedClientLoginMsgType` (`Connection + 0x1cc`): while `LoggingIn`
//!   (`1`), only `ExpectedClientLoginMsgType` (`0` = `Hello` initially, then
//!   `5` = `Login` after `Challenge`) is accepted; once `Welcomed` (`2`) or
//!   `Joined` (`3`), `Hello` (`0`) and `Login` (`5`) are rejected and post-login
//!   messages (`Netspeed = 4`, `Join = 9`, `JoinSplit = 10`, `PCSwap = 15`,
//!   `DebugText = 17`) are accepted.
//! - In `AGameModeBase::PreLogin` (`0x14386a160`), if the `Login` message's
//!   `FUniqueNetIdRepl` wraps a valid `FUniqueNetId`, the engine first queries
//!   `UOnlineEngineInterfaceImpl::IsCompatibleUniqueNetId` (`0x1409cbee0`),
//!   which accepts the ID when its subsystem type matches the default online
//!   subsystem or any entry in `CompatibleUniqueNetIdTypes` (`+0x78`), and
//!   otherwise rejects with [`INCOMPATIBLE_UNIQUE_NET_ID`] before calling
//!   `ApproveLogin`. An invalid/absent `FUniqueNetIdRepl` skips the check.
//! - Options are `?`-delimited from their start; each option splits at its
//!   first `=` (no `=` means an empty value). Keys compare ASCII
//!   case-insensitively and other units exactly; the first match wins
//!   (literals M-591, M-595; helpers M-593, M-594).
//! - Integer options read like the C runtime's `_wtoi`: leading whitespace,
//!   an optional sign, then decimal digits up to the first other unit,
//!   saturating at the `i32` bounds; anything else reads 0. The saturation is
//!   the documented runtime behavior, not an observation of this build.
//! - `SpectatorOnly=1` asks for a spectator slot; a full server answers
//!   [`SERVER_FULL`]. Otherwise a `SplitscreenCount` above the
//!   per-connection limit answers [`SPLITSCREEN_EXCEEDED`].
//! - On `NMT_Netspeed` (`case 4` in `UWorld::NotifyControlMessage` at
//!   `0x1439ef03b` server / `0x143cbab55` client, `F-740`), the engine reads
//!   `int32 Rate`, clamps as signed `i32` via
//!   `(Rate < 1800) ? 1800 : min(Rate, NetDriver->MaxClientRate)` (`0x708 = 1800`,
//!   `MaxClientRate` at `NetDriver + 0x64`), and writes
//!   `Connection->CurrentNetSpeed` (`+0x38`).
//! - On `NMT_Join` (`case 9` in `UWorld::NotifyControlMessage` `0x143cba590`),
//!   `UWorld::SpawnPlayActor` (`0x14392dbb0`) invokes `AGameModeBase::Login`
//!   (`0x143866c80`) -> `ApproveLogin` (`0x14384f9c0`) ->
//!   `SpawnPlayerController` (`0x143872120`) -> `InitNewPlayer` (`0x1438633f0`),
//!   checking `GameSession`, `PlayerState`, and `FindPlayerStart`, parsing
//!   `?bIsFromInvite`, `?SpectatorOnly=1`, and `?Name=` (truncated to 20 UTF-16
//!   units or formatted as `%s%i` from `DefaultPlayerName` and `PlayerId`),
//!   setting `PlayerController->NetPlayerIndex` (`+0x3d4`), and advancing
//!   `ClientLoginState` to `Joined` (`3`).
//! - On `NMT_PCSwap` (`case 15` in `UWorld::NotifyControlMessage` at
//!   `0x1439f031c` server / `0x143cbb7f3` client, `F-713`, `F-740`), a signed
//!   `ChildIndex < 0` selects the parent connection, `0 <= ChildIndex < Children.Num()`
//!   selects `Connection->Children[ChildIndex]` (`+0x1460`), and
//!   `ChildIndex >= Children.Num()` is ignored.
//! - On `NMT_ActorChannelFailure` (`0x10` in `UControlChannel::ReceivedBunch` at
//!   `0x143536420` server / `0x1437c4940` client, `F-740`), the server resolves
//!   `OpenChannels[ChannelIndex]` (`+0x1310`); when it is a non-null
//!   `UActorChannel` whose `Actor` (`+0x68`) equals `Connection->PlayerController`
//!   (`+0x30`), `UNetConnection::Close` (`0x1436de420` / `0x143984190`) is called.

use super::control_messages::{NetIdType, NetIdValue, UniqueNetId};

/// PreLogin's unique-net-id subsystem compatibility rejection (`0x1454b7728`).
pub const INCOMPATIBLE_UNIQUE_NET_ID: &str = "incompatible_unique_net_id";
/// ApproveLogin's capacity rejection.
pub const SERVER_FULL: &str = "Server full.";
/// ApproveLogin's splitscreen rejection.
pub const SPLITSCREEN_EXCEEDED: &str = "Maximum splitscreen players";
/// `AGameModeBase::Login` (`0x143866c80`) rejection when `GameSession` is null (`0x1454b7760`).
pub const GAMESESSION_NULL_ERROR: &str = "Failed to spawn player controller, GameSession is null";
/// `AGameModeBase::Login` (`0x143866c80`) rejection when `SpawnPlayerController` returns null (`0x1454b77d0`).
pub const SPAWN_PLAYER_CONTROLLER_ERROR: &str = "Failed to spawn player controller";
/// `AGameModeBase::InitNewPlayer` (`0x1438633f0`) rejection when `PlayerState` is null (`0x1454b7838`).
pub const PLAYER_STATE_NULL_ERROR: &str = "PlayerState is null";
/// `AGameModeBase::InitNewPlayer` (`0x1438633f0`) rejection when `FindPlayerStart` returns null (`0x1454b7880`).
pub const FIND_PLAYER_START_ERROR: &str = "Failed to find PlayerStart";
/// Maximum UTF-16 units retained from `?Name=` in `AGameModeBase::InitNewPlayer` (`0x1438633f0`, `0x14`).
pub const MAX_PLAYER_NAME_UNITS: usize = 20;
/// Minimum client netspeed floor (`0x708 = 1800`) enforced on `NMT_Netspeed` (`F-740`).
pub const MIN_CLIENT_NETSPEED: i32 = 1_800;
/// Default `UNetDriver::MaxClientRate` (`0x3a98 = 15000`) (`F-740`).
pub const DEFAULT_MAX_CLIENT_RATE: i32 = 15_000;

const OPTION_DELIMITER: u16 = b'?' as u16;
const KEY_SEPARATOR: u16 = b'=' as u16;

/// Clamps a client-requested `NMT_Netspeed` rate (`case 4` in
/// `UWorld::NotifyControlMessage`, `0x1439ef03b` server / `0x143cbab55` client,
/// `F-740`): signed `i32` values below [`MIN_CLIENT_NETSPEED`] (`1800`) return
/// `1800`; values `>= 1800` return `requested_rate.min(max_client_rate)`.
pub const fn clamp_client_netspeed(requested_rate: i32, max_client_rate: i32) -> i32 {
    if requested_rate < MIN_CLIENT_NETSPEED {
        MIN_CLIENT_NETSPEED
    } else if requested_rate < max_client_rate {
        requested_rate
    } else {
        max_client_rate
    }
}

/// Target selected by `UWorld::NotifyControlMessage` `case 15` (`NMT_PCSwap`,
/// `0x1439f031c` server / `0x143cbb7f3` client, `F-713`, `F-740`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcSwapTarget {
    /// `ChildIndex < 0`: calls `Connection->HandleClientPlayer(PreviousPlayerController, Connection)`.
    ParentConnection,
    /// `0 <= ChildIndex < Children.Num()`: calls
    /// `Children[index]->HandleClientPlayer(Children[index]->PreviousPlayerController, Connection)`.
    ChildConnection { index: usize },
}

/// Resolves the `NMT_PCSwap` (`case 15`) target from signed `child_index` and
/// `Connection->Children.Num()` (`+0x1468`), returning `None` when
/// `child_index >= child_connection_count` (`F-713`, `F-740`).
pub fn evaluate_pc_swap_target(
    child_index: i32,
    child_connection_count: usize,
) -> Option<PcSwapTarget> {
    if child_index < 0 {
        return Some(PcSwapTarget::ParentConnection);
    }
    let index = usize::try_from(child_index).ok()?;
    (index < child_connection_count).then_some(PcSwapTarget::ChildConnection { index })
}

/// Server-side disposition of `NMT_ActorChannelFailure` (`0x10`) in
/// `UControlChannel::ReceivedBunch` (`0x143536420` server / `0x1437c4940`
/// client, `F-740`).
///
/// Standalone model of the control-channel branch (`peer_lab` maintains only
/// channel 0 and no `UActorChannel` table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorChannelFailureAction {
    /// Ignored (client side, out-of-range channel index, non-`UActorChannel`,
    /// null `Actor`, or null `Connection->PlayerController`).
    Ignore,
    /// `Ch->Actor == Connection->PlayerController`: calls `UNetConnection::Close`
    /// (`0x1436de420` server / `0x143984190` client).
    CloseConnection,
    /// `Ch->Actor != Connection->PlayerController` with non-null
    /// `Connection->PlayerController`: calls
    /// `PlayerController->NotifyActorChannelFailure(Ch)` (`vtable +0xc50`;
    /// base `APlayerController::NotifyActorChannelFailure` at `0x1408c9710` is a no-op).
    NotifyPlayerController,
}

/// Evaluates `UControlChannel::ReceivedBunch` (`0x143536420` / `0x1437c4940`)
/// on `NMT_ActorChannelFailure` (`0x10`, `F-740`).
///
/// Standalone model of the server-side `OpenChannels` lookup (`peer_lab`
/// operates on control channel 0 only and does not open actor channels).
pub fn evaluate_actor_channel_failure(
    is_server: bool,
    channel_index: i32,
    open_channels_len: usize,
    channel_is_actor_channel: bool,
    channel_has_actor: bool,
    actor_is_player_controller: bool,
    has_player_controller: bool,
) -> ActorChannelFailureAction {
    if !is_server || channel_index < 0 {
        return ActorChannelFailureAction::Ignore;
    }
    let Ok(index) = usize::try_from(channel_index) else {
        return ActorChannelFailureAction::Ignore;
    };
    if index >= open_channels_len || !channel_is_actor_channel || !channel_has_actor {
        return ActorChannelFailureAction::Ignore;
    }
    if actor_is_player_controller {
        ActorChannelFailureAction::CloseConnection
    } else if has_player_controller {
        ActorChannelFailureAction::NotifyPlayerController
    } else {
        ActorChannelFailureAction::Ignore
    }
}

/// `EClientLoginState::Type` stored at `UNetConnection + 0x1c8` (`SetClientLoginState` `0x14399add0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ClientLoginState {
    Invalid = 0,
    LoggingIn = 1,
    Welcomed = 2,
    Joined = 3,
    CleanedUp = 4,
}

/// Per-connection control-message admission state mirroring `UNetConnection::IsClientMsgTypeValid`
/// (`0x143991c70`), `SendChallengeControlMessage` (`0x143998730`), `WelcomePlayer` (`0x143cc9170`),
/// and `UWorld::NotifyControlMessage` (`case 9` `NMT_Join`, `0x143cba590`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionLoginAdmission {
    pub state: ClientLoginState,
    pub expected_login_msg_type: u8,
    pub has_player_controller: bool,
}

impl Default for ConnectionLoginAdmission {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionLoginAdmission {
    /// Initial `UNetConnection` state (`LoggingIn = 1`, expecting `NMT_Hello = 0`).
    pub const fn new() -> Self {
        Self {
            state: ClientLoginState::LoggingIn,
            expected_login_msg_type: 0,
            has_player_controller: false,
        }
    }

    /// `UNetConnection::IsClientMsgTypeValid` (`0x143991c70`): while `LoggingIn`,
    /// only `expected_login_msg_type` is valid; in all other states, `NMT_Hello` (`0`)
    /// and `NMT_Login` (`5`) are rejected and all other control message IDs pass.
    pub const fn is_client_msg_type_valid(&self, msg_type: u8) -> bool {
        if matches!(self.state, ClientLoginState::LoggingIn) {
            msg_type == self.expected_login_msg_type
        } else {
            msg_type != 0 && msg_type != 5
        }
    }

    /// `UNetConnection::SendChallengeControlMessage` (`0x143998730`): advances
    /// `ExpectedClientLoginMsgType` (`+0x1cc`) from `0` (`Hello`) to `5` (`Login`).
    pub fn on_challenge_sent(&mut self) {
        self.expected_login_msg_type = 5;
    }

    /// `UWorld::WelcomePlayer` (`0x143cc9170`): sets `ClientLoginState` (`+0x1c8`)
    /// to `Welcomed` (`2`).
    pub fn on_welcome_sent(&mut self) {
        self.state = ClientLoginState::Welcomed;
    }

    /// `UWorld::NotifyControlMessage` `case 9` (`NMT_Join`, `0x143cba590`):
    /// ignores duplicate `Join` when `Connection->PlayerController` (`+0x30`) is
    /// already non-null (returning `false`), else records the spawned controller
    /// and sets `ClientLoginState` to `Joined` (`3`).
    pub fn on_join_spawned(&mut self) -> bool {
        if self.has_player_controller {
            return false;
        }
        self.has_player_controller = true;
        self.state = ClientLoginState::Joined;
        true
    }
}

/// The value of the first option named `key`, or `None` when absent.
pub fn login_option<'a>(options: &'a [u16], key: &str) -> Option<&'a [u16]> {
    // The engine only reads options that start with the delimiter.
    let rest = options.strip_prefix(&[OPTION_DELIMITER])?;
    rest.split(|unit| *unit == OPTION_DELIMITER)
        .find_map(|option| {
            let (name, value) = match option.iter().position(|unit| *unit == KEY_SEPARATOR) {
                Some(at) => (&option[..at], &option[at + 1..]),
                None => (option, &option[option.len()..]),
            };
            key_matches(name, key).then_some(value)
        })
}

/// Whether `options` contains `key` (with or without `=value`), matching
/// `UGameplayStatics::HasOption` (`0x14385dd70`).
pub fn has_login_option(options: &[u16], key: &str) -> bool {
    login_option(options, key).is_some()
}

/// Resolve the player's initial name as in `AGameModeBase::InitNewPlayer`
/// (`0x1438633f0`): reads `?Name=`, truncating to at most [`MAX_PLAYER_NAME_UNITS`]
/// (`20`) UTF-16 code units, or when empty formats `"{default_player_name}{player_id}"`
/// (`"%s%i"` at `0x14544af60`).
pub fn resolve_player_name(options: &[u16], default_player_name: &str, player_id: i32) -> Vec<u16> {
    if let Some(raw) = login_option(options, "Name") {
        let end = raw.len().min(MAX_PLAYER_NAME_UNITS);
        if end > 0 {
            return raw[..end].to_vec();
        }
    }
    format!("{default_player_name}{player_id}")
        .encode_utf16()
        .collect()
}

fn key_matches(name: &[u16], key: &str) -> bool {
    let key: Vec<u16> = key.encode_utf16().collect();
    name.len() == key.len()
        && name.iter().zip(&key).all(|(left, right)| {
            if (left | right) & 0xff80 != 0 {
                left == right
            } else {
                (*left as u8).eq_ignore_ascii_case(&(*right as u8))
            }
        })
}

/// An integer option, 0 when absent or unparsable.
pub fn int_option(options: &[u16], key: &str) -> i32 {
    login_option(options, key).map_or(0, wtoi)
}

fn wtoi(units: &[u16]) -> i32 {
    let is_space = |unit: u16| matches!(unit, 0x09..=0x0d | 0x20);
    let mut rest = units;
    while let Some((first, tail)) = rest.split_first() {
        if !is_space(*first) {
            break;
        }
        rest = tail;
    }
    let negative = match rest.first() {
        Some(&unit) if unit == u16::from(b'-') => {
            rest = &rest[1..];
            true
        }
        Some(&unit) if unit == u16::from(b'+') => {
            rest = &rest[1..];
            false
        }
        _ => false,
    };
    let mut value: i64 = 0;
    for &unit in rest {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&unit) {
            break;
        }
        let digit = i64::from(unit - u16::from(b'0'));
        // Past the i32 range the value saturates; one more keeps i32::MIN exact.
        value = (value * 10 + digit).min(i64::from(i32::MAX) + 1);
    }
    let value = if negative { -value } else { value };
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

/// Whether `id` wraps a non-empty `FUniqueNetId` (`FUniqueNetId::IsValid`).
pub fn unique_net_id_is_valid(id: &UniqueNetId) -> bool {
    match &id.value {
        NetIdValue::EncodedEmpty | NetIdValue::Absent { .. } => false,
        NetIdValue::Encoded { bytes, .. } => !bytes.is_empty(),
        NetIdValue::Text(text) => !text.units().is_empty(),
    }
}

/// Check `UOnlineEngineInterfaceImpl::IsCompatibleUniqueNetId` (`0x1409cbee0`)
/// as invoked by `AGameModeBase::PreLogin` (`0x14386a160`): an invalid or
/// empty `UniqueNetId` skips the check (`Ok(())`); a valid `Default`-typed ID
/// matches the default subsystem (`Ok(())`); an `Indexed` or `Named` ID must
/// match `default_subsystem` or one of `compatible_subsystems` (ASCII
/// case-insensitively, as `FName` comparison).
pub fn check_unique_net_id_compatible(
    id: &UniqueNetId,
    default_subsystem: Option<&str>,
    compatible_subsystems: &[&str],
    indexed_subsystems: &[(u8, &str)],
) -> Result<(), &'static str> {
    if !unique_net_id_is_valid(id) {
        return Ok(());
    }
    match &id.kind {
        NetIdType::Default => Ok(()),
        NetIdType::Named(name) => {
            let units = name.units();
            if default_subsystem.is_some_and(|default| key_matches(units, default))
                || compatible_subsystems
                    .iter()
                    .any(|allowed| key_matches(units, allowed))
            {
                Ok(())
            } else {
                Err(INCOMPATIBLE_UNIQUE_NET_ID)
            }
        }
        NetIdType::Indexed(index) => {
            let Some((_, resolved)) = indexed_subsystems
                .iter()
                .find(|(candidate, _)| candidate == index)
            else {
                return Err(INCOMPATIBLE_UNIQUE_NET_ID);
            };
            let units: Vec<u16> = resolved.encode_utf16().collect();
            if default_subsystem.is_some_and(|default| key_matches(&units, default))
                || compatible_subsystems
                    .iter()
                    .any(|allowed| key_matches(&units, allowed))
            {
                Ok(())
            } else {
                Err(INCOMPATIBLE_UNIQUE_NET_ID)
            }
        }
    }
}

/// Dedicated-server capacity and splitscreen limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdmissionPolicy {
    /// Players are capped only when positive.
    pub max_players: i32,
    pub max_spectators: i32,
    pub max_splitscreens_per_connection: u8,
}

/// Connected counts at the time of a Login.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Occupancy {
    pub players: i32,
    pub spectators: i32,
}

/// Inputs to `UWorld::SpawnPlayActor` (`0x14392dbb0`) -> `AGameModeBase::Login`
/// (`0x143866c80`) -> `AGameModeBase::InitNewPlayer` (`0x1438633f0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JoinSpawnContext<'a> {
    pub has_game_session: bool,
    pub can_spawn_controller: bool,
    pub has_player_state: bool,
    pub has_player_start: bool,
    pub must_spectate: bool,
    pub default_player_name: &'a str,
    pub player_id: i32,
    pub net_player_index: u8,
}

/// Resolved player controller state produced by `UWorld::SpawnPlayActor`
/// (`0x14392dbb0`) and `AGameModeBase::InitNewPlayer` (`0x1438633f0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitNewPlayerOutcome {
    pub is_from_invite: bool,
    pub spectator_only: bool,
    pub player_name_units: Vec<u16>,
    pub net_player_index: u8,
}

impl AdmissionPolicy {
    /// The engine's ApproveLogin decision: `Err` carries its message.
    pub fn approve_login(&self, options: &[u16], occupancy: Occupancy) -> Result<(), &'static str> {
        let spectator = int_option(options, "SpectatorOnly") == 1;
        if self.at_capacity(spectator, occupancy) {
            return Err(SERVER_FULL);
        }
        if int_option(options, "SplitscreenCount") > i32::from(self.max_splitscreens_per_connection)
        {
            return Err(SPLITSCREEN_EXCEEDED);
        }
        Ok(())
    }

    /// Full `AGameModeBase::PreLogin` (`0x14386a160`) check sequence:
    /// `IsCompatibleUniqueNetId` (when valid), then `AGameSession::ApproveLogin`.
    pub fn pre_login(
        &self,
        options: &[u16],
        unique_net_id: &UniqueNetId,
        occupancy: Occupancy,
        default_subsystem: Option<&str>,
        compatible_subsystems: &[&str],
        indexed_subsystems: &[(u8, &str)],
    ) -> Result<(), &'static str> {
        check_unique_net_id_compatible(
            unique_net_id,
            default_subsystem,
            compatible_subsystems,
            indexed_subsystems,
        )?;
        self.approve_login(options, occupancy)
    }

    /// Full `UWorld::SpawnPlayActor` (`0x14392dbb0`) -> `AGameModeBase::Login`
    /// (`0x143866c80`) -> `AGameModeBase::InitNewPlayer` (`0x1438633f0`) check
    /// and option-resolution sequence on `NMT_Join` / `NMT_JoinSplit`.
    pub fn login_and_init_new_player(
        &self,
        options: &[u16],
        occupancy: Occupancy,
        ctx: JoinSpawnContext<'_>,
    ) -> Result<InitNewPlayerOutcome, &'static str> {
        if !ctx.has_game_session {
            return Err(GAMESESSION_NULL_ERROR);
        }
        self.approve_login(options, occupancy)?;
        if !ctx.can_spawn_controller {
            return Err(SPAWN_PLAYER_CONTROLLER_ERROR);
        }
        if !ctx.has_player_state {
            return Err(PLAYER_STATE_NULL_ERROR);
        }
        let is_from_invite = has_login_option(options, "bIsFromInvite");
        let spectator_only =
            login_option(options, "SpectatorOnly") == Some(&[u16::from(b'1')]) || ctx.must_spectate;
        let player_name_units =
            resolve_player_name(options, ctx.default_player_name, ctx.player_id);
        if !ctx.has_player_start {
            return Err(FIND_PLAYER_START_ERROR);
        }
        Ok(InitNewPlayerOutcome {
            is_from_invite,
            spectator_only,
            player_name_units,
            net_player_index: ctx.net_player_index,
        })
    }

    fn at_capacity(&self, spectator: bool, occupancy: Occupancy) -> bool {
        if spectator {
            occupancy.spectators >= self.max_spectators
        } else {
            self.max_players > 0 && occupancy.players >= self.max_players
        }
    }
}
