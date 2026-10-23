//! An immutable, owned input for import assessment and future semantic adapters.
//! Decoding and lossless typing do not authorize world publication.
use std::{fmt, io::Read, mem::size_of, ops::Range};

use anyhow::{Context, Result, ensure};

use crate::{
    experimental::{
        archive::{ByteOrder, decode_string},
        component::MAX_BOUNDED_COMPONENT_BYTES,
        property_tree::{BaseComponentTree, MAX_TREE_DEPTH, MAX_TREE_NODES, TreeLimits},
        recorders::{
            StagedProspectSaveState, StagedRecorderDecision, StagedRootValues,
            lookup_recorder_ctor_defaults, read_recorders, stage_prospect_save_state_for_import,
        },
        restore_rules::RecorderCtorDefaults,
        struct_array::MAX_BOUNDED_STRUCT_ARRAY_BYTES,
        tagged::{TagProfile, read_root},
        typed_value::{TypedBudgetExceeded, TypedComponent, TypedSummary, TypingBudget},
    },
    preservation::{PreserveLimits, PreservedSection},
    prospect::{EnvelopeInspection, decode_envelope},
};

pub const MAX_CANDIDATE_OWNED_BYTES: usize = 256 * 1024 * 1024;
pub const MAX_CANDIDATE_TYPED_ELEMENTS: usize = 1 << 20;
const PROFILE: TagProfile = TagProfile::MemoryArchive522;
const TREE_LIMITS: TreeLimits = TreeLimits {
    max_depth: MAX_TREE_DEPTH,
    max_nodes: MAX_TREE_NODES,
    struct_array_bytes: MAX_BOUNDED_STRUCT_ARRAY_BYTES,
    container_struct_bytes: None,
};

/// Retained allocation allowances, not a process RSS or peak-memory guarantee.
/// Envelope, staging and preservation retain their independent input ceilings.
/// The defaults also govern `plan_import`, `planned_actors` and `import-save`.
/// Exhaustion rejects the whole candidate, even when a dropped recorder caused
/// it: resource-policy errors are never downgraded to optional typing outcomes.
#[derive(Clone, Copy, Debug)]
pub struct CandidateLimits {
    pub owned_bytes: usize,
    pub typed_elements: usize,
}

impl Default for CandidateLimits {
    fn default() -> Self {
        Self {
            owned_bytes: MAX_CANDIDATE_OWNED_BYTES,
            typed_elements: MAX_CANDIDATE_TYPED_ELEMENTS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecorderTypingFailure {
    Framing,
    Typing,
    Encoding,
    RoundTripMismatch,
}

pub struct LosslessRecorder {
    component: TypedComponent,
    summary: TypedSummary,
}

pub enum RecorderTyping {
    Lossless(Box<LosslessRecorder>),
    Unavailable(RecorderTypingFailure),
}

impl RecorderTyping {
    pub fn lossless(&self) -> Option<(&TypedComponent, &TypedSummary)> {
        match self {
            Self::Lossless(value) => Some((&value.component, &value.summary)),
            Self::Unavailable(_) => None,
        }
    }
}

pub struct DecodedRecorder {
    source_index: usize,
    record_span: Range<usize>,
    class_value_span: Range<usize>,
    binary_data_span: Range<usize>,
    class_units: Box<[u16]>,
    defaults: Option<RecorderCtorDefaults>,
    decision: StagedRecorderDecision,
    typing: RecorderTyping,
}

impl DecodedRecorder {
    pub fn source_index(&self) -> usize {
        self.source_index
    }
    /// Exact identity, independent of the CRC-based experimental class lookup.
    pub fn class_units(&self) -> &[u16] {
        &self.class_units
    }
    pub fn defaults(&self) -> Option<RecorderCtorDefaults> {
        self.defaults
    }
    pub fn decision(&self) -> &StagedRecorderDecision {
        &self.decision
    }
    pub fn typing(&self) -> &RecorderTyping {
        &self.typing
    }
}

pub(crate) struct ImportRootReferences {
    pub(crate) prospect_dt_key: Vec<u16>,
    pub(crate) prospect_map_name: Vec<u16>,
    pub(crate) faction_mission_dt_key: Vec<u16>,
    pub(crate) setting_row_names: Vec<Vec<u16>>,
}

pub struct DecodedImportCandidate {
    envelope: EnvelopeInspection,
    payload: Box<[u8]>,
    preserved: PreservedSection,
    staged: StagedProspectSaveState,
    root_values: StagedRootValues,
    root_references: ImportRootReferences,
    recorders: Box<[DecodedRecorder]>,
    owned_bytes: usize,
    typed_elements: usize,
}

fn charge(total: &mut usize, bytes: usize, limit: usize) -> Result<()> {
    *total = total
        .checked_add(bytes)
        .context("import candidate allocation overflow")?;
    ensure!(*total <= limit, "import candidate allocation limit");
    Ok(())
}

fn span(payload: &[u8], bytes: &[u8]) -> Result<Range<usize>> {
    let start = (bytes.as_ptr() as usize)
        .checked_sub(payload.as_ptr() as usize)
        .context("import candidate source span invalid")?;
    let end = start
        .checked_add(bytes.len())
        .context("import candidate source span overflow")?;
    ensure!(
        end <= payload.len(),
        "import candidate source span outside payload"
    );
    Ok(start..end)
}

impl DecodedImportCandidate {
    pub fn decode(reader: impl Read) -> Result<Self> {
        Self::decode_with_limits(reader, CandidateLimits::default())
    }

    pub fn decode_with_limits(reader: impl Read, limits: CandidateLimits) -> Result<Self> {
        ensure!(
            limits.owned_bytes <= MAX_CANDIDATE_OWNED_BYTES
                && limits.typed_elements <= MAX_CANDIDATE_TYPED_ELEMENTS,
            "import candidate limits exceed implementation ceilings"
        );
        let validated = decode_envelope(reader)?;
        let tags =
            read_root(&validated.payload, PROFILE).context("invalid experimental root tags")?;
        let staged = stage_prospect_save_state_for_import(
            &tags.tags,
            PROFILE,
            MAX_BOUNDED_COMPONENT_BYTES,
            None,
        )
        .context("invalid staged FProspectSaveState")?;
        let framed =
            read_recorders(&tags.tags, PROFILE).context("invalid experimental recorder framing")?;
        ensure!(
            staged.decisions.len() == framed.records().len(),
            "staging decisions do not match the recorder array"
        );

        // Opaque preservation avoids constructing a second set of recorder
        // trees. It still verifies exact reconstruction of the unchanged payload.
        // Complete typing is retained separately below, including dropped records.
        let preserved = PreservedSection::from_inner_payload(
            &validated.payload,
            PreserveLimits {
                component_bytes: 1,
                ..PreserveLimits::default()
            },
        )?;
        let references = ImportRootReferences {
            prospect_dt_key: staged.prospect_dt_key,
            prospect_map_name: staged.prospect_map_name,
            faction_mission_dt_key: staged.faction_mission_dt_key,
            setting_row_names: staged.setting_row_names,
        };
        let mut owned = size_of::<Self>();
        let limit = limits.owned_bytes;
        charge(&mut owned, validated.payload.len(), limit)?;
        charge(&mut owned, preserved.held_bytes(), limit)?;
        charge(&mut owned, staged.root_values.owned_bytes()?, limit)?;
        for units in [
            &references.prospect_dt_key,
            &references.prospect_map_name,
            &references.faction_mission_dt_key,
        ] {
            charge(
                &mut owned,
                units
                    .capacity()
                    .checked_mul(size_of::<u16>())
                    .context("import candidate allocation overflow")?,
                limit,
            )?;
        }
        charge(
            &mut owned,
            references
                .setting_row_names
                .capacity()
                .checked_mul(size_of::<Vec<u16>>())
                .context("import candidate allocation overflow")?,
            limit,
        )?;
        for units in &references.setting_row_names {
            charge(
                &mut owned,
                units
                    .capacity()
                    .checked_mul(size_of::<u16>())
                    .context("import candidate allocation overflow")?,
                limit,
            )?;
        }
        // Include the possible boxed typed result and summary for every record
        // before allocating the recorder collection. Unavailable outcomes use less.
        charge(
            &mut owned,
            framed
                .records()
                .len()
                .checked_mul(size_of::<DecodedRecorder>() + size_of::<LosslessRecorder>())
                .context("import candidate allocation overflow")?,
            limit,
        )?;
        let mut recorders = Vec::with_capacity(framed.records().len());
        let mut elements = 0usize;
        for (index, (recorder, decision)) in
            framed.records().iter().zip(staged.decisions).enumerate()
        {
            let class = decode_string(recorder.class_value, ByteOrder::Little)
                .context("invalid experimental recorder class string")?;
            ensure!(
                class.consumed == recorder.class_value.len(),
                "trailing recorder class bytes"
            );
            charge(
                &mut owned,
                class
                    .units
                    .len()
                    .checked_mul(size_of::<u16>())
                    .context("import candidate allocation overflow")?,
                limit,
            )?;
            let defaults = lookup_recorder_ctor_defaults(&class.units);
            ensure!(
                !decision.retained || defaults.is_some(),
                "retained recorder class has no constructor defaults"
            );
            let mut budget = TypingBudget::new(limit - owned, limits.typed_elements - elements);
            let typing = match BaseComponentTree::parse(
                recorder.binary_data,
                PROFILE,
                MAX_BOUNDED_COMPONENT_BYTES,
                TREE_LIMITS,
            ) {
                Err(_) => RecorderTyping::Unavailable(RecorderTypingFailure::Framing),
                Ok(tree) => match TypedComponent::from_component_with_budget(&tree, &mut budget) {
                    Err(error) if error.is::<TypedBudgetExceeded>() => return Err(error),
                    Err(_) => RecorderTyping::Unavailable(RecorderTypingFailure::Typing),
                    Ok((component, summary)) => match component.to_bytes() {
                        Err(_) => RecorderTyping::Unavailable(RecorderTypingFailure::Encoding),
                        Ok(bytes) if bytes != recorder.binary_data => {
                            RecorderTyping::Unavailable(RecorderTypingFailure::RoundTripMismatch)
                        }
                        Ok(_) => {
                            charge(&mut owned, budget.charged_bytes(), limit)?;
                            charge(
                                &mut elements,
                                budget.charged_elements(),
                                limits.typed_elements,
                            )?;
                            RecorderTyping::Lossless(Box::new(LosslessRecorder {
                                component,
                                summary,
                            }))
                        }
                    },
                },
            };
            recorders.push(DecodedRecorder {
                source_index: index,
                record_span: span(&validated.payload, recorder.record)?,
                class_value_span: span(&validated.payload, recorder.class_value)?,
                binary_data_span: span(&validated.payload, recorder.binary_data)?,
                class_units: class.units.into_boxed_slice(),
                defaults,
                decision,
                typing,
            });
        }
        Ok(Self {
            envelope: validated.inspection,
            payload: validated.payload.into_boxed_slice(),
            preserved,
            staged: staged.summary,
            root_values: staged.root_values,
            root_references: references,
            recorders: recorders.into_boxed_slice(),
            owned_bytes: owned,
            typed_elements: elements,
        })
    }

    pub fn envelope(&self) -> &EnvelopeInspection {
        &self.envelope
    }
    pub fn staged(&self) -> &StagedProspectSaveState {
        &self.staged
    }
    pub fn root_values(&self) -> &StagedRootValues {
        &self.root_values
    }
    pub fn recorders(&self) -> &[DecodedRecorder] {
        &self.recorders
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
    pub fn preserved(&self) -> &PreservedSection {
        &self.preserved
    }
    /// Conservative retained allocation allowance; not measured heap use or RSS.
    pub fn owned_bytes(&self) -> usize {
        self.owned_bytes
    }
    pub fn typed_elements(&self) -> usize {
        self.typed_elements
    }
    pub(crate) fn root_references(&self) -> &ImportRootReferences {
        &self.root_references
    }
    pub fn recorder_bytes(&self, source_index: usize) -> Option<&[u8]> {
        self.recorders
            .get(source_index)
            .map(|r| &self.payload[r.record_span.clone()])
    }
    pub fn recorder_class_encoding(&self, source_index: usize) -> Option<&[u8]> {
        self.recorders
            .get(source_index)
            .map(|r| &self.payload[r.class_value_span.clone()])
    }
    pub fn recorder_binary_data(&self, source_index: usize) -> Option<&[u8]> {
        self.recorders
            .get(source_index)
            .map(|r| &self.payload[r.binary_data_span.clone()])
    }
}

impl fmt::Debug for DecodedImportCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedImportCandidate")
            .field("recorders", &self.recorders.len())
            .field("payload_bytes", &self.payload.len())
            .field("owned_bytes", &self.owned_bytes)
            .finish_non_exhaustive()
    }
}
impl fmt::Debug for DecodedRecorder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DecodedRecorder")
            .field("source_index", &self.source_index)
            .field("retained", &self.decision.retained)
            .field("lossless", &self.typing.lossless().is_some())
            .finish_non_exhaustive()
    }
}
