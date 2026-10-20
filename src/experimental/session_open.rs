//! Native composition of the server's first post-Join NetGUID exports.
//!
//! The original's first bunch after Join opens the PlayerController channel;
//! its first partial part carries an export prefix (`F-778`):
//!
//! - the archetype (`Default__<Class>_C`, its class network checksum) with its
//!   package as outer (checksum `0`) and the null GUID above that;
//! - the level (`PersistentLevel`, world, map package), each `no_load`, `0`;
//! - every replicated component in serialization order: a dynamic GUID with
//!   its name, `no_load` and its class checksum, whose outer is the actor's
//!   dynamic GUID written without a path but with `no_load` and the bare
//!   checksum flag (`F-692`).
//!
//! GUIDs follow a deterministic policy that reproduces the capture's numbering:
//! static GUIDs are odd and assigned leaf-first along an outer chain, dynamic
//! GUIDs are even and assigned in serialization order, and each connection
//! exports a given static path once. Identifiers and checksums are caller
//! data (per release); nothing here reads original files. Checksums come from
//! [`super::class_checksum`].
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};

use super::actor_channel::{
    ActorChannelCodec, ExportPath, GuidExport, NetGuidRef, write_export_prefix,
};
use super::control_messages::ArchiveText;
use super::packet::{BitWriter, Payload};

/// One segment of a static object's outer chain, leaf first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSegment {
    pub path: String,
    /// The class network checksum for objects; `0` for packages and levels.
    pub checksum: u32,
    pub no_load: bool,
}

impl PathSegment {
    pub fn new(path: &str, checksum: u32, no_load: bool) -> Self {
        Self {
            path: path.to_owned(),
            checksum,
            no_load,
        }
    }
}

/// A replicated component of the PlayerController, in serialization order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentExport {
    pub name: String,
    pub checksum: u32,
}

/// Release-specific identifiers for the PlayerController open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerControllerProfile {
    /// `Default__<Class>_C`, then its package.
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    pub components: Vec<ComponentExport>,
}

/// NetGUID assignment shared by the driver, plus per-connection export state.
#[derive(Clone, Debug, Default)]
pub struct NetGuidCache {
    static_index: u32,
    dynamic_index: u32,
    statics: BTreeMap<Vec<String>, u32>,
    exported: BTreeSet<u32>,
}

/// GUIDs assigned while composing a PlayerController open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerControllerGuids {
    pub actor: u32,
    pub archetype: u32,
    pub level: u32,
    pub player_state: u32,
    pub components: Vec<u32>,
}

impl NetGuidCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Next dynamic GUID (even, from 2).
    pub fn assign_dynamic(&mut self) -> u32 {
        self.dynamic_index += 1;
        self.dynamic_index << 1
    }

    /// The static GUID of the leaf of `chain` (leaf first), assigning
    /// unassigned segments leaf-first (odd, from 3).
    pub fn assign_static(&mut self, chain: &[PathSegment]) -> u32 {
        let mut leaf = 0;
        for depth in 0..chain.len() {
            let key: Vec<String> = chain[depth..].iter().map(|s| s.path.clone()).collect();
            let guid = match self.statics.get(&key) {
                Some(guid) => *guid,
                None => {
                    self.static_index += 1;
                    let guid = (self.static_index << 1) | 1;
                    self.statics.insert(key, guid);
                    guid
                }
            };
            if depth == 0 {
                leaf = guid;
            }
        }
        leaf
    }

    /// The reference for a static chain: a full export (with outers) the first
    /// time this connection sees the leaf, a plain reference afterwards.
    pub fn export_static(&mut self, chain: &[PathSegment]) -> Result<NetGuidRef> {
        let guid = self.assign_static(chain);
        if !self.exported.insert(guid) {
            return Ok(NetGuidRef::plain(guid));
        }
        let segment = chain.first().context("empty static chain")?;
        let outer = if chain.len() > 1 {
            self.export_static(&chain[1..])?
        } else {
            NetGuidRef::null()
        };
        Ok(NetGuidRef {
            guid,
            export: Some(GuidExport {
                no_load: segment.no_load,
                path: Some(ExportPath {
                    outer: Box::new(outer),
                    path: ArchiveText::text(&segment.path)?,
                }),
                checksum: Some(segment.checksum),
                bare_checksum_flag: false,
            }),
        })
    }

    /// A component (subobject) export under a dynamic actor GUID.
    pub fn export_subobject(
        &mut self,
        actor: u32,
        component: &ComponentExport,
    ) -> Result<(u32, NetGuidRef)> {
        let guid = self.assign_dynamic();
        self.exported.insert(guid);
        let outer = NetGuidRef {
            guid: actor,
            export: Some(GuidExport {
                no_load: true,
                path: None,
                checksum: None,
                bare_checksum_flag: true,
            }),
        };
        let reference = NetGuidRef {
            guid,
            export: Some(GuidExport {
                no_load: true,
                path: Some(ExportPath {
                    outer: Box::new(outer),
                    path: ArchiveText::text(&component.name)?,
                }),
                checksum: Some(component.checksum),
                bare_checksum_flag: false,
            }),
        };
        Ok((guid, reference))
    }

    /// Register the level chain at world start, as the original does before
    /// any player joins.
    pub fn register_level(&mut self, level: &[PathSegment]) -> u32 {
        self.assign_static(level)
    }

    /// Compose the export references of the PlayerController open in the
    /// original's serialization order: actor, archetype, level, the player
    /// state (dynamic, not exported), then the components.
    pub fn player_controller_exports(
        &mut self,
        profile: &PlayerControllerProfile,
    ) -> Result<(PlayerControllerGuids, Vec<NetGuidRef>)> {
        let actor = self.assign_dynamic();
        let archetype_ref = self.export_static(&profile.archetype)?;
        let level_ref = self.export_static(&profile.level)?;
        let player_state = self.assign_dynamic();
        let mut references = vec![archetype_ref.clone(), level_ref.clone()];
        let mut components = Vec::with_capacity(profile.components.len());
        for component in &profile.components {
            let (guid, reference) = self.export_subobject(actor, component)?;
            components.push(guid);
            references.push(reference);
        }
        Ok((
            PlayerControllerGuids {
                actor,
                archetype: archetype_ref.guid,
                level: level_ref.guid,
                player_state,
                components,
            },
            references,
        ))
    }
}

/// Encode an export-only prefix (`ExportNetGUIDHeader` + `ExportNetGUID`).
pub fn encode_exports(references: &[NetGuidRef], codec: &ActorChannelCodec) -> Result<Payload> {
    let mut writer = BitWriter::new();
    write_export_prefix(&mut writer, references, codec)?;
    Ok(Payload::from_writer(writer))
}
