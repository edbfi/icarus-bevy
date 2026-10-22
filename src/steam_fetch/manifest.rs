//! Depot manifest decoding.
//!
//! A CDN manifest download is a zip holding one binary manifest: sections of
//! little-endian `u32` magic, `u32` length and a protobuf body (payload,
//! metadata, signature), closed by the end-of-manifest magic. Encrypted file
//! names are base64 of depot-encrypted bytes. Steam's documented/observed
//! format (SteamKit2 as reference, independently implemented).

use anyhow::{Context, Result, bail, ensure};
use base64::Engine as _;
use prost::Message as _;

use super::{
    chunk::{ChunkSpec, symmetric_decrypt, unzip_single},
    proto::{ManifestMetadata, ManifestPayload, ManifestSignature},
};

pub const PAYLOAD_MAGIC: u32 = 0x71F6_17D0;
pub const METADATA_MAGIC: u32 = 0x1F48_12BE;
pub const SIGNATURE_MAGIC: u32 = 0x1B81_B817;
pub const END_MAGIC: u32 = 0x32C4_15AB;

/// `EDepotFileFlag` bit marking a directory entry.
pub const FLAG_DIRECTORY: u32 = 64;

/// Bounds applied while decoding one manifest.
#[derive(Debug, Clone, Copy)]
pub struct ManifestLimits {
    /// Bytes of the unzipped binary manifest.
    pub max_manifest_bytes: usize,
    pub max_files: usize,
    pub max_chunks: usize,
    pub max_name_bytes: usize,
}

impl Default for ManifestLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 256 * 1024 * 1024,
            max_files: 1_000_000,
            max_chunks: 8_000_000,
            max_name_bytes: 4096,
        }
    }
}

/// One file entry with a normalized `/`-separated path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub flags: u32,
    /// SHA-1 of the content; empty when absent.
    pub sha_content: Vec<u8>,
    pub chunks: Vec<ChunkSpec>,
}

impl ManifestFile {
    pub fn is_directory(&self) -> bool {
        self.flags & FLAG_DIRECTORY != 0
    }
}

/// A decoded manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub depot_id: u32,
    pub gid: u64,
    pub creation_time: u32,
    pub filenames_encrypted: bool,
    pub files: Vec<ManifestFile>,
}

impl Manifest {
    /// Find a file by `/`- or `\`-separated path, ASCII case-insensitively
    /// as the Windows depot is.
    pub fn find(&self, path: &str) -> Option<&ManifestFile> {
        let wanted = normalize(path);
        self.files
            .iter()
            .find(|file| file.path == wanted)
            .or_else(|| {
                self.files
                    .iter()
                    .find(|file| file.path.eq_ignore_ascii_case(&wanted))
            })
    }
}

/// Use `/` separators and drop leading or doubled separators.
pub fn normalize(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// Decode a manifest as downloaded from the CDN (zip-wrapped).
pub fn decode_zipped(
    bytes: &[u8],
    key: Option<&[u8]>,
    limits: &ManifestLimits,
) -> Result<Manifest> {
    let binary = unzip_single(bytes, limits.max_manifest_bytes).context("unzip depot manifest")?;
    decode_binary(&binary, key, limits)
}

/// Decode a binary (unzipped) manifest. `key` decrypts file names when the
/// metadata marks them encrypted.
pub fn decode_binary(
    bytes: &[u8],
    key: Option<&[u8]>,
    limits: &ManifestLimits,
) -> Result<Manifest> {
    ensure!(
        bytes.len() <= limits.max_manifest_bytes,
        "manifest exceeds {} bytes",
        limits.max_manifest_bytes
    );
    let mut payload = None;
    let mut metadata = None;
    let mut signature = None;
    let mut rest = bytes;
    loop {
        ensure!(rest.len() >= 4, "manifest ends without its end marker");
        let magic = u32::from_le_bytes(rest[..4].try_into().expect("four bytes"));
        rest = &rest[4..];
        if magic == END_MAGIC {
            break;
        }
        ensure!(
            rest.len() >= 4,
            "manifest section {magic:#010x} has no length"
        );
        let len = u32::from_le_bytes(rest[..4].try_into().expect("four bytes")) as usize;
        rest = &rest[4..];
        ensure!(
            len <= rest.len(),
            "manifest section {magic:#010x} overruns the data"
        );
        let (body, tail) = rest.split_at(len);
        rest = tail;
        match magic {
            PAYLOAD_MAGIC if payload.is_none() => {
                payload = Some(ManifestPayload::decode(body).context("manifest payload")?);
            }
            METADATA_MAGIC if metadata.is_none() => {
                metadata = Some(ManifestMetadata::decode(body).context("manifest metadata")?);
            }
            SIGNATURE_MAGIC if signature.is_none() => {
                signature = Some(ManifestSignature::decode(body).context("manifest signature")?);
            }
            PAYLOAD_MAGIC | METADATA_MAGIC | SIGNATURE_MAGIC => {
                bail!("duplicate manifest section {magic:#010x}")
            }
            other => bail!("unknown manifest section magic {other:#010x}"),
        }
    }
    let payload = payload.context("manifest has no payload section")?;
    let metadata = metadata.context("manifest has no metadata section")?;
    let encrypted = metadata.filenames_encrypted.unwrap_or(false);
    ensure!(
        payload.mappings.len() <= limits.max_files,
        "manifest lists {} files, above {}",
        payload.mappings.len(),
        limits.max_files
    );
    let mut chunk_total = 0usize;
    let mut files = Vec::with_capacity(payload.mappings.len());
    for mapping in payload.mappings {
        chunk_total += mapping.chunks.len();
        ensure!(
            chunk_total <= limits.max_chunks,
            "manifest lists more than {} chunks",
            limits.max_chunks
        );
        let raw_name = mapping.filename.unwrap_or_default();
        let name = if encrypted {
            let key = key.context("manifest file names are encrypted and no depot key is known")?;
            decrypt_filename(&raw_name, key, limits.max_name_bytes)?
        } else {
            raw_name
        };
        ensure!(
            name.len() <= limits.max_name_bytes,
            "manifest file name exceeds {} bytes",
            limits.max_name_bytes
        );
        let chunks = mapping
            .chunks
            .into_iter()
            .map(|chunk| {
                let id: [u8; 20] = chunk
                    .sha
                    .unwrap_or_default()
                    .as_slice()
                    .try_into()
                    .context("chunk id is not a 20-byte SHA-1")?;
                Ok(ChunkSpec {
                    id,
                    checksum: chunk.crc.unwrap_or(0),
                    offset: chunk.offset.unwrap_or(0),
                    original_len: chunk.cb_original.unwrap_or(0),
                    compressed_len: chunk.cb_compressed.unwrap_or(0),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        files.push(ManifestFile {
            path: normalize(&name),
            size: mapping.size.unwrap_or(0),
            flags: mapping.flags.unwrap_or(0),
            sha_content: mapping.sha_content.unwrap_or_default(),
            chunks,
        });
    }
    Ok(Manifest {
        depot_id: metadata.depot_id.unwrap_or(0),
        gid: metadata.gid_manifest.unwrap_or(0),
        creation_time: metadata.creation_time.unwrap_or(0),
        filenames_encrypted: encrypted,
        files,
    })
}

/// Decrypt one base64 file name and drop trailing NULs.
pub fn decrypt_filename(encoded: &str, key: &[u8], max: usize) -> Result<String> {
    ensure!(
        encoded.len() <= max * 2 + 64,
        "encrypted file name too long"
    );
    let compact: String = encoded.chars().filter(|c| !c.is_whitespace()).collect();
    let raw = base64::engine::general_purpose::STANDARD
        .decode(compact.as_bytes())
        .context("encrypted file name is not base64")?;
    let mut plain = symmetric_decrypt(&raw, key).context("decrypt file name")?;
    while plain.last() == Some(&0) {
        plain.pop();
    }
    String::from_utf8(plain).context("decrypted file name is not UTF-8")
}

/// Encode manifest sections in the binary layout. Used to author tests.
pub fn encode_binary(
    payload: &ManifestPayload,
    metadata: &ManifestMetadata,
    signature: &ManifestSignature,
) -> Vec<u8> {
    let mut out = Vec::new();
    for (magic, body) in [
        (PAYLOAD_MAGIC, payload.encode_to_vec()),
        (METADATA_MAGIC, metadata.encode_to_vec()),
        (SIGNATURE_MAGIC, signature.encode_to_vec()),
    ] {
        out.extend_from_slice(&magic.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&body);
    }
    out.extend_from_slice(&END_MAGIC.to_le_bytes());
    out
}
