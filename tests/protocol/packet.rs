use icarus_bevy::experimental::handshake::initial_request;
use icarus_bevy::experimental::packet::{
    BitReader, BitWriter, Bunch, BunchHeader, BunchLimits, HandshakePacket, Name, Packet,
    PacketConfig, PacketHeader, PacketInfo, Payload, Receiver, finish_with_termination, read_bunch,
    read_bunches, read_data_prefix, read_packet, read_packet_header, split_termination,
    write_bunch, write_data_prefix, write_packet, write_packet_header,
};

// Independently authored bit vectors; no captured client or server packets.

const LIMITS: BunchLimits = BunchLimits {
    max_channels: 16,
    max_packet_bytes: 64,
    max_string_units: 64,
};

fn reader(writer: &BitWriter) -> BitReader<'_> {
    BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap()
}

fn payload(bits: &[bool]) -> Payload {
    let mut writer = BitWriter::new();
    for bit in bits {
        writer.write_bit(*bit);
    }
    Payload::from_writer(writer)
}

fn pattern(length: usize) -> Vec<bool> {
    (0..length)
        .map(|index| (index * 7 + index / 3) % 5 < 2)
        .collect()
}

fn ascii(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn plain_header() -> BunchHeader {
    BunchHeader {
        control: false,
        open: false,
        close: false,
        close_reason: None,
        paused: false,
        reliable: false,
        channel_index: 0,
        has_package_map_exports: false,
        has_must_be_mapped_guids: false,
        partial: false,
        reliable_sequence_raw: None,
        partial_initial: false,
        partial_final: false,
        channel_name: None,
    }
}

#[test]
fn bits_are_read_and_written_lsb_first_from_any_position() {
    let bytes = [0b1010_1011, 0b0000_0001, 0x34, 0x12, 0x78, 0x56, 0x34, 0x12];
    let mut input = BitReader::new(&bytes, 64).unwrap();
    assert!(input.read_bit());
    // Bits 1..9: byte 0 shifted right, plus bit 0 of byte 1 as bit 7.
    assert_eq!(input.read_bits(8), 0xd5);
    assert_eq!(input.position(), 9);
    assert_eq!(input.read_bits(7), 0);
    assert_eq!(input.read_bits(16), 0x1234);
    assert_eq!(input.read_u32(), 0x1234_5678);
    assert!(input.at_end() && !input.is_error());

    let mut unaligned = BitReader::new(&[0b1000_0000, 0b0111_1111, 0xff], 24).unwrap();
    assert_eq!(unaligned.read_bits(7), 0);
    assert_eq!(unaligned.read_bytes(2), [0xff, 0xfe]);
    assert_eq!(unaligned.remaining(), 1);

    let mut writer = BitWriter::new();
    writer.write_bit(true);
    writer.write_bits(0b10, 2);
    assert_eq!(writer.as_bytes(), [0b101]);
    writer.write_bits(0, 5);
    writer.write_u32(0x1234_5678);
    writer.write_bit(true);
    writer.write_bytes(&[0xa5]);
    assert_eq!(writer.bit_len(), 49);
    assert_eq!(
        writer.as_bytes(),
        [0b101, 0x78, 0x56, 0x34, 0x12, 0x4b, 0x01]
    );
    assert!(BitReader::new(&[0], 9).is_err());
}

#[test]
fn overflow_is_sticky_consumes_nothing_and_returns_zero() {
    let bytes = [0xff];
    let mut input = BitReader::new(&bytes, 5).unwrap();
    assert_eq!(input.read_bits(8), 0);
    assert!(input.is_error());
    assert_eq!((input.position(), input.remaining()), (0, 5));
    // Bits remain, but the error state is sticky.
    assert!(!input.read_bit());
    assert_eq!(input.read_bits(1), 0);
    assert!(input.read_bytes(0).is_empty());
    assert_eq!(input.read_int_packed(), 0);
    assert_eq!(input.read_int_max(32), 0);
    assert_eq!(input.position(), 0);

    let mut wide = BitReader::new(&bytes, 8).unwrap();
    assert_eq!(wide.read_bits(33), 0);
    assert!(wide.is_error());
    let mut huge = BitReader::new(&bytes, 8).unwrap();
    assert!(huge.read_bytes(usize::MAX).is_empty());
    assert!(huge.is_error());
}

#[test]
fn int_max_round_trips_exhaustively_with_clamping() {
    for max in 0..=70u32 {
        let mut empty = BitReader::new(&[], 0).unwrap();
        assert_eq!(empty.read_int_max(max.min(1)), 0);
        for value in 0..=72u32 {
            let mut writer = BitWriter::new();
            if max == 0 {
                assert!(writer.write_int_max(value, max).is_err());
                continue;
            }
            writer.write_int_max(value, max).unwrap();
            let mut input = reader(&writer);
            assert_eq!(
                input.read_int_max(max),
                value.min(max - 1),
                "{value} < {max}"
            );
            assert!(input.at_end() && !input.is_error());
        }
    }
    // Max 0 and 1 read nothing even when bits are available.
    let mut input = BitReader::new(&[0xff], 8).unwrap();
    assert_eq!((input.read_int_max(0), input.read_int_max(1)), (0, 0));
    assert_eq!(input.position(), 0);
    for (value, max, expected) in [
        (1023, 1024, 1023),
        (1024, 1024, 1023),
        (0, 1024, 0),
        (1 << 31, (1 << 31) + 1, 1 << 31),
        ((1 << 31) - 1, 1 << 31, (1 << 31) - 1),
        (u32::MAX, 1 << 31, (1 << 31) - 1),
        (u32::MAX, u32::MAX, u32::MAX - 1),
        (u32::MAX - 1, u32::MAX, u32::MAX - 1),
        (0x8000_0001, u32::MAX, 0x8000_0001),
    ] {
        let mut writer = BitWriter::new();
        writer.write_int_max(value, max).unwrap();
        let mut input = reader(&writer);
        assert_eq!(input.read_int_max(max), expected);
        assert!(input.at_end() && !input.is_error());
    }
}

#[test]
fn int_wrapped_matches_int_max_in_range_and_keeps_low_bits_beyond_it() {
    for max in 1..=70u32 {
        for value in 0..max {
            let (mut clamped, mut wrapped) = (BitWriter::new(), BitWriter::new());
            clamped.write_int_max(value, max).unwrap();
            wrapped.write_int_wrapped(value, max);
            assert_eq!(clamped.as_bytes(), wrapped.as_bytes());
            assert_eq!(clamped.bit_len(), wrapped.bit_len());
        }
    }
    for log in 0..=31u32 {
        let max = 1u32 << log;
        for value in [max, max + 5, max.wrapping_mul(3) | 1, u32::MAX] {
            let mut writer = BitWriter::new();
            writer.write_int_wrapped(value, max);
            assert_eq!(writer.bit_len(), log as usize);
            assert_eq!(reader(&writer).read_int_max(max), value & (max - 1));
        }
    }
    let mut nothing = BitWriter::new();
    nothing.write_int_wrapped(u32::MAX, 0);
    nothing.write_int_wrapped(u32::MAX, 1);
    assert_eq!(nothing.bit_len(), 0);
    // Non-power-of-two maxima: the loop stops once written + mask reaches max,
    // so an out-of-range value decodes as its greedy low-bit prefix.
    for (value, max, bits, decoded) in [(20, 10, 3, 4), (15, 10, 3, 7), (10, 10, 3, 2)] {
        let mut writer = BitWriter::new();
        writer.write_int_wrapped(value, max);
        assert_eq!(writer.bit_len(), bits, "{value} below {max}");
        assert_eq!(reader(&writer).read_int_max(max), decoded);
    }
    // The clamping writer differs: 15 below 10 becomes 9 in four bits.
    let mut clamped = BitWriter::new();
    clamped.write_int_max(15, 10).unwrap();
    assert_eq!(clamped.bit_len(), 4);
    assert_eq!(reader(&clamped).read_int_max(10), 9);
}

#[test]
fn packed_integers_round_trip_with_exact_group_bytes() {
    for value in [
        0,
        1,
        127,
        128,
        300,
        16_383,
        16_384,
        (1 << 28) - 1,
        1 << 28,
        u32::MAX,
    ] {
        let mut writer = BitWriter::new();
        writer.write_int_packed(value);
        let expected_bytes = match value {
            0..=127 => 1,
            128..=16_383 => 2,
            16_384..=2_097_151 => 3,
            2_097_152..=268_435_455 => 4,
            _ => 5,
        };
        assert_eq!(writer.as_bytes().len(), expected_bytes, "{value}");
        let mut input = reader(&writer);
        assert_eq!(input.read_int_packed(), value);
        assert!(input.at_end() && !input.is_error());
    }
    // 300 = 44 + 2 * 128: (44 << 1) | 1 = 0x59, then (2 << 1) = 0x04.
    let mut writer = BitWriter::new();
    writer.write_int_packed(300);
    assert_eq!(writer.as_bytes(), [0x59, 0x04]);
    // 0xd9 carries group 108, so 0xd9 0x04 decodes as 108 + 256 = 364.
    let bytes = [0xd9, 0x04];
    assert_eq!(BitReader::new(&bytes, 16).unwrap().read_int_packed(), 364);
    let mut writer = BitWriter::new();
    writer.write_int_packed(u32::MAX);
    assert_eq!(writer.as_bytes(), [0xff, 0xff, 0xff, 0xff, 0x1e]);
    // A fifth continuation flag is ignored and high group bits are discarded.
    let bytes = [0xff; 6];
    let mut input = BitReader::new(&bytes, 48).unwrap();
    assert_eq!(input.read_int_packed(), u32::MAX);
    assert_eq!(input.position(), 40);
}

#[test]
fn packed_integers_read_unaligned_and_stop_at_missing_bits() {
    for value in [0, 300, 16_384, u32::MAX] {
        let mut writer = BitWriter::new();
        writer.write_bits(0b101, 3);
        writer.write_int_packed(value);
        let mut input = reader(&writer);
        assert_eq!(input.read_bits(3), 0b101);
        assert_eq!(input.read_int_packed(), value);
        assert!(input.at_end() && !input.is_error());
    }
    for offset in [0u32, 3] {
        let mut writer = BitWriter::new();
        writer.write_bits(0, offset);
        writer.write_int_packed(300);
        // Second byte cut short: error, value of the first group only.
        let mut input = BitReader::new(writer.as_bytes(), writer.bit_len() - 1).unwrap();
        input.read_bits(offset);
        assert_eq!(input.read_int_packed(), 44);
        assert!(input.is_error());
        // First byte cut short: error, nothing read.
        let mut input = BitReader::new(writer.as_bytes(), offset as usize + 7).unwrap();
        input.read_bits(offset);
        assert_eq!(input.read_int_packed(), 0);
        assert!(input.is_error());
        assert_eq!(input.position(), offset as usize);
    }
}

#[test]
fn termination_bit_is_split_and_appended_exactly() {
    assert_eq!(split_termination(&[0x01]).unwrap().1, 0);
    assert_eq!(split_termination(&[0x80]).unwrap().1, 7);
    assert_eq!(split_termination(&[0xab, 0x01]).unwrap().1, 8);
    assert_eq!(split_termination(&[0x00, 0x0f]).unwrap().1, 11);
    for invalid in [&[][..], &[0x00], &[0x12, 0x00]] {
        assert!(split_termination(invalid).is_err());
    }
    for length in 0..=24 {
        let bits = pattern(length);
        let mut writer = BitWriter::new();
        for bit in &bits {
            writer.write_bit(*bit);
        }
        let datagram = finish_with_termination(writer);
        assert_eq!(datagram.len(), length / 8 + 1);
        let (bytes, bit_len) = split_termination(&datagram).unwrap();
        assert_eq!(bit_len, length);
        let mut input = BitReader::new(bytes, bit_len).unwrap();
        let read: Vec<bool> = (0..length).map(|_| input.read_bit()).collect();
        assert_eq!(read, bits);
        assert!(input.at_end() && !input.is_error());
    }
}

#[test]
fn data_prefix_reads_magic_then_rejects_handshake_packets() {
    for (magic_bits, magic) in [(0, 0), (7, 0x55), (32, 0xdead_beef)] {
        let mut writer = BitWriter::new();
        write_data_prefix(&mut writer, magic_bits, magic).unwrap();
        assert_eq!(writer.bit_len(), magic_bits as usize + 2);
        assert_eq!(
            read_data_prefix(&mut reader(&writer), magic_bits).unwrap(),
            magic
        );

        let mut handshake = BitWriter::new();
        handshake.write_bits(magic, magic_bits);
        handshake.write_bit(true);
        let error = read_data_prefix(&mut reader(&handshake), magic_bits).unwrap_err();
        assert!(error.is::<HandshakePacket>());

        // A set Steam-auth bit marks that component's own packet (F-688).
        let mut auth = BitWriter::new();
        auth.write_bits(magic, magic_bits);
        auth.write_bit(false);
        auth.write_bit(true);
        let error = read_data_prefix(&mut reader(&auth), magic_bits).unwrap_err();
        assert!(!error.is::<HandshakePacket>() && error.to_string().contains("Steam-auth"));
    }
    let mut writer = BitWriter::new();
    assert!(write_data_prefix(&mut writer, 7, 0x80).is_err());
    assert!(write_data_prefix(&mut writer, 33, 0).is_err());
    assert!(read_data_prefix(&mut BitReader::new(&[0], 7).unwrap(), 7).is_err());
    assert!(read_data_prefix(&mut BitReader::new(&[0], 8).unwrap(), 33).is_err());
    // The authored 29-byte initial request is a handshake, never a data packet.
    let request = initial_request();
    let (bytes, bit_len) = split_termination(&request).unwrap();
    let mut input = BitReader::new(bytes, bit_len).unwrap();
    assert!(
        read_data_prefix(&mut input, 0)
            .unwrap_err()
            .is::<HandshakePacket>()
    );
}

#[test]
fn packet_header_round_trips_and_caps_history_at_eight_words() {
    let header = PacketHeader {
        sequence: 0x3fff,
        acked_sequence: 0x1234,
        history: vec![0xdead_beef, 7],
    };
    let infos = [
        (None, Receiver::Client),
        (
            Some(PacketInfo {
                jitter_clock: 1023,
                has_frame_time: false,
                server_frame_time: None,
            }),
            Receiver::Client,
        ),
        (
            Some(PacketInfo {
                jitter_clock: 0,
                has_frame_time: true,
                server_frame_time: Some(0xa5),
            }),
            Receiver::Client,
        ),
        (
            Some(PacketInfo {
                jitter_clock: 512,
                has_frame_time: true,
                server_frame_time: None,
            }),
            Receiver::Server,
        ),
    ];
    for (info, receiver) in infos {
        let mut writer = BitWriter::new();
        writer.write_bits(0b11, 2);
        write_packet_header(&mut writer, &header, info.as_ref(), receiver).unwrap();
        let mut input = reader(&writer);
        input.read_bits(2);
        assert_eq!(
            read_packet_header(&mut input, receiver).unwrap(),
            (header.clone(), info)
        );
        assert!(input.at_end());
    }
    // A count field of 15 reads only eight history words.
    let mut writer = BitWriter::new();
    writer.write_u32(15 | 5 << 4 | 9 << 18);
    for word in 0..9 {
        writer.write_u32(word);
    }
    let mut input = reader(&writer);
    let (capped, info) = read_packet_header(&mut input, Receiver::Client).unwrap();
    assert_eq!(capped.history, (0..8).collect::<Vec<_>>());
    assert_eq!((capped.acked_sequence, capped.sequence), (5, 9));
    // The ninth word's bit 0 was read as the packet-info bit.
    assert!(info.is_none());
    assert_eq!(input.position(), 9 * 32 + 1);

    let mut bad = header.clone();
    for history in [vec![], vec![0; 9]] {
        bad.history = history;
        assert!(write_packet_header(&mut BitWriter::new(), &bad, None, Receiver::Client).is_err());
    }
    bad = header.clone();
    bad.sequence = 0x4000;
    assert!(write_packet_header(&mut BitWriter::new(), &bad, None, Receiver::Client).is_err());
    for (info, receiver) in [
        ((1024, false, None), Receiver::Client),
        ((0, true, None), Receiver::Client),
        ((0, false, Some(1)), Receiver::Client),
        ((0, true, Some(1)), Receiver::Server),
    ] {
        let info = PacketInfo {
            jitter_clock: info.0,
            has_frame_time: info.1,
            server_frame_time: info.2,
        };
        assert!(
            write_packet_header(&mut BitWriter::new(), &header, Some(&info), receiver).is_err()
        );
    }
    let mut writer = BitWriter::new();
    write_packet_header(&mut writer, &header, None, Receiver::Client).unwrap();
    let truncated = BitReader::new(writer.as_bytes(), writer.bit_len() - 1).unwrap();
    assert!(read_packet_header(&mut { truncated }, Receiver::Client).is_err());
}

fn sample_bunches() -> Vec<Bunch> {
    let mut bunches = Vec::new();
    let mut open = plain_header();
    (open.control, open.open, open.channel_index) = (true, true, 3);
    open.channel_name = Some(Name::Hardcoded(255));
    bunches.push(Bunch {
        header: open,
        payload: payload(&pattern(37)),
    });
    let mut close = plain_header();
    (close.control, close.close, close.close_reason) = (true, true, Some(14));
    (close.paused, close.has_package_map_exports) = (true, true);
    bunches.push(Bunch {
        header: close,
        payload: payload(&[]),
    });
    for (initial, last) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut reliable = plain_header();
        (reliable.reliable, reliable.reliable_sequence_raw) = (true, Some(1023));
        (
            reliable.partial,
            reliable.partial_initial,
            reliable.partial_final,
        ) = (true, initial, last);
        reliable.has_must_be_mapped_guids = initial;
        reliable.channel_index = 15;
        reliable.channel_name = Some(Name::string(&ascii("Control"), 0).unwrap());
        bunches.push(Bunch {
            header: reliable,
            payload: payload(&pattern(64 * 8 - 1)),
        });
    }
    let mut both = plain_header();
    (both.control, both.open, both.close, both.close_reason) = (true, true, true, Some(0));
    (both.reliable, both.reliable_sequence_raw) = (true, Some(0));
    both.channel_name = Some(Name::string(&[0x41, 0xd83d, 0xde00], 5).unwrap());
    bunches.push(Bunch {
        header: both,
        payload: payload(&pattern(9)),
    });
    let mut none_name = plain_header();
    (none_name.reliable, none_name.reliable_sequence_raw) = (true, Some(1));
    none_name.channel_name = Some(Name::Hardcoded(0));
    bunches.push(Bunch {
        header: none_name,
        payload: payload(&pattern(1)),
    });
    bunches.push(Bunch {
        header: plain_header(),
        payload: payload(&pattern(8)),
    });
    bunches
}

#[test]
fn bunch_headers_round_trip_at_unaligned_positions_with_payload_from_bit_zero() {
    for bunch in sample_bunches() {
        let mut writer = BitWriter::new();
        writer.write_bits(0b10110, 5);
        write_bunch(&mut writer, &bunch, &LIMITS).unwrap();
        let mut input = reader(&writer);
        input.read_bits(5);
        let read = read_bunch(&mut input, &LIMITS).unwrap();
        assert!(input.at_end() && !input.is_error());
        assert_eq!(read, bunch);
        let bits = bunch.payload.bit_len();
        let mut copied = read.payload.reader();
        let mut original = bunch.payload.reader();
        for _ in 0..bits {
            assert_eq!(copied.read_bit(), original.read_bit());
        }
        assert!(copied.at_end());
        assert_eq!(read.payload.bytes().len(), bits.div_ceil(8));
        assert!(!format!("{read:?}").contains("bytes"));
    }
}

#[test]
fn bunch_reader_accepts_control_without_open_or_close_but_writer_rejects_it() {
    let mut writer = BitWriter::new();
    writer.write_bit(true); // control
    writer.write_bits(0, 2); // open, close
    writer.write_bits(0, 2); // paused, reliable
    writer.write_int_packed(2);
    writer.write_bits(0, 3); // exports, mapped GUIDs, partial
    writer.write_int_wrapped(3, 64 * 8);
    writer.write_bits(0b101, 3);
    let bunch = read_bunch(&mut reader(&writer), &LIMITS).unwrap();
    assert!(bunch.header.control && !bunch.header.open && !bunch.header.close);
    assert_eq!(bunch.payload.bytes(), [0b101]);
    assert!(write_bunch(&mut BitWriter::new(), &bunch, &LIMITS).is_err());
}

#[test]
fn bunch_limits_and_inconsistent_headers_are_rejected() {
    let base = Bunch {
        header: plain_header(),
        payload: payload(&[]),
    };
    let mut cases = Vec::new();
    let mut bunch = base.clone();
    bunch.header.channel_index = 16;
    cases.push(bunch);
    let mut bunch = base.clone();
    bunch.payload = payload(&pattern(64 * 8));
    cases.push(bunch);
    let mut bunch = base.clone();
    (bunch.header.control, bunch.header.close) = (true, true);
    cases.push(bunch.clone());
    bunch.header.close_reason = Some(15);
    cases.push(bunch);
    let mut bunch = base.clone();
    bunch.header.reliable = true;
    bunch.header.channel_name = Some(Name::Hardcoded(1));
    cases.push(bunch.clone());
    bunch.header.reliable_sequence_raw = Some(1024);
    cases.push(bunch.clone());
    bunch.header.reliable_sequence_raw = Some(0);
    bunch.header.channel_name = Some(Name::Hardcoded(703));
    cases.push(bunch.clone());
    bunch.header.channel_name = None;
    cases.push(bunch.clone());
    bunch.header.channel_name = Some(Name::string(&[65; 64], 0).unwrap());
    cases.push(bunch);
    let mut bunch = base.clone();
    bunch.header.partial_final = true;
    cases.push(bunch);
    let mut bunch = base.clone();
    bunch.header.channel_name = Some(Name::Hardcoded(1));
    cases.push(bunch);
    for (index, bunch) in cases.iter().enumerate() {
        assert!(
            write_bunch(&mut BitWriter::new(), bunch, &LIMITS).is_err(),
            "case {index}"
        );
    }

    // Reader-side limits, from independently written bits.
    let reliable_prefix = |writer: &mut BitWriter, channel: u32| {
        writer.write_bit(false); // control
        writer.write_bits(0b10, 2); // paused 0, reliable 1
        writer.write_int_packed(channel);
        writer.write_bits(0, 3);
        writer.write_int_max(0, 1024).unwrap();
    };
    let mut writer = BitWriter::new();
    reliable_prefix(&mut writer, 16);
    let error = read_bunch(&mut reader(&writer), &LIMITS).unwrap_err();
    assert!(format!("{error:#}").contains("channel limit"));
    for (index, accepted) in [(702, true), (703, false)] {
        let mut writer = BitWriter::new();
        reliable_prefix(&mut writer, 1);
        writer.write_bit(true);
        writer.write_int_packed(index);
        writer.write_int_max(0, 64 * 8).unwrap();
        let outcome = read_bunch(&mut reader(&writer), &LIMITS);
        assert_eq!(outcome.is_ok(), accepted, "{index}");
    }
    for (count, message) in [
        (i32::MIN, "i32::MIN"),
        (65, "unit limit"),
        (-65, "unit limit"),
        (2, "truncated archive string payload"),
    ] {
        let mut writer = BitWriter::new();
        reliable_prefix(&mut writer, 1);
        writer.write_bit(false);
        writer.write_u32(count as u32);
        let error = read_bunch(&mut reader(&writer), &LIMITS).unwrap_err();
        assert!(format!("{error:#}").contains(message), "{count}: {error:#}");
    }
    // Strict archive rules apply to names read from bits.
    let mut writer = BitWriter::new();
    reliable_prefix(&mut writer, 1);
    writer.write_bit(false);
    writer.write_u32(3);
    writer.write_bytes(b"AB");
    writer.write_u32(0);
    assert!(read_bunch(&mut reader(&writer), &LIMITS).is_err());
    // A declared payload longer than the remaining bits fails.
    let mut writer = BitWriter::new();
    writer.write_bits(0, 3);
    writer.write_int_packed(0);
    writer.write_bits(0, 3);
    writer.write_int_max(9, 64 * 8).unwrap();
    writer.write_bits(0, 8);
    let error = read_bunch(&mut reader(&writer), &LIMITS).unwrap_err();
    assert!(format!("{error:#}").contains("truncated bunch payload"));
}

fn config() -> PacketConfig {
    PacketConfig {
        magic_bits: 3,
        receiver: Receiver::Client,
        limits: LIMITS,
    }
}

fn sample_packet() -> Packet {
    Packet {
        magic: 0b101,
        header: PacketHeader {
            sequence: 17,
            acked_sequence: 16,
            history: vec![1],
        },
        info: Some(PacketInfo {
            jitter_clock: 77,
            has_frame_time: true,
            server_frame_time: Some(9),
        }),
        bunches: sample_bunches().into_iter().take(4).collect(),
    }
}

#[test]
fn multi_bunch_packets_read_until_the_terminated_end() {
    let packet = sample_packet();
    let datagram = write_packet(&packet, &config()).unwrap();
    assert_eq!(read_packet(&datagram, &config()).unwrap(), packet);
    let mut empty = packet.clone();
    empty.bunches.clear();
    let datagram = write_packet(&empty, &config()).unwrap();
    assert_eq!(read_packet(&datagram, &config()).unwrap(), empty);
}

#[test]
fn truncation_at_every_bit_errors_except_at_bunch_boundaries() {
    let packet = sample_packet();
    let config = config();
    let mut writer = BitWriter::new();
    write_data_prefix(&mut writer, config.magic_bits, packet.magic).unwrap();
    write_packet_header(
        &mut writer,
        &packet.header,
        packet.info.as_ref(),
        config.receiver,
    )
    .unwrap();
    let mut boundaries = vec![writer.bit_len()];
    for bunch in &packet.bunches {
        write_bunch(&mut writer, bunch, &config.limits).unwrap();
        boundaries.push(writer.bit_len());
    }
    let full = writer.bit_len();
    let mut accepted = 0;
    for length in 0..full {
        let mut input = BitReader::new(writer.as_bytes(), length).unwrap();
        let outcome = read_data_prefix(&mut input, config.magic_bits)
            .and_then(|_| read_packet_header(&mut input, config.receiver))
            .and_then(|_| read_bunches(&mut input, &config.limits));
        match outcome {
            Ok(bunches) => {
                // A boundary truncation is indistinguishable from fewer bunches.
                let index = boundaries.iter().position(|bit| *bit == length);
                assert_eq!(index, Some(bunches.len()), "accepted at bit {length}");
                assert_eq!(bunches, packet.bunches[..bunches.len()]);
                accepted += 1;
            }
            Err(_) => assert!(!boundaries.contains(&length), "rejected boundary {length}"),
        }
    }
    assert_eq!(accepted, packet.bunches.len());
    // Byte truncations re-read their own last byte as a terminator; no panics.
    let datagram = write_packet(&packet, &config).unwrap();
    for length in 0..datagram.len() {
        let _ = read_packet(&datagram[..length], &config);
    }
}

#[test]
fn data_packets_carry_the_connection_and_handler_termination_bits() {
    // The original server strips the handler's bit, then rejects a packet whose
    // remaining last byte is zero (F-688); the connection's bit prevents that.
    let config = PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Server,
        limits: BunchLimits {
            max_channels: 32,
            max_packet_bytes: 1024,
            max_string_units: 64,
        },
    };
    let packet = Packet {
        magic: 0,
        header: PacketHeader {
            sequence: 5,
            acked_sequence: 4,
            history: vec![0],
        },
        info: None,
        bunches: Vec::new(),
    };
    let datagram = write_packet(&packet, &config).unwrap();
    let (bytes, bit_len) = split_termination(&datagram).unwrap();
    let last = bit_len - 1;
    assert_eq!(
        bytes[last / 8] >> (last % 8) & 1,
        1,
        "connection termination bit"
    );
    assert_eq!(read_packet(&datagram, &config).unwrap(), packet);

    // The same bits under a single termination bit are malformed.
    let mut reader = BitReader::new(bytes, last).unwrap();
    let mut single = BitWriter::new();
    for _ in 0..last {
        single.write_bit(reader.read_bit());
    }
    let single = finish_with_termination(single);
    assert!(read_packet(&single, &config).is_err());
}
