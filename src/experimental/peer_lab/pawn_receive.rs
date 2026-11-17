//! Bounded, numeric-only receive metadata for client bunches on a joined
//! peer's pawn channel. Diagnostics only: framing runs after the bunches'
//! existing handling, reads nothing it retains beyond field indices, sizes,
//! counts and fixed enums, and never alters what the session applies,
//! acknowledges, sends or persists. No GUIDs, names, addresses, payloads,
//! sequences or timestamps are retained or reported.
use super::super::{
    actor_channel::{ActorChannelCodec, BlockTarget, read_client_actor_bunch_body},
    actor_open::ComponentActorGuids,
    packet::Bunch,
    packet_lab::{ReliableActorDrop, ReliableActorDropReason},
    pawn_open::PawnOpenPlan,
    rep_rpc::{
        BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND, SERVER_MOVE_PACKED_FIELD_INDEX,
        read_field_header,
    },
};

/// Distinct non-movement field rows retained per table.
pub const PAWN_FIELD_ENTRIES: usize = 64;
/// Distinct skipped-block rows retained per table.
pub const PAWN_SKIPPED_BLOCK_ENTRIES: usize = 16;
/// Sub-objects of a committed pawn open retained for relation lookup.
const PAWN_MAPPING_ENTRIES: usize = 64;

/// Which object of the pawn's channel a block addressed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum PawnFieldTarget {
    Actor,
    Subobject,
}

/// A sub-object's relation to this connection's committed pawn open, not a
/// decoded class or RPC.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum PawnSubobjectRelation {
    /// A component the open serialized; the ordinal is its plan position.
    PawnComponent,
    /// A runtime inventory the open created; the ordinal is its content position.
    PawnInventory,
    /// Not among the open's retained components or inventories.
    Unmapped,
    /// No open was committed, or the bounded mapping may omit this target.
    MappingOverflow,
}

/// What the server did with a framed field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum PawnFieldDisposition {
    /// The existing move handling took it as a move.
    Recognized,
    /// Framed, not read by the server.
    Ignored,
    /// The existing move handling refused it (alone or with its bunch).
    Malformed,
}

/// Why a block's fields were not framed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
pub enum PawnSkipReason {
    /// A client property (rep-layout) block; property streams are not read.
    PropertyBlock,
    /// No release-provided field-index bound exists for the target's class.
    NoFieldIndexBound,
}

/// One distinct framed field key with its occurrence count.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PawnFieldSummary {
    pub target: PawnFieldTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<PawnSubobjectRelation>,
    /// Position in the committed open's plan or content, never an identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u8>,
    pub index: u32,
    pub payload_bits: u64,
    pub reliable: bool,
    pub disposition: PawnFieldDisposition,
    pub count: u64,
}

/// The pawn's own move field (actor field 39): one row per reliability and
/// disposition, never charged to the field table or its overflow.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PawnMoveFieldSummary {
    pub index: u32,
    pub reliable: bool,
    pub disposition: PawnFieldDisposition,
    /// Largest framed payload among the counted occurrences.
    pub max_payload_bits: u64,
    pub count: u64,
}

/// One distinct skipped block key with its occurrence count.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct PawnSkippedBlockSummary {
    pub target: PawnFieldTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relation: Option<PawnSubobjectRelation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u8>,
    pub reason: PawnSkipReason,
    pub has_rep_layout: bool,
    pub payload_bits: u64,
    pub reliable: bool,
    pub count: u64,
}

/// Reliable pawn-channel bunches the transport dropped before the session.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct PawnTransportDrops {
    pub queue_refusal: u64,
    pub package_map_exports: u64,
    pub must_be_mapped_guids: u64,
    pub other_unsupported_header: u64,
    pub partial_policy: u64,
    pub partial_reassembly: u64,
}

impl PawnTransportDrops {
    fn record(&mut self, reason: ReliableActorDropReason) {
        let counter = match reason {
            ReliableActorDropReason::QueueRefusal => &mut self.queue_refusal,
            ReliableActorDropReason::PackageMapExports => &mut self.package_map_exports,
            ReliableActorDropReason::MustBeMappedGuids => &mut self.must_be_mapped_guids,
            ReliableActorDropReason::OtherUnsupportedHeader => &mut self.other_unsupported_header,
            ReliableActorDropReason::PartialPolicy => &mut self.partial_policy,
            ReliableActorDropReason::PartialReassembly => &mut self.partial_reassembly,
        };
        *counter = counter.saturating_add(1);
    }

    fn merge(&mut self, other: &Self) {
        self.queue_refusal = self.queue_refusal.saturating_add(other.queue_refusal);
        self.package_map_exports = self
            .package_map_exports
            .saturating_add(other.package_map_exports);
        self.must_be_mapped_guids = self
            .must_be_mapped_guids
            .saturating_add(other.must_be_mapped_guids);
        self.other_unsupported_header = self
            .other_unsupported_header
            .saturating_add(other.other_unsupported_header);
        self.partial_policy = self.partial_policy.saturating_add(other.partial_policy);
        self.partial_reassembly = self
            .partial_reassembly
            .saturating_add(other.partial_reassembly);
    }
}

/// At most 64 field rows, the move rows (two reliabilities by three
/// dispositions) and 16 skipped-block rows; every counter saturates. Rows
/// exist only for blocks framed completely; anything else is counted, so an
/// absent row is never evidence of an absent call.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct PawnFieldDiagnostics {
    pub entries: Vec<PawnFieldSummary>,
    /// Occurrences of field keys discarded because `entries` was full.
    pub overflow: u64,
    pub movement: Vec<PawnMoveFieldSummary>,
    pub skipped_blocks: Vec<PawnSkippedBlockSummary>,
    pub skipped_blocks_overflow: u64,
    /// Unreliable partial bunches (reliable parts reach the session whole).
    pub partial_bunches: u64,
    pub export_bunches: u64,
    pub must_be_mapped_bunches: u64,
    /// Actor-body failures before any block was framed.
    pub body_parse_failures: u64,
    /// Blocks whose field stream failed to frame; their rows are discarded.
    pub malformed_blocks: u64,
    /// Bunches with at least one malformed block.
    pub malformed_bunches: u64,
    /// Client property blocks (also in `skipped_blocks`).
    pub property_blocks: u64,
    /// Mapped sub-object blocks without a field-index bound (also skipped).
    pub unbounded_subobject_blocks: u64,
    /// Unmapped or overflowed sub-object blocks (also skipped).
    pub unknown_subobject_blocks: u64,
    pub transport_drops: PawnTransportDrops,
}

/// One framed field before commit.
enum StagedRow {
    Field(PawnFieldSummary),
    Move {
        disposition: PawnFieldDisposition,
        payload_bits: u64,
    },
}

impl PawnFieldDiagnostics {
    pub(in super::super) fn merge(&mut self, other: &Self) {
        self.overflow = self.overflow.saturating_add(other.overflow);
        self.skipped_blocks_overflow = self
            .skipped_blocks_overflow
            .saturating_add(other.skipped_blocks_overflow);
        for (mine, theirs) in [
            (&mut self.partial_bunches, other.partial_bunches),
            (&mut self.export_bunches, other.export_bunches),
            (
                &mut self.must_be_mapped_bunches,
                other.must_be_mapped_bunches,
            ),
            (&mut self.body_parse_failures, other.body_parse_failures),
            (&mut self.malformed_blocks, other.malformed_blocks),
            (&mut self.malformed_bunches, other.malformed_bunches),
            (&mut self.property_blocks, other.property_blocks),
            (
                &mut self.unbounded_subobject_blocks,
                other.unbounded_subobject_blocks,
            ),
            (
                &mut self.unknown_subobject_blocks,
                other.unknown_subobject_blocks,
            ),
        ] {
            *mine = mine.saturating_add(theirs);
        }
        self.transport_drops.merge(&other.transport_drops);
        for entry in &other.entries {
            self.record_field(entry.clone());
        }
        for entry in &other.movement {
            self.record_move(
                entry.reliable,
                entry.disposition,
                entry.max_payload_bits,
                entry.count,
            );
        }
        for entry in &other.skipped_blocks {
            self.record_skipped(entry.clone());
        }
    }

    fn record_field(&mut self, row: PawnFieldSummary) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.target == row.target
                && entry.relation == row.relation
                && entry.ordinal == row.ordinal
                && entry.index == row.index
                && entry.payload_bits == row.payload_bits
                && entry.reliable == row.reliable
                && entry.disposition == row.disposition
        }) {
            entry.count = entry.count.saturating_add(row.count);
        } else if self.entries.len() < PAWN_FIELD_ENTRIES {
            self.entries.push(row);
        } else {
            self.overflow = self.overflow.saturating_add(row.count);
        }
    }

    /// Two reliabilities by three dispositions: at most six rows, unbounded
    /// by traffic, so movement can neither evict nor crowd out field rows.
    fn record_move(
        &mut self,
        reliable: bool,
        disposition: PawnFieldDisposition,
        payload_bits: u64,
        count: u64,
    ) {
        if let Some(entry) = self
            .movement
            .iter_mut()
            .find(|entry| entry.reliable == reliable && entry.disposition == disposition)
        {
            entry.count = entry.count.saturating_add(count);
            entry.max_payload_bits = entry.max_payload_bits.max(payload_bits);
        } else {
            self.movement.push(PawnMoveFieldSummary {
                index: SERVER_MOVE_PACKED_FIELD_INDEX,
                reliable,
                disposition,
                max_payload_bits: payload_bits,
                count,
            });
        }
    }

    fn record_skipped(&mut self, row: PawnSkippedBlockSummary) {
        if let Some(entry) = self.skipped_blocks.iter_mut().find(|entry| {
            entry.target == row.target
                && entry.relation == row.relation
                && entry.ordinal == row.ordinal
                && entry.reason == row.reason
                && entry.has_rep_layout == row.has_rep_layout
                && entry.payload_bits == row.payload_bits
                && entry.reliable == row.reliable
        }) {
            entry.count = entry.count.saturating_add(row.count);
        } else if self.skipped_blocks.len() < PAWN_SKIPPED_BLOCK_ENTRIES {
            self.skipped_blocks.push(row);
        } else {
            self.skipped_blocks_overflow = self.skipped_blocks_overflow.saturating_add(row.count);
        }
    }

    fn commit(&mut self, rows: &[StagedRow], reliable: bool) {
        for row in rows {
            match row {
                StagedRow::Field(row) => self.record_field(row.clone()),
                StagedRow::Move {
                    disposition,
                    payload_bits,
                } => self.record_move(reliable, *disposition, *payload_bits, 1),
            }
        }
    }
}

/// Since-bind pawn-channel receive metadata, retained after peer removal.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct PawnReceiveDiagnostics {
    /// Whole reliable bunches released to a joined peer's pawn session.
    pub reliable_bunches: u64,
    /// Unreliable bunches on a joined peer's pawn channel.
    pub unreliable_bunches: u64,
    pub fields: PawnFieldDiagnostics,
}

/// One retained sub-object of a committed pawn open.
struct MappedSubobject {
    guid: u32,
    relation: PawnSubobjectRelation,
    ordinal: u8,
    bound: Option<u32>,
}

/// Private, diagnostic-only references from this connection's committed pawn
/// open; GUIDs are compared only, never reported. At most 64 retained.
#[derive(Default)]
pub(in super::super) struct PawnSubobjectMapping {
    targets: Vec<MappedSubobject>,
    committed: bool,
    complete: bool,
}

impl PawnSubobjectMapping {
    /// Components in plan order, then runtime inventories in content order.
    /// Only `UIcarusStatContainer` and `UInventory` have release-provided
    /// field-index bounds (F-708); every other component has none.
    pub(in super::super) fn committed(guids: &ComponentActorGuids, plan: &PawnOpenPlan) -> Self {
        let components = guids
            .components
            .iter()
            .enumerate()
            .filter_map(|(index, guid)| {
                let guid = (*guid)?;
                Some(MappedSubobject {
                    guid,
                    relation: PawnSubobjectRelation::PawnComponent,
                    ordinal: u8::try_from(index).ok()?,
                    bound: (index == plan.stat_container)
                        .then_some(plan.content.stat_container_bound),
                })
            });
        let inventories = guids
            .subobjects
            .iter()
            .enumerate()
            .filter_map(|(index, guid)| {
                Some(MappedSubobject {
                    guid: *guid,
                    relation: PawnSubobjectRelation::PawnInventory,
                    ordinal: u8::try_from(index).ok()?,
                    bound: Some(plan.content.inventory_bound),
                })
            });
        let total = guids.components.iter().flatten().count() + guids.subobjects.len();
        Self {
            targets: components
                .chain(inventories)
                .take(PAWN_MAPPING_ENTRIES)
                .collect(),
            committed: true,
            complete: total <= PAWN_MAPPING_ENTRIES,
        }
    }

    /// Test fixtures address a mapped sub-object without reporting its GUID.
    #[cfg(test)]
    pub(in super::super) fn guid_of(
        &self,
        relation: PawnSubobjectRelation,
        ordinal: u8,
    ) -> Option<u32> {
        self.targets
            .iter()
            .find(|target| target.relation == relation && target.ordinal == ordinal)
            .map(|target| target.guid)
    }

    fn lookup(&self, guid: u32) -> (PawnSubobjectRelation, Option<u8>, Option<u32>) {
        if let Some(target) = self.targets.iter().find(|target| target.guid == guid) {
            (target.relation, Some(target.ordinal), target.bound)
        } else if self.committed && self.complete {
            (PawnSubobjectRelation::Unmapped, None, None)
        } else {
            (PawnSubobjectRelation::MappingOverflow, None, None)
        }
    }
}

/// How the existing move handling treated one unreliable bunch's moves.
pub(in super::super) enum MoveOutcomes {
    /// Move handling is off: the server reads no field.
    NotRead,
    /// The move reader refused the whole bunch.
    Refused,
    /// Per field-39 occurrence in actor blocks, in order: taken as a move.
    Read(Vec<bool>),
}

/// Count a reliable transport drop on the pawn channel.
pub(in super::super) fn observe_pawn_transport_drop(
    drop: &ReliableActorDrop,
    diagnostics: &mut [&mut PawnFieldDiagnostics],
) {
    for diagnostics in diagnostics.iter_mut() {
        diagnostics.transport_drops.record(drop.reason);
    }
}

/// Frame one client pawn bunch for metadata only, after its handling. Every
/// read is bounded by the bunch; failures are counted and never propagate.
pub(in super::super) fn observe_pawn_bunch(
    bunch: &Bunch,
    actor: &ActorChannelCodec,
    mapping: &PawnSubobjectMapping,
    moves: &MoveOutcomes,
    diagnostics: &mut [&mut PawnFieldDiagnostics],
) {
    let header = &bunch.header;
    let reliable = header.reliable;
    if header.partial {
        bump_all(diagnostics, |d| &mut d.partial_bunches);
        return;
    }
    if header.has_package_map_exports {
        bump_all(diagnostics, |d| &mut d.export_bunches);
        return;
    }
    if header.has_must_be_mapped_guids {
        bump_all(diagnostics, |d| &mut d.must_be_mapped_bunches);
        return;
    }
    let Ok(body) = read_client_actor_bunch_body(&mut bunch.payload.reader(), actor) else {
        bump_all(diagnostics, |d| &mut d.body_parse_failures);
        return;
    };
    let mut move_position = 0usize;
    let mut malformed = false;
    for block in &body.blocks {
        let payload_bits = block.payload.bit_len() as u64;
        let (target, relation, ordinal, bound) = match &block.target {
            BlockTarget::Actor => (
                PawnFieldTarget::Actor,
                None,
                None,
                Some(BP_ICARUS_PLAYER_CHARACTER_SURVIVAL_FIELD_INDEX_BOUND),
            ),
            BlockTarget::SubObject { object, .. } => {
                let (relation, ordinal, bound) = mapping.lookup(object.guid);
                (PawnFieldTarget::Subobject, Some(relation), ordinal, bound)
            }
        };
        let bound = match (block.has_rep_layout, bound) {
            (false, Some(bound)) => bound,
            (has_rep_layout, _) => {
                let reason = if has_rep_layout {
                    bump_all(diagnostics, |d| &mut d.property_blocks);
                    PawnSkipReason::PropertyBlock
                } else {
                    if matches!(
                        relation,
                        Some(
                            PawnSubobjectRelation::Unmapped
                                | PawnSubobjectRelation::MappingOverflow
                        )
                    ) {
                        bump_all(diagnostics, |d| &mut d.unknown_subobject_blocks);
                    } else {
                        bump_all(diagnostics, |d| &mut d.unbounded_subobject_blocks);
                    }
                    PawnSkipReason::NoFieldIndexBound
                };
                for diagnostics in diagnostics.iter_mut() {
                    diagnostics.record_skipped(PawnSkippedBlockSummary {
                        target,
                        relation,
                        ordinal,
                        reason,
                        has_rep_layout,
                        payload_bits,
                        reliable,
                        count: 1,
                    });
                }
                continue;
            }
        };
        // Stage the block's rows: only a completely framed block commits.
        let mut rows = Vec::new();
        let mut reader = block.payload.reader();
        let mut framed = true;
        while !reader.at_end() {
            // Bounded by the block, exactly as the move reader frames it.
            let Ok(field) = read_field_header(&mut reader, bound, usize::MAX) else {
                framed = false;
                break;
            };
            let field_bits = field.payload.bit_len() as u64;
            if target == PawnFieldTarget::Actor && field.index == SERVER_MOVE_PACKED_FIELD_INDEX {
                let disposition = match moves {
                    _ if reliable => PawnFieldDisposition::Ignored,
                    MoveOutcomes::NotRead => PawnFieldDisposition::Ignored,
                    MoveOutcomes::Refused => PawnFieldDisposition::Malformed,
                    MoveOutcomes::Read(outcomes) => match outcomes.get(move_position) {
                        Some(true) => PawnFieldDisposition::Recognized,
                        _ => PawnFieldDisposition::Malformed,
                    },
                };
                move_position += 1;
                rows.push(StagedRow::Move {
                    disposition,
                    payload_bits: field_bits,
                });
            } else {
                rows.push(StagedRow::Field(PawnFieldSummary {
                    target,
                    relation,
                    ordinal,
                    index: field.index,
                    payload_bits: field_bits,
                    reliable,
                    disposition: PawnFieldDisposition::Ignored,
                    count: 1,
                }));
            }
        }
        if framed {
            for diagnostics in diagnostics.iter_mut() {
                diagnostics.commit(&rows, reliable);
            }
        } else {
            malformed = true;
            bump_all(diagnostics, |d| &mut d.malformed_blocks);
        }
    }
    if malformed {
        bump_all(diagnostics, |d| &mut d.malformed_bunches);
    }
}

/// Saturating increment of one counter in every table.
fn bump_all(
    diagnostics: &mut [&mut PawnFieldDiagnostics],
    counter: fn(&mut PawnFieldDiagnostics) -> &mut u64,
) {
    for diagnostics in diagnostics.iter_mut() {
        let value = counter(diagnostics);
        *value = value.saturating_add(1);
    }
}
