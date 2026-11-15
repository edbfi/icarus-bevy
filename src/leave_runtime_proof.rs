//! Explicit private, bounded evidence for the two-invocation authored leave case.
//! No ordinary server creates this sink. Its failures never revoke a save receipt.
use crate::experimental::{
    archive::ByteOrder,
    packet::Payload,
    pawn_open::{CharacterVitals, PawnOpenPlan},
    rep_rpc::LeaveProspectSessionType,
};
use crate::{
    native_players::{PlayerKey, PlayerProjection, PlayerRecord, PlayerSnapshot},
    persistence::{LoadedSnapshot, SaveReceipt, SnapshotStore},
    simulation::WorldSnapshot,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Write},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub const GENERATION_CAP: usize = 8 * 1024 * 1024;
const JOURNAL_CAP: usize = 1024 * 1024;
const EVENT_CAP: usize = 16 * 1024;
const EVENTS: usize = 128;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofFailure {
    Binding,
    Io,
    Limit,
    Order,
    Mismatch,
    Poisoned,
    Abandoned,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofError(pub ProofFailure);
impl std::fmt::Display for ProofError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "private leave proof refused ({:?})", self.0)
    }
}
impl std::error::Error for ProofError {}
type Result<T> = std::result::Result<T, ProofError>;
fn error(kind: ProofFailure) -> ProofError {
    ProofError(kind)
}
fn require(ok: bool, kind: ProofFailure) -> Result<()> {
    if ok { Ok(()) } else { Err(error(kind)) }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Descriptor {
    format: u8,
    case_id: String,
    phase: u8,
    output: String,
    predecessor: Option<String>,
    exit_seal: Option<String>,
}
fn filename(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
        && name != "."
        && name != ".."
}
fn invocation(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

struct Directory {
    file: File,
    path: PathBuf,
    uid: u32,
}
impl Directory {
    fn absent(&self, name: &str) -> Result<()> {
        use rustix::fs::{AtFlags, statat};
        match statat(&self.file, name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(rustix::io::Errno::NOENT) => Ok(()),
            _ => Err(error(ProofFailure::Binding)),
        }
    }
    fn open(path: &Path) -> Result<Self> {
        use rustix::fs::{CWD, Mode, OFlags, ResolveFlags, openat2};
        let uid = std::fs::metadata("/proc/self")
            .map_err(|_| error(ProofFailure::Binding))?
            .uid();
        let fd = openat2(
            CWD,
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| error(ProofFailure::Binding))?;
        let file = File::from(fd);
        let m = file.metadata().map_err(|_| error(ProofFailure::Binding))?;
        require(
            m.is_dir() && m.uid() == uid && m.mode() & 0o777 == 0o700,
            ProofFailure::Binding,
        )?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
            uid,
        })
    }
    fn relative<'a>(&self, path: &'a Path) -> Result<&'a Path> {
        let p = if path.is_absolute() {
            path.strip_prefix(&self.path)
                .map_err(|_| error(ProofFailure::Binding))?
        } else {
            path
        };
        require(
            !p.as_os_str().is_empty()
                && p.components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            ProofFailure::Binding,
        )?;
        Ok(p)
    }
    fn read(&self, path: &Path, cap: usize) -> Result<Vec<u8>> {
        use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
        let p = self.relative(path)?;
        let fd = openat2(
            &self.file,
            p,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| error(ProofFailure::Io))?;
        let file = File::from(fd);
        let m = file.metadata().map_err(|_| error(ProofFailure::Io))?;
        require(
            m.is_file() && m.uid() == self.uid && m.mode() & 0o777 == 0o600 && m.nlink() == 1,
            ProofFailure::Binding,
        )?;
        require(m.len() <= cap as u64, ProofFailure::Limit)?;
        let mut bytes = Vec::new();
        file.take(cap as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| error(ProofFailure::Io))?;
        require(bytes.len() <= cap, ProofFailure::Limit)?;
        Ok(bytes)
    }
    fn create(&self, name: &str) -> Result<File> {
        use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
        require(filename(name), ProofFailure::Binding)?;
        let fd = openat2(
            &self.file,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
        )
        .map_err(|_| error(ProofFailure::Io))?;
        let file = File::from(fd);
        let m = file.metadata().map_err(|_| error(ProofFailure::Io))?;
        require(
            m.mode() & 0o777 == 0o600 && m.uid() == self.uid && m.nlink() == 1,
            ProofFailure::Binding,
        )?;
        Ok(file)
    }
    fn publish(&self, name: &str, bytes: &[u8]) -> Result<()> {
        let mut file = self.create(name)?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .and_then(|_| self.file.sync_all())
            .map_err(|_| error(ProofFailure::Io))
    }
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    serde_json::from_slice::<crate::json::JsonObject<T>>(bytes)
        .map(|v| v.0)
        .map_err(|_| error(ProofFailure::Binding))
}
fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| error(ProofFailure::Binding))
}
pub fn read_private_input(root: &Path, path: &Path, max_bytes: usize) -> Result<Vec<u8>> {
    require(max_bytes <= JOURNAL_CAP, ProofFailure::Limit)?;
    Directory::open(root)?.read(path, max_bytes)
}
fn descriptor(dir: &Directory, path: &Path) -> Result<Descriptor> {
    require(
        dir.relative(path)?.components().count() == 1,
        ProofFailure::Binding,
    )?;
    let d: Descriptor = decode(&dir.read(path, 4096)?)?;
    require(
        d.format == 1
            && filename(&d.case_id)
            && filename(&d.output)
            && (d.phase == 1 || d.phase == 2),
        ProofFailure::Binding,
    )?;
    require(
        dir.path
            .file_name()
            .is_some_and(|n| n == d.case_id.as_str()),
        ProofFailure::Binding,
    )?;
    require(
        if d.phase == 1 {
            d.predecessor.is_none() && d.exit_seal.is_none()
        } else {
            d.predecessor.as_ref().is_some_and(|p| filename(p))
                && d.exit_seal.as_ref().is_some_and(|p| filename(p))
        },
        ProofFailure::Binding,
    )?;
    Ok(d)
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    account: u64,
    slot: i32,
    id: u64,
    revision: u64,
    location: [u64; 3],
    vitals: [i32; 10],
    biome: u8,
    growth: u8,
    growth_name: String,
}
fn vital_values(v: CharacterVitals) -> [i32; 10] {
    [
        v.health,
        v.max_health,
        v.stamina,
        v.max_stamina,
        v.oxygen,
        v.max_oxygen,
        v.water,
        v.max_water,
        v.food,
        v.max_food,
    ]
}
impl Record {
    /// Storage interns only referenced names, so its local row index may differ
    /// from the installed runtime index. Every persisted value, including the
    /// raw row name, must still match exactly.
    fn same_stored_record(&self, other: &Self) -> bool {
        let mut normalized = other.clone();
        normalized.growth = self.growth;
        self == &normalized
    }

    fn from_projection(key: PlayerKey, p: PlayerProjection, growth_name: &str) -> Result<Self> {
        p.checkpoint
            .validate()
            .map_err(|_| error(ProofFailure::Mismatch))?;
        require(
            p.player_id > 0
                && p.revision > 0
                && !growth_name.is_empty()
                && growth_name.len() <= 1024,
            ProofFailure::Mismatch,
        )?;
        Ok(Self {
            account: key.account,
            slot: key.slot,
            id: p.player_id,
            revision: p.revision,
            location: p.checkpoint.location_cm.map(f64::to_bits),
            vitals: vital_values(p.checkpoint.vitals),
            biome: p.checkpoint.vitals.biome_row,
            growth: p.checkpoint.vitals.growth_row,
            growth_name: growth_name.to_owned(),
        })
    }
    fn key(&self) -> Result<PlayerKey> {
        PlayerKey::new(self.account, self.slot).map_err(|_| error(ProofFailure::Mismatch))
    }
    fn valid(&self) -> Result<()> {
        self.key()?;
        require(
            !self.growth_name.is_empty()
                && self.growth_name.len() <= 1024
                && self.id > 0
                && self.revision > 0
                && self.location.iter().all(|b| {
                    let f = f64::from_bits(*b);
                    f.is_finite() && f.abs() <= 1_048_576.
                }),
            ProofFailure::Mismatch,
        )?;
        require(
            self.biome == 0
                && self
                    .vitals
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .all(|p| p[0] >= 0 && p[0] <= p[1] && (1..=10_000_000).contains(&p[1])),
            ProofFailure::Mismatch,
        )
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    prospect_id: Vec<u16>,
    map_url: String,
    inputs: Inputs,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Inputs {
    authored_gameplay: [u8; 32],
    spawn: [u8; 32],
    authored_world: [u8; 32],
    declaration: [u8; 32],
    initial_values: [u8; 32],
}
fn context(players: &PlayerSnapshot) -> Result<Context> {
    require(players.records.len() <= 1, ProofFailure::Mismatch)?;
    let value = serde_json::to_value(players).map_err(|_| error(ProofFailure::Mismatch))?;
    serde_json::from_value(
        value
            .get("context")
            .cloned()
            .ok_or(error(ProofFailure::Mismatch))?,
    )
    .map_err(|_| error(ProofFailure::Mismatch))
}
fn capture_record(
    key: PlayerKey,
    p: PlayerProjection,
    ctx: &crate::native_players::PlayerContext,
) -> Result<Record> {
    let name = ctx
        .growth_row_name(p.checkpoint.vitals.growth_row)
        .ok_or(error(ProofFailure::Mismatch))?;
    Record::from_projection(key, p, name)
}
fn selected(players: &PlayerSnapshot, key: PlayerKey) -> Result<Record> {
    capture_record(
        key,
        players
            .projection(key)
            .ok_or(error(ProofFailure::Mismatch))?,
        players.context(),
    )
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct GenerationProof {
    pub number: u64,
    pub sha256: String,
    pub tick: u64,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
pub(crate) enum ProofJoinKind {
    Join,
    SplitJoin,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "event", deny_unknown_fields)]
enum Event {
    Load {
        generation: Option<GenerationProof>,
        record: Option<Record>,
        context: Option<Context>,
    },
    Opened {
        context: Context,
        record: Option<Record>,
    },
    Joined {
        connection: u32,
        kind: ProofJoinKind,
    },
    Reserved {
        epoch: u64,
        record: Record,
        is_new: bool,
    },
    Bound {
        connection: u32,
        epoch: u64,
    },
    Pawn {
        epoch: u64,
        channel: u32,
        pawn: u32,
        first: u32,
        last: u32,
        payload_bits: usize,
        payload_sha256: String,
    },
    Activated {
        epoch: u64,
    },
    Consumed {
        epoch: u64,
        before: Record,
        after: Record,
    },
    Intent {
        epoch: u64,
        kind: u8,
    },
    Cutoff {
        epoch: u64,
        attempt: u64,
        record: Record,
    },
    Submitted {
        epoch: u64,
        attempt: u64,
        record: Record,
    },
    Readback {
        epoch: u64,
        attempt: u64,
        record: Record,
        generation: GenerationProof,
    },
    Durable {
        epoch: u64,
        attempt: u64,
        generation: u64,
        tick: u64,
    },
    Completion {
        epoch: u64,
        channel: u32,
        first: u32,
        last: u32,
    },
    PrefixAcked {
        epoch: u64,
        channel: u32,
        first: u32,
        last: u32,
    },
    Detached {
        epoch: u64,
    },
    Ready,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Row {
    format: u8,
    case_id: String,
    phase: u8,
    invocation: String,
    ordinal: usize,
    data: Event,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PhaseVerdict {
    pub phase: u8,
    pub invocation: String,
    pub joins: u64,
    pub normal_type: u8,
    pub exact_durable: bool,
    pub prefix_acked: bool,
    pub detached: bool,
    pub actual_load_equal: bool,
    pub first_projection_equal: bool,
    pub pawn_admitted_before_activation: bool,
    pub food_debited: bool,
    pub phase_ready: bool,
}

#[derive(Clone)]
struct RestartExpectation {
    record: Record,
    context: Context,
    generation: GenerationProof,
}

struct Evidence {
    phase: u8,
    expected: Option<RestartExpectation>,
    loaded: bool,
    opened: Option<Context>,
    loaded_record: Option<Record>,
    join: Option<u32>,
    reserved: Option<(u64, Record)>,
    bound: bool,
    pawn: bool,
    activated: bool,
    consumed: Option<Record>,
    intent: Option<u8>,
    cutoff: Option<(u64, Record)>,
    submitted: bool,
    readback: Option<GenerationProof>,
    durable: bool,
    completion: Option<(u32, u32, u32)>,
    acked: bool,
    detached: bool,
    ready: bool,
}
impl Evidence {
    fn new(phase: u8, expected: Option<RestartExpectation>) -> Self {
        Self {
            phase,
            expected,
            loaded: false,
            opened: None,
            loaded_record: None,
            join: None,
            reserved: None,
            bound: false,
            pawn: false,
            activated: false,
            consumed: None,
            intent: None,
            cutoff: None,
            submitted: false,
            readback: None,
            durable: false,
            completion: None,
            acked: false,
            detached: false,
            ready: false,
        }
    }
    fn epoch(&self, epoch: u64) -> bool {
        self.reserved.as_ref().is_some_and(|(e, _)| *e == epoch)
    }
    fn apply(&mut self, event: &Event) -> Result<()> {
        require(!self.ready, ProofFailure::Order)?;
        match event {
            Event::Load {
                generation,
                record,
                context,
            } => {
                require(!self.loaded, ProofFailure::Order)?;
                if self.phase == 1 {
                    require(
                        generation.is_none() && record.is_none() && context.is_none(),
                        ProofFailure::Mismatch,
                    )?;
                } else {
                    let expected = self.expected.as_ref().ok_or(error(ProofFailure::Order))?;
                    require(
                        generation.as_ref() == Some(&expected.generation)
                            && record
                                .as_ref()
                                .is_some_and(|r| r.same_stored_record(&expected.record))
                            && context.as_ref() == Some(&expected.context),
                        ProofFailure::Mismatch,
                    )?;
                }
                self.loaded_record = record.clone();
                self.loaded = true;
            }
            Event::Opened { context, record } => {
                require(self.loaded && self.opened.is_none(), ProofFailure::Order)?;
                if let Some(expected) = &mut self.expected {
                    require(
                        record
                            .as_ref()
                            .is_some_and(|r| r.same_stored_record(&expected.record))
                            && context == &expected.context,
                        ProofFailure::Mismatch,
                    )?;
                    // The actual opened snapshot has resolved the raw name
                    // against the installed context. Subsequent runtime/pawn
                    // comparisons must retain that exact installed index.
                    expected.record = record.clone().ok_or(error(ProofFailure::Mismatch))?;
                } else {
                    require(record.is_none(), ProofFailure::Mismatch)?;
                }
                self.opened = Some(context.clone());
            }
            Event::Joined {
                connection,
                kind: _,
            } => {
                require(
                    self.opened.is_some() && self.join.is_none() && *connection > 0,
                    ProofFailure::Order,
                )?;
                self.join = Some(*connection);
            }
            Event::Reserved {
                epoch,
                record,
                is_new,
            } => {
                record.valid()?;
                require(
                    self.join.is_some() && self.reserved.is_none() && *epoch > 0,
                    ProofFailure::Order,
                )?;
                if let Some(expected) = &self.expected {
                    require(
                        record == &expected.record && !is_new,
                        ProofFailure::Mismatch,
                    )?;
                } else {
                    require(*is_new, ProofFailure::Mismatch)?;
                }
                self.reserved = Some((*epoch, record.clone()));
            }
            Event::Bound { connection, epoch } => {
                require(
                    self.epoch(*epoch) && self.join == Some(*connection) && !self.bound,
                    ProofFailure::Order,
                )?;
                self.bound = true;
            }
            Event::Pawn {
                epoch,
                channel,
                pawn,
                first,
                last,
                payload_bits,
                payload_sha256,
            } => {
                require(
                    self.epoch(*epoch)
                        && self.bound
                        && !self.pawn
                        && !self.activated
                        && *channel > 0
                        && *pawn > 1
                        && first <= last
                        && *payload_bits > 0
                        && *payload_bits <= 64 * 1024 * 8
                        && payload_sha256.len() == 64,
                    ProofFailure::Order,
                )?;
                self.pawn = true;
            }
            Event::Activated { epoch } => {
                require(
                    self.epoch(*epoch) && self.pawn && !self.activated,
                    ProofFailure::Order,
                )?;
                self.activated = true;
            }
            Event::Intent { epoch, kind } => {
                require(
                    self.epoch(*epoch)
                        && self.activated
                        && self.intent.is_none()
                        && *kind == if self.phase == 1 { 2 } else { 4 },
                    ProofFailure::Order,
                )?;
                self.intent = Some(*kind);
            }
            Event::Consumed {
                epoch,
                before,
                after,
            } => {
                require(
                    self.epoch(*epoch) && self.activated && self.cutoff.is_none(),
                    ProofFailure::Order,
                )?;
                let prior = self
                    .consumed
                    .as_ref()
                    .or_else(|| self.reserved.as_ref().map(|(_, r)| r))
                    .ok_or(error(ProofFailure::Order))?;
                require(
                    prior == before
                        && after.account == before.account
                        && after.slot == before.slot
                        && after.id == before.id
                        && after.revision
                            == before
                                .revision
                                .checked_add(1)
                                .ok_or(error(ProofFailure::Limit))?
                        && after.location == before.location
                        && after.growth == before.growth
                        && after.growth_name == before.growth_name
                        && after.biome == before.biome
                        && after.vitals[8] < before.vitals[8]
                        && after
                            .vitals
                            .iter()
                            .enumerate()
                            .all(|(i, v)| i == 8 || *v == before.vitals[i]),
                    ProofFailure::Mismatch,
                )?;
                after.valid()?;
                self.consumed = Some(after.clone());
            }
            Event::Cutoff {
                epoch,
                attempt,
                record,
            } => {
                record.valid()?;
                require(
                    self.epoch(*epoch)
                        && self.intent.is_some()
                        && self.cutoff.is_none()
                        && *attempt == 1,
                    ProofFailure::Order,
                )?;
                let initial = &self.reserved.as_ref().ok_or(error(ProofFailure::Order))?.1;
                require(
                    record.account == initial.account
                        && record.slot == initial.slot
                        && record.id == initial.id
                        && record.revision >= initial.revision
                        && (self.phase == 2 || record.vitals[8] < initial.vitals[8]),
                    ProofFailure::Mismatch,
                )?;
                require(
                    self.consumed
                        .as_ref()
                        .or_else(|| (self.phase == 2).then_some(initial))
                        == Some(record),
                    ProofFailure::Mismatch,
                )?;
                self.cutoff = Some((*attempt, record.clone()));
            }
            Event::Submitted {
                epoch,
                attempt,
                record,
            } => {
                require(
                    self.epoch(*epoch)
                        && !self.submitted
                        && self.cutoff.as_ref() == Some(&(*attempt, record.clone())),
                    ProofFailure::Order,
                )?;
                self.submitted = true;
            }
            Event::Readback {
                epoch,
                attempt,
                record,
                generation,
            } => {
                require(
                    self.epoch(*epoch)
                        && self.submitted
                        && self.readback.is_none()
                        && self.cutoff.as_ref() == Some(&(*attempt, record.clone()))
                        && generation.number > 0
                        && generation.sha256.len() == 64,
                    ProofFailure::Mismatch,
                )?;
                self.readback = Some(generation.clone());
            }
            Event::Durable {
                epoch,
                attempt,
                generation,
                tick,
            } => {
                require(
                    self.epoch(*epoch)
                        && !self.durable
                        && self.cutoff.as_ref().is_some_and(|(a, _)| a == attempt)
                        && self
                            .readback
                            .as_ref()
                            .is_some_and(|g| g.number == *generation && g.tick == *tick),
                    ProofFailure::Order,
                )?;
                self.durable = true;
            }
            Event::Completion {
                epoch,
                channel,
                first,
                last,
            } => {
                require(
                    self.epoch(*epoch)
                        && self.durable
                        && self.completion.is_none()
                        && *channel > 0
                        && first <= last,
                    ProofFailure::Order,
                )?;
                self.completion = Some((*channel, *first, *last));
            }
            Event::PrefixAcked {
                epoch,
                channel,
                first,
                last,
            } => {
                require(
                    self.epoch(*epoch)
                        && !self.acked
                        && self.completion == Some((*channel, *first, *last)),
                    ProofFailure::Order,
                )?;
                self.acked = true;
            }
            Event::Detached { epoch } => {
                require(
                    self.epoch(*epoch) && self.acked && !self.detached,
                    ProofFailure::Order,
                )?;
                self.detached = true;
            }
            Event::Ready => {
                require(
                    self.detached && self.durable && self.acked && self.pawn && self.activated,
                    ProofFailure::Order,
                )?;
                self.ready = true;
            }
        }
        Ok(())
    }
    fn verdict(&self, id: &str) -> PhaseVerdict {
        PhaseVerdict {
            phase: self.phase,
            invocation: id.into(),
            joins: u64::from(self.join.is_some()),
            normal_type: self.intent.unwrap_or(0),
            exact_durable: self.durable,
            prefix_acked: self.acked,
            detached: self.detached,
            actual_load_equal: self.phase == 2 && self.loaded_record.is_some(),
            first_projection_equal: self.phase == 2 && self.reserved.is_some(),
            pawn_admitted_before_activation: self.pawn && self.activated,
            food_debited: self.cutoff.is_some() && self.consumed.is_some(),
            phase_ready: self.ready,
        }
    }
}

struct Candidate {
    epoch: u64,
    channel: u32,
    pawn: u32,
    bits: usize,
    bytes: Vec<u8>,
}
struct State {
    dir: Directory,
    descriptor: Descriptor,
    id: String,
    journal: File,
    bytes: usize,
    ordinal: usize,
    failure: Option<ProofFailure>,
    evidence: Evidence,
    candidate: Option<Candidate>,
    opened_player_context: Option<crate::native_players::PlayerContext>,
}
impl State {
    fn emit(&mut self, event: Event) -> Result<()> {
        if let Some(f) = self.failure {
            return Err(error(f));
        }
        let result = (|| {
            self.evidence.apply(&event)?;
            let row = Row {
                format: 1,
                case_id: self.descriptor.case_id.clone(),
                phase: self.descriptor.phase,
                invocation: self.id.clone(),
                ordinal: self.ordinal + 1,
                data: event,
            };
            let mut bytes = encode(&row)?;
            bytes.push(b'\n');
            require(
                bytes.len() <= EVENT_CAP
                    && self.ordinal < EVENTS
                    && self.bytes + bytes.len() <= JOURNAL_CAP,
                ProofFailure::Limit,
            )?;
            self.journal
                .write_all(&bytes)
                .and_then(|_| self.journal.sync_all())
                .map_err(|_| error(ProofFailure::Io))?;
            self.ordinal += 1;
            self.bytes += bytes.len();
            Ok(())
        })();
        if let Err(ProofError(f)) = result {
            self.failure = Some(f);
        }
        result
    }
}
#[derive(Clone)]
pub struct PrivateLeaveProof {
    state: Arc<Mutex<State>>,
}
pub(crate) struct ProofWriteSelection {
    proof: PrivateLeaveProof,
    epoch: u64,
    attempt: u64,
    record: Record,
    context: Context,
}
impl ProofWriteSelection {
    pub(crate) fn fail(&self, kind: ProofFailure) -> ProofError {
        self.proof.fail(kind)
    }
    pub(crate) fn observed(
        &self,
        snapshot: &WorldSnapshot,
        receipt: &SaveReceipt,
        sha256: &str,
        manifest: &[u8],
        generation: &[u8],
    ) -> Result<()> {
        self.proof.access(|s| {
            let players = snapshot
                .native_players
                .as_ref()
                .ok_or(error(ProofFailure::Mismatch))?;
            require(
                selected(players, self.record.key()?)?.same_stored_record(&self.record)
                    && context(players)? == self.context,
                ProofFailure::Mismatch,
            )?;
            s.dir.publish(
                &format!("phase-{}-leave-manifest.json", s.descriptor.phase),
                manifest,
            )?;
            s.dir.publish(
                &format!("phase-{}-leave-generation.json", s.descriptor.phase),
                generation,
            )?;
            s.emit(Event::Readback {
                epoch: self.epoch,
                attempt: self.attempt,
                record: self.record.clone(),
                generation: GenerationProof {
                    number: receipt.generation,
                    sha256: sha256.into(),
                    tick: receipt.tick,
                },
            })
        })
    }
}
impl std::fmt::Debug for PrivateLeaveProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PrivateLeaveProof(private)")
    }
}
impl PrivateLeaveProof {
    #[cfg(test)]
    pub(crate) fn open_authored_fixture(path: &Path, root: &Path, id: &str) -> Result<Self> {
        Self::open_bound(path, root, id)
    }
    pub fn open(path: &Path, root: &Path) -> Result<Self> {
        let id = std::env::var("INVOCATION_ID").map_err(|_| error(ProofFailure::Binding))?;
        Self::open_bound(path, root, &id)
    }
    fn open_bound(path: &Path, root: &Path, id: &str) -> Result<Self> {
        require(invocation(id), ProofFailure::Binding)?;
        let dir = Directory::open(root)?;
        dir.absent("abort.json")?;
        let descriptor = descriptor(&dir, path)?;
        let expected = predecessor(&dir, &descriptor)?;
        let journal = dir.create(&descriptor.output)?;
        journal
            .sync_all()
            .and_then(|_| dir.file.sync_all())
            .map_err(|_| error(ProofFailure::Io))?;
        Ok(Self {
            state: Arc::new(Mutex::new(State {
                evidence: Evidence::new(descriptor.phase, expected),
                dir,
                descriptor,
                id: id.into(),
                journal,
                bytes: 0,
                ordinal: 0,
                failure: None,
                candidate: None,
                opened_player_context: None,
            })),
        })
    }
    fn access<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| error(ProofFailure::Poisoned))?;
        if let Some(kind) = s.failure {
            return Err(error(kind));
        }
        if let Err(ProofError(kind)) = s.dir.absent("abort.json") {
            s.failure = Some(kind);
            return Err(error(kind));
        }
        let result = f(&mut s);
        if let Err(ProofError(kind)) = result {
            s.failure = Some(kind);
        }
        result
    }
    pub fn health(&self) -> Result<()> {
        self.access(|_| Ok(()))
    }
    pub fn fail(&self, kind: ProofFailure) -> ProofError {
        let _ = self.access::<()>(|_| Err(error(kind)));
        error(kind)
    }
    pub fn phase_ready(&self) -> Result<bool> {
        self.access(|s| Ok(s.evidence.ready))
    }
    pub fn finish(&self) -> Result<()> {
        self.access(|s| require(s.evidence.ready, ProofFailure::Order))
    }
    pub fn observe_load(&self, loaded: Option<&LoadedSnapshot>) -> Result<()> {
        self.access(|s| {
            let (generation, record, ctx) = if let Some(loaded) = loaded {
                require(
                    !loaded.recovered_previous
                        && loaded.snapshot.schema_version
                            == crate::simulation::SNAPSHOT_SCHEMA_PLAYERS
                        && loaded.snapshot.preserved_prospect.is_none(),
                    ProofFailure::Mismatch,
                )?;
                let expected = s
                    .evidence
                    .expected
                    .as_ref()
                    .ok_or(error(ProofFailure::Mismatch))?;
                let players = loaded
                    .snapshot
                    .native_players
                    .as_ref()
                    .ok_or(error(ProofFailure::Mismatch))?;
                (
                    Some(GenerationProof {
                        number: loaded.selected_generation,
                        sha256: loaded.selected_sha256.clone(),
                        tick: loaded.snapshot.tick,
                    }),
                    Some(selected(players, expected.record.key()?)?),
                    Some(context(players)?),
                )
            } else {
                (None, None, None)
            };
            s.emit(Event::Load {
                generation,
                record,
                context: ctx,
            })
        })
    }
    pub fn observe_opened(&self, snapshot: &WorldSnapshot) -> Result<()> {
        self.access(|s| {
            require(
                snapshot.schema_version == crate::simulation::SNAPSHOT_SCHEMA_PLAYERS
                    && snapshot.preserved_prospect.is_none(),
                ProofFailure::Mismatch,
            )?;
            let players = snapshot
                .native_players
                .as_ref()
                .ok_or(error(ProofFailure::Mismatch))?;
            let record = s
                .evidence
                .expected
                .as_ref()
                .map(|e| selected(players, e.record.key()?))
                .transpose()?;
            require(
                if s.descriptor.phase == 1 {
                    players.is_empty()
                } else {
                    players.len() == 1
                },
                ProofFailure::Mismatch,
            )?;
            s.emit(Event::Opened {
                context: context(players)?,
                record,
            })?;
            s.opened_player_context = Some(players.context().clone());
            Ok(())
        })
    }
    pub(crate) fn joined(&self, connection: u32, kind: ProofJoinKind) -> Result<()> {
        self.access(|s| s.emit(Event::Joined { connection, kind }))
    }
    pub(crate) fn registry_context(&self, players: &PlayerSnapshot) -> Result<()> {
        self.access(|s| {
            require(
                s.opened_player_context.as_ref() == Some(players.context())
                    && s.evidence.opened.as_ref() == Some(&context(players)?),
                ProofFailure::Mismatch,
            )?;
            if let Some(expected) = &s.evidence.expected {
                require(
                    players.len() == 1
                        && selected(players, expected.record.key()?)? == expected.record,
                    ProofFailure::Mismatch,
                )
            } else {
                require(players.is_empty(), ProofFailure::Mismatch)
            }
        })
    }
    pub(crate) fn reserved(
        &self,
        key: PlayerKey,
        epoch: u64,
        p: PlayerProjection,
        ctx: &crate::native_players::PlayerContext,
    ) -> Result<()> {
        self.access(|s| {
            s.emit(Event::Reserved {
                epoch,
                record: capture_record(key, p, ctx)?,
                is_new: p.is_new,
            })
        })
    }
    pub(crate) fn bind_reservation(
        &self,
        connection: u32,
        key: PlayerKey,
        epoch: u64,
        p: PlayerProjection,
        activated: bool,
    ) -> Result<()> {
        self.access(|s| {
            let (_, reserved) = s
                .evidence
                .reserved
                .as_ref()
                .ok_or(error(ProofFailure::Order))?;
            require(
                !activated
                    && s.evidence.reserved.as_ref()
                        == Some(&(
                            epoch,
                            Record::from_projection(key, p, &reserved.growth_name)?,
                        )),
                ProofFailure::Mismatch,
            )?;
            s.emit(Event::Bound { connection, epoch })
        })
    }
    pub(crate) fn activated(&self, epoch: u64) -> Result<()> {
        self.access(|s| s.emit(Event::Activated { epoch }))
    }
    pub(crate) fn consumed(
        &self,
        epoch: u64,
        before: &PlayerRecord,
        after: &PlayerRecord,
        ctx: &crate::native_players::PlayerContext,
    ) -> Result<()> {
        self.access(|s| {
            s.emit(Event::Consumed {
                epoch,
                before: capture_record(before.key, before.projection(false), ctx)?,
                after: capture_record(after.key, after.projection(false), ctx)?,
            })
        })
    }
    pub(crate) fn normal_intent(
        &self,
        connection: u32,
        key: PlayerKey,
        epoch: u64,
        kind: LeaveProspectSessionType,
    ) -> Result<()> {
        self.access(|s| {
            require(
                s.evidence.join == Some(connection)
                    && s.evidence
                        .reserved
                        .as_ref()
                        .is_some_and(|(_, r)| r.key().ok() == Some(key)),
                ProofFailure::Mismatch,
            )?;
            s.emit(Event::Intent {
                epoch,
                kind: kind as u8,
            })
        })
    }
    pub(crate) fn cutoff(
        &self,
        epoch: u64,
        attempt: u64,
        record: &PlayerRecord,
        ctx: &crate::native_players::PlayerContext,
    ) -> Result<()> {
        self.access(|s| {
            s.emit(Event::Cutoff {
                epoch,
                attempt,
                record: capture_record(record.key, record.projection(false), ctx)?,
            })
        })
    }
    pub(crate) fn writer_selection(
        &self,
        epoch: u64,
        attempt: u64,
        record: &PlayerRecord,
        players: &PlayerSnapshot,
    ) -> Result<ProofWriteSelection> {
        self.access(|s| {
            let captured = selected(players, record.key)?;
            require(
                captured
                    == capture_record(record.key, record.projection(false), players.context())?,
                ProofFailure::Mismatch,
            )?;
            let context = context(players)?;
            require(
                s.evidence.opened.as_ref() == Some(&context),
                ProofFailure::Mismatch,
            )?;
            s.emit(Event::Submitted {
                epoch,
                attempt,
                record: captured.clone(),
            })?;
            Ok(ProofWriteSelection {
                proof: self.clone(),
                epoch,
                attempt,
                record: captured,
                context,
            })
        })
    }
    pub(crate) fn durable(&self, epoch: u64, attempt: u64, receipt: &SaveReceipt) -> Result<()> {
        self.access(|s| {
            s.emit(Event::Durable {
                epoch,
                attempt,
                generation: receipt.generation,
                tick: receipt.tick,
            })
        })
    }
    pub(crate) fn completion_admitted(
        &self,
        epoch: u64,
        channel: u32,
        first: u32,
        last: u32,
    ) -> Result<()> {
        self.access(|s| {
            s.emit(Event::Completion {
                epoch,
                channel,
                first,
                last,
            })
        })
    }
    pub(crate) fn prefix_acked(
        &self,
        epoch: u64,
        channel: u32,
        first: u32,
        last: u32,
    ) -> Result<()> {
        self.access(|s| {
            s.emit(Event::PrefixAcked {
                epoch,
                channel,
                first,
                last,
            })
        })
    }
    pub(crate) fn abandoned(&self, _epoch: u64) -> Result<()> {
        Err(self.fail(ProofFailure::Abandoned))
    }
    pub(crate) fn detached(
        &self,
        epoch: u64,
        registry_detached: bool,
        peer_detached: bool,
    ) -> Result<()> {
        self.access(|s| {
            require(registry_detached && peer_detached, ProofFailure::Mismatch)?;
            s.emit(Event::Detached { epoch })?;
            s.emit(Event::Ready)
        })
    }
    pub(crate) fn first_pawn_admitted(&self, epoch: u64) -> bool {
        self.access(|s| Ok(s.evidence.epoch(epoch) && s.evidence.pawn))
            .unwrap_or(false)
    }
}

#[path = "leave_runtime_proof/pawn.rs"]
mod pawn;
#[path = "leave_runtime_proof/reader.rs"]
mod reader;
use reader::predecessor;
#[cfg(test)]
mod tests;
pub use reader::validate_descriptor;
pub use reader::{
    OwnerConfirmation, inspect_phase, publish_abort, publish_completion, seal_exit_store,
};
