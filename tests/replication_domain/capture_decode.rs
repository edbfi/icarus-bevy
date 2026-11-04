//! The capture decoder against synthetic datagrams built with this
//! repository's own writers: no private capture is read.
#[allow(dead_code)]
#[path = "../../examples/capture_decode.rs"]
mod decoder;

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
fn handler_frames_do_not_consume_connection_sequences_or_expose_credentials() -> Result<()> {
    use icarus_bevy::experimental::{
        handshake::initial_request,
        steam_auth::{SteamAuthMessage, encode_datagram},
    };
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("probe.jsonl");
    let hex = |bytes: Vec<u8>| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let handler = [
        initial_request().to_vec(),
        encode_datagram(
            &SteamAuthMessage::UserData {
                ticket: "123456ABCDEF".encode_utf16().collect(),
                steam_id: 42,
            },
            ByteOrder::Little,
            0,
            0,
        )?,
        encode_datagram(
            &SteamAuthMessage::Result { accepted: true },
            ByteOrder::Little,
            0,
            0,
        )?,
    ];
    let mut rows = vec![json!({"event":"reply", "hex":datagram(10, vec![])?})];
    rows.extend(
        handler
            .into_iter()
            .map(|frame| json!({"event":"reply", "hex":hex(frame)})),
    );
    rows.push(json!({"event":"reply", "hex":datagram(11, vec![])?}));
    fs::write(
        &input,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )?;
    let detail = work.join("detail.jsonl");
    let report = decoder::run_in(&input, &detail, &work)?;
    assert_eq!(at(&report, "/wire/datagrams"), 5);
    assert_eq!(at(&report, "/wire/packets_decoded"), 2);
    assert_eq!(at(&report, "/wire/packets_missing"), 0);
    assert_eq!(at(&report, "/wire/stateless_handshakes"), 1);
    assert_eq!(at(&report, "/wire/auth_messages/user_data"), 1);
    assert_eq!(at(&report, "/wire/auth_messages/result_success"), 1);
    assert!(!report.to_string().contains("123456ABCDEF"));
    assert!(!fs::read_to_string(detail)?.contains("123456ABCDEF"));
    Ok(())
}

#[test]
fn reassembles_partials_and_separates_the_export_hypothesis() -> Result<()> {
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("probe.jsonl");
    fs::write(&input, synthetic_capture()?)?;
    let report = decoder::run_in(&input, &work.join("detail.jsonl"), &work)?;

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
    let report = decoder::run_in(&input, &work.join("detail.jsonl"), &work)?;

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
    assert!(decoder::run(&input, &root.path().join("detail.jsonl")).is_err());
    // A `work` ancestor elsewhere does not count: only the given work root does.
    let repo = root.path().join("work/repo");
    fs::create_dir_all(repo.join("docs"))?;
    fs::create_dir_all(repo.join("work"))?;
    let outside = repo.join("docs/detail.jsonl");
    assert!(decoder::run_in(&input, &outside, &repo.join("work")).is_err());
    assert!(decoder::run_in(&input, &repo.join("work/detail.jsonl"), &repo.join("work")).is_ok());
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
    let ordinary_report = decoder::run_in(&input, &ordinary, &work)?;
    assert!(!fs::read_to_string(ordinary)?.contains("\"object_guid\""));

    let detail = work.join("values.jsonl");
    let report = decoder::run_recording(
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

#[test]
fn inbound_metadata_keeps_unknown_targets_opaque_and_bounds_known_fields() -> Result<()> {
    let mut writer = BitWriter::new();
    writer.write_bit(true);
    let malformed = Payload::from_writer(writer);
    assert_eq!(
        decoder::inbound_field_metadata(&malformed, None, 8, 1)?["framing_disposition"],
        "unknown_target_opaque"
    );
    assert!(decoder::inbound_field_metadata(&malformed, Some(183), 8, 1).is_err());
    assert!(decoder::inbound_field_metadata(&malformed, None, 0, 1).is_err());
    assert!(decoder::inbound_field_metadata(&malformed, Some(0), 8, 1).is_err());
    Ok(())
}

fn inbound_config() -> decoder::InboundConfig {
    decoder::InboundConfig {
        controller_channel: 3,
        engine_net_version: 17,
        byte_order: decoder::InboundByteOrder::Little,
        backend_proxy_guid: 10,
        player_data_guid: 12,
        bestiary_guid: 14,
        limits: decoder::InboundLimits::default(),
    }
}

fn inbound_datagram(sequence: u16, bunches: Vec<Bunch>) -> Result<String> {
    let mut config = config();
    config.receiver = Receiver::Server;
    // A client-bound decoder would consume a nonexistent frame-time byte and
    // misframe this packet. Exercise Receiver::Server rather than just its CLI.
    let packet = Packet {
        magic: 0,
        header: PacketHeader {
            sequence,
            acked_sequence: 0,
            history: vec![0],
        },
        info: Some(icarus_bevy::experimental::packet::PacketInfo {
            jitter_clock: 7,
            has_frame_time: true,
            server_frame_time: None,
        }),
        bunches,
    };
    Ok(write_packet(&packet, &config)?
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn inbound_row(ms: u64, sequence: u16, bunches: Vec<Bunch>) -> Result<Value> {
    Ok(
        json!({"event":"datagram", "direction":"client_to_server", "ms":ms,
        "hex":inbound_datagram(sequence, bunches)?}),
    )
}

fn field_payload(index: u32, bound: u32) -> Result<Payload> {
    use icarus_bevy::experimental::rep_rpc::{FieldHeader, write_field_header};
    let mut secret = BitWriter::new();
    secret.write_bytes(b"SYNTHETIC_PLAYER_ID");
    let mut writer = BitWriter::new();
    write_field_header(
        &mut writer,
        &FieldHeader {
            index,
            payload: Payload::from_writer(secret),
        },
        bound,
    )?;
    Ok(Payload::from_writer(writer))
}

fn inbound_block(guid: Option<u32>, payload: Payload) -> ContentBlock {
    ContentBlock {
        has_rep_layout: false,
        target: guid.map_or(BlockTarget::Actor, |guid| BlockTarget::SubObject {
            object: NetGuidRef::plain(guid),
            class: SubObjectClass::ClientSent,
        }),
        payload,
    }
}

fn inbound_bunch(raw: Option<u32>, blocks: Vec<ContentBlock>) -> Result<Bunch> {
    Ok(Bunch {
        header: header(raw),
        payload: encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks,
            },
            &codec(),
        )?,
    })
}

fn inbound_actor(raw: Option<u32>) -> Result<Bunch> {
    inbound_bunch(raw, vec![inbound_block(None, field_payload(124, 183)?)])
}

fn json_lines(rows: &[Value]) -> String {
    rows.iter().map(|row| format!("{row}\n")).collect()
}

fn run_inbound_rows(
    rows: &[Value],
    config: &decoder::InboundConfig,
) -> Result<(Value, Vec<Value>)> {
    use icarus_bevy::experimental::release::ReleaseProfile;
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("authored.jsonl");
    let output = work.join("metadata.jsonl");
    fs::write(&input, json_lines(rows))?;
    let report = decoder::run_inbound_metadata(
        &input,
        &output,
        &work,
        &ReleaseProfile::release_3_0_32(),
        config,
    )?;
    let rows = fs::read_to_string(output)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<Vec<Value>, _>>()?;
    Ok((report, rows))
}

#[test]
fn inbound_controller_and_bound_components_emit_only_framing_metadata() -> Result<()> {
    use icarus_bevy::experimental::rep_rpc::{
        BACKEND_PROXY_FIELD_INDEX_BOUND, BESTIARY_MANAGER_FIELD_INDEX_BOUND,
        PLAYER_DATA_FIELD_INDEX_BOUND,
    };
    let mut unknown = BitWriter::new();
    unknown.write_bit(true); // Invalid field stream, but an unknown target is opaque.
    let bunch = inbound_bunch(
        Some(100),
        vec![
            inbound_block(None, field_payload(182, 183)?),
            inbound_block(Some(16), Payload::from_writer(unknown)),
            inbound_block(
                Some(10),
                field_payload(148, BACKEND_PROXY_FIELD_INDEX_BOUND)?,
            ),
            inbound_block(Some(12), field_payload(26, PLAYER_DATA_FIELD_INDEX_BOUND)?),
            inbound_block(
                Some(14),
                field_payload(7, BESTIARY_MANAGER_FIELD_INDEX_BOUND)?,
            ),
        ],
    )?;
    let (report, rows) = run_inbound_rows(&[inbound_row(9, 10, vec![bunch])?], &inbound_config())?;
    assert_eq!(report["mode"], "inbound_controller_metadata");
    assert_eq!(report["stats"]["bunches_dispatched"], 1);
    assert_eq!(rows.len(), 5);
    for (row, (target, index)) in rows.iter().zip([
        ("BP_IcarusPlayerControllerSurvival_C", Some(182)),
        ("unknown_component", None),
        ("UBackendProxyComponent", Some(148)),
        ("UPlayerDataComponent", Some(26)),
        ("UBestiaryManagerComponent", Some(7)),
    ]) {
        let object = row.as_object().unwrap();
        assert_eq!(object.len(), 6);
        assert!(object.keys().all(|key| {
            [
                "ms",
                "channel",
                "target_class",
                "field_index",
                "bit_length",
                "framing_disposition",
            ]
            .contains(&key.as_str())
        }));
        assert_eq!(row["ms"], 9);
        assert_eq!(row["channel"], 3);
        assert_eq!(row["target_class"], target);
        assert_eq!(row["field_index"], json!(index));
        assert_eq!(
            row["framing_disposition"],
            if index.is_some() {
                "framed"
            } else {
                "unknown_target_opaque"
            }
        );
        assert_eq!(
            row["bit_length"],
            if index.is_some() {
                b"SYNTHETIC_PLAYER_ID".len() * 8
            } else {
                1
            }
        );
    }
    assert!(!json!(rows).to_string().contains("SYNTHETIC_PLAYER_ID"));
    assert!(!report.to_string().contains("SYNTHETIC_PLAYER_ID"));
    Ok(())
}

#[test]
fn inbound_malformed_fields_do_not_publish_prefix_or_contaminate_siblings() -> Result<()> {
    use icarus_bevy::experimental::rep_rpc::{FieldHeader, write_field_header};
    let mut malformed = BitWriter::new();
    write_field_header(
        &mut malformed,
        &FieldHeader {
            index: 124,
            payload: Payload::from_writer(BitWriter::new()),
        },
        183,
    )?;
    malformed.write_bit(true); // Truncated second field header.
    let mut rep_layout = inbound_block(None, field_payload(124, 183)?);
    rep_layout.has_rep_layout = true;
    let bunch = inbound_bunch(
        None,
        vec![
            inbound_block(None, Payload::from_writer(malformed)),
            rep_layout,
            inbound_block(Some(12), field_payload(24, 26)?),
        ],
    )?;
    let mut bad_body = BitWriter::new();
    bad_body.write_bit(true); // Truncated content block header.
    let malformed_body = Bunch {
        header: header(None),
        payload: Payload::from_writer(bad_body),
    };
    let (report, rows) = run_inbound_rows(
        &[
            inbound_row(1, 10, vec![bunch])?,
            inbound_row(2, 11, vec![malformed_body])?,
            json!({"event":"datagram", "direction":"client_to_server", "ms":3, "hex":"00"}),
            inbound_row(4, 12, vec![inbound_actor(None)?])?,
        ],
        &inbound_config(),
    )?;
    assert_eq!(
        rows.iter()
            .map(|row| row["framing_disposition"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "fields_rejected",
            "unsupported_rep_layout",
            "framed",
            "actor_body_rejected",
            "malformed_packet",
            "framed"
        ]
    );
    assert_eq!(report["stats"]["packets_rejected"], 1);
    assert_eq!(rows[2]["field_index"], 24);
    assert_eq!(rows[5]["field_index"], 124);
    Ok(())
}

#[test]
fn inbound_reliable_ordering_drains_bounded_pending_and_drops_duplicates() -> Result<()> {
    let bunch = inbound_actor(Some(100))?;
    let (report, rows) = run_inbound_rows(
        &[
            inbound_row(1, 10, vec![bunch.clone()])?,
            inbound_row(2, 11, vec![inbound_actor(Some(102))?])?,
            inbound_row(3, 12, vec![inbound_actor(Some(102))?])?,
            inbound_row(4, 13, vec![inbound_actor(Some(101))?])?,
            inbound_row(5, 14, vec![bunch])?,
            inbound_row(6, 14, vec![inbound_actor(Some(103))?])?,
        ],
        &inbound_config(),
    )?;
    assert_eq!(report["stats"]["reliable_delivered"], 3);
    assert_eq!(report["stats"]["reliable_queued"], 1);
    assert_eq!(report["stats"]["reliable_duplicates"], 2);
    assert_eq!(report["stats"]["packets_stale_or_duplicate"], 1);
    assert_eq!(report["stats"]["reliable_pending_at_end"], 0);
    assert_eq!(report["stats"]["reliable_pending_bytes_at_end"], 0);
    assert_eq!(
        rows.iter()
            .map(|row| row["ms"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [1, 4, 2]
    );
    Ok(())
}

fn partial_actor() -> Result<(Bunch, Bunch)> {
    let bunch = inbound_actor(Some(100))?;
    let mut reader = bunch.payload.reader();
    let mut first = BitWriter::new();
    first.write_bits(reader.read_bits(8), 8);
    let mut last = BitWriter::new();
    while reader.remaining() > 0 {
        let bits = reader.remaining().min(32) as u32;
        last.write_bits(reader.read_bits(bits), bits);
    }
    Ok((
        Bunch {
            header: BunchHeader {
                partial: true,
                partial_initial: true,
                ..header(Some(100))
            },
            payload: Payload::from_writer(first),
        },
        Bunch {
            header: BunchHeader {
                partial: true,
                partial_final: true,
                ..header(Some(101))
            },
            payload: Payload::from_writer(last),
        },
    ))
}

#[test]
fn inbound_only_dispatches_successful_bounded_partial_completion() -> Result<()> {
    let (first, last) = partial_actor()?;
    let (report, rows) = run_inbound_rows(
        &[
            inbound_row(1, 10, vec![first])?,
            inbound_row(2, 11, vec![last.clone()])?,
            inbound_row(3, 12, vec![last])?,
        ],
        &inbound_config(),
    )?;
    assert_eq!(report["stats"]["partial_completed"], 1);
    assert_eq!(report["stats"]["bunches_dispatched"], 1);
    assert_eq!(report["stats"]["reliable_duplicates"], 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["field_index"], 124);
    assert_eq!(rows[0]["ms"], 2);
    Ok(())
}

#[test]
fn inbound_partial_errors_discard_assembly_before_any_dispatch() -> Result<()> {
    let (first, last) = partial_actor()?;
    let mut non_aligned = first.clone();
    let mut bit = BitWriter::new();
    bit.write_bit(true);
    non_aligned.payload = Payload::from_writer(bit);
    let mut replacement_initial = first.clone();
    replacement_initial.header.reliable_sequence_raw = Some(101);
    let mut final_after_error = last.clone();
    final_after_error.header.reliable_sequence_raw = Some(102);
    for rows in [
        vec![
            inbound_row(1, 10, vec![non_aligned])?,
            inbound_row(2, 11, vec![last.clone()])?,
        ],
        vec![
            inbound_row(1, 10, vec![first.clone()])?,
            inbound_row(2, 11, vec![replacement_initial])?,
            inbound_row(3, 12, vec![final_after_error])?,
        ],
    ] {
        let (report, output) = run_inbound_rows(&rows, &inbound_config())?;
        assert_eq!(report["stats"]["bunches_dispatched"], 0);
        assert_eq!(report["stats"]["partial_completed"], 0);
        assert_eq!(report["stats"]["partial_in_progress_at_end"], false);
        assert!(
            output
                .iter()
                .all(|row| row["framing_disposition"] == "partial_error")
        );
    }
    // Unreliable continuation cannot join a reliable initial.
    let mut unreliable_last = last;
    unreliable_last.header = BunchHeader {
        partial: true,
        partial_final: true,
        ..header(None)
    };
    let (report, output) = run_inbound_rows(
        &[
            inbound_row(1, 10, vec![first])?,
            inbound_row(2, 11, vec![unreliable_last])?,
        ],
        &inbound_config(),
    )?;
    assert_eq!(report["stats"]["bunches_dispatched"], 0);
    assert_eq!(output[0]["framing_disposition"], "partial_error");
    Ok(())
}

#[test]
fn inbound_partial_bytes_and_zero_data_part_counts_are_bounded() -> Result<()> {
    let (first, last) = partial_actor()?;
    let mut config = inbound_config();
    config.limits.max_partial_bytes = 1;
    let (report, rows) = run_inbound_rows(
        &[
            inbound_row(1, 10, vec![first.clone()])?,
            inbound_row(2, 11, vec![last.clone()])?,
        ],
        &config,
    )?;
    assert_eq!(report["stats"]["partial_completed"], 0);
    assert_eq!(report["stats"]["partial_in_progress_at_end"], false);
    assert_eq!(rows[0]["framing_disposition"], "partial_error");

    let middle = Bunch {
        header: BunchHeader {
            partial: true,
            ..header(Some(101))
        },
        payload: Payload::from_writer(BitWriter::new()),
    };
    let mut final_part = last;
    final_part.header.reliable_sequence_raw = Some(103);
    let mut middle2 = middle.clone();
    middle2.header.reliable_sequence_raw = Some(102);
    let mut config = inbound_config();
    config.limits.max_partial_parts = 2;
    let (report, rows) = run_inbound_rows(
        &[
            inbound_row(1, 10, vec![first])?,
            inbound_row(2, 11, vec![middle])?,
            inbound_row(3, 12, vec![middle2])?,
            inbound_row(4, 13, vec![final_part])?,
        ],
        &config,
    )?;
    assert_eq!(report["stats"]["partial_completed"], 0);
    assert_eq!(report["stats"]["bunches_dispatched"], 0);
    assert_eq!(rows[0]["framing_disposition"], "partial_part_limit");
    assert_eq!(rows[1]["framing_disposition"], "partial_error");
    Ok(())
}

#[test]
fn inbound_rejects_exports_must_map_and_client_opens_without_guessing() -> Result<()> {
    let (first, last) = partial_actor()?;
    for flag in 0..3 {
        let mut rejected = inbound_actor(Some(101))?;
        match flag {
            0 => rejected.header.has_package_map_exports = true,
            1 => rejected.header.has_must_be_mapped_guids = true,
            _ => {
                rejected.header.control = true;
                rejected.header.open = true;
            }
        }
        let mut last = last.clone();
        last.header.reliable_sequence_raw = Some(102);
        let (report, rows) = run_inbound_rows(
            &[
                inbound_row(1, 10, vec![first.clone()])?,
                inbound_row(2, 11, vec![rejected])?,
                inbound_row(3, 12, vec![last])?,
            ],
            &inbound_config(),
        )?;
        assert_eq!(report["stats"]["bunches_dispatched"], 0);
        assert_eq!(
            rows[0]["framing_disposition"],
            [
                "unsupported_exports",
                "unsupported_must_map",
                "unsupported_client_open"
            ][flag]
        );
        assert_eq!(rows[1]["framing_disposition"], "partial_error");
    }
    Ok(())
}

#[test]
fn inbound_pending_count_and_byte_limits_apply_before_enqueue() -> Result<()> {
    for limit_bytes in [false, true] {
        let mut config = inbound_config();
        if limit_bytes {
            config.limits.max_pending_reliable_bytes =
                inbound_actor(Some(102))?.payload.bit_len().div_ceil(8);
        } else {
            config.limits.max_pending_reliable = 1;
        }
        let (report, rows) = run_inbound_rows(
            &[
                inbound_row(1, 10, vec![inbound_actor(Some(100))?])?,
                inbound_row(2, 11, vec![inbound_actor(Some(102))?])?,
                inbound_row(3, 12, vec![inbound_actor(Some(103))?])?,
                inbound_row(4, 13, vec![inbound_actor(Some(102))?])?,
            ],
            &config,
        )?;
        assert_eq!(report["stats"]["reliable_queued"], 1);
        assert_eq!(report["stats"]["reliable_rejected"], 1);
        assert_eq!(report["stats"]["reliable_duplicates"], 1);
        assert_eq!(report["stats"]["reliable_pending_at_end"], 1);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["framing_disposition"], "reliable_pending_limit");
    }
    Ok(())
}

#[test]
fn inbound_stream_contract_rejects_missing_invalid_and_ambiguous_values() -> Result<()> {
    let valid = inbound_row(1, 10, vec![inbound_actor(None)?])?;
    for (key, bad) in [
        ("event", Value::Null),
        ("direction", Value::Null),
        ("direction", json!("inbound")),
        ("ms", Value::Null),
        ("ms", json!(-1)),
        ("ms", json!(1.5)),
        ("hex", json!("0")),
        ("hex", json!("zz")),
        ("hex", json!("")),
        ("hex", json!("éé")),
    ] {
        let mut row = valid.clone();
        row[key] = bad;
        assert!(
            run_inbound_rows(&[row], &inbound_config()).is_err(),
            "accepted invalid {key}"
        );
    }
    assert!(
        run_inbound_rows(
            &[inbound_row(2, 10, vec![])?, inbound_row(1, 11, vec![])?],
            &inbound_config()
        )
        .is_err()
    );
    let mut opposite = valid.clone();
    opposite["direction"] = json!("server_to_client");
    let (report, rows) = run_inbound_rows(
        &[json!({"event":"reply", "hex":"00"}), opposite, valid],
        &inbound_config(),
    )?;
    assert_eq!(report["stats"]["datagrams"], 1);
    assert_eq!(report["stats"]["skipped_rows"], 2);
    assert_eq!(rows.len(), 1);
    Ok(())
}

#[test]
fn inbound_enforces_file_line_datagram_and_output_bounds() -> Result<()> {
    use icarus_bevy::experimental::release::ReleaseProfile;
    let rows = [
        inbound_row(1, 10, vec![inbound_actor(None)?])?,
        inbound_row(2, 11, vec![inbound_actor(None)?])?,
    ];
    let text = json_lines(&rows);
    for bound in 0..8 {
        let root = tempfile::tempdir()?;
        let work = root.path().join("work");
        fs::create_dir(&work)?;
        let input = work.join("authored.jsonl");
        let output = work.join("metadata.jsonl");
        fs::write(&input, &text)?;
        let mut config = inbound_config();
        match bound {
            0 => config.limits.max_file_bytes = text.len() - 1,
            1 => config.limits.max_line_bytes = rows[0].to_string().len(),
            2 => config.limits.max_lines = 1,
            3 => config.limits.max_datagrams = 1,
            4 => config.limits.max_datagram_bytes = rows[0]["hex"].as_str().unwrap().len() / 2 - 1,
            5 => {
                config.limits.max_total_datagram_bytes = rows[0]["hex"].as_str().unwrap().len() / 2
            }
            6 => config.limits.max_output_rows = 1,
            _ => config.limits.max_output_bytes = 1,
        }
        assert!(
            decoder::run_inbound_metadata(
                &input,
                &output,
                &work,
                &ReleaseProfile::release_3_0_32(),
                &config
            )
            .is_err(),
            "bound {bound}"
        );
        if bound == 0 {
            assert!(!output.exists());
        }
        if let Ok(contents) = fs::read_to_string(output) {
            assert!(contents.len() <= config.limits.max_output_bytes);
            assert!(contents.lines().count() <= config.limits.max_output_rows);
            assert!(
                contents
                    .lines()
                    .all(|row| serde_json::from_str::<Value>(row).is_ok())
            );
        }
    }
    // Field and content-block limits reject before publishing a success prefix.
    let payload = field_payload(124, 183)?;
    let mut two = BitWriter::new();
    for _ in 0..2 {
        let mut reader = payload.reader();
        while reader.remaining() > 0 {
            let bits = reader.remaining().min(32) as u32;
            two.write_bits(reader.read_bits(bits), bits);
        }
    }
    let mut config = inbound_config();
    config.limits.max_fields = 1;
    let (_, rows) = run_inbound_rows(
        &[inbound_row(
            1,
            10,
            vec![inbound_bunch(
                None,
                vec![inbound_block(None, Payload::from_writer(two))],
            )?],
        )?],
        &config,
    )?;
    assert_eq!(rows[0]["framing_disposition"], "fields_rejected");
    config.limits.max_blocks = 1;
    let (_, rows) = run_inbound_rows(
        &[inbound_row(
            1,
            10,
            vec![inbound_bunch(
                None,
                vec![
                    inbound_block(None, payload.clone()),
                    inbound_block(None, payload),
                ],
            )?],
        )?],
        &config,
    )?;
    assert_eq!(rows[0]["framing_disposition"], "actor_body_rejected");
    Ok(())
}

fn inbound_cli_args(
    input: &std::path::Path,
    output: &std::path::Path,
    profile: &std::path::Path,
) -> Vec<String> {
    vec![
        "capture_decode".into(),
        input.display().to_string(),
        output.display().to_string(),
        "--inbound-controller-metadata".into(),
        "--release-profile".into(),
        profile.display().to_string(),
        "--controller-channel".into(),
        "3".into(),
        "--engine-net-version".into(),
        "17".into(),
        "--byte-order".into(),
        "little".into(),
        "--backend-proxy-guid".into(),
        "10".into(),
        "--player-data-guid".into(),
        "12".into(),
        "--bestiary-guid".into(),
        "14".into(),
    ]
}

#[test]
fn inbound_cli_validates_all_bindings_before_exclusive_output_creation() -> Result<()> {
    use icarus_bevy::experimental::release::ReleaseProfile;
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("authored.jsonl");
    let output = work.join("metadata.jsonl");
    let profile_path = work.join("release.toml");
    let profile = ReleaseProfile::release_3_0_32();
    fs::write(
        &profile_path,
        format!(
            "[release]\napp_id={}\nbuild_id={}\nproject_version={:?}\nnetwork_checksum={}\n",
            profile.pin.app_id,
            profile.pin.build_id,
            profile.pin.project_version,
            profile.pin.network_checksum
        ),
    )?;
    fs::write(
        &input,
        json_lines(&[inbound_row(1, 10, vec![inbound_actor(None)?])?]),
    )?;
    let valid = inbound_cli_args(&input, &output, &profile_path);
    for flag in [
        "--release-profile",
        "--controller-channel",
        "--engine-net-version",
        "--byte-order",
        "--backend-proxy-guid",
        "--player-data-guid",
        "--bestiary-guid",
    ] {
        let mut args = valid.clone();
        let index = args.iter().position(|arg| arg == flag).unwrap();
        args.drain(index..index + 2);
        assert!(decoder::run_cli_in(args, &work).is_err(), "missing {flag}");
        assert!(!output.exists());
    }
    for (flag, value) in [
        ("--controller-channel", "0"),
        ("--controller-channel", "32767"),
        ("--engine-net-version", "16"),
        ("--byte-order", "native"),
        ("--backend-proxy-guid", "0"),
        ("--backend-proxy-guid", "1"),
        ("--player-data-guid", "10"),
        ("--bestiary-guid", "12"),
    ] {
        let mut args = valid.clone();
        let index = args.iter().position(|arg| arg == flag).unwrap();
        args[index + 1] = value.into();
        assert!(decoder::run_cli_in(args, &work).is_err(), "invalid {flag}");
        assert!(!output.exists());
    }
    let mut with_values = valid.clone();
    with_values.extend(["--values-channel".into(), "3".into()]);
    assert!(decoder::run_cli_in(with_values, &work).is_err());
    assert!(!output.exists());
    let mut no_mode = valid.clone();
    no_mode.retain(|arg| arg != "--inbound-controller-metadata");
    assert!(decoder::run_cli_in(no_mode, &work).is_err());
    assert!(!output.exists());

    let report = decoder::run_cli_in(valid.clone(), &work)?;
    assert_eq!(report["stats"]["output_rows"], 1);
    let original = fs::read(&output)?;
    assert!(decoder::run_cli_in(valid, &work).is_err());
    assert_eq!(fs::read(&output)?, original);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(fs::metadata(&output)?.permissions().mode() & 0o777, 0o600);
    Ok(())
}

#[test]
fn inbound_unknown_release_and_invalid_limits_fail_before_output() -> Result<()> {
    use icarus_bevy::experimental::release::ReleaseProfile;
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("empty.jsonl");
    let output = work.join("metadata.jsonl");
    fs::write(&input, "")?;
    let mut profile = ReleaseProfile::release_3_0_32();
    profile.pin.build_id += 1;
    assert!(
        decoder::run_inbound_metadata(&input, &output, &work, &profile, &inbound_config()).is_err()
    );
    assert!(!output.exists());
    let mut config = inbound_config();
    config.limits.max_partial_bytes += 1;
    assert!(
        decoder::run_inbound_metadata(
            &input,
            &output,
            &work,
            &ReleaseProfile::release_3_0_32(),
            &config
        )
        .is_err()
    );
    assert!(!output.exists());
    config = inbound_config();
    config.limits.max_pending_reliable = 0;
    assert!(
        decoder::run_inbound_metadata(
            &input,
            &output,
            &work,
            &ReleaseProfile::release_3_0_32(),
            &config
        )
        .is_err()
    );
    assert!(!output.exists());
    Ok(())
}

#[test]
fn capture_cli_defaults_still_run_both_server_to_client_passes() -> Result<()> {
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("authored.jsonl");
    let cli_output = work.join("cli.jsonl");
    let ordinary_output = work.join("ordinary.jsonl");
    fs::write(&input, synthetic_capture()?)?;
    let report = decoder::run_cli_in(
        [
            "capture_decode".to_owned(),
            input.display().to_string(),
            cli_output.display().to_string(),
        ],
        &work,
    )?;
    let expected = decoder::run_in(&input, &ordinary_output, &work)?;
    assert_eq!(report, expected);
    assert_eq!(fs::read(cli_output)?, fs::read(ordinary_output)?);
    assert!(report["passes"]["strict"].is_object());
    assert!(report["passes"]["export_hypothesis"].is_object());
    Ok(())
}

#[test]
fn inbound_stream_rejects_truncated_json_and_duplicate_contract_keys() -> Result<()> {
    use icarus_bevy::experimental::release::ReleaseProfile;
    for text in [
        "{",
        "[]\n",
        "[\"datagram\",\"client_to_server\",1,\"00\"]\n",
        "{\"event\":\"datagram\",\"direction\":\"server_to_client\",\"direction\":\"client_to_server\",\"ms\":1,\"hex\":\"00\"}\n",
        "{\"event\":\"datagram\",\"event\":\"reply\"}\n",
    ] {
        let root = tempfile::tempdir()?;
        let work = root.path().join("work");
        fs::create_dir(&work)?;
        let input = work.join("authored.jsonl");
        let output = work.join("metadata.jsonl");
        fs::write(&input, text)?;
        assert!(
            decoder::run_inbound_metadata(
                &input,
                &output,
                &work,
                &ReleaseProfile::release_3_0_32(),
                &inbound_config()
            )
            .is_err()
        );
        assert!(fs::read(output)?.is_empty());
    }
    Ok(())
}

#[test]
fn inbound_output_confines_paths_and_preserves_existing_inputs_and_symlinks() -> Result<()> {
    use icarus_bevy::experimental::release::ReleaseProfile;
    let root = tempfile::tempdir()?;
    let work = root.path().join("work");
    fs::create_dir(&work)?;
    let input = work.join("authored.jsonl");
    let sentinel = b"preserved input";
    fs::write(&input, sentinel)?;
    let outside = root.path().join("outside.jsonl");
    assert!(
        decoder::run_inbound_metadata(
            &input,
            &outside,
            &work,
            &ReleaseProfile::release_3_0_32(),
            &inbound_config()
        )
        .is_err()
    );
    assert!(!outside.exists());
    assert!(
        decoder::run_inbound_metadata(
            &input,
            &input,
            &work,
            &ReleaseProfile::release_3_0_32(),
            &inbound_config()
        )
        .is_err()
    );
    assert_eq!(fs::read(&input)?, sentinel);
    let output = work.join("metadata.jsonl");
    std::os::unix::fs::symlink(&input, &output)?;
    assert!(
        decoder::run_inbound_metadata(
            &input,
            &output,
            &work,
            &ReleaseProfile::release_3_0_32(),
            &inbound_config()
        )
        .is_err()
    );
    assert_eq!(fs::read(&input)?, sentinel);
    assert!(fs::symlink_metadata(output)?.file_type().is_symlink());
    Ok(())
}

#[test]
fn inbound_qualified_profiles_and_explicit_byte_orders_share_only_framing() -> Result<()> {
    use icarus_bevy::experimental::release::ReleaseProfile;
    for profile in [
        ReleaseProfile::release_3_0_32(),
        ReleaseProfile::release_3_0_34(),
    ] {
        for order in [
            decoder::InboundByteOrder::Little,
            decoder::InboundByteOrder::Big,
        ] {
            let root = tempfile::tempdir()?;
            let work = root.path().join("work");
            fs::create_dir(&work)?;
            let input = work.join("authored.jsonl");
            let output = work.join("metadata.jsonl");
            fs::write(
                &input,
                json_lines(&[inbound_row(1, 10, vec![inbound_actor(None)?])?]),
            )?;
            let mut config = inbound_config();
            config.byte_order = order;
            let report = decoder::run_inbound_metadata(&input, &output, &work, &profile, &config)?;
            assert_eq!(report["stats"]["output_rows"], 1);
        }
    }
    Ok(())
}

#[test]
fn inbound_inline_exports_are_explicitly_rejected_and_other_channels_are_isolated() -> Result<()> {
    use icarus_bevy::experimental::actor_channel::GuidExport;
    let exported = ContentBlock {
        has_rep_layout: false,
        target: BlockTarget::SubObject {
            object: NetGuidRef {
                guid: 1,
                export: Some(GuidExport {
                    no_load: false,
                    path: None,
                    checksum: None,
                    bare_checksum_flag: false,
                }),
            },
            class: SubObjectClass::ClientSent,
        },
        payload: Payload::from_writer(BitWriter::new()),
    };
    let with_export = inbound_bunch(
        None,
        vec![inbound_block(None, field_payload(124, 183)?), exported],
    )?;
    let mut other_channel = inbound_actor(None)?;
    other_channel.header.channel_index = 4;
    other_channel.header.has_package_map_exports = true;
    let (report, rows) = run_inbound_rows(
        &[
            inbound_row(1, 10, vec![with_export])?,
            inbound_row(2, 11, vec![other_channel, inbound_actor(None)?])?,
        ],
        &inbound_config(),
    )?;
    assert_eq!(report["stats"]["other_channel_bunches"], 1);
    assert_eq!(report["stats"]["bunches_dispatched"], 1);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["framing_disposition"], "unsupported_exports");
    assert_eq!(rows[1]["framing_disposition"], "framed");
    Ok(())
}

#[test]
fn inbound_handlers_preserve_partial_and_queued_reliable_state_without_values() -> Result<()> {
    use icarus_bevy::experimental::{
        handshake::initial_request,
        steam_auth::{SteamAuthMessage, encode_datagram},
    };
    for (byte_order, archive_order) in [
        (decoder::InboundByteOrder::Little, ByteOrder::Little),
        (decoder::InboundByteOrder::Big, ByteOrder::Big),
    ] {
        let mut config = inbound_config();
        config.byte_order = byte_order;
        let (first, last) = partial_actor()?;
        // Queue sequence 102 while the partial waits for sequence 101. Handler
        // frames must neither clear that partial nor consume packet sequences.
        let queued = inbound_actor(Some(102))?;
        let connection = [
            inbound_row(1, 10, vec![first])?,
            inbound_row(3, 11, vec![queued])?,
            inbound_row(5, 12, vec![last])?,
        ];
        let handlers = [
            initial_request().to_vec(),
            encode_datagram(
                &SteamAuthMessage::UserData {
                    ticket: "SYNTHETIC_AUTH_SECRET".encode_utf16().collect(),
                    steam_id: 987654321012345678,
                },
                archive_order,
                0,
                0,
            )?,
        ];
        let mut interleaved = vec![connection[0].clone()];
        for (index, frame) in handlers.into_iter().enumerate() {
            let hex: String = frame.iter().map(|byte| format!("{byte:02x}")).collect();
            interleaved.push(json!({"event":"datagram", "direction":"client_to_server",
                "ms":2 + 2 * index, "hex":hex}));
            interleaved.push(connection[index + 1].clone());
        }
        let (baseline, expected_rows) = run_inbound_rows(&connection, &config)?;
        let (report, rows) = run_inbound_rows(&interleaved, &config)?;
        assert_eq!(rows, expected_rows);
        assert_eq!(report["stats"]["stateless_handshakes"], 1);
        assert_eq!(report["stats"]["steam_auth_frames"], 1);
        assert_eq!(report["stats"]["packets_rejected"], 0);
        assert_eq!(report["stats"]["partial_completed"], 1);
        assert_eq!(report["stats"]["reliable_queued"], 1);
        for key in [
            "packets_decoded",
            "packets_stale_or_duplicate",
            "reliable_delivered",
            "reliable_pending_at_end",
            "partial_in_progress_at_end",
            "bunches_dispatched",
        ] {
            assert_eq!(report["stats"][key], baseline["stats"][key], "{key}");
        }
        let serialized = format!("{report}{rows:?}");
        for secret in [
            "SYNTHETIC_AUTH_SECRET",
            "987654321012345678",
            "steam_id",
            "ticket",
        ] {
            assert!(!serialized.contains(secret));
        }
    }
    Ok(())
}

#[test]
fn inbound_malformed_handler_prefixes_are_rejected_and_clear_partial() -> Result<()> {
    for hex in ["01", "02"] {
        let (first, last) = partial_actor()?;
        let (report, rows) = run_inbound_rows(
            &[
                inbound_row(1, 10, vec![first])?,
                json!({"event":"datagram", "direction":"client_to_server", "ms":2, "hex":hex}),
                inbound_row(3, 11, vec![last])?,
            ],
            &inbound_config(),
        )?;
        assert_eq!(report["stats"]["stateless_handshakes"], 0);
        assert_eq!(report["stats"]["steam_auth_frames"], 0);
        assert_eq!(report["stats"]["packets_rejected"], 1);
        assert_eq!(report["stats"]["partial_completed"], 0);
        assert_eq!(rows[0]["framing_disposition"], "malformed_packet");
        assert!(rows[0]["channel"].is_null());
        assert_eq!(rows[1]["framing_disposition"], "partial_error");
    }
    Ok(())
}

#[test]
fn inbound_reliable_partial_survives_unreliable_whole_bunch() -> Result<()> {
    let (first, last) = partial_actor()?;
    let (report, rows) = run_inbound_rows(
        &[
            inbound_row(1, 10, vec![first])?,
            inbound_row(2, 11, vec![inbound_actor(None)?])?,
            inbound_row(3, 12, vec![last])?,
        ],
        &inbound_config(),
    )?;
    assert_eq!(report["stats"]["partial_completed"], 1);
    assert_eq!(report["stats"]["partial_rejected"], 0);
    assert_eq!(report["stats"]["bunches_dispatched"], 2);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row["field_index"] == 124));
    assert_eq!(rows[0]["ms"], 2);
    assert_eq!(rows[1]["ms"], 3);
    Ok(())
}

#[test]
fn inbound_rejects_tampered_3_0_34_before_creating_output() -> Result<()> {
    use icarus_bevy::experimental::release::ReleaseProfile;
    let root = tempfile::tempdir()?;
    let input = root.path().join("input.jsonl");
    let output = root.path().join("output.jsonl");
    fs::write(&input, "")?;
    let mut tampered_pin = ReleaseProfile::release_3_0_34();
    tampered_pin.pin.network_checksum ^= 1;
    let mut tampered_bound = ReleaseProfile::release_3_0_34();
    tampered_bound
        .class_net_cache
        .bp_icarus_player_controller_survival += 1;
    for profile in [tampered_pin, tampered_bound] {
        assert!(
            decoder::run_inbound_metadata(
                &input,
                &output,
                root.path(),
                &profile,
                &inbound_config(),
            )
            .is_err()
        );
        assert!(!output.exists());
    }
    Ok(())
}

#[test]
fn inbound_whole_bunch_limit_has_separate_counter() -> Result<()> {
    let mut config = inbound_config();
    config.limits.max_partial_bytes = 1;
    let (report, rows) =
        run_inbound_rows(&[inbound_row(1, 10, vec![inbound_actor(None)?])?], &config)?;
    assert_eq!(report["stats"]["whole_bunches_rejected"], 1);
    assert_eq!(report["stats"]["partial_rejected"], 0);
    assert_eq!(rows[0]["framing_disposition"], "bunch_byte_limit");
    Ok(())
}
