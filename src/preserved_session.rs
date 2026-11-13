//! Offline consistency assessment of DECLARED saved-session bindings only.
//! No catalog authentication, world import, publication or hosting authority.
use crate::{
    experimental::{
        recorders::{StagedRootMigration, StagedRootValues},
        release::ReleasePin,
        session_open::map_level_chain,
    },
    import_plan::ContentCatalog,
};
use serde::Serialize;

const MAX_UNITS: usize = 1024;
const MAX_URL_BYTES: usize = 512;

/// Explicit independently authored correspondence. No inferred aliases.
/// Deliberately no Debug/Serialize: map and saved values may be private.
pub struct DeclaredBinding<'a> {
    pub saved_map: &'a [u16],
    pub canonical_map_url: &'a str,
    pub release: &'a ReleasePin,
    pub catalog_sha256: [u8; 32],
}

/// Caller-prepared typed catalog context. Matching a supplied digest cannot
/// authenticate its preparation or release provenance.
/// No new table registry, parser or file I/O is introduced here.
pub struct PreparedContext<'a> {
    pub release: &'a ReleasePin,
    pub catalog_sha256: [u8; 32],
    pub authored_catalog: &'a ContentCatalog,
}

/// Optional caller intent must agree exactly with staged effective identity.
/// No Debug/Serialize; no override can repair or replace a saved identity.
#[derive(Default)]
pub struct IdentityOverrides<'a> {
    pub prospect_id: Option<&'a [u16]>,
    pub prospect_row: Option<&'a [u16]>,
    pub lobby_name: Option<&'a [u16]>,
    pub canonical_map_url: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    ResetIdentity,
    UnknownEnums,
    MissingIdentity,
    InvalidUtf16,
    Limit,
    InvalidBinding,
    ReleaseMismatch,
    InvalidRelease,
    CatalogMismatch,
    MapMismatch,
    UndeclaredMap,
    MissingProspectRow,
    ConflictingOverride,
}

/// Borrowed consistency candidate. Never activates or mutates a session.
/// Raw identity access is explicit; default diagnostic output contains counts.
pub struct Candidate<'a> {
    prospect_id: &'a [u16],
    prospect_row: &'a [u16],
    lobby_name: &'a [u16],
    saved_map: &'a [u16],
    canonical_map_url: &'a str,
    difficulty: u8,
    legacy_overlay: bool,
}

/// The only serializable diagnostic DTO: no saved spellings or identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Diagnostics {
    pub prospect_id_units: usize,
    pub prospect_row_units: usize,
    pub lobby_name_units: usize,
    pub saved_map_units: usize,
    pub legacy_overlay: bool,
    pub catalog_authenticated: bool,
    pub world_imported: bool,
}

impl std::fmt::Debug for Candidate<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&self.diagnostics(), f)
    }
}
impl Candidate<'_> {
    pub fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            prospect_id_units: self.prospect_id.len(),
            prospect_row_units: self.prospect_row.len(),
            lobby_name_units: self.lobby_name.len(),
            saved_map_units: self.saved_map.len(),
            legacy_overlay: self.legacy_overlay,
            catalog_authenticated: false,
            world_imported: false,
        }
    }
    pub fn prospect_id(&self) -> &[u16] {
        self.prospect_id
    }
    pub fn prospect_row(&self) -> &[u16] {
        self.prospect_row
    }
    pub fn lobby_name(&self) -> &[u16] {
        self.lobby_name
    }
    pub fn saved_map(&self) -> &[u16] {
        self.saved_map
    }
    pub fn canonical_map_url(&self) -> &str {
        self.canonical_map_url
    }
    pub fn difficulty(&self) -> u8 {
        self.difficulty
    }
}

fn text(units: &[u16]) -> Result<(), Refusal> {
    if units.len() > MAX_UNITS {
        return Err(Refusal::Limit);
    }
    if char::decode_utf16(units.iter().copied()).any(|unit| unit.is_err()) {
        return Err(Refusal::InvalidUtf16);
    }
    Ok(())
}

/// Caller stages with existing strict root APIs before invoking this bounded
/// read-only assessment. This does not decode/clone any recorder or member.
pub fn assess<'a>(
    roots: &'a StagedRootValues,
    binding: &'a DeclaredBinding<'_>,
    context: &PreparedContext<'_>,
    overrides: &IdentityOverrides<'_>,
) -> Result<Candidate<'a>, Refusal> {
    if roots.migration() == StagedRootMigration::Version0Reset {
        return Err(Refusal::ResetIdentity);
    }
    let info = roots
        .resolved_prospect_info()
        .ok_or(Refusal::UnknownEnums)?;
    let saved_map = roots.prospect_map_name();
    for units in [&info.prospect_id, &info.prospect_dt_key, &info.lobby_name] {
        text(units)?;
    }
    text(saved_map)?;
    text(binding.saved_map)?;
    if info.prospect_id.is_empty() || info.prospect_dt_key.is_empty() || saved_map.is_empty() {
        return Err(Refusal::MissingIdentity);
    }
    let url = binding.canonical_map_url;
    if binding.saved_map.is_empty()
        || url.is_empty()
        || url.len() > MAX_URL_BYTES
        || url.contains('?')
        || url.chars().any(char::is_control)
        || url
            .split('/')
            .skip(1)
            .any(|part| part.is_empty() || part == "." || part == "..")
        || map_level_chain(url).is_err()
    {
        return Err(Refusal::InvalidBinding);
    }
    if binding.release.project_version.len() > MAX_URL_BYTES
        || context.release.project_version.len() > MAX_URL_BYTES
    {
        return Err(Refusal::Limit);
    }
    for release in [binding.release, context.release] {
        if release.validate().is_err()
            || release.app_id != crate::experimental::release::DEDICATED_SERVER_APP_ID
            || release.build_id == 0
        {
            return Err(Refusal::InvalidRelease);
        }
    }
    if binding.release != context.release {
        return Err(Refusal::ReleaseMismatch);
    }
    if binding.catalog_sha256 != context.catalog_sha256 {
        return Err(Refusal::CatalogMismatch);
    }
    if saved_map != binding.saved_map {
        return Err(Refusal::MapMismatch);
    }
    if !context.authored_catalog.declares_session_map(saved_map) {
        return Err(Refusal::UndeclaredMap);
    }
    if !context
        .authored_catalog
        .contains_protocol_prospect(&info.prospect_dt_key)
    {
        return Err(Refusal::MissingProspectRow);
    }
    for value in [
        overrides.prospect_id,
        overrides.prospect_row,
        overrides.lobby_name,
    ]
    .into_iter()
    .flatten()
    {
        text(value)?;
    }
    if overrides
        .canonical_map_url
        .is_some_and(|value| value.len() > MAX_URL_BYTES)
    {
        return Err(Refusal::Limit);
    }
    if overrides
        .prospect_id
        .is_some_and(|value| value != info.prospect_id)
        || overrides
            .prospect_row
            .is_some_and(|value| value != info.prospect_dt_key)
        || overrides
            .lobby_name
            .is_some_and(|value| value != info.lobby_name)
        || overrides
            .canonical_map_url
            .is_some_and(|value| value != url)
    {
        return Err(Refusal::ConflictingOverride);
    }
    Ok(Candidate {
        prospect_id: &info.prospect_id,
        prospect_row: &info.prospect_dt_key,
        lobby_name: &info.lobby_name,
        saved_map,
        canonical_map_url: url,
        difficulty: info.difficulty,
        legacy_overlay: roots.migration() == StagedRootMigration::Version1LegacyOverlay,
    })
}
