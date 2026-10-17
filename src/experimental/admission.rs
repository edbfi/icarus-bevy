//! EXPERIMENTAL login approval mirroring the client build's engine checks.
//!
//! The ICARUS game mode and session add no PreLogin or ApproveLogin checks of
//! their own; the engine's run instead (F-712). This module reproduces
//! the parts a dedicated server decides from its own state and the Login
//! fields:
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
//!
//! Capacity follows the engine's dedicated-server branch: players are capped
//! only by a positive maximum, spectators always by theirs. The engine's
//! global player-cap override and runtime `GameModePreLoginEvent` delegates
//! (`0x146056cf8`) are outside this module. Approval sends no Welcome;
//! admission stays gated.

use super::control_messages::{NetIdType, NetIdValue, UniqueNetId};

/// PreLogin's unique-net-id subsystem compatibility rejection (`0x1454b7728`).
pub const INCOMPATIBLE_UNIQUE_NET_ID: &str = "incompatible_unique_net_id";
/// ApproveLogin's capacity rejection.
pub const SERVER_FULL: &str = "Server full.";
/// ApproveLogin's splitscreen rejection.
pub const SPLITSCREEN_EXCEEDED: &str = "Maximum splitscreen players";

const OPTION_DELIMITER: u16 = b'?' as u16;
const KEY_SEPARATOR: u16 = b'=' as u16;

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

    /// Full `AGameModeBase::PreLogin` (`0x14386a160`) check sequence before
    /// `GameModePreLoginEvent`: `IsCompatibleUniqueNetId` (when valid), then
    /// `AGameSession::ApproveLogin`.
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

    fn at_capacity(&self, spectator: bool, occupancy: Occupancy) -> bool {
        if spectator {
            occupancy.spectators >= self.max_spectators
        } else {
            self.max_players > 0 && occupancy.players >= self.max_players
        }
    }
}
