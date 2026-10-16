'use strict';

// All package bytes here are authored fixtures. No installed game inputs.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const zlib = require('node:zlib');
const {spawnSync} = require('node:child_process');
const {LIMITS, MEMBER, FOOTER_SIZE, parseFooter, parsePrimaryIndex,
  parseFullDirectoryIndex, decodeEntry, decodeMember, decodeGameplayTagsIni,
  inspectProtocolMetadata} = require('../pak_protocol_metadata.cjs');

const hash = (b, type = 'sha1') => crypto.createHash(type).update(b).digest();
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32LE(n >>> 0); return b; };
const u64 = n => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const str = s => { const b = Buffer.from(s + '\0'); return Buffer.concat([u32(b.length), b]); };
const ini = '[Unrelated]\nPrivateValue=never-export-this\n' +
  '[/Script/GameplayTags.GameplayTagsSettings]\nImportTagsFromConfig=True\nFastReplication=False\n' +
  'NumBitsForContainerSize=6\nNetIndexFirstBitSegment=16\n' +
  '+GameplayTagList=(Tag="Fixture.One",DevComment="private developer comment")\n' +
  '+GameplayTagList=(Tag="Fixture.Two")\n';

function fixture({compressed = true, text = ini, method = 'Zlib', member = MEMBER, pathHash = false, rootMarker = false} = {}) {
  const plain = Buffer.from(text), payload = compressed ? zlib.deflateSync(plain) : plain;
  const methodIndex = compressed ? 1 : 0, headerSize = compressed ? 73 : 53;
  const header = Buffer.concat([u64(0), u64(payload.length), u64(plain.length), u32(methodIndex), hash(payload),
    ...(compressed ? [u32(1), u64(headerSize), u64(headerSize + payload.length)] : []),
    Buffer.from([0]), u32(compressed ? plain.length : 0)]);
  const bits = 0xc0000000 | (compressed ? 0x20000000 | (1 << 23) | (1 << 6) | 63 : 0);
  const encoded = Buffer.concat([u32(bits), ...(compressed ? [u32(plain.length)] : []),
    u32(0), u32(plain.length), ...(compressed ? [u32(payload.length)] : [])]);
  const slash = member.lastIndexOf('/');
  const directory = Buffer.concat([u32(rootMarker ? 2 : 1), ...(rootMarker ? [str('/'), u32(0)] : []),
    str(member.slice(0, slash + 1)), u32(1), str(member.slice(slash + 1)), u32(0)]);
  const primaryAt = header.length + payload.length, extra = Buffer.from('authored path-hash fixture');
  const primary = fullOffset => Buffer.concat([str('../../../'), u32(1), u64(0),
    ...(pathHash ? [u32(1), u64(fullOffset + directory.length), u64(extra.length), hash(extra)] : [u32(0)]),
    u32(1), u64(fullOffset), u64(directory.length), hash(directory), u32(encoded.length), encoded, u32(0)]);
  const index = primary(primaryAt + primary(0).length);
  const footer = Buffer.alloc(FOOTER_SIZE); footer.writeUInt32LE(0x5a6f12e1, 17); footer.writeUInt32LE(11, 21);
  footer.writeBigUInt64LE(BigInt(primaryAt), 25); footer.writeBigUInt64LE(BigInt(index.length), 33);
  hash(index).copy(footer, 41); footer.write(method, 61);
  const bytes = Buffer.concat([header, payload, index, directory, ...(pathHash ? [extra] : []), footer]);
  return {bytes, footer, index, directory, encoded, header, payload, plain, primaryAt};
}

function withPackage(t, bytes) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'authored-pak-protocol-'));
  t.after(() => fs.rmSync(dir, {recursive: true, force: true}));
  const file = path.join(dir, 'fixture.pak'); fs.writeFileSync(file, bytes);
  return {file, dir, sha256: hash(bytes, 'sha256').toString('hex')};
}
function parsed(f) {
  const footer = parseFooter(f.footer, f.bytes.length), primary = parsePrimaryIndex(f.index, footer);
  const selected = parseFullDirectoryIndex(f.directory, primary.sections.directory, primary);
  return {footer, primary, selected};
}

test('authored Zlib package emits declared protocol fields and no other config', t => {
  const f = fixture({pathHash: true}), input = withPackage(t, f.bytes);
  const result = inspectProtocolMetadata(input.file, input);
  assert.equal(result.effective_runtime_settings, false);
  assert.equal(result.hashes_verified, true);
  assert.deepEqual(result.declared_configuration, {settings: {ImportTagsFromConfig: true,
    FastReplication: false, NumBitsForContainerSize: 6, NetIndexFirstBitSegment: 16},
  tag_names: ['Fixture.One', 'Fixture.Two'], ascii_case_variant_count: 0, external_source_declaration_count: 0,
  unresolved_source_count: 0, redirects_count: 0, unrecognized_setting_count: 0,
  net_index_mapping_complete: false});
  const output = JSON.stringify(result); assert(!output.includes('private')); assert(!output.includes(input.file));
});

test('authored uncompressed package and pure parsing helpers agree', t => {
  const f = fixture({compressed: false}), input = withPackage(t, f.bytes), p = parsed(f);
  const entry = decodeEntry(p.primary.encoded, p.selected.location, p.footer.methods, p.selected.locations);
  assert.equal(entry.method, 'None'); assert.deepEqual(decodeMember(f.header, f.payload, entry), f.plain);
  assert.deepEqual(inspectProtocolMetadata(input.file, input).declared_configuration.tag_names, ['Fixture.One', 'Fixture.Two']);
});

test('footer refuses unknown versions, encryption, truncation, invalid numbers and index bounds', () => {
  const f = fixture();
  for (const mutate of [b => b.writeUInt32LE(10, 21), b => { b[16] = 1; }, b => { b[0] = 1; },
    b => b.writeBigUInt64LE(2n ** 63n, 25), b => b.writeBigUInt64LE(BigInt(LIMITS.index + 1), 33),
    b => b.writeBigUInt64LE(BigInt(f.bytes.length), 25), b => { b[92] = 1; }]) {
    const bytes = Buffer.from(f.footer); mutate(bytes); assert.throws(() => parseFooter(bytes, f.bytes.length));
  }
  assert.throws(() => parseFooter(f.footer.subarray(1), f.bytes.length));
  assert.throws(() => parseFooter(f.footer, LIMITS.package + 1));
});

test('primary and full directory require valid SHA1 and exact EOF', () => {
  const f = fixture(), p = parsed(f), corrupt = Buffer.from(f.index); corrupt[0] ^= 1;
  assert.throws(() => parsePrimaryIndex(corrupt, p.footer));
  const extra = Buffer.concat([f.index, Buffer.from([0])]);
  assert.throws(() => parsePrimaryIndex(extra, {...p.footer, indexSize: extra.length, indexHash: hash(extra)}));
  const directory = Buffer.from(f.directory); directory[4] ^= 1;
  assert.throws(() => parseFullDirectoryIndex(directory, p.primary.sections.directory, p.primary));
  const trailing = Buffer.concat([f.directory, Buffer.from([0])]);
  assert.throws(() => parseFullDirectoryIndex(trailing, {size: trailing.length, hash: hash(trailing)}, p.primary));
});

test('directory rejects traversal, absent allowlisted member, deleted and overlapping locations', () => {
  for (const member of ['Other/DefaultGameplayTags.ini', '../Icarus/Config/DefaultGameplayTags.ini']) {
    const f = fixture({member}), footer = parseFooter(f.footer, f.bytes.length), primary = parsePrimaryIndex(f.index, footer);
    assert.throws(() => parseFullDirectoryIndex(f.directory, primary.sections.directory, primary));
  }
  const f = fixture(), p = parsed(f), bytes = Buffer.from(f.directory); bytes.writeInt32LE(-1, bytes.length - 4);
  assert.throws(() => parseFullDirectoryIndex(bytes, {size: bytes.length, hash: hash(bytes)}, p.primary));
  assert.throws(() => decodeEntry(f.encoded, 0, ['Zlib'], new Set([0, 4])));
});

test('directory permits an empty root marker and refuses root entries or absolute member parents', t => {
  const f = fixture({rootMarker: true}), input = withPackage(t, f.bytes), p = parsed(f);
  assert.equal(inspectProtocolMetadata(input.file, input).declared_configuration.tag_names.length, 2);
  const populatedRoot = Buffer.from(f.directory); populatedRoot.writeUInt32LE(1, 10);
  assert.throws(() => parseFullDirectoryIndex(populatedRoot,
    {size: populatedRoot.length, hash: hash(populatedRoot)}, p.primary));
  const absolute = fixture({member: '/' + MEMBER});
  const footer = parseFooter(absolute.footer, absolute.bytes.length), primary = parsePrimaryIndex(absolute.index, footer);
  assert.throws(() => parseFullDirectoryIndex(absolute.directory, primary.sections.directory, primary));
});

test('entry rejects encrypted, unsupported compression, multiblock and oversized members', () => {
  const f = fixture();
  for (const change of [bits => bits | (1 << 22), bits => (bits & ~(63 << 23)) | (2 << 23),
    bits => (bits & ~(65535 << 6)) | (2 << 6)]) {
    const encoded = Buffer.from(f.encoded); encoded.writeUInt32LE(change(encoded.readUInt32LE()) >>> 0);
    assert.throws(() => decodeEntry(encoded, 0, ['Zlib', 'Oodle'], new Set([0])));
  }
  const encoded = Buffer.from(f.encoded); encoded.writeUInt32LE(LIMITS.member + 1, 12);
  assert.throws(() => decodeEntry(encoded, 0, ['Zlib'], new Set([0])));
  assert.throws(() => decodeEntry(f.encoded, 0, ['Oodle'], new Set([0])));
  assert.throws(() => decodeEntry(Buffer.concat([f.encoded, Buffer.from([0])]), 0, ['Zlib'], new Set([0])));
  assert.throws(() => decodeEntry(f.encoded.subarray(0, -1), 0, ['Zlib'], new Set([0])));
});

test('selected member requires matching header, compressed SHA1 and exact zlib stream', () => {
  const f = fixture(), p = parsed(f);
  const entry = decodeEntry(f.encoded, 0, p.footer.methods, new Set([0]));
  for (const offset of [0, 8, 16, 24, 28, 48, 52, 60, 68, 69]) {
    const header = Buffer.from(f.header); header[offset] ^= 1;
    assert.throws(() => decodeMember(header, f.payload, entry));
  }
  const payload = Buffer.from(f.payload); payload[0] ^= 1;
  assert.throws(() => decodeMember(f.header, payload, entry));
  const trailing = Buffer.concat([f.payload, Buffer.from([0])]), header = Buffer.from(f.header);
  header.writeBigUInt64LE(BigInt(trailing.length), 8); hash(trailing).copy(header, 28);
  header.writeBigUInt64LE(BigInt(entry.headerSize + trailing.length), 60);
  assert.throws(() => decodeMember(header, trailing, {...entry, size: trailing.length}));
  const wrong = {...entry, uncompressed: entry.uncompressed + 1}, wrongHeader = Buffer.from(f.header);
  wrongHeader.writeBigUInt64LE(BigInt(wrong.uncompressed), 16);
  assert.throws(() => decodeMember(wrongHeader, f.payload, wrong));
});

test('selected zlib expansion has a hard output bound', () => {
  const f = fixture(), p = parsed(f), entry = decodeEntry(f.encoded, 0, p.footer.methods, new Set([0]));
  const payload = zlib.deflateSync(Buffer.alloc(LIMITS.member + 1)), header = Buffer.from(f.header);
  header.writeBigUInt64LE(BigInt(payload.length), 8); hash(payload).copy(header, 28);
  header.writeBigUInt64LE(BigInt(entry.headerSize + payload.length), 60);
  assert.throws(() => decodeMember(header, payload, {...entry, size: payload.length}));
});

test('config filters comments, handles BOM and duplicates, refuses ambiguous list operations and malformed fields', () => {
  const profile = decodeGameplayTagsIni(Buffer.from('\uFEFF; comment\r\n' + ini + '+GameplayTagList=(Tag="Fixture.One")\n'));
  assert.deepEqual(profile.tag_names, ['Fixture.One', 'Fixture.Two']);
  for (const text of [ini + '!GameplayTagList=ClearArray', ini + '-GameplayTagList=(Tag="Fixture.One")',
    ini + 'FastReplication=perhaps', ini + 'NetIndexFirstBitSegment=100', ini + '+GameplayTagList=(Tag="Fixture..Invalid")',
    ini + '+GameplayTagList=(Tag="Fixture.Three",DevComment="unterminated)', ini + '\0', '[Unrelated]\nValue=yes']) {
    assert.throws(() => decodeGameplayTagsIni(Buffer.from(text)));
  }
  assert.throws(() => decodeGameplayTagsIni(Buffer.from([0xff])));
});

test('declared names allow printable ASCII and ignored comments support general escapes', () => {
  const declaration = String.raw`+GameplayTagList=(Tag="Fixture.Cost$",DevComment="line\n apostrophe\' tab\t quote\" slash\\")`;
  const profile = decodeGameplayTagsIni(Buffer.from(ini + declaration + '\n'));
  assert.deepEqual(profile.tag_names, ['Fixture.One', 'Fixture.Two', 'Fixture.Cost$']);
  assert.equal(profile.net_index_mapping_complete, false);
  assert(!JSON.stringify(profile).includes('apostrophe'));
  const printable = decodeGameplayTagsIni(Buffer.from(ini + '+GameplayTagList=(Tag="Fixture.A value-$")\n'));
  assert.equal(printable.tag_names.at(-1), 'Fixture.A value-$');
  for (const malformed of [
    String.raw`+GameplayTagList=(Tag="Fixture.Bad",DevComment="dangling\")`,
    String.raw`+GameplayTagList=(Tag="Fixture.Bad",DevComment="missing close)`,
    '+GameplayTagList=(Tag="Fixture.Bad",DevComment="actual\nnewline")',
    '+GameplayTagList=(Tag=".Leading")', '+GameplayTagList=(Tag="Trailing.")',
    '+GameplayTagList=(Tag="Fixture.\u007f")',
  ]) assert.throws(() => decodeGameplayTagsIni(Buffer.from(ini + malformed)));
});

test('external sources, redirects and unknown settings cannot imply a complete runtime tag map', () => {
  const profile = decodeGameplayTagsIni(Buffer.from(ini + '+GameplayTagTableList=\n' +
    '+GameplayTagTableList=/Private/Source.NoExport\n' +
    '+GameplayTagRedirects=(OldTagName="Fixture.Old",NewTagName="Fixture.New")\n' +
    'UnknownTagInfluencingSetting=private-value\n'));
  assert.equal(profile.external_source_declaration_count, 2); assert.equal(profile.unresolved_source_count, 1);
  assert.equal(profile.redirects_count, 1); assert.equal(profile.unrecognized_setting_count, 1);
  assert.equal(profile.net_index_mapping_complete, false);
  assert(!JSON.stringify(profile).includes('Private')); assert(!JSON.stringify(profile).includes('private-value'));
  const externalOnly = decodeGameplayTagsIni(Buffer.from('[/Script/GameplayTags.GameplayTagsSettings]\n+GameplayTagTableList=/Private/Source\n'));
  assert.deepEqual(externalOnly.tag_names, []); assert.equal(externalOnly.unresolved_source_count, 1);
  assert.throws(() => decodeGameplayTagsIni(Buffer.from(ini + '!GameplayTagTableList=ClearArray')));
});

test('zero first-segment declaration preserves the codec unsplit setting', () => {
  const profile = decodeGameplayTagsIni(Buffer.from(ini.replace('NetIndexFirstBitSegment=16', 'NetIndexFirstBitSegment=0')));
  assert.equal(profile.settings.NetIndexFirstBitSegment, 0);
  assert.throws(() => decodeGameplayTagsIni(Buffer.from(ini.replace('NumBitsForContainerSize=6', 'NumBitsForContainerSize=0'))));
});

test('declaration widths remain observable outside the native codec input profile', () => {
  const profile = decodeGameplayTagsIni(Buffer.from(ini
    .replace('NetIndexFirstBitSegment=16', 'NetIndexFirstBitSegment=32')
    .replace('NumBitsForContainerSize=6', 'NumBitsForContainerSize=32')));
  assert.equal(profile.settings.NetIndexFirstBitSegment, 32);
  assert.equal(profile.settings.NumBitsForContainerSize, 32);
  assert.equal(profile.net_index_mapping_complete, false);
  for (const key of ['NetIndexFirstBitSegment', 'NumBitsForContainerSize']) {
    assert.throws(() => decodeGameplayTagsIni(Buffer.from(ini + key + '=33\n')));
  }
});

test('case variants are counted without silently changing declared spellings or order', () => {
  const profile = decodeGameplayTagsIni(Buffer.from(ini +
    '+GameplayTagList=(Tag="fixture.ONE")\n' +
    '+GameplayTagList=(Tag="FIXTURE.One")\n' +
    '+GameplayTagList=(Tag="fixture.ONE")\n'));
  assert.deepEqual(profile.tag_names, ['Fixture.One', 'Fixture.Two', 'fixture.ONE', 'FIXTURE.One']);
  assert.equal(profile.ascii_case_variant_count, 2);
  assert.equal(profile.net_index_mapping_complete, false);
});

test('whole-file pins, primary/directory/path-hash and payload corruption fail generically', t => {
  const f = fixture({pathHash: true});
  const input = withPackage(t, f.bytes);
  assert.throws(() => inspectProtocolMetadata(input.file, {sha256: '0'.repeat(64)}), /^Error: Unsupported or invalid protocol metadata$/);
  for (const offset of [53, f.header.length, f.primaryAt, f.primaryAt + f.index.length,
    f.bytes.length - FOOTER_SIZE - 1]) {
    const bytes = Buffer.from(f.bytes); bytes[offset] ^= 1; fs.writeFileSync(input.file, bytes);
    assert.throws(() => inspectProtocolMetadata(input.file, {sha256: hash(bytes, 'sha256').toString('hex')}),
      /^Error: Unsupported or invalid protocol metadata$/);
  }
});

test('symlinks, directories, FIFO and CLI errors reveal no paths or content', t => {
  const f = fixture(), input = withPackage(t, f.bytes), link = path.join(input.dir, 'sensitive-link');
  fs.symlinkSync(input.file, link);
  assert.throws(() => inspectProtocolMetadata(link, input));
  assert.throws(() => inspectProtocolMetadata(input.dir, input));
  const fifo = path.join(input.dir, 'sensitive-fifo');
  assert.equal(spawnSync('mkfifo', [fifo]).status, 0);
  const run = spawnSync(process.execPath, [path.resolve(__dirname, '../pak_protocol_metadata.cjs'),
    '--path', fifo, '--sha256', input.sha256], {encoding: 'utf8', timeout: 2000});
  assert.equal(run.status, 1); assert.equal(run.stdout, ''); assert.equal(run.stderr, 'Protocol metadata inspection failed.\n');
});

test('mutation during initial full-file hash is rejected', t => {
  const f = fixture(), input = withPackage(t, f.bytes), original = fs.readSync;
  let changed = false;
  fs.readSync = function(fd, buffer, offset, length, position) {
    const got = original(fd, buffer, offset, length, position);
    if (!changed && position === 0 && length === f.bytes.length) {
      changed = true; const bytes = Buffer.from(f.bytes); bytes[0] ^= 1; fs.writeFileSync(input.file, bytes);
    }
    return got;
  };
  try { assert.throws(() => inspectProtocolMetadata(input.file, input)); }
  finally { fs.readSync = original; }
  assert.equal(changed, true);
});
