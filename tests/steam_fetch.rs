//! Offline tests for the anonymous SteamPipe fetcher. Every input is authored
//! here; nothing touches the network or original distribution content.

use std::io::{Cursor, Write};

use icarus_bevy::steam_fetch::{
    assemble::{self, AssemblyLimits},
    chunk::{self, ChunkSpec},
    frame::{self, emsg},
    manifest::{self, ManifestLimits},
    net,
    proto::{
        ContentServerInfo, ManifestChunk, ManifestFileMapping, ManifestMetadata, ManifestPayload,
        ManifestSignature, ProtoHeader,
    },
    vdf,
};
use prost::Message as _;
use sha1::{Digest, Sha1};
use sha2::Sha256;

const KEY: [u8; 32] = *b"0123456789abcdef0123456789ABCDEF";
const IV: [u8; 16] = *b"authored-iv-0001";

fn spec_for(data: &[u8], offset: u64, compressed_len: u32) -> ChunkSpec {
    ChunkSpec {
        id: Sha1::digest(data).into(),
        checksum: chunk::adler32_seed0(data),
        offset,
        original_len: data.len() as u32,
        compressed_len,
    }
}

fn sample(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) ^ ((i / 7) as u8))
        .collect()
}

fn zip_one(name: &str, data: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file(name, options).unwrap();
    writer.write_all(data).unwrap();
    writer.finish().unwrap().into_inner()
}

// ---- VDF ----------------------------------------------------------------

const APP_INFO: &str = r#""appinfo"
{
	"appid"		"4242"
	"common"
	{
		"name"		"Authored \"Server\"\tTest"
		"type"		"Tool"
	}
	// comment line
	"depots"
	{
		"4243"
		{
			"config"
			{
				"oslist"		"linux"
			}
			"manifests"
			{
				"public"
				{
					"gid"		"1111"
					"size"		"10"
					"download"		"5"
				}
			}
		}
		"4244"
		{
			"config" { "oslist" "windows" }
			"MANIFESTS" { "public" { "gid" "18446744073709551000" "size" "777" } }
		}
		"4245"
		{
			"manifests" { "public" "2222" }
		}
		"4246"
		{
			"depotfromapp"		"7"
		}
		"4247"	[$WIN32]
		{
			"manifests" { "beta" { "gid" "3333" } }
		}
		"branches"
		{
			"public"
			{
				"buildid"		"987654"
				"timeupdated"		"1700000000"
			}
			"beta" { "buildid" "987655" }
		}
	}
}
"#;

#[test]
fn vdf_parses_authored_app_info() {
    let mut buffer = APP_INFO.as_bytes().to_vec();
    buffer.push(0);
    let doc = vdf::parse(&buffer).unwrap();
    assert_eq!(
        doc.path(&["appinfo", "common", "name"])
            .and_then(vdf::Value::as_str),
        Some("Authored \"Server\"\tTest")
    );
    let public = vdf::app_depots(&doc, "public").unwrap();
    assert_eq!(public.build_id, Some(987_654));
    let ids: Vec<(u32, u64)> = public
        .depots
        .iter()
        .map(|d| (d.depot_id, d.manifest_gid))
        .collect();
    assert_eq!(
        ids,
        vec![
            (4243, 1111),
            (4244, 18_446_744_073_709_551_000),
            (4245, 2222)
        ]
    );
    assert_eq!(public.depots[1].size, Some(777));
    let ordered: Vec<u32> = vdf::windows_first(&public.depots)
        .iter()
        .map(|d| d.depot_id)
        .collect();
    assert_eq!(ordered, vec![4244, 4245, 4243]);

    let beta = vdf::app_depots(&doc, "beta").unwrap();
    assert_eq!(beta.build_id, Some(987_655));
    assert_eq!(beta.depots.len(), 1);
    assert_eq!(beta.depots[0].manifest_gid, 3333);
}

#[test]
fn vdf_rejects_malformed_input() {
    assert!(vdf::parse(b"\"a\" { \"b\" \"c\"").is_err());
    assert!(vdf::parse(b"\"a\" }").is_err());
    assert!(vdf::parse(b"\"a\" \"unterminated").is_err());
    assert!(vdf::parse(b"\"a\"").is_err());
    let deep = "\"k\" {".repeat(100) + &"}".repeat(100);
    assert!(vdf::parse(deep.as_bytes()).is_err());
    let doc = vdf::parse(
        b"\"appinfo\" { \"depots\" { \"1\" { \"manifests\" { \"public\" { \"gid\" \"x\" } } } } }",
    )
    .unwrap();
    assert!(vdf::app_depots(&doc, "public").is_err());
}

// ---- framing ------------------------------------------------------------

#[test]
fn proto_header_and_emsg_round_trip() {
    let header = ProtoHeader {
        steamid: Some(0x01A0_0000_0000_1234),
        client_sessionid: Some(-77),
        routing_appid: Some(2_089_300),
        jobid_source: Some(42),
        jobid_target: None,
        target_job_name: Some("ContentServerDirectory.GetServersForSteamPipe#1".into()),
        eresult: Some(1),
        error_message: None,
    };
    let body = vec![1, 2, 3, 0xff];
    let bytes = frame::encode(emsg::SERVICE_METHOD_CALL_FROM_CLIENT, &header, &body);
    assert_eq!(
        u32::from_le_bytes(bytes[..4].try_into().unwrap()),
        151 | frame::PROTO_MASK
    );
    let packet = frame::decode(&bytes).unwrap();
    assert!(packet.proto);
    assert_eq!(packet.emsg, emsg::SERVICE_METHOD_CALL_FROM_CLIENT);
    assert_eq!(packet.header, header);
    assert_eq!(packet.body, body);
    assert_eq!(packet.eresult(), 1);

    let empty = frame::decode(&frame::encode(703, &ProtoHeader::default(), &[])).unwrap();
    assert_eq!(empty.eresult(), 2, "absent eresult defaults to Fail");

    let legacy = frame::decode(&[0x10, 0x27, 0, 0, 9, 9]).unwrap();
    assert!(!legacy.proto);
    assert_eq!(legacy.emsg, 10_000);

    let mut truncated = bytes.clone();
    truncated[4..8].copy_from_slice(&10_000u32.to_le_bytes());
    assert!(frame::decode(&truncated).is_err());
    assert!(frame::decode(&[1, 0, 0]).is_err());
}

#[test]
fn multi_unpacks_plain_gzip_and_nested() {
    let inner: Vec<Vec<u8>> = (0..3u32)
        .map(|i| {
            let header = ProtoHeader {
                jobid_target: Some(u64::from(i)),
                ..ProtoHeader::default()
            };
            frame::encode(8904, &header, &vec![i as u8; 100 + i as usize])
        })
        .collect();
    for gzip in [false, true] {
        let multi = frame::encode_multi(&inner, gzip);
        let packets = frame::decode_expanded(&multi, 1 << 20).unwrap();
        assert_eq!(packets.len(), 3);
        for (i, packet) in packets.iter().enumerate() {
            assert_eq!(packet.emsg, 8904);
            assert_eq!(packet.header.jobid_target, Some(i as u64));
            assert_eq!(packet.body, vec![i as u8; 100 + i]);
        }
    }
    // Multi inside Multi expands once more.
    let nested = frame::encode_multi(&[frame::encode_multi(&inner, true)], true);
    assert_eq!(frame::decode_expanded(&nested, 1 << 20).unwrap().len(), 3);
    // Too deep, oversize and overrun inputs are rejected.
    let deeper = frame::encode_multi(std::slice::from_ref(&nested), false);
    assert!(frame::decode_expanded(&deeper, 1 << 20).is_err());
    assert!(frame::decode_expanded(&frame::encode_multi(&inner, true), 64).is_err());
    let mut overrun = inner[0].clone();
    overrun.truncate(10);
    let bad = frame::encode_multi(&[overrun], false);
    assert!(frame::decode_expanded(&bad, 1 << 20).is_err());
}

// ---- CM directory and CDN selection ---------------------------------------

#[test]
fn cm_list_and_cdn_selection() {
    let body = br#"{"response":{"serverlist":[
        {"endpoint":"cm1.example.net:27017","legacy_endpoint":"1.2.3.4:27017","type":"netfilter","dc":"x","realm":"steamglobal","load":1,"wtd_load":1.0},
        {"endpoint":"cm2.example.net:443","type":"websockets","dc":"x","realm":"steamglobal","load":1,"wtd_load":1.0},
        {"endpoint":"cm3.example.net:443","type":"websockets","dc":"x","realm":"steamchina"}
    ],"success":true,"message":""}}"#;
    assert_eq!(
        net::parse_cm_list(body).unwrap(),
        vec!["cm2.example.net:443".to_owned()]
    );
    assert!(net::parse_cm_list(br#"{"response":{"serverlist":[],"success":true}}"#).is_err());

    let server = |kind: &str, host: &str, load: f32, https: &str| ContentServerInfo {
        r#type: Some(kind.into()),
        vhost: Some(host.into()),
        weighted_load: Some(load),
        https_support: Some(https.into()),
        ..ContentServerInfo::default()
    };
    let mut proxy = server("CDN", "proxy.example.net", 0.0, "mandatory");
    proxy.use_as_proxy = Some(true);
    let mut other_app = server("SteamCache", "cache.example.net", 0.0, "optional");
    other_app.allowed_app_ids = vec![1];
    let list = vec![
        server("CDN", "b.example.net", 50.0, "none"),
        server("SteamCache", "a.example.net", 10.0, "mandatory"),
        server("OpenCache", "c.example.net", 0.0, "mandatory"),
        server("CDN", "bad/host", 0.0, "mandatory"),
        proxy,
        other_app,
    ];
    let selected = net::select_cdn_servers(&list, 4242);
    assert_eq!(
        selected,
        vec![
            net::CdnServer {
                host: "a.example.net".into(),
                https: true
            },
            net::CdnServer {
                host: "b.example.net".into(),
                https: false
            },
        ]
    );
    assert_eq!(
        selected[0].url("depot/1/chunk/ab"),
        "https://a.example.net/depot/1/chunk/ab"
    );
}

// ---- manifests ------------------------------------------------------------

fn authored_payload(names: &[&str], encrypt: bool) -> (ManifestPayload, Vec<u8>) {
    let content = sample(5000, 3);
    let chunk = ManifestChunk {
        sha: Some(Sha1::digest(&content).to_vec()),
        crc: Some(chunk::adler32_seed0(&content)),
        offset: Some(0),
        cb_original: Some(content.len() as u32),
        cb_compressed: Some(1234),
    };
    let mappings = names
        .iter()
        .map(|name| {
            let filename = if encrypt {
                let mut plain = name.as_bytes().to_vec();
                plain.push(0);
                let raw = chunk::symmetric_encrypt(&plain, &KEY, IV).unwrap();
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode(raw)
            } else {
                (*name).to_owned()
            };
            ManifestFileMapping {
                filename: Some(filename),
                size: Some(content.len() as u64),
                flags: Some(0),
                sha_filename: None,
                sha_content: Some(Sha1::digest(&content).to_vec()),
                chunks: vec![chunk.clone()],
                linktarget: None,
            }
        })
        .collect();
    (ManifestPayload { mappings }, content)
}

fn metadata(encrypted: bool) -> ManifestMetadata {
    ManifestMetadata {
        depot_id: Some(4244),
        gid_manifest: Some(18_446_744_073_709_551_000),
        creation_time: Some(1_700_000_000),
        filenames_encrypted: Some(encrypted),
        ..ManifestMetadata::default()
    }
}

#[test]
fn manifest_decodes_plain_and_encrypted_names() {
    let names = [
        "Game\\Content\\Data\\data.pak",
        "Game\\Binaries\\Win64\\Server.exe",
    ];
    let limits = ManifestLimits::default();
    for encrypt in [false, true] {
        let (payload, content) = authored_payload(&names, encrypt);
        let binary = manifest::encode_binary(
            &payload,
            &metadata(encrypt),
            &ManifestSignature {
                signature: Some(vec![7; 128]),
            },
        );
        let zipped = zip_one("z", &binary);
        let decoded = manifest::decode_zipped(&zipped, Some(&KEY), &limits).unwrap();
        assert_eq!(decoded.depot_id, 4244);
        assert_eq!(decoded.gid, 18_446_744_073_709_551_000);
        assert_eq!(decoded.filenames_encrypted, encrypt);
        let paths: Vec<&str> = decoded.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "Game/Content/Data/data.pak",
                "Game/Binaries/Win64/Server.exe"
            ]
        );
        let file = decoded.find("game/content/data/DATA.pak").unwrap();
        assert_eq!(file.size, content.len() as u64);
        assert_eq!(file.chunks.len(), 1);
        assert_eq!(file.chunks[0].id, <[u8; 20]>::from(Sha1::digest(&content)));
        assert_eq!(file.chunks[0].compressed_len, 1234);
        assert!(decoded.find("Game/Content/Data/other.pak").is_none());
        if encrypt {
            assert!(manifest::decode_zipped(&zipped, None, &limits).is_err());
            let wrong = [9u8; 32];
            assert!(manifest::decode_zipped(&zipped, Some(&wrong), &limits).is_err());
        }
    }
}

#[test]
fn manifest_rejects_bad_structure() {
    let limits = ManifestLimits::default();
    let (payload, _) = authored_payload(&["a"], false);
    let good = manifest::encode_binary(&payload, &metadata(false), &ManifestSignature::default());
    // Missing end marker.
    assert!(manifest::decode_binary(&good[..good.len() - 4], None, &limits).is_err());
    // Unknown section magic.
    let mut unknown = 0xDEAD_BEEFu32.to_le_bytes().to_vec();
    unknown.extend_from_slice(&0u32.to_le_bytes());
    unknown.extend_from_slice(&good);
    assert!(manifest::decode_binary(&unknown, None, &limits).is_err());
    // Section overrunning the data.
    let mut overrun = good.clone();
    overrun[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(manifest::decode_binary(&overrun, None, &limits).is_err());
    // No payload section.
    let mut no_payload = manifest::METADATA_MAGIC.to_le_bytes().to_vec();
    let meta = metadata(false).encode_to_vec();
    no_payload.extend_from_slice(&(meta.len() as u32).to_le_bytes());
    no_payload.extend_from_slice(&meta);
    no_payload.extend_from_slice(&manifest::END_MAGIC.to_le_bytes());
    assert!(manifest::decode_binary(&no_payload, None, &limits).is_err());
    // File-count bound.
    let (many, _) = authored_payload(&["a", "b", "c"], false);
    let binary = manifest::encode_binary(&many, &metadata(false), &ManifestSignature::default());
    let tight = ManifestLimits {
        max_files: 2,
        ..ManifestLimits::default()
    };
    assert!(manifest::decode_binary(&binary, None, &tight).is_err());
    // Zip with two members.
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for name in ["a", "b"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&good).unwrap();
    }
    let two = writer.finish().unwrap().into_inner();
    assert!(manifest::decode_zipped(&two, None, &limits).is_err());
}

// ---- chunks ---------------------------------------------------------------

#[test]
fn adler32_uses_seed_zero() {
    // Standard Adler-32 of "Wikipedia" is 0x11E60398; seed 0 drops the
    // initial 1 from `a` (once) and from `b` (once per byte).
    assert_eq!(chunk::adler32_seed0(b"Wikipedia"), 0x11DD_0397);
    assert_eq!(chunk::adler32_seed0(b""), 0);
    let big = vec![0xffu8; 100_000];
    let (mut a, mut b) = (0u64, 0u64);
    for &byte in &big {
        a = (a + u64::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    assert_eq!(chunk::adler32_seed0(&big), ((b << 16) | a) as u32);
}

#[test]
fn symmetric_crypto_round_trips() {
    for len in [0usize, 1, 15, 16, 17, 1000] {
        let plain = sample(len, 9);
        let encrypted = chunk::symmetric_encrypt(&plain, &KEY, IV).unwrap();
        assert_eq!(encrypted.len(), 16 + (len / 16 + 1) * 16);
        assert_ne!(&encrypted[..16], &IV, "the IV is stored ECB-encrypted");
        assert_eq!(chunk::symmetric_decrypt(&encrypted, &KEY).unwrap(), plain);
    }
    assert!(chunk::symmetric_decrypt(&[0; 31], &KEY).is_err());
    assert!(chunk::symmetric_decrypt(&[0; 32], &KEY[..16]).is_err());
}

#[test]
fn chunk_round_trips_vzip_vzstd_and_zip() {
    let data = sample(70_000, 1);
    let framed = [
        ("vzip", chunk::compress_vzip(&data)),
        ("vzstd", chunk::compress_vzstd(&data)),
        ("zip", zip_one("chunk", &data)),
    ];
    for (name, compressed) in framed {
        let raw = chunk::symmetric_encrypt(&compressed, &KEY, IV).unwrap();
        let spec = spec_for(&data, 0, raw.len() as u32);
        assert_eq!(
            chunk::process_chunk(&raw, &KEY, &spec).unwrap(),
            data,
            "{name}"
        );
        // Unknown compressed length is accepted.
        let loose = ChunkSpec {
            compressed_len: 0,
            ..spec.clone()
        };
        assert_eq!(chunk::process_chunk(&raw, &KEY, &loose).unwrap(), data);
    }
}

#[test]
fn chunk_rejects_corruption() {
    let data = sample(20_000, 2);
    let vzip = chunk::compress_vzip(&data);
    let encrypt = |plain: &[u8]| chunk::symmetric_encrypt(plain, &KEY, IV).unwrap();
    let raw = encrypt(&vzip);
    let spec = spec_for(&data, 0, raw.len() as u32);

    // Footer CRC-32 corrupted.
    let mut bad_crc = vzip.clone();
    let at = bad_crc.len() - 10;
    bad_crc[at] ^= 1;
    let error = chunk::process_chunk(
        &encrypt(&bad_crc),
        &KEY,
        &ChunkSpec {
            compressed_len: 0,
            ..spec.clone()
        },
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("CRC"), "{error:#}");

    // Footer size disagrees with the stream.
    let mut bad_size = vzip.clone();
    let at = bad_size.len() - 6;
    bad_size[at..at + 4].copy_from_slice(&((data.len() - 1) as u32).to_le_bytes());
    assert!(
        chunk::process_chunk(
            &encrypt(&bad_size),
            &KEY,
            &ChunkSpec {
                compressed_len: 0,
                ..spec.clone()
            }
        )
        .is_err()
    );

    // SHA-1 id does not match the content.
    let mut wrong_id = spec.clone();
    wrong_id.id[0] ^= 0x80;
    let error = chunk::process_chunk(&raw, &KEY, &wrong_id).unwrap_err();
    assert!(format!("{error:#}").contains("SHA-1"), "{error:#}");

    // Adler checksum disagrees.
    let mut wrong_sum = spec.clone();
    wrong_sum.checksum ^= 1;
    assert!(chunk::process_chunk(&raw, &KEY, &wrong_sum).is_err());

    // Manifest size disagrees (larger and smaller).
    for original_len in [data.len() as u32 + 1, data.len() as u32 - 1] {
        let wrong = ChunkSpec {
            original_len,
            ..spec.clone()
        };
        assert!(chunk::process_chunk(&raw, &KEY, &wrong).is_err());
    }

    // Download length disagrees with the manifest.
    assert!(chunk::process_chunk(&raw[..raw.len() - 16], &KEY, &spec).is_err());

    // Wrong key, unknown format and corrupt zstd frame.
    assert!(chunk::process_chunk(&raw, &[1u8; 32], &spec).is_err());
    let unknown = encrypt(b"XXXXnot a known format");
    assert!(
        chunk::process_chunk(
            &unknown,
            &KEY,
            &ChunkSpec {
                compressed_len: 0,
                ..spec.clone()
            }
        )
        .is_err()
    );
    let mut vzstd = chunk::compress_vzstd(&data);
    vzstd[12] ^= 0xff;
    assert!(chunk::decompress_vzstd(&vzstd, data.len()).is_err());
    // Size bound applies before decompression.
    assert!(chunk::decompress_vzip(&vzip, data.len() - 1).is_err());
}

// ---- assembly ---------------------------------------------------------------

fn authored_file(path: &str, pieces: &[Vec<u8>]) -> (manifest::ManifestFile, Vec<u8>) {
    let mut whole = Vec::new();
    let mut chunks = Vec::new();
    for piece in pieces {
        chunks.push(spec_for(piece, whole.len() as u64, 0));
        whole.extend_from_slice(piece);
    }
    chunks.reverse(); // manifests need not list chunks in order
    let file = manifest::ManifestFile {
        path: path.to_owned(),
        size: whole.len() as u64,
        flags: 0,
        sha_content: Sha1::digest(&whole).to_vec(),
        chunks,
    };
    (file, whole)
}

fn lookup(pieces: &[Vec<u8>]) -> impl Fn(&ChunkSpec) -> anyhow::Result<Vec<u8>> + Sync + '_ {
    move |spec| {
        pieces
            .iter()
            .find(|piece| <[u8; 20]>::from(Sha1::digest(piece)) == spec.id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown chunk"))
    }
}

fn leftovers(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".icarus-fetch-"))
        .collect()
}

#[test]
fn assembly_writes_verified_file_atomically() {
    let pieces = vec![
        sample(1000, 1),
        sample(4096, 2),
        sample(17, 3),
        sample(2048, 4),
    ];
    let (file, whole) = authored_file("Game/Content/Data/data.pak", &pieces);
    let out = tempfile::tempdir().unwrap();
    let limits = AssemblyLimits::default();
    for workers in [1, 3] {
        let written =
            assemble::assemble(out.path(), &file, &limits, workers, lookup(&pieces)).unwrap();
        assert_eq!(written.path, out.path().join("Game/Content/Data/data.pak"));
        assert_eq!(std::fs::read(&written.path).unwrap(), whole);
        assert_eq!(written.size, whole.len() as u64);
        assert_eq!(
            written.sha1,
            icarus_bevy::steam_fetch::hex(&Sha1::digest(&whole))
        );
        assert_eq!(
            written.sha256,
            icarus_bevy::steam_fetch::hex(&Sha256::digest(&whole))
        );
        assert!(leftovers(&out.path().join("Game/Content/Data")).is_empty());
    }

    // A wrong whole-file SHA-1 leaves neither the file nor a temporary.
    let fresh = tempfile::tempdir().unwrap();
    let mut wrong = file.clone();
    wrong.sha_content[0] ^= 1;
    let error = assemble::assemble(fresh.path(), &wrong, &limits, 2, lookup(&pieces)).unwrap_err();
    assert!(format!("{error:#}").contains("SHA-1"), "{error:#}");
    let dir = fresh.path().join("Game/Content/Data");
    assert!(!dir.join("data.pak").exists());
    assert!(leftovers(&dir).is_empty());

    // A chunk whose bytes do not match its id is rejected.
    let corrupt = |spec: &ChunkSpec| {
        let mut data = lookup(&pieces)(spec)?;
        data[0] ^= 1;
        Ok(data)
    };
    assert!(assemble::assemble(fresh.path(), &file, &limits, 2, corrupt).is_err());
    assert!(!dir.join("data.pak").exists());
    assert!(leftovers(&dir).is_empty());
}

#[test]
fn assembly_rejects_bad_layouts_and_paths() {
    let pieces = vec![sample(100, 1), sample(100, 2)];
    let (file, _) = authored_file("a/b.bin", &pieces);
    let limits = AssemblyLimits::default();

    let mut gap = file.clone();
    gap.chunks[0].offset += 1;
    assert!(assemble::plan_chunks(&gap, &limits).is_err());
    let mut short = file.clone();
    short.size += 1;
    assert!(assemble::plan_chunks(&short, &limits).is_err());
    let mut overlap = file.clone();
    overlap.chunks[1].offset = 50;
    assert!(assemble::plan_chunks(&overlap, &limits).is_err());
    let tight = AssemblyLimits {
        max_file_bytes: 199,
        ..limits
    };
    assert!(assemble::plan_chunks(&file, &tight).is_err());
    let mut directory = file.clone();
    directory.flags = manifest::FLAG_DIRECTORY;
    assert!(assemble::plan_chunks(&directory, &limits).is_err());

    for bad in ["../x", "a/../../x", "/", "", "a/./b"] {
        assert!(assemble::safe_relative_path(bad).is_err(), "{bad:?}");
    }
    assert_eq!(
        assemble::safe_relative_path("/Game\\Content//x.pak").unwrap(),
        std::path::PathBuf::from("Game/Content/x.pak")
    );

    let out = tempfile::tempdir().unwrap();
    let mut escape = file.clone();
    escape.path = "../escape.bin".into();
    assert!(assemble::assemble(out.path(), &escape, &limits, 1, lookup(&pieces)).is_err());
    assert!(!out.path().parent().unwrap().join("escape.bin").exists());
}

/// A body exactly as long as the limit is accepted (CDN chunks are served
/// at their manifest size), and one byte more is rejected.
#[test]
fn a_body_at_exactly_the_limit_is_accepted() {
    use icarus_bevy::steam_fetch::net::{HttpTimeouts, Tls, get_bounded, http_agent};
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                )
                .unwrap();
        }
    });
    let Ok(tls) = Tls::from_native_roots() else {
        // No system roots on this host: the agent cannot be built.
        return;
    };
    let agent = http_agent(&tls, &HttpTimeouts::default());
    let url = format!("http://{address}/chunk");
    assert_eq!(get_bounded(&agent, &url, 5).unwrap(), b"hello");
    assert!(get_bounded(&agent, &url, 4).is_err());
    server.join().unwrap();
}
