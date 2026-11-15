use super::*;
use sha2::{Digest, Sha256};

pub(super) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExitSeal {
    format: u8,
    case_id: String,
    phase: u8,
    invocation: String,
    record: Record,
    context: Context,
    current: u64,
    previous: Option<u64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u8,
    current: Generation,
    previous: Option<Generation>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Generation {
    number: u64,
    sha256: String,
}

fn read_evidence(
    dir: &Directory,
    name: &str,
    case: &str,
    phase: u8,
    id: Option<&str>,
    expected: Option<RestartExpectation>,
) -> Result<(Evidence, String)> {
    let bytes = dir.read(Path::new(name), JOURNAL_CAP)?;
    require(
        !bytes.is_empty() && bytes.last() == Some(&b'\n'),
        ProofFailure::Binding,
    )?;
    let mut evidence = Evidence::new(phase, expected);
    let mut invocation_id = String::new();
    for (index, line) in bytes.split_inclusive(|b| *b == b'\n').enumerate() {
        require(
            index < EVENTS && line.len() <= EVENT_CAP && line.len() > 1,
            ProofFailure::Limit,
        )?;
        let row: Row = decode(line)?;
        require(
            row.format == 1
                && row.ordinal == index + 1
                && row.case_id == case
                && row.phase == phase
                && invocation(&row.invocation),
            ProofFailure::Binding,
        )?;
        if index == 0 {
            invocation_id = row.invocation.clone();
        }
        require(
            row.invocation == invocation_id && id.is_none_or(|id| id == row.invocation),
            ProofFailure::Binding,
        )?;
        if let Event::Pawn {
            payload_bits,
            payload_sha256,
            ..
        } = &row.data
        {
            let payload = dir.read(Path::new(&format!("phase-{phase}-pawn.bin")), 64 * 1024)?;
            require(
                payload.len() == payload_bits.div_ceil(8) && digest(&payload) == *payload_sha256,
                ProofFailure::Mismatch,
            )?;
            if !payload_bits.is_multiple_of(8) {
                require(
                    payload
                        .last()
                        .is_some_and(|b| *b >> (payload_bits % 8) == 0),
                    ProofFailure::Mismatch,
                )?;
            }
        }
        evidence.apply(&row.data)?;
    }
    Ok((evidence, invocation_id))
}

fn check_commit(dir: &Directory, e: &Evidence) -> Result<()> {
    let actual = e.readback.as_ref().ok_or(error(ProofFailure::Order))?;
    let manifest: Manifest = decode(&dir.read(
        Path::new(&format!("phase-{}-leave-manifest.json", e.phase)),
        16 * 1024,
    )?)?;
    let bytes = dir.read(
        Path::new(&format!("phase-{}-leave-generation.json", e.phase)),
        GENERATION_CAP,
    )?;
    require(
        manifest.schema_version == 1
            && manifest.current.number == actual.number
            && manifest.current.sha256 == actual.sha256
            && digest(&bytes) == actual.sha256,
        ProofFailure::Mismatch,
    )?;
    let snapshot: WorldSnapshot = decode(&bytes)?;
    require(
        snapshot.schema_version == crate::simulation::SNAPSHOT_SCHEMA_PLAYERS
            && snapshot.preserved_prospect.is_none()
            && snapshot.tick == actual.tick,
        ProofFailure::Mismatch,
    )?;
    SnapshotStore::preflight(&snapshot).map_err(|_| error(ProofFailure::Mismatch))?;
    let record = &e.cutoff.as_ref().ok_or(error(ProofFailure::Order))?.1;
    let players = snapshot
        .native_players
        .as_ref()
        .ok_or(error(ProofFailure::Mismatch))?;
    require(
        selected(players, record.key()?)?.same_stored_record(record)
            && Some(context(players)?) == e.opened,
        ProofFailure::Mismatch,
    )
}

pub(super) fn predecessor(dir: &Directory, d: &Descriptor) -> Result<Option<RestartExpectation>> {
    if d.phase == 1 {
        return Ok(None);
    }
    let predecessor = d
        .predecessor
        .as_deref()
        .ok_or(error(ProofFailure::Binding))?;
    require(predecessor != d.output, ProofFailure::Binding)?;
    let (e, id) = read_evidence(dir, predecessor, &d.case_id, 1, None, None)?;
    require(e.ready, ProofFailure::Order)?;
    check_commit(dir, &e)?;
    let seal: ExitSeal = decode(&dir.read(
        Path::new(d.exit_seal.as_deref().ok_or(error(ProofFailure::Binding))?),
        EVENT_CAP,
    )?)?;
    let record = e
        .cutoff
        .as_ref()
        .ok_or(error(ProofFailure::Order))?
        .1
        .clone();
    let ctx = e.opened.as_ref().ok_or(error(ProofFailure::Order))?.clone();
    require(
        seal.format == 1
            && seal.case_id == d.case_id
            && seal.phase == 1
            && seal.invocation == id
            && seal.record == record
            && seal.context == ctx
            && seal.current > 0,
        ProofFailure::Mismatch,
    )?;
    // Exit artifacts are immutable copies of the actual lock-held current/previous
    // manifest selection, separately from the leave's own durable generation.
    let generation = check_exit_files(dir, 1, &seal, &record, &ctx)?;
    Ok(Some(RestartExpectation {
        record,
        context: ctx,
        generation,
    }))
}
fn check_exit_files(
    dir: &Directory,
    phase: u8,
    seal: &ExitSeal,
    record: &Record,
    ctx: &Context,
) -> Result<GenerationProof> {
    let m: Manifest = decode(&dir.read(
        Path::new(&format!("phase-{phase}-exit-manifest.json")),
        16 * 1024,
    )?)?;
    require(
        m.schema_version == 1
            && m.current.number == seal.current
            && m.previous.as_ref().map(|g| g.number) == seal.previous,
        ProofFailure::Mismatch,
    )?;
    let mut current = None;
    for (label, g) in
        std::iter::once(("current", &m.current)).chain(m.previous.as_ref().map(|g| ("previous", g)))
    {
        let bytes = dir.read(
            Path::new(&format!("phase-{phase}-exit-{label}.json")),
            GENERATION_CAP,
        )?;
        require(digest(&bytes) == g.sha256, ProofFailure::Mismatch)?;
        let snapshot: WorldSnapshot = decode(&bytes)?;
        require(
            snapshot.schema_version == crate::simulation::SNAPSHOT_SCHEMA_PLAYERS
                && snapshot.preserved_prospect.is_none(),
            ProofFailure::Mismatch,
        )?;
        SnapshotStore::preflight(&snapshot).map_err(|_| error(ProofFailure::Mismatch))?;
        let players = snapshot
            .native_players
            .as_ref()
            .ok_or(error(ProofFailure::Mismatch))?;
        // Current must equal the cutoff; previous may predate the cutoff, so
        // checksum/type validation is sufficient for that retained fallback.
        if label == "current" {
            current = Some(GenerationProof {
                number: g.number,
                sha256: g.sha256.clone(),
                tick: snapshot.tick,
            });
            require(
                selected(players, record.key()?)?.same_stored_record(record)
                    && context(players)? == *ctx,
                ProofFailure::Mismatch,
            )?;
        }
    }
    current.ok_or(error(ProofFailure::Mismatch))
}
pub fn validate_descriptor(path: &Path, root: &Path) -> Result<()> {
    let dir = Directory::open(root)?;
    dir.absent("abort.json")?;
    let d = descriptor(&dir, path)?;
    predecessor(&dir, &d)?;
    use rustix::fs::{AtFlags, statat};
    match statat(&dir.file, d.output.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
        Err(rustix::io::Errno::NOENT) => Ok(()),
        _ => Err(error(ProofFailure::Binding)),
    }
}
pub fn inspect_phase(path: &Path, root: &Path, manager_invocation: &str) -> Result<PhaseVerdict> {
    require(invocation(manager_invocation), ProofFailure::Binding)?;
    let dir = Directory::open(root)?;
    dir.absent("abort.json")?;
    let d = descriptor(&dir, path)?;
    let expected = predecessor(&dir, &d)?;
    let (e, id) = read_evidence(
        &dir,
        &d.output,
        &d.case_id,
        d.phase,
        Some(manager_invocation),
        expected,
    )?;
    require(e.ready, ProofFailure::Order)?;
    check_commit(&dir, &e)?;
    Ok(e.verdict(&id))
}
pub fn seal_exit_store(
    path: &Path,
    root: &Path,
    manager_invocation: &str,
    store_path: &Path,
) -> Result<()> {
    let dir = Directory::open(root)?;
    let d = descriptor(&dir, path)?;
    let verdict = inspect_phase(path, root, manager_invocation)?;
    require(verdict.phase_ready, ProofFailure::Order)?;
    let (e, _) = read_evidence(
        &dir,
        &d.output,
        &d.case_id,
        d.phase,
        Some(manager_invocation),
        predecessor(&dir, &d)?,
    )?;
    let record = e
        .cutoff
        .as_ref()
        .ok_or(error(ProofFailure::Order))?
        .1
        .clone();
    let ctx = e.opened.as_ref().ok_or(error(ProofFailure::Order))?.clone();
    use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
    let fd = openat2(
        &dir.file,
        dir.relative(store_path)?,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::BENEATH | ResolveFlags::NO_SYMLINKS,
    )
    .map_err(|_| error(ProofFailure::Binding))?;
    let pinned = File::from(fd);
    let m = pinned
        .metadata()
        .map_err(|_| error(ProofFailure::Binding))?;
    require(
        m.uid() == dir.uid && m.mode() & 0o777 == 0o700,
        ProofFailure::Binding,
    )?;
    use std::os::fd::AsRawFd;
    let store =
        SnapshotStore::open_existing(Path::new(&format!("/proc/self/fd/{}", pinned.as_raw_fd())))
            .map_err(|_| error(ProofFailure::Io))?;
    let loaded = store
        .load()
        .map_err(|_| error(ProofFailure::Io))?
        .ok_or(error(ProofFailure::Mismatch))?;
    require(!loaded.recovered_previous, ProofFailure::Mismatch)?;
    let players = loaded
        .snapshot
        .native_players
        .as_ref()
        .ok_or(error(ProofFailure::Mismatch))?;
    require(
        selected(players, record.key()?)?.same_stored_record(&record) && context(players)? == ctx,
        ProofFailure::Mismatch,
    )?;
    let (manifest, generations) = store
        .private_exit_generations(&pinned)
        .map_err(|_| error(ProofFailure::Mismatch))?;
    dir.publish(&format!("phase-{}-exit-manifest.json", d.phase), &manifest)?;
    for (index, (_, _, bytes)) in generations.iter().enumerate() {
        dir.publish(
            &format!(
                "phase-{}-exit-{}.json",
                d.phase,
                if index == 0 { "current" } else { "previous" }
            ),
            bytes,
        )?;
    }
    let seal = ExitSeal {
        format: 1,
        case_id: d.case_id,
        phase: d.phase,
        invocation: manager_invocation.into(),
        record,
        context: ctx,
        current: loaded.selected_generation,
        previous: generations.get(1).map(|g| g.0),
    };
    dir.publish(&format!("phase-{}-exit.json", d.phase), &encode(&seal)?)
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerConfirmation {
    pub phase: u8,
    pub invocation: String,
    pub menu_confirmed: bool,
}
/// Record an explicit owner report; this does not inspect or infer a menu.
/// Failed pending files are retained. The visible name is never overwritten.
pub fn publish_owner_confirmation(
    root: &Path,
    path: &Path,
    owner: &OwnerConfirmation,
) -> Result<()> {
    require(
        invocation(&owner.invocation) && owner.menu_confirmed,
        ProofFailure::Binding,
    )?;
    let dir = Directory::open(root)?;
    dir.absent("abort.json")?;
    let d = descriptor(&dir, path)?;
    require(owner.phase == d.phase, ProofFailure::Mismatch)?;
    let name = format!("phase-{}-owner.json", d.phase);
    let pending = format!("phase-{}-owner.pending.json", d.phase);
    dir.absent(&name)?;
    let bytes = encode(owner)?;
    let mut file = dir.create(&pending)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| error(ProofFailure::Io))?;
    finish_owner_confirmation(&dir, &pending, &name, &file)
}
pub(super) fn finish_owner_confirmation(
    dir: &Directory,
    pending: &str,
    name: &str,
    file: &File,
) -> Result<()> {
    use rustix::fs::{AtFlags, RenameFlags, renameat_with, statat};
    dir.absent("abort.json")?;
    // Direct Linux no-replace operation: unsupported/error cases have no
    // copy, hardlink or overwriting-rename fallback, and retain the pending file.
    renameat_with(&dir.file, pending, &dir.file, name, RenameFlags::NOREPLACE)
        .map_err(|_| error(ProofFailure::Io))?;
    dir.file.sync_all().map_err(|_| error(ProofFailure::Io))?;
    let held = file.metadata().map_err(|_| error(ProofFailure::Io))?;
    let named =
        statat(&dir.file, name, AtFlags::SYMLINK_NOFOLLOW).map_err(|_| error(ProofFailure::Io))?;
    require(
        held.nlink() == 1
            && held.uid() == dir.uid
            && held.mode() & 0o777 == 0o600
            && named.st_ino == held.ino()
            && named.st_dev == held.dev()
            && named.st_nlink == 1,
        ProofFailure::Binding,
    )
}
pub fn publish_completion(
    root: &Path,
    path: &Path,
    verdict: &PhaseVerdict,
    owner: &OwnerConfirmation,
) -> Result<()> {
    let actual = inspect_phase(path, root, &verdict.invocation)?;
    require(
        actual == *verdict
            && actual.phase_ready
            && owner.menu_confirmed
            && owner.phase == actual.phase
            && owner.invocation == actual.invocation,
        ProofFailure::Mismatch,
    )?;
    let value = serde_json::json!({"phase":actual.phase,"invocation":actual.invocation,"joins":actual.joins,"normal_type":actual.normal_type,"exact_durable":actual.exact_durable,"prefix_acked":actual.prefix_acked,"detached":actual.detached,"owner_menu_confirmed":true,"restored_record_equal":actual.actual_load_equal && actual.first_projection_equal,"committed_readback":true,"actual_load_equal":actual.actual_load_equal,"first_projection_composed_before_activation":actual.pawn_admitted_before_activation,"food_debited":actual.food_debited});
    let dir = Directory::open(root)?;
    dir.absent("abort.json")?;
    dir.publish(
        &format!("phase-{}-complete.json", actual.phase),
        &encode(&value)?,
    )
}
pub fn publish_abort(root: &Path) -> Result<()> {
    Directory::open(root)?.publish("abort.json", b"{\"abort\":true}\n")
}
