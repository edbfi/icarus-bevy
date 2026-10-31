//! The untagged little-endian `ProspectInfo_s` archive observed in one
//! retained server advertisement. This is distinct from the replicated
//! property stream; selected drop point and custom settings were absent.

use anyhow::{Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::experimental::{
    archive::{ByteOrder, encode_string},
    prospect_info::ProspectInfo,
};

// Advert permits at most 4096 base64 bytes, including padding.
const MAX_ARCHIVE_BYTES: usize = 4096 / 4 * 3;
const EMPTY_ARCHIVE_BYTES: usize = 58;
const EMPTY_MEMBER_BYTES: usize = 29;
const MAX_MEMBERS: usize = (MAX_ARCHIVE_BYTES - EMPTY_ARCHIVE_BYTES) / EMPTY_MEMBER_BYTES;

struct Archive(Vec<u8>);

impl Archive {
    fn check_room(&self, additional: usize) -> Result<()> {
        ensure!(
            additional <= MAX_ARCHIVE_BYTES - self.0.len(),
            "prospect advertisement exceeds its byte limit"
        );
        Ok(())
    }

    fn bytes(&mut self, bytes: &[u8]) -> Result<()> {
        self.check_room(bytes.len())?;
        self.0.extend_from_slice(bytes);
        Ok(())
    }

    fn string(&mut self, units: &[u16]) -> Result<()> {
        // Bound arithmetic, scanning and allocation before encoding caller text.
        ensure!(
            units.len() <= MAX_ARCHIVE_BYTES,
            "prospect advertisement string exceeds its unit limit"
        );
        let width = if units.iter().any(|unit| *unit > 127) {
            2
        } else {
            1
        };
        let size = 4 + if units.is_empty() {
            0
        } else {
            (units.len() + 1) * width
        };
        self.check_room(size)?;
        self.bytes(&encode_string(units, ByteOrder::Little)?)
    }

    fn boolean(&mut self, value: bool) -> Result<()> {
        self.bytes(&u32::from(value).to_le_bytes())
    }
}

/// Encode caller-authored prospect state as the observed `ProspectInfo_s`
/// base64 archive, bounded to [`super::Advert`]'s 4096-byte rule limit.
///
/// This observed format omits `selected_drop_point` and `custom_settings`.
/// It does not validate prospect row names or establish client acceptance.
/// Errors contain no prospect or member values.
pub fn encode_prospect_info(info: &ProspectInfo) -> Result<String> {
    ensure!(
        info.associated_members.len() <= MAX_MEMBERS,
        "prospect advertisement exceeds its member limit"
    );
    info.validate_enumerators()?;
    let mut archive = Archive(Vec::with_capacity(MAX_ARCHIVE_BYTES));
    archive.string(&info.prospect_id)?;
    archive.string(&info.claimed_account_id)?;
    archive.bytes(&info.claimed_account_character.to_le_bytes())?;
    archive.string(&info.prospect_dt_key)?;
    archive.string(&info.faction_mission_dt_key)?;
    archive.string(&info.lobby_name)?;
    archive.bytes(&info.expire_time.to_le_bytes())?;
    archive.bytes(&[info.prospect_state])?;
    archive.bytes(&info.cost.to_le_bytes())?;
    archive.bytes(&info.reward.to_le_bytes())?;
    archive.bytes(&[info.difficulty])?;
    archive.boolean(info.insurance)?;
    archive.boolean(info.no_respawns)?;
    archive.bytes(&info.elapsed_time.to_le_bytes())?;
    archive.bytes(&i32::try_from(info.associated_members.len())?.to_le_bytes())?;
    for member in &info.associated_members {
        archive.string(&member.account_name)?;
        archive.string(&member.character_name)?;
        archive.string(&member.user_id)?;
        archive.bytes(&member.chr_slot.to_le_bytes())?;
        archive.bytes(&member.experience.to_le_bytes())?;
        archive.bytes(&[member.status])?;
        archive.boolean(member.settled)?;
        archive.boolean(member.is_currently_playing)?;
    }
    Ok(STANDARD.encode(archive.0))
}
