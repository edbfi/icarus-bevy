'use strict';

// Inspection reads metadata only; class counting privately decodes selected members.
const fs = require('node:fs');
const crypto = require('node:crypto');
const {countCookedClassFields, LIMITS: CLASS_LIMITS, PROFILE, ZERO_VERSION_PROFILE} = require('./cooked_class_field_counts.cjs');
const {LIMITS: PAK_LIMITS, FOOTER_SIZE, parseFooter, parsePrimaryIndexWithMount,
  parseDirectoryIndex} = require('./pak_protocol_metadata.cjs');
const MEMBERS = Object.freeze(['Icarus/Content/BP/Player/BP_IcarusPlayerControllerSurvival.uasset',
  'Icarus/Content/BP/Player/BP_IcarusPlayerControllerSurvival.uexp']);
const LIMITS = Object.freeze({entries: 131072, blocks: 1024, packages: 8, path: 4096, member: 64 * 1024 * 1024,
  totalPackage: PAK_LIMITS.package, totalIndex: 128 * 1024 * 1024,
  totalMember: 128 * 1024 * 1024});
const FAILURE = 'Unsupported or invalid controller package metadata';
function check(value) { if (!value) throw new Error(FAILURE); }
function integer(value, max = Number.MAX_SAFE_INTEGER) {
  check(Number.isSafeInteger(value) && value >= 0 && value <= max); return value;
}
function range(offset, length, end) {
  integer(offset); integer(length); integer(end); check(offset <= end && length <= end - offset);
}
function cursor(bytes) {
  let at = 0;
  return {u32() { range(at, 4, bytes.length); const n = bytes.readUInt32LE(at); at += 4; return n; },
    u64() { range(at, 8, bytes.length); const n = bytes.readBigUInt64LE(at); at += 8;
      check(n <= BigInt(Number.MAX_SAFE_INTEGER)); return Number(n); },
    get at() { return at; }};
}
function entryMetadata(encoded, location, methods, locations) {
  const c = cursor(encoded.subarray(location)), bits = c.u32();
  const code = bits & 63, blockSize = code === 63 ? c.u32() : code << 11;
  const methodIndex = (bits >>> 23) & 63, blockCount = (bits >>> 6) & 65535;
  check(!(bits & (1 << 22)));
  const offset = bits >>> 31 ? c.u32() : c.u64();
  const uncompressed = integer(bits & 0x40000000 ? c.u32() : c.u64(), LIMITS.member);
  const size = integer(methodIndex ? (bits & 0x20000000 ? c.u32() : c.u64()) : uncompressed, LIMITS.member);
  check(size > 0 && uncompressed > 0);
  const blockLengths = [];
  let method = 'None';
  if (!methodIndex) check(blockCount === 0 && blockSize === 0);
  else {
    check(methodIndex <= methods.length && methods[methodIndex - 1] !== '');
    method = methods[methodIndex - 1];
    // Independent implementation of the compact layout described in repak:
    // https://github.com/trumank/repak/blob/355b5f62f51959c7cc6dd5a51708646ef483065d/repak/src/entry.rs#L184-L314
    // Accepted profile: unencrypted, nonempty fixed-width output blocks except
    // a possibly shorter final block. No decompression is performed here.
    integer(blockCount, LIMITS.blocks); check(blockCount > 0);
    integer(blockSize, LIMITS.member); check(blockSize > 0);
    check(Math.ceil(uncompressed / blockSize) === blockCount);
    if (blockCount === 1) blockLengths.push(size);
    else {
      let total = 0;
      for (let i = 0; i < blockCount; i++) {
        const length = integer(c.u32(), LIMITS.member); check(length > 0);
        total += length; integer(total, size); blockLengths.push(length);
      }
      check(total === size);
    }
  }
  let next = encoded.length;
  for (const other of locations) if (other > location) next = Math.min(next, other);
  check(location + c.at === next);
  return {offset, size, uncompressed, methodIndex, method, blockSize, blockCount,
    encodedStart: location, encodedEnd: next, blockLengths, headerSize: methodIndex ? 57 + 16 * blockCount : 53};
}
function validateHeader(bytes, entry) {
  const c = cursor(bytes);
  check(c.u64() === 0 && c.u64() === entry.size && c.u64() === entry.uncompressed && c.u32() === entry.methodIndex);
  // SHA1 is a payload checksum; metadata inspection does not claim to verify it.
  const hashEnd = c.at + 20; range(c.at, 20, bytes.length);
  const rest = cursor(bytes.subarray(hashEnd));
  const blocks = [];
  if (entry.methodIndex) {
    check(rest.u32() === entry.blockCount);
    let expected = entry.headerSize;
    for (const length of entry.blockLengths) {
      const start = rest.u64(), end = rest.u64();
      check(start === expected && end === start + length);
      range(start, length, entry.headerSize + entry.size);
      blocks.push({start: entry.offset + start, end: entry.offset + end});
      expected = end;
    }
    check(expected === entry.headerSize + entry.size);
  }
  const flagsAt = hashEnd + rest.at;
  range(flagsAt, 5, bytes.length);
  check(bytes[flagsAt] === 0 && bytes.readUInt32LE(flagsAt + 1) === entry.blockSize && flagsAt + 5 === bytes.length);
  return blocks;
}
function selectControllerPackageMembers(packages, process) {
  const opened = [];
  try {
    check(Array.isArray(packages) && packages.length > 0 && packages.length <= LIMITS.packages);
    const selected = new Map(), privateMembers = new Map(), identities = new Set(), paths = new Set();
    let totalPackage = 0, totalIndex = 0, totalMember = 0;
    for (const input of packages) {
      check(input && typeof input.path === 'string' && input.path.length > 0 && input.path.length <= LIMITS.path &&
        !input.path.includes('\0') && typeof input.sha256 === 'string' && /^[a-f0-9]{64}$/.test(input.sha256));
      check(!paths.has(input.path)); paths.add(input.path);
      const fd = fs.openSync(input.path, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
      opened.push(fd);
      const initial = fs.fstatSync(fd, {bigint: true});
      check(initial.isFile() && initial.size <= BigInt(PAK_LIMITS.package));
      const identity = s => [s.dev, s.ino, s.size, s.mtimeNs, s.ctimeNs].join(':');
      const before = identity(initial), fileIdentity = `${initial.dev}:${initial.ino}`;
      check(!identities.has(fileIdentity)); identities.add(fileIdentity);
      const size = Number(initial.size); totalPackage += size; integer(totalPackage, LIMITS.totalPackage);
      const unchanged = () => check(identity(fs.fstatSync(fd, {bigint: true})) === before);
      const read = (offset, length) => {
        integer(length, PAK_LIMITS.index); range(offset, length, size); const bytes = Buffer.alloc(length);
        for (let at = 0; at < length;) { const n = fs.readSync(fd, bytes, at, length - at, offset + at); check(n > 0); at += n; }
        return bytes;
      };
      const verify = () => {
        unchanged(); const hash = crypto.createHash('sha256'), chunk = Buffer.alloc(1024 * 1024);
        for (let at = 0; at < size;) {
          const length = Math.min(chunk.length, size - at); let got = 0;
          while (got < length) { const n = fs.readSync(fd, chunk, got, length - got, at + got); check(n > 0); got += n; }
          hash.update(chunk.subarray(0, length)); at += length;
        }
        check(hash.digest('hex') === input.sha256); unchanged();
      };
      verify();
      const footer = parseFooter(read(size - FOOTER_SIZE, FOOTER_SIZE), size);
      totalIndex += footer.indexSize; integer(totalIndex, LIMITS.totalIndex);
      const declared = footer.methods.filter(method => method !== '');
      check(new Set(declared).size === declared.length && footer.methods.slice(0, declared.length).every(method => method !== ''));
      const primary = parsePrimaryIndexWithMount(read(footer.indexOffset, footer.indexSize), footer, {maxEntries: LIMITS.entries});
      const encodedOffset = footer.indexOffset + footer.indexSize - 4 - primary.encoded.length;
      for (const section of Object.values(primary.sections)) {
        totalIndex += section.size; integer(totalIndex, LIMITS.totalIndex);
        check(crypto.createHash('sha1').update(read(section.offset, section.size)).digest().equals(section.hash));
      }
      const directory = primary.sections.directory;
      const relativeMembers = new Set(MEMBERS.filter(member => member.startsWith(primary.mountPrefix))
        .map(member => member.slice(primary.mountPrefix.length)));
      const found = parseDirectoryIndex(read(directory.offset, directory.size), directory, primary, relativeMembers, {maxEntries: LIMITS.entries});
      const spans = [];
      for (const [relativeMember, location] of found.selected) {
        const member = primary.mountPrefix + relativeMember;
        check(MEMBERS.includes(member));
        check(!selected.has(member));
        const entry = entryMetadata(primary.encoded, location, footer.methods, found.locations);
        range(entry.offset, entry.headerSize + entry.size, footer.indexOffset);
        for (const span of spans) check(entry.offset + entry.headerSize + entry.size <= span.start || span.end <= entry.offset);
        spans.push({start: entry.offset, end: entry.offset + entry.headerSize + entry.size});
        totalMember += entry.uncompressed; integer(totalMember, LIMITS.totalMember);
        const header = read(entry.offset, entry.headerSize);
        const blocks = validateHeader(header, entry);
        privateMembers.set(member, {entry, read, checksum: Buffer.from(header.subarray(28, 48))});
        selected.set(member, {package_path: input.path, package_sha256: input.sha256, member_path: member,
          pak_version: 11, encoded_entry_range: {start: encodedOffset + entry.encodedStart, end: encodedOffset + entry.encodedEnd},
          header_range: {start: entry.offset, end: entry.offset + entry.headerSize},
          payload_range: {start: entry.offset + entry.headerSize, end: entry.offset + entry.headerSize + entry.size},
          compression_method: entry.method, compression_method_index: entry.methodIndex,
          compression_block_size: entry.blockSize, compression_blocks: blocks,
          declared_compressed_length: entry.size, declared_uncompressed_length: entry.uncompressed});
      }
      verify();
      // Keep descriptors open until all packages have been checked.
      opened[opened.length - 1] = {fd, unchanged, verify};
    }
    check(MEMBERS.every(member => selected.has(member)));
    for (const record of opened) record.unchanged();
    const metadata = {profile: 'icarus-controller-package-members', version: 1, metadata_only: true,
      package_hashes_verified: true, payload_checksums_verified: false, release_qualified: false,
      members: MEMBERS.map(member => selected.get(member))};
    const result = process ? process(metadata, privateMembers) : metadata;
    if (process) for (const record of opened) record.verify();
    for (const record of opened) record.unchanged();
    return result;
  } catch { throw new Error(FAILURE); }
  finally { for (const record of opened) fs.closeSync(typeof record === 'number' ? record : record.fd); }
}
function inspectControllerPackageMembers(packages) {
  return selectControllerPackageMembers(packages);
}
// decodeBlock(compressedBuffer, expectedOutputLength) must synchronously return
// an exact-size Buffer. The caller owns decoder execution/time isolation.
function readControllerClassMetadata(packages, decodeBlock, options = {}) {
  return selectControllerPackageMembers(packages, (metadata, members) => {
    check(decodeBlock === undefined || typeof decodeBlock === 'function');
    check(options && typeof options === 'object' && !Array.isArray(options));
    check(Object.keys(options).every(key => key === 'profile'));
    const profile = options.profile === undefined ? PROFILE : options.profile;
    check(profile === PROFILE || profile === ZERO_VERSION_PROFILE);
    const payloads = [];
    // Verify both payload checksums before handing any bytes to a decoder.
    for (const member of MEMBERS) {
      const {entry, read, checksum} = members.get(member);
      const cap = member.endsWith('.uasset') ? CLASS_LIMITS.uassetBytes : CLASS_LIMITS.uexpBytes;
      integer(entry.uncompressed, cap);
      check(entry.method === 'None' || (entry.method === 'Oodle' && typeof decodeBlock === 'function'));
      const payload = read(entry.offset + entry.headerSize, entry.size);
      check(crypto.createHash('sha1').update(payload).digest().equals(checksum));
      payloads.push(payload);
    }
    const decoded = MEMBERS.map((member, index) => {
      const {entry} = members.get(member), payload = payloads[index];
      if (entry.method === 'None') return payload;
      const output = Buffer.alloc(entry.uncompressed); let sourceAt = 0, outputAt = 0;
      for (const length of entry.blockLengths) {
        const expected = Math.min(entry.blockSize, entry.uncompressed - outputAt);
        const block = decodeBlock(Buffer.from(payload.subarray(sourceAt, sourceAt + length)), expected);
        check(Buffer.isBuffer(block) && !(block.buffer instanceof SharedArrayBuffer) && block.length === expected);
        block.copy(output, outputAt); sourceAt += length; outputAt += expected;
      }
      check(sourceAt === payload.length && outputAt === output.length); return output;
    });
    const counts = countCookedClassFields(decoded[0], decoded[1], {
      package: '/Game/BP/Player/BP_IcarusPlayerControllerSurvival',
      object_path: ['BP_IcarusPlayerControllerSurvival_C']}, {profile});
    return {...metadata, profile: 'icarus-controller-class-metadata', payload_checksums_verified: true, ...counts};
  });
}
module.exports = {MEMBERS, LIMITS, inspectControllerPackageMembers, readControllerClassMetadata};
