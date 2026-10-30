//! The capture decoder against synthetic datagrams built with this
//! repository's own writers: no private capture is read.
#[allow(dead_code)]
#[path = "../examples/capture_decode.rs"]
mod capture_decode;

use std::fs;

use anyhow::Result;
use icarus_bevy::experimental::{
    actor_channel::{
        ActorBunchBody, ActorChannelCodec, ActorChannelLimits, BlockTarget, CompressedRotator,
        ContentBlock, DynamicSpawn, NetGuidRef, SpawnRecord, SpawnVector, SubObjectClass,
        encode_actor_bunch_body,
    },
    archive::ByteOrder,
    packet::{
        BitWriter, Bunch, BunchHeader, BunchLimits, Name, Packet, PacketConfig, PacketHeader,
        Payload, Receiver, write_packet,
    },
    peer_lab::PeerLabConfig,
};
use serde_json::{Value, json};

const ACTOR_NAME: u32 = 102;

fn config() -> PacketConfig {
    PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Client,
        limits: BunchLimits {
            max_channels: 32_767,
            ..PeerLabConfig::default().packet.packet.limits
        },
    }
}

fn codec() -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    }
}

fn header(reliable_sequence: Option<u32>) -> BunchHeader {
    BunchHeader {
        control: false,
        open: false,
        close: false,
        close_reason: None,
        paused: false,
        reliable: reliable_sequence.is_some(),
        channel_index: 3,
        has_package_map_exports: false,
        has_must_be_mapped_guids: false,
        partial: false,
        reliable_sequence_raw: reliable_sequence,
        partial_initial: false,
        partial_final: false,
        channel_name: reliable_sequence.map(|_| Name::Hardcoded(ACTOR_NAME)),
    }
}

fn datagram(sequence: u16, bunches: Vec<Bunch>) -> Result<String> {
    let packet = Packet {
        magic: 0,
        header: PacketHeader {
            sequence,
            acked_sequence: 0,
            history: vec![0],
        },
        info: None,
        bunches,
    };
    Ok(write_packet(&packet, &config())?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// One export: GUID 5 with path and checksum flags, a null outer, a narrow
/// path and a checksum; then a GUID whose checksum flag stands without a path.
fn export_payload() -> Payload {
    let mut writer = BitWriter::new();
    writer.write_bit(false);
    writer.write_u32(2);
    writer.write_int_packed(5);
    writer.write_bits(0x05, 8);
    writer.write_int_packed(0);
    let path = b"/Synthetic/Archetype\0";
    writer.write_u32(path.len() as u32);
    writer.write_bytes(path);
    writer.write_u32(0x1234_5678);
    writer.write_int_packed(3);
    writer.write_bits(0x04, 8);
    Payload::from_writer(writer)
}

fn actor_block(bits: u32) -> ContentBlock {
    let mut payload = BitWriter::new();
    payload.write_bits(0x2a, bits);
    ContentBlock {
        has_rep_layout: true,
        target: BlockTarget::Actor,
        payload: Payload::from_writer(payload),
    }
}

fn spawn_body() -> Result<Payload> {
    let body = ActorBunchBody {
        spawn: Some(SpawnRecord {
            actor: NetGuidRef::plain(2),
            dynamic: Some(DynamicSpawn {
                archetype: NetGuidRef::plain(5),
                level: Some(NetGuidRef::plain(3)),
                location: Some(SpawnVector::Quantized {
                    bits: 19,
                    components: [1 << 20, (1 << 20) + 10, (1 << 20) - 1000],
                }),
                rotation: Some(CompressedRotator::from_degrees(0.0, 90.0, 0.0)?),
                scale: None,
                velocity: None,
            }),
        }),
        blocks: vec![actor_block(8)],
    };
    encode_actor_bunch_body(&body, &codec())
}

/// Reliable partial open with an export-only initial part and a data final
/// part, a resend of the final part, then an unreliable update.
fn synthetic_capture() -> Result<String> {
    let initial = Bunch {
        header: BunchHeader {
            control: true,
            open: true,
            has_package_map_exports: true,
            partial: true,
            partial_initial: true,
            ..header(Some(100))
        },
        payload: export_payload(),
    };
    let last = Bunch {
        header: BunchHeader {
            partial: true,
            partial_final: true,
            ..header(Some(101))
        },
        payload: spawn_body()?,
    };
    let update = Bunch {
        header: header(None),
        payload: encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks: vec![actor_block(5)],
            },
            &codec(),
        )?,
    };
    let lines = [
        json!({"event":"join_sent"}),
        json!({"event":"reply", "ms":1, "hex":datagram(10, vec![initial])?}),
        json!({"event":"reply", "ms":2, "hex":datagram(11, vec![last.clone()])?}),
        json!({"event":"reply", "ms":3, "hex":datagram(12, vec![last])?}),
        json!({"event":"reply", "ms":4, "hex":datagram(14, vec![update])?}),
    ];
    Ok(lines.iter().map(|line| format!("{line}\n")).collect())
}

fn at<'a>(report: &'a Value, pointer: &str) -> &'a Value {
    report
        .pointer(pointer)
        .unwrap_or_else(|| panic!("missing {pointer}"))
}

#[test]
fn reassembles_partials_and_separates_the_export_hypothesis() -> Result<()> {
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("probe.jsonl");
    fs::write(&input, synthetic_capture()?)?;
    let report = capture_decode::run_in(&input, &work.join("detail.jsonl"), &work)?;

    assert_eq!(at(&report, "/input/replies"), 4);
    assert_eq!(at(&report, "/wire/packets_decoded"), 4);
    assert_eq!(at(&report, "/wire/packets_missing"), 1);
    assert_eq!(at(&report, "/wire/reliable_delivered"), 2);
    assert_eq!(at(&report, "/wire/reliable_duplicates"), 1);
    assert_eq!(at(&report, "/wire/channels_opened"), 1);
    assert_eq!(at(&report, "/wire/opened_names/hardcoded:102"), 1);

    let strict = at(&report, "/passes/strict");
    assert_eq!(at(strict, "/partial/completed"), 1);
    assert_eq!(
        at(
            strict,
            "/actor/hardcoded:102/open_failed/data unknown: export prefix unparsed (layout undocumented)"
        ),
        1
    );
    assert_eq!(at(strict, "/actor/hardcoded:102/update_parsed"), 1);
    assert_eq!(at(strict, "/spawn/records"), 0);

    let hypothesis = at(&report, "/passes/export_hypothesis");
    assert_eq!(at(hypothesis, "/exports/parsed"), 1);
    assert_eq!(at(hypothesis, "/exports/codec_reader_parsed"), 1);
    assert_eq!(at(hypothesis, "/exports/codec_reencoded_exact"), 1);
    assert_eq!(at(hypothesis, "/exports/with_path"), 1);
    assert_eq!(at(hypothesis, "/exports/without_path"), 1);
    assert_eq!(at(hypothesis, "/exports/leftover_bits_after_exports"), 0);
    assert_eq!(at(hypothesis, "/partial/completed_parts/2"), 1);
    assert_eq!(at(hypothesis, "/partial/codec_resplit_exact"), 1);
    assert_eq!(at(hypothesis, "/actor/hardcoded:102/open_parsed"), 1);
    assert_eq!(at(hypothesis, "/actor/hardcoded:102/update_parsed"), 1);
    assert_eq!(at(hypothesis, "/level_gate/with_level_only"), 1);
    assert_eq!(
        at(hypothesis, "/spawn/archetype_resolved_by_earlier_export"),
        1
    );
    assert_eq!(at(hypothesis, "/spawn/location/finite_within_1e6"), 1);
    assert_eq!(at(hypothesis, "/spawn/location/z/min"), -100.0);
    assert_eq!(at(hypothesis, "/spawn/yaw_degrees/max"), 90.0);
    assert_eq!(at(hypothesis, "/blocks/total"), 2);
    Ok(())
}

#[test]
fn zero_bit_subobject_blocks_require_classification_and_separate_lifecycle_counts() -> Result<()> {
    let mut exp = BitWriter::new();
    exp.write_bit(false);
    exp.write_u32(2);
    exp.write_int_packed(10);
    exp.write_bits(0x01, 8);
    exp.write_int_packed(0);
    let known_path = b"BPC_AccumulationComponent\0";
    exp.write_u32(known_path.len() as u32);
    exp.write_bytes(known_path);
    exp.write_int_packed(12);
    exp.write_bits(0x01, 8);
    exp.write_int_packed(0);
    let unknown_path = b"UnknownSyntheticSubObject\0";
    exp.write_u32(unknown_path.len() as u32);
    exp.write_bytes(unknown_path);

    let initial = Bunch {
        header: BunchHeader {
            control: true,
            open: true,
            has_package_map_exports: true,
            partial: true,
            partial_initial: true,
            ..header(Some(100))
        },
        payload: Payload::from_writer(exp),
    };
    let body = ActorBunchBody {
        spawn: Some(SpawnRecord {
            actor: NetGuidRef::plain(3),
            dynamic: None,
        }),
        blocks: vec![
            ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::SubObject {
                    object: NetGuidRef::plain(10),
                    class: SubObjectClass::StablyNamed,
                },
                payload: Payload::from_writer(BitWriter::new()),
            },
            ContentBlock {
                has_rep_layout: false,
                target: BlockTarget::SubObject {
                    object: NetGuidRef::plain(12),
                    class: SubObjectClass::StablyNamed,
                },
                payload: Payload::from_writer(BitWriter::new()),
            },
        ],
    };
    let last = Bunch {
        header: BunchHeader {
            partial: true,
            partial_final: true,
            ..header(Some(101))
        },
        payload: encode_actor_bunch_body(&body, &codec())?,
    };
    let lines = [
        json!({"event":"reply", "ms":1, "hex":datagram(10, vec![initial])?}),
        json!({"event":"reply", "ms":2, "hex":datagram(11, vec![last])?}),
    ];
    let capture: String = lines.iter().map(|line| format!("{line}\n")).collect();

    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("probe.jsonl");
    fs::write(&input, capture)?;
    let report = capture_decode::run_in(&input, &work.join("detail.jsonl"), &work)?;

    let rep = at(&report, "/passes/export_hypothesis/rep_replay");
    assert_eq!(at(rep, "/subobject_lifecycle_blocks_attempted"), 1);
    assert_eq!(at(rep, "/subobject_lifecycle_blocks_decoded"), 1);
    assert_eq!(at(rep, "/subobject_lifecycle_blocks_reencoded_exact"), 1);
    assert_eq!(at(rep, "/custom_delta_blocks_attempted"), 0);
    assert_eq!(at(rep, "/custom_delta_blocks_reencoded_exact"), 0);
    assert_eq!(
        at(
            rep,
            "/unclassified_subobject_targets/UnknownSyntheticSubObject"
        ),
        1
    );
    assert_eq!(at(rep, "/all_blocks_coverage_complete"), false);
    Ok(())
}

#[test]
fn refuses_detail_output_outside_work() -> Result<()> {
    let root = tempfile::tempdir()?;
    let input = root.path().join("probe.jsonl");
    fs::write(&input, "")?;
    assert!(capture_decode::run(&input, &root.path().join("detail.jsonl")).is_err());
    // A `work` ancestor elsewhere does not count: only the given work root does.
    let repo = root.path().join("work/repo");
    fs::create_dir_all(repo.join("docs"))?;
    fs::create_dir_all(repo.join("work"))?;
    let outside = repo.join("docs/detail.jsonl");
    assert!(capture_decode::run_in(&input, &outside, &repo.join("work")).is_err());
    assert!(
        capture_decode::run_in(&input, &repo.join("work/detail.jsonl"), &repo.join("work")).is_ok()
    );
    Ok(())
}

#[test]
fn private_values_follow_same_class_subobjects_across_reordered_updates() -> Result<()> {
    use icarus_bevy::experimental::{
        release::ReleaseProfile,
        rep_layout::{
            PropertyStream, RepEntry, RepLayoutCodec, RepValue, encode_rep_properties,
            modifier_state_component_rep_layout,
        },
    };

    let mut export = BitWriter::new();
    export.write_bit(false);
    export.write_u32(1);
    export.write_int_packed(5);
    export.write_bits(0x01, 8);
    export.write_int_packed(0);
    let class = b"ModifierStateComponent\0";
    export.write_u32(class.len() as u32);
    export.write_bytes(class);
    let initial = Bunch {
        header: BunchHeader {
            control: true,
            open: true,
            has_package_map_exports: true,
            partial: true,
            partial_initial: true,
            ..header(Some(100))
        },
        payload: Payload::from_writer(export),
    };
    let block = |guid: u32, uid: Option<i32>, remaining: u16| -> Result<ContentBlock> {
        let mut entries = Vec::new();
        if let Some(uid) = uid {
            entries.push(RepEntry {
                handle: 5,
                checksum: None,
                value: RepValue::Int(uid),
            });
        }
        entries.push(RepEntry {
            handle: 8,
            checksum: None,
            value: RepValue::UInt16(remaining),
        });
        Ok(ContentBlock {
            has_rep_layout: true,
            target: BlockTarget::SubObject {
                object: NetGuidRef::plain(guid),
                class: SubObjectClass::Class(NetGuidRef::plain(5)),
            },
            payload: encode_rep_properties(
                &PropertyStream {
                    entries,
                    terminator_checksum: None,
                },
                &modifier_state_component_rep_layout(),
                &RepLayoutCodec {
                    actor: codec(),
                    checksums: false,
                    limits: Default::default(),
                },
            )?,
        })
    };
    let opened = Bunch {
        header: BunchHeader {
            partial: true,
            partial_final: true,
            ..header(Some(101))
        },
        payload: encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: Some(SpawnRecord {
                    actor: NetGuidRef::plain(3),
                    dynamic: None,
                }),
                blocks: vec![block(10, Some(9001), 100)?, block(12, Some(9002), 200)?],
            },
            &codec(),
        )?,
    };
    let updated = Bunch {
        header: header(Some(102)),
        payload: encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks: vec![block(12, None, 199)?, block(10, None, 99)?],
            },
            &codec(),
        )?,
    };
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("probe.jsonl");
    let lines = [
        json!({"event":"reply", "ms":1, "hex":datagram(10, vec![initial])?}),
        json!({"event":"reply", "ms":2, "hex":datagram(11, vec![opened])?}),
        json!({"event":"reply", "ms":3, "hex":datagram(12, vec![updated])?}),
    ];
    fs::write(
        &input,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )?;
    let ordinary = work.join("ordinary.jsonl");
    let ordinary_report = capture_decode::run_in(&input, &ordinary, &work)?;
    assert!(!fs::read_to_string(ordinary)?.contains("\"object_guid\""));

    let detail = work.join("values.jsonl");
    let report = capture_decode::run_recording(
        &input,
        &detail,
        &work,
        &ReleaseProfile::release_3_0_32(),
        Some(3),
    )?;
    assert_eq!(report["passes"], ordinary_report["passes"]);
    assert_eq!(report["wire"], ordinary_report["wire"]);
    let rows: Vec<Value> = fs::read_to_string(detail)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let values: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["pass"] == "export_hypothesis"
                && matches!(
                    row["kind"].as_str(),
                    Some("open_block_values" | "update_block_values")
                )
        })
        .collect();
    assert_eq!(values.len(), 4);
    for (row, (guid, ms, remaining)) in
        values
            .iter()
            .zip([(10, 2, 100), (12, 2, 200), (12, 3, 199), (10, 3, 99)])
    {
        assert_eq!(row["object_guid"], guid);
        assert_eq!(row["ms"], ms);
        assert!(row["object_path"].is_null());
        assert_eq!(row["class_path"], "ModifierStateComponent");
        let properties = row["values"]["Ok"]["properties"].as_array().unwrap();
        let timer = properties.iter().find(|entry| entry[0] == 8).unwrap();
        assert_eq!(timer[1]["uint16"], remaining);
    }
    Ok(())
}
