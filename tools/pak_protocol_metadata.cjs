'use strict';

// A narrow protocol-configuration reader, not a general asset extractor.
const fs = require('node:fs');
const crypto = require('node:crypto');
const zlib = require('node:zlib');
const {TextDecoder} = require('node:util');

const LIMITS = Object.freeze({index: 64 * 1024 * 1024, entries: 100000,
  member: 4 * 1024 * 1024, string: 4096, package: 64 * 1024 * 1024 * 1024});
const MEMBER = 'Icarus/Config/DefaultGameplayTags.ini';
const FOOTER_SIZE = 221;
const decoder = new TextDecoder('utf-8', {fatal: true});

function check(condition) {
  if (!condition) throw new Error('Unsupported or invalid protocol metadata');
}
function digest(bytes, algorithm = 'sha1') {
  return crypto.createHash(algorithm).update(bytes).digest();
}
function integer(value, max = Number.MAX_SAFE_INTEGER) {
  check(Number.isSafeInteger(value) && value >= 0 && value <= max);
  return value;
}
function range(offset, size, end) {
  integer(offset); integer(size); integer(end);
  check(offset <= end && size <= end - offset);
}

class Cursor {
  constructor(bytes) { check(Buffer.isBuffer(bytes)); this.bytes = bytes; this.at = 0; }
  take(size) { range(this.at, size, this.bytes.length); const result = this.bytes.subarray(this.at, this.at + size); this.at += size; return result; }
  u8() { return this.take(1)[0]; }
  u32() { return this.take(4).readUInt32LE(); }
  i32() { return this.take(4).readInt32LE(); }
  u64() { const n = this.take(8).readBigUInt64LE(); check(n <= BigInt(Number.MAX_SAFE_INTEGER)); return Number(n); }
  str() {
    const size = this.i32(); check(size > 0 && size <= LIMITS.string);
    const bytes = this.take(size); check(bytes[size - 1] === 0 && !bytes.subarray(0, -1).includes(0));
    return decoder.decode(bytes.subarray(0, -1));
  }
  eof() { check(this.at === this.bytes.length); }
}

function parseFooter(bytes, fileSize) {
  integer(fileSize, LIMITS.package); check(fileSize >= FOOTER_SIZE && bytes.length === FOOTER_SIZE);
  const c = new Cursor(bytes); check(c.take(16).every(x => x === 0) && c.u8() === 0);
  check(c.u32() === 0x5a6f12e1 && c.u32() === 11);
  const indexOffset = c.u64(), indexSize = integer(c.u64(), LIMITS.index), indexHash = c.take(20);
  range(indexOffset, indexSize, fileSize - FOOTER_SIZE); check(indexSize > 0);
  const methods = [];
  for (let i = 0; i < 5; i++) {
    const slot = c.take(32), end = slot.indexOf(0);
    check(end >= 0 && slot.subarray(end).every(x => x === 0));
    const name = decoder.decode(slot.subarray(0, end)); check(/^[A-Za-z0-9_]*$/.test(name)); methods.push(name);
  }
  c.eof(); return {indexOffset, indexSize, indexHash, methods, footerOffset: fileSize - FOOTER_SIZE};
}

function parsePrimaryIndex(bytes, footer) {
  check(bytes.length === footer.indexSize && digest(bytes).equals(footer.indexHash));
  const c = new Cursor(bytes), mount = c.str(); check(mount === '../../../');
  const count = integer(c.i32(), LIMITS.entries); c.u64();
  const sections = {};
  for (const kind of ['pathHash', 'directory']) {
    const present = c.u32(); check(present === 0 || present === 1);
    if (present) {
      const offset = c.u64(), size = integer(c.u64(), LIMITS.index), hash = c.take(20);
      check(size > 0 && offset >= footer.indexOffset + footer.indexSize);
      range(offset, size, footer.footerOffset); sections[kind] = {offset, size, hash};
    }
  }
  const encodedSize = integer(c.i32(), LIMITS.index), encoded = c.take(encodedSize);
  check(c.i32() === 0); c.eof(); check(sections.directory);
  if (sections.pathHash) {
    const a = sections.pathHash, b = sections.directory;
    check(a.offset + a.size <= b.offset || b.offset + b.size <= a.offset);
  }
  return {count, encoded, sections};
}

function parseFullDirectoryIndex(bytes, section, primary) {
  check(bytes.length === section.size && digest(bytes).equals(section.hash));
  const c = new Cursor(bytes), directories = integer(c.i32(), LIMITS.entries);
  const paths = new Set(), locations = new Set(); let selected, count = 0;
  for (let i = 0; i < directories; i++) {
    const parent = c.str(), n = integer(c.i32(), LIMITS.entries);
    // An empty root directory is a marker, never a selected member path.
    if (parent === '/') { check(n === 0); continue; }
    check(parent === '' || (parent.endsWith('/') && !parent.startsWith('/')));
    check(!parent.includes('\\') && !parent.split('/').some(s => s === '.' || s === '..'));
    check(n <= primary.count - count);
    for (let j = 0; j < n; j++) {
      const name = c.str(), location = c.i32();
      check(name !== '' && name !== '.' && name !== '..' && !/[\/\\]/.test(name));
      const path = parent + name; check(!paths.has(path)); paths.add(path);
      check(location >= 0 && location < primary.encoded.length && !locations.has(location)); locations.add(location);
      if (path === MEMBER) selected = location;
      count++;
    }
  }
  c.eof(); check(count === primary.count && selected !== undefined);
  return {location: selected, locations};
}

function decodeEntry(encoded, location, methods, locations) {
  integer(location); check(location < encoded.length);
  const c = new Cursor(encoded.subarray(location)), bits = c.u32();
  const sizeCode = bits & 63, blockSize = sizeCode === 63 ? c.u32() : sizeCode << 11;
  const methodIndex = (bits >>> 23) & 63, blockCount = (bits >>> 6) & 65535;
  check(((bits >>> 22) & 1) === 0);
  const offset = bits >>> 31 ? c.u32() : c.u64();
  const uncompressed = integer((bits & 0x40000000) ? c.u32() : c.u64(), LIMITS.member);
  const size = integer(methodIndex === 0 ? uncompressed : ((bits & 0x20000000) ? c.u32() : c.u64()), LIMITS.member);
  check(size > 0 && uncompressed > 0);
  let method = 'None';
  if (methodIndex === 0) check(blockCount === 0 && blockSize === 0);
  else {
    check(methodIndex <= methods.length && methods[methodIndex - 1] === 'Zlib');
    check(blockCount === 1 && blockSize >= uncompressed && blockSize <= LIMITS.member); method = 'Zlib';
  }
  const end = location + c.at; let next = encoded.length;
  for (const other of locations) if (other > location) next = Math.min(next, other);
  check(end === next);
  return {offset, size, uncompressed, methodIndex, method, blockSize, headerSize: methodIndex ? 73 : 53};
}

function decodeMember(header, payload, entry) {
  check(header.length === entry.headerSize && payload.length === entry.size);
  const c = new Cursor(header); check(c.u64() === 0 && c.u64() === entry.size && c.u64() === entry.uncompressed);
  check(c.u32() === entry.methodIndex); const hash = c.take(20);
  if (entry.methodIndex) {
    check(c.u32() === 1 && c.u64() === entry.headerSize && c.u64() === entry.headerSize + entry.size);
  }
  check(c.u8() === 0 && c.u32() === entry.blockSize); c.eof();
  check(digest(payload).equals(hash));
  if (entry.method === 'None') return payload;
  const result = zlib.inflateSync(payload, {maxOutputLength: LIMITS.member, info: true});
  check(result.engine.bytesWritten === payload.length && result.buffer.length === entry.uncompressed);
  return result.buffer;
}

function decodeGameplayTagsIni(bytes) {
  check(bytes.length <= LIMITS.member && !bytes.includes(0));
  const text = decoder.decode(bytes).replace(/^\uFEFF/, ''), settings = {}, tags = [];
  const booleans = new Set(['ImportTagsFromConfig', 'FastReplication']);
  const widths = new Set(['NumBitsForContainerSize', 'NetIndexFirstBitSegment']);
  const seen = new Set(); let selected = false, foundSection = false;
  let externalSourceDeclarations = 0, unresolvedSources = 0, redirects = 0, unrecognizedSettings = 0;
  for (const original of text.split(/\r?\n/)) {
    const line = original.trim(); check(line.length <= 16384);
    if (!line || line.startsWith(';') || line.startsWith('#')) continue;
    if (line.startsWith('[')) {
      check(/^\[[^\[\]\r\n]+\]$/.test(line));
      selected = line === '[/Script/GameplayTags.GameplayTagsSettings]'; foundSection ||= selected; continue;
    }
    if (!selected) continue;
    const split = line.indexOf('='); check(split > 0);
    const key = line.slice(0, split).trim(), value = line.slice(split + 1).trim();
    if (booleans.has(key)) { check(/^(true|false)$/i.test(value)); settings[key] = value.toLowerCase() === 'true'; }
    else if (widths.has(key)) { check(/^\d{1,2}$/.test(value)); settings[key] = integer(Number(value), 32); check(key === 'NetIndexFirstBitSegment' || settings[key] > 0); }
    else if (key === '+GameplayTagList') {
      const match = /^\(\s*Tag\s*=\s*"([^"\\\r\n]+)"\s*(?:,\s*DevComment\s*=\s*"(?:[^"\\\r\n]|\\[^\r\n])*"\s*)?\)$/.exec(value);
      check(match && match[1].length <= 1024 && /^[\x20-\x7e]+$/.test(match[1]) &&
        match[1].split('.').every(segment => segment.length > 0));
      if (!seen.has(match[1])) { seen.add(match[1]); tags.push(match[1]); check(tags.length <= LIMITS.entries); }
    } else if (key === '+GameplayTagTableList') {
      // Only count references; never follow or export original asset paths.
      externalSourceDeclarations++; if (value !== '' && value !== 'None') unresolvedSources++;
    } else if (key === '+GameplayTagRedirects') {
      check(/^\(\s*OldTagName\s*=\s*"[A-Za-z0-9_.]+"\s*,\s*NewTagName\s*=\s*"[A-Za-z0-9_.]+"\s*\)$/.test(value));
      redirects++;
    } else {
      check(/^[+!.-]?[A-Za-z][A-Za-z0-9_]*$/.test(key));
      check(!/^[+!\-.]?(GameplayTagList|GameplayTagTableList|GameplayTagRedirects)$/.test(key));
      unrecognizedSettings++;
    }
  }
  check(foundSection);
  return {settings, tag_names: tags, external_source_declaration_count: externalSourceDeclarations,
    unresolved_source_count: unresolvedSources, redirects_count: redirects,
    unrecognized_setting_count: unrecognizedSettings, net_index_mapping_complete: false};
}

function inspectProtocolMetadata(path, {sha256} = {}) {
  check(typeof path === 'string' && path.length > 0 && typeof sha256 === 'string' && /^[a-f0-9]{64}$/.test(sha256));
  let fd;
  try {
    fd = fs.openSync(path, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW | fs.constants.O_NONBLOCK);
    const initial = fs.fstatSync(fd, {bigint: true}); check(initial.isFile() && initial.size <= BigInt(LIMITS.package));
    const size = Number(initial.size); integer(size);
    const identity = stat => [stat.dev, stat.ino, stat.size, stat.mtimeNs, stat.ctimeNs].join(':');
    const before = identity(initial);
    const unchanged = () => { check(identity(fs.fstatSync(fd, {bigint: true})) === before); };
    const read = (offset, length) => {
      integer(length, LIMITS.index); range(offset, length, size); const bytes = Buffer.alloc(length);
      for (let at = 0; at < length;) { const got = fs.readSync(fd, bytes, at, length - at, offset + at); check(got > 0); at += got; }
      return bytes;
    };
    const verify = () => {
      unchanged(); const hash = crypto.createHash('sha256'), chunk = Buffer.alloc(1024 * 1024);
      for (let offset = 0; offset < size;) {
        const length = Math.min(chunk.length, size - offset); let got = 0;
        while (got < length) { const n = fs.readSync(fd, chunk, got, length - got, offset + got); check(n > 0); got += n; }
        hash.update(chunk.subarray(0, length)); offset += length;
      }
      check(hash.digest('hex') === sha256); unchanged();
    };
    verify();
    const footer = parseFooter(read(size - FOOTER_SIZE, FOOTER_SIZE), size);
    const primary = parsePrimaryIndex(read(footer.indexOffset, footer.indexSize), footer);
    for (const section of Object.values(primary.sections)) check(digest(read(section.offset, section.size)).equals(section.hash));
    const directory = primary.sections.directory;
    const selected = parseFullDirectoryIndex(read(directory.offset, directory.size), directory, primary);
    const entry = decodeEntry(primary.encoded, selected.location, footer.methods, selected.locations);
    range(entry.offset, entry.headerSize + entry.size, footer.indexOffset);
    const configuration = decodeGameplayTagsIni(decodeMember(read(entry.offset, entry.headerSize), read(entry.offset + entry.headerSize, entry.size), entry));
    verify();
    return {profile: 'icarus-declared-gameplay-tag-configuration', version: 1,
      source_sha256: sha256, pak_version: 11, hashes_verified: true,
      effective_runtime_settings: false, declared_configuration: configuration};
  } catch {
    throw new Error('Unsupported or invalid protocol metadata');
  } finally { if (fd !== undefined) fs.closeSync(fd); }
}

module.exports = {LIMITS, MEMBER, FOOTER_SIZE, parseFooter, parsePrimaryIndex,
  parseFullDirectoryIndex, decodeEntry, decodeMember, decodeGameplayTagsIni, inspectProtocolMetadata};

if (require.main === module) {
  try {
    const args = process.argv.slice(2); check(args.length === 4 && args[0] === '--path' && args[2] === '--sha256');
    process.stdout.write(JSON.stringify(inspectProtocolMetadata(args[1], {sha256: args[3]})) + '\n');
  } catch { process.stderr.write('Protocol metadata inspection failed.\n'); process.exitCode = 1; }
}
