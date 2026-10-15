use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::Write,
    os::unix::fs::{FileTypeExt, MetadataExt, symlink},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

const MODES: [&[&str]; 5] = [
    &[],
    &["--inner-tags"],
    &["--recorders"],
    &["--inner-tags", "--recorders"],
    &["--preserve-check"],
];
const MARKER: &str = "AUTHORED_INSPECT_VALUE_MARKER";

struct OwnedChild(Option<Child>);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn inspect_bounded(root: &Path, source: &Path, mode: &[&str]) -> Output {
    let mut child = OwnedChild(Some(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .current_dir(root)
            .args(["inspect-save", "--source"])
            .arg(source)
            .args(mode)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "inspection did not exit");
        thread::sleep(Duration::from_millis(1));
    }
    child.0.take().unwrap().wait_with_output().unwrap()
}

#[derive(Debug, PartialEq)]
struct Entry {
    kind: &'static str,
    device: u64,
    inode: u64,
    mode: u32,
    bytes: Option<Vec<u8>>,
    link: Option<PathBuf>,
}

fn inventory(root: &Path) -> BTreeMap<OsString, Entry> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let file_type = metadata.file_type();
            let kind = if file_type.is_symlink() {
                "symlink"
            } else if file_type.is_file() {
                "regular"
            } else if file_type.is_fifo() {
                "fifo"
            } else if file_type.is_dir() {
                assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
                "directory"
            } else {
                panic!("unexpected authored source kind");
            };
            (
                entry.file_name(),
                Entry {
                    kind,
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    mode: metadata.mode(),
                    bytes: file_type.is_file().then(|| fs::read(&path).unwrap()),
                    link: file_type
                        .is_symlink()
                        .then(|| fs::read_link(&path).unwrap()),
                },
            )
        })
        .collect()
}

fn string(value: &str) -> Vec<u8> {
    [
        i32::try_from(value.len() + 1)
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        value.as_bytes(),
        &[0],
    ]
    .concat()
}

fn tag(name: &str, kind: &str, extra: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        string(name).as_slice(),
        &string(kind),
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &[0; 4],
        extra,
        &[0],
        payload,
    ]
    .concat()
}

fn authored_envelope() -> Vec<u8> {
    // Manually authored recorder/root tags, not the production archive encoder.
    let fields = [
        tag("ComponentClassName", "StrProperty", &[], &string(MARKER)),
        tag(
            "BinaryData",
            "ArrayProperty",
            &string("ByteProperty"),
            &[
                i32::try_from(MARKER.len())
                    .unwrap()
                    .to_le_bytes()
                    .as_slice(),
                MARKER.as_bytes(),
            ]
            .concat(),
        ),
        string("None"),
    ]
    .concat();
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &[string("StateRecorderBlob"), vec![0; 16]].concat(),
        &fields,
    );
    let payload = [
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &string("StructProperty"),
            &[1_i32.to_le_bytes().as_slice(), &shared].concat(),
        ),
        string("None"),
    ]
    .concat();
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&payload).unwrap();
    let compressed = encoder.finish().unwrap();
    let hash = Sha1::digest(&payload)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    serde_json::to_vec(&json!({"ProspectBlob": {
        "Hash":hash,"TotalLength":compressed.len(),"DataLength":0,
        "UncompressedLength":payload.len(),"BinaryBlob":STANDARD.encode(compressed)
    }}))
    .unwrap()
}

#[test]
fn special_sources_fail_in_every_inspection_mode_without_writes() {
    for mode in MODES {
        for kind in ["fifo", "fifo-symlink", "directory"] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("authored-source");
            if kind == "directory" {
                fs::create_dir(&source).unwrap();
            } else {
                rustix::fs::mkfifoat(
                    rustix::fs::CWD,
                    &source,
                    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                )
                .unwrap();
            }
            let link = directory.path().join("authored-link");
            let selected = if kind == "fifo-symlink" {
                symlink(&source, &link).unwrap();
                &link
            } else {
                &source
            };
            let before = inventory(directory.path());
            let output = inspect_bounded(directory.path(), selected, mode);
            assert_eq!(output.status.code(), Some(1), "accepted {kind}: {mode:?}");
            assert!(output.stdout.is_empty());
            let error: Value = serde_json::from_slice(&output.stderr).unwrap();
            assert_eq!(error["event"], "fatal");
            assert!(
                error["error"]
                    .as_str()
                    .unwrap()
                    .contains("source must be a regular file")
            );
            assert_eq!(inventory(directory.path()), before);
        }
    }
}

#[test]
fn regular_sources_and_regular_symlinks_preserve_private_values_and_files() {
    for mode in MODES {
        for linked in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("authored.json");
            fs::write(&source, authored_envelope()).unwrap();
            let link = directory.path().join("authored-link.json");
            let selected = if linked {
                symlink(&source, &link).unwrap();
                &link
            } else {
                &source
            };
            let before = inventory(directory.path());
            let output = inspect_bounded(directory.path(), selected, mode);
            assert!(output.status.success(), "{:?}", output.stderr);
            assert!(output.stderr.is_empty());
            assert!(!String::from_utf8_lossy(&output.stdout).contains(MARKER));
            let report: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(report["world_imported"], false);
            let validation = if mode.contains(&"--preserve-check") {
                let preservation = &report["preservation"];
                assert_eq!(preservation["recorder_count"], 1);
                assert_eq!(preservation["byte_identical"], true);
                assert_eq!(preservation["values_interpreted"], false);
                assert_eq!(report["native_schema"], 3);
                assert_eq!(report["native_round_trip"], true);
                assert!(report["native_generation_bytes"].as_u64().unwrap() > 0);
                assert!(report["native_second_generation_bytes"].as_u64().unwrap() > 0);
                assert!(report["native_sidecar_bytes"].as_u64().unwrap() > 0);
                assert_eq!(report["native_sidecar_rewritten"], false);
                assert!(report["second_save_ms"].is_u64());
                "experimental_lossless_preservation"
            } else if mode.contains(&"--recorders") {
                assert_eq!(report["recorders"]["recorder_count"], 1);
                assert_eq!(report["inner_tags"]["root_property_count"], 1);
                "experimental_recorder_framing_only"
            } else if mode.contains(&"--inner-tags") {
                assert_eq!(report["inner_tags"]["root_property_count"], 1);
                "experimental_root_tags_only"
            } else {
                assert_eq!(report["legacy_sha1_matches"], true);
                "experimental_envelope_only"
            };
            assert_eq!(report["validation"], validation);
            assert_eq!(inventory(directory.path()), before);
        }
    }
}

#[test]
fn regular_file_size_and_private_json_diagnostics_remain_bounded() {
    for mode in MODES {
        for oversized in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let source = directory.path().join("authored.json");
            let bytes = if oversized {
                let mut bytes = authored_envelope();
                bytes.resize(icarus_bevy::prospect::MAX_ENVELOPE_BYTES + 1, b' ');
                bytes
            } else {
                let mut envelope: Value = serde_json::from_slice(&authored_envelope()).unwrap();
                envelope["ProspectBlob"]["TotalLength"] = MARKER.into();
                serde_json::to_vec(&envelope).unwrap()
            };
            fs::write(&source, &bytes).unwrap();
            let before = inventory(directory.path());
            let output = inspect_bounded(directory.path(), &source, mode);
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            let error: Value = serde_json::from_slice(&output.stderr).unwrap();
            assert_eq!(error["event"], "fatal");
            let message = error["error"].as_str().unwrap();
            let stage = if oversized {
                "envelope exceeds 4 MiB research limit"
            } else {
                "invalid prospect envelope JSON: Data at line"
            };
            assert!(message.contains(stage));
            assert!(!message.contains(MARKER));
            assert_eq!(inventory(directory.path()), before);
        }
    }
}

fn server_bounded(root: &Path, args: &[&std::ffi::OsStr]) -> Output {
    let mut child = OwnedChild(Some(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .current_dir(root)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "server command did not exit");
        thread::sleep(Duration::from_millis(1));
    }
    child.0.take().unwrap().wait_with_output().unwrap()
}

#[test]
fn preserve_save_seeds_a_new_world_that_runs_and_never_overwrites() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("authored.json");
    fs::write(&source, authored_envelope()).unwrap();
    let world = directory.path().join("world");
    let preserve = |root: &Path| {
        server_bounded(
            root,
            &[
                "preserve-save".as_ref(),
                "--source".as_ref(),
                source.as_os_str(),
                "--data-dir".as_ref(),
                world.as_os_str(),
            ],
        )
    };
    let output = preserve(directory.path());
    assert!(output.status.success(), "{:?}", output.stderr);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(MARKER));
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["event"], "preserved_world_created");
    assert_eq!(report["native_schema"], 3);
    assert_eq!(report["world_imported"], false);
    assert_eq!(report["preservation"]["byte_identical"], true);
    let sidecars = || {
        fs::read_dir(&world)
            .unwrap()
            .filter(|entry| {
                let name = entry.as_ref().unwrap().file_name();
                name.to_string_lossy().starts_with("preserved-")
            })
            .count()
    };
    assert_eq!(sidecars(), 1);

    // A second preservation into the same world is refused without writes.
    let before = inventory(&world);
    let refused = preserve(directory.path());
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("refusing to overwrite"));
    assert_eq!(inventory(&world), before);

    // The world runs, autosaves keep its section and report schema 3.
    let run = server_bounded(
        directory.path(),
        &[
            "run".as_ref(),
            "--data-dir".as_ref(),
            world.as_os_str(),
            "--max-ticks".as_ref(),
            "5".as_ref(),
        ],
    );
    assert!(run.status.success(), "{:?}", run.stderr);
    let started = String::from_utf8_lossy(&run.stderr)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|event| event["event"] == "started")
        .unwrap();
    assert_eq!(started["native_schema"], 3);
    let committed = String::from_utf8_lossy(&run.stderr)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .rfind(|event| event["event"] == "save_committed")
        .unwrap();
    assert!(committed["save_latency_micros"].as_u64().is_some());
    assert_eq!(sidecars(), 1);
}
