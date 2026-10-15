//! EXPERIMENTAL login approval mirroring the client build's engine checks.
//!
//! The ICARUS game mode and session add no PreLogin or ApproveLogin checks of
//! their own; the engine's run instead (F-621). This module reproduces the
//! parts a dedicated server decides from its own state and the Login options:
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
//! global player-cap override, net-id compatibility check and runtime PreLogin
//! delegates are outside this module. Approval sends no Welcome; admission
//! stays gated.

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

    fn at_capacity(&self, spectator: bool, occupancy: Occupancy) -> bool {
        if spectator {
            occupancy.spectators >= self.max_spectators
        } else {
            self.max_players > 0 && occupancy.players >= self.max_players
        }
    }
}
