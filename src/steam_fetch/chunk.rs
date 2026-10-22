//! Depot symmetric crypto and chunk decompression.
//!
//! Depot data and encrypted file names use AES-256: the first 16 bytes are
//! the IV encrypted with ECB, the rest is CBC with PKCS#7 padding. A
//! decrypted chunk is VZip (LZMA), VZstd (zstd) or a one-member PKZip
//! archive. Steam's documented/observed protocol (SteamKit2 as reference,
//! independently implemented).

use std::io::{Cursor, Read};

use aes::{
    Aes256,
    cipher::{
        BlockCipherDecrypt, BlockCipherEncrypt, BlockModeDecrypt, BlockModeEncrypt, KeyInit,
        KeyIvInit, block_padding::Pkcs7,
    },
};
use anyhow::{Context, Result, bail, ensure};
use sha1::{Digest, Sha1};

/// Depot keys are AES-256 keys.
pub const KEY_LEN: usize = 32;
const BLOCK: usize = 16;

/// VZip framing: "VZ" + version 'a' + 4-byte timestamp/CRC, 5 LZMA property
/// bytes, raw LZMA, then CRC-32, size and "zv".
const VZIP_HEADER: usize = 7;
const VZIP_FOOTER: usize = 10;
const LZMA_PROPS: usize = 5;
/// VZstd framing: "VSZa" + 4-byte CRC, zstd frame, then CRC-32, size, four
/// unused bytes and "zsv".
const VZSTD_HEADER: usize = 8;
const VZSTD_FOOTER: usize = 15;
/// LZMA dictionary bound; VZip writers use an 8 MiB dictionary.
const LZMA_MEMLIMIT: usize = 64 * 1024 * 1024;

/// One manifest chunk as the fetcher uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSpec {
    /// SHA-1 of the uncompressed data; also its CDN id.
    pub id: [u8; 20],
    /// Adler-32 with seed 0 of the uncompressed data.
    pub checksum: u32,
    pub offset: u64,
    pub original_len: u32,
    /// Length of the encrypted chunk as served; zero when unknown.
    pub compressed_len: u32,
}

impl ChunkSpec {
    /// Lowercase hex chunk id used in CDN paths.
    pub fn id_hex(&self) -> String {
        crate::hex_lower(&self.id)
    }
}

fn cipher(key: &[u8]) -> Result<Aes256> {
    ensure!(
        key.len() == KEY_LEN,
        "depot key is {} bytes, not 32",
        key.len()
    );
    Aes256::new_from_slice(key).map_err(|_| anyhow::anyhow!("invalid AES-256 key"))
}

/// Decrypt depot-encrypted data (ECB-encrypted IV, then CBC with PKCS#7).
pub fn symmetric_decrypt(data: &[u8], key: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        data.len() >= 2 * BLOCK && data.len().is_multiple_of(BLOCK),
        "encrypted length {} is not a whole number of blocks after the IV",
        data.len()
    );
    let aes = cipher(key)?;
    let mut iv = aes::Block::default();
    iv.copy_from_slice(&data[..BLOCK]);
    aes.decrypt_block(&mut iv);
    let mut buf = data[BLOCK..].to_vec();
    let plain_len = cbc::Decryptor::<Aes256>::new_from_slices(key, &iv)
        .map_err(|_| anyhow::anyhow!("invalid AES-256-CBC key or IV"))?
        .decrypt_padded::<Pkcs7>(&mut buf)
        .map_err(|_| anyhow::anyhow!("bad PKCS#7 padding: wrong depot key or corrupt data"))?
        .len();
    buf.truncate(plain_len);
    Ok(buf)
}

/// Encrypt like [`symmetric_decrypt`] expects, with a caller-chosen IV.
/// Used to author offline test vectors.
pub fn symmetric_encrypt(plain: &[u8], key: &[u8], iv: [u8; BLOCK]) -> Result<Vec<u8>> {
    let aes = cipher(key)?;
    let mut encrypted_iv = aes::Block::from(iv);
    aes.encrypt_block(&mut encrypted_iv);
    let body = cbc::Encryptor::<Aes256>::new_from_slices(key, &iv)
        .map_err(|_| anyhow::anyhow!("invalid AES-256-CBC key or IV"))?
        .encrypt_padded_vec::<Pkcs7>(plain);
    let mut out = encrypted_iv.to_vec();
    out.extend_from_slice(&body);
    Ok(out)
}

/// Adler-32 starting from 0 rather than 1, as Steam's chunk checksum does.
pub fn adler32_seed0(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let (mut a, mut b) = (0u32, 0u32);
    // 5552 bytes keep the running sums below u32 overflow.
    for block in data.chunks(5552) {
        for &byte in block {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

fn read_bounded(reader: impl Read, max: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    reader
        .take(max as u64 + 1)
        .read_to_end(&mut out)
        .context("decompress chunk")?;
    ensure!(out.len() <= max, "decompressed data exceeds {max} bytes");
    Ok(out)
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes[..4].try_into().expect("four bytes"))
}

/// Decompress a VZip (LZMA) buffer; verifies the footer size and CRC-32.
pub fn decompress_vzip(data: &[u8], max: usize) -> Result<Vec<u8>> {
    ensure!(
        data.len() >= VZIP_HEADER + LZMA_PROPS + VZIP_FOOTER,
        "VZip buffer too short"
    );
    ensure!(&data[..3] == b"VZa", "missing VZip header");
    let footer = &data[data.len() - VZIP_FOOTER..];
    ensure!(&footer[8..] == b"zv", "missing VZip footer");
    let crc = le_u32(footer);
    let size = le_u32(&footer[4..]) as usize;
    ensure!(size <= max, "VZip size {size} exceeds {max}");
    let props = &data[VZIP_HEADER..VZIP_HEADER + LZMA_PROPS];
    let stream = &data[VZIP_HEADER + LZMA_PROPS..data.len() - VZIP_FOOTER];
    // lzma-rs reads properties and the dictionary size from the stream head.
    let mut input = Vec::with_capacity(LZMA_PROPS + stream.len());
    input.extend_from_slice(props);
    input.extend_from_slice(stream);
    let mut out = Vec::with_capacity(size);
    let options = lzma_rs::decompress::Options {
        unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(size as u64)),
        memlimit: Some(LZMA_MEMLIMIT),
        allow_incomplete: false,
    };
    lzma_rs::lzma_decompress_with_options(&mut input.as_slice(), &mut out, &options)
        .map_err(|error| anyhow::anyhow!("VZip LZMA stream: {error:?}"))?;
    ensure!(
        out.len() == size,
        "VZip produced {} bytes, footer says {size}",
        out.len()
    );
    ensure!(crc32fast::hash(&out) == crc, "VZip CRC-32 mismatch");
    Ok(out)
}

/// Wrap LZMA-compressed data in VZip framing. Used to author test vectors.
pub fn compress_vzip(plain: &[u8]) -> Vec<u8> {
    let mut lzma = Vec::new();
    let options = lzma_rs::compress::Options {
        unpacked_size: lzma_rs::compress::UnpackedSize::SkipWritingToHeader,
    };
    lzma_rs::lzma_compress_with_options(&mut &plain[..], &mut lzma, &options)
        .expect("writing to a Vec cannot fail");
    let crc = crc32fast::hash(plain);
    let mut out = b"VZa".to_vec();
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&lzma);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    out.extend_from_slice(b"zv");
    out
}

/// Decompress a VZstd buffer; verifies the footer size and CRC-32.
pub fn decompress_vzstd(data: &[u8], max: usize) -> Result<Vec<u8>> {
    ensure!(
        data.len() >= VZSTD_HEADER + VZSTD_FOOTER,
        "VZstd buffer too short"
    );
    ensure!(&data[..4] == b"VSZa", "missing VZstd header");
    let footer = &data[data.len() - VZSTD_FOOTER..];
    ensure!(&footer[12..] == b"zsv", "missing VZstd footer");
    let crc = le_u32(footer);
    let size = le_u32(&footer[4..]) as usize;
    ensure!(size <= max, "VZstd size {size} exceeds {max}");
    let frame = &data[VZSTD_HEADER..data.len() - VZSTD_FOOTER];
    let decoder = ruzstd::decoding::StreamingDecoder::new(frame)
        .map_err(|error| anyhow::anyhow!("VZstd frame header: {error}"))?;
    let out = read_bounded(decoder, size)?;
    ensure!(
        out.len() == size,
        "VZstd produced {} bytes, footer says {size}",
        out.len()
    );
    ensure!(crc32fast::hash(&out) == crc, "VZstd CRC-32 mismatch");
    Ok(out)
}

/// Wrap a zstd frame in VZstd framing. Used to author test vectors.
pub fn compress_vzstd(plain: &[u8]) -> Vec<u8> {
    let frame =
        ruzstd::encoding::compress_to_vec(plain, ruzstd::encoding::CompressionLevel::Fastest);
    let crc = crc32fast::hash(plain);
    let mut out = b"VSZa".to_vec();
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&frame);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(b"zsv");
    out
}

/// Read the single member of a PKZip archive; the zip reader checks its
/// CRC-32 at end of stream.
pub fn unzip_single(data: &[u8], max: usize) -> Result<Vec<u8>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(data)).context("open zip")?;
    ensure!(
        archive.len() == 1,
        "zip holds {} members, expected one",
        archive.len()
    );
    let member = archive.by_index(0).context("open zip member")?;
    ensure!(
        member.size() <= max as u64,
        "zip member is {} bytes, above {max}",
        member.size()
    );
    read_bounded(member, max)
}

/// Decrypt, decompress and verify one downloaded chunk.
pub fn process_chunk(raw: &[u8], key: &[u8], spec: &ChunkSpec) -> Result<Vec<u8>> {
    let data = decode_chunk(raw, key, spec)?;
    verify_chunk(&data, spec)?;
    Ok(data)
}

/// Decrypt and decompress one downloaded chunk without the final
/// [`verify_chunk`]; the size bound still applies.
pub fn decode_chunk(raw: &[u8], key: &[u8], spec: &ChunkSpec) -> Result<Vec<u8>> {
    ensure!(
        spec.compressed_len == 0 || raw.len() == spec.compressed_len as usize,
        "chunk {} is {} bytes, manifest says {}",
        spec.id_hex(),
        raw.len(),
        spec.compressed_len
    );
    let decrypted =
        symmetric_decrypt(raw, key).with_context(|| format!("decrypt chunk {}", spec.id_hex()))?;
    let max = spec.original_len as usize;
    let data = if decrypted.starts_with(b"VSZa") {
        decompress_vzstd(&decrypted, max)
    } else if decrypted.starts_with(b"VZa") {
        decompress_vzip(&decrypted, max)
    } else if decrypted.starts_with(b"PK\x03\x04") {
        unzip_single(&decrypted, max)
    } else {
        bail!(
            "chunk {} has unknown compression {:02x?}",
            spec.id_hex(),
            &decrypted[..decrypted.len().min(4)]
        )
    }
    .with_context(|| format!("decompress chunk {}", spec.id_hex()))?;
    Ok(data)
}

/// Check an uncompressed chunk's size, SHA-1 id and Adler-32.
pub fn verify_chunk(data: &[u8], spec: &ChunkSpec) -> Result<()> {
    ensure!(
        data.len() == spec.original_len as usize,
        "chunk {} is {} bytes uncompressed, manifest says {}",
        spec.id_hex(),
        data.len(),
        spec.original_len
    );
    ensure!(
        Sha1::digest(data).as_slice() == spec.id,
        "chunk {} SHA-1 mismatch",
        spec.id_hex()
    );
    ensure!(
        adler32_seed0(data) == spec.checksum,
        "chunk {} checksum mismatch",
        spec.id_hex()
    );
    Ok(())
}
