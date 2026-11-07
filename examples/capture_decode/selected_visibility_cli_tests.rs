use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
};

#[test]
fn output_is_exclusive_private_and_cannot_alias_input() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.jsonl");
    let output = temp.path().join("output.json");
    fs::write(&input, b"\n").unwrap();
    let (_, file) = open_files(&input, &output, temp.path(), || {}).unwrap();
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    assert!(matches!(
        open_files(&input, &output, temp.path(), || {}),
        Err(CliError::Output)
    ));
    assert!(matches!(
        open_files(&input, &input, temp.path(), || {}),
        Err(CliError::Output)
    ));
    let alias = temp.path().join("alias.jsonl");
    fs::hard_link(&input, &alias).unwrap();
    assert!(matches!(
        open_files(&input, &alias, temp.path(), || {}),
        Err(CliError::Output)
    ));
    assert_eq!(fs::read(&input).unwrap(), b"\n");
}

#[test]
fn input_symlinks_parent_traversal_and_nonregular_inputs_are_refused() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.jsonl");
    let output = temp.path().join("output.json");
    fs::write(&input, b"\n").unwrap();
    let link = temp.path().join("link.jsonl");
    symlink(&input, &link).unwrap();
    assert!(matches!(
        open_files(&link, &output, temp.path(), || {}),
        Err(CliError::Input)
    ));
    assert!(matches!(
        open_files(temp.path(), &output, temp.path(), || {}),
        Err(CliError::Path)
    ));
    let nested = temp.path().join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(matches!(
        open_files(&nested, &output, temp.path(), || {}),
        Err(CliError::Input)
    ));
    assert!(matches!(
        open_files(
            &temp.path().join("nested/../input.jsonl"),
            &output,
            temp.path(),
            || {}
        ),
        Err(CliError::Path)
    ));
    assert!(!output.exists());
}

#[test]
fn swapped_output_parent_cannot_redirect_creation_outside_work_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("work");
    let outside = temp.path().join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    let parent = root.join("case");
    fs::create_dir(&parent).unwrap();
    let input = root.join("input.jsonl");
    fs::write(&input, b"\n").unwrap();
    let output = parent.join("result.json");
    let result = open_files(&input, &output, &root, || {
        fs::rename(&parent, root.join("original-case")).unwrap();
        symlink(&outside, &parent).unwrap();
    });
    assert!(matches!(result, Err(CliError::Output)));
    assert!(!outside.join("result.json").exists());
    assert!(!root.join("original-case/result.json").exists());
}

#[test]
fn invalid_cli_values_never_retain_raw_paths_or_clap_excerpts() {
    let error = run_cli_in(
        [
            "selected-visibility",
            "SECRET_INPUT",
            "SECRET_OUTPUT",
            "--archive",
            "SECRET_BAD_VALUE",
        ],
        Path::new("work"),
    )
    .unwrap_err();
    assert_eq!(error, CliError::Usage);
    assert_eq!(format!("{error:?}"), "Usage");
}

#[test]
fn valid_cli_writes_only_neutral_counts_and_keeps_completion_unverified() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("input.jsonl");
    let output = temp.path().join("output.json");
    let release = temp.path().join("release.toml");
    fs::write(
        &release,
        // This replay is qualified for 3.0.34 independently of the current pin.
        "[release]\napp_id = 2089300\nbuild_id = 26313077\nproject_version = \"3.0.34.159385-rel-DangerousHorizons\"\nnetwork_checksum = 4087436304\n",
    )
    .unwrap();
    let row = serde_json::json!({
        "format":1,"field_index":82,"offset_origin":"complete_bunch_payload",
        "ordinal":1,"connection_ordinal":1,"channel":7,"elapsed_ms":10,
        "engine_net_version":17,"archive_byte_order":"little","reliable":true,
        "partial":false,"reassembled":false,"parts":1,"bunch_payload_bits":17,
        "block_offset_bits":0,"block_length_bits":17,"block_payload_offset_bits":8,
        "block_payload_bits":9,"field_offset_bits":8,"field_length_bits":9,
        "payload_offset_bits":16,"payload_bits":1,"payload_hex":"00"
    });
    fs::write(&input, format!("{row}\n")).unwrap();
    let args = vec![
        OsString::from("selected-visibility"),
        input.into_os_string(),
        output.clone().into_os_string(),
        "--release-profile".into(),
        release.into_os_string(),
        "--archive".into(),
        "normal-little-endian-package-map-custom".into(),
        "--layout".into(),
        "normal-single-custom-struct".into(),
    ];
    let summary = run_cli_in(args, temp.path()).unwrap();
    assert_eq!(summary.accepted_absent, 1);
    assert!(!summary.capture_complete_verified && !summary.provenance_verified);
    let text = fs::read_to_string(&output).unwrap();
    assert!(
        !text.contains("payload_hex") && !text.contains("channel") && !text.contains("input.jsonl")
    );
    assert!(text.contains("\"capture_complete_verified\":false"));
}
