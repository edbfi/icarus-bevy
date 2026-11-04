'use strict';
// Authored package fixtures only; arbitrary payload bytes are never decoded.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const crypto = require('node:crypto');
const {MEMBERS, LIMITS, inspectControllerPackageMembers: inspect, readControllerClassMetadata: readClass} = require('../controller_package_members.cjs');
const {FOOTER_SIZE} = require('../pak_protocol_metadata.cjs');
const {PROFILE, ZERO_VERSION_PROFILE} = require('../cooked_class_field_counts.cjs');
const hash = (b, algorithm = 'sha1') => crypto.createHash(algorithm).update(b).digest();
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32LE(n >>> 0); return b; };
const u64 = n => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const str = s => { const b = Buffer.from(s + '\0'); return Buffer.concat([u32(b.length), b]); };
function fixture({members = MEMBERS, method = 'Oodle', compressed = true, badHeader = false,
  overlap = false, count = 1, wide = false, fault = '', mount = '../../../',
  parent = 'Icarus/Content/BP/Player/', extraEntries = 0, payloads, outputLengths, outputBlockSizes} = {}) {
  const headers = [], entries = [], locations = []; let offset = 0, encodedAt = 0;
  for (let i = 0; i < members.length; i++) {
    const payload = payloads?.[i] ?? Buffer.from('private-original-payload-never-export');
    const headerSize = compressed ? 57 + 16 * count : 53, size = payload.length;
    const lengths = Array.from({length: count}, (_, n) => Math.floor(size / count) + (n < size % count ? 1 : 0));
    const blockSize = outputBlockSizes?.[i] ?? (count === 1 ? (outputLengths?.[i] ?? size) : 16);
    const uncompressed = compressed ? (outputLengths?.[i] ?? blockSize * (count - 1) + 7) : size;
    const physical = []; let at = headerSize;
    for (let j = 0; j < count; j++) {
      physical.push(u64(fault === 'header-start' && j === 1 ? at - 1 : at),
        u64(fault === 'header-end' && j === count - 1 ? headerSize + size + 1 : at + lengths[j]));
      at += lengths[j];
    }
    const header = Buffer.concat([u64(badHeader ? 1 : 0), u64(size), u64(uncompressed), u32(compressed ? 1 : 0), hash(payload),
      ...(compressed ? [u32(count), ...physical] : []), Buffer.from([0]), u32(compressed ? blockSize : 0)]);
    let encodedLengths = lengths.map((n, j) => u32(j === 0 && fault === 'zero' ? 0 :
      j === 0 && fault === 'overflow' ? 0xffffffff : j === 0 && fault === 'sum' ? n + 1 : n));
    if (fault === 'truncated') encodedLengths = encodedLengths.slice(0, -1);
    const bits = (wide ? 0 : 0xc0000000) | (compressed ? (wide ? 0 : 0x20000000) |
      (1 << 23) | ((fault === 'count-cap' ? LIMITS.blocks + 1 : count) << 6) | 63 : 0) |
      (fault === 'encrypted' ? 1 << 22 : 0);
    const word = wide ? u64 : u32;
    const entry = Buffer.concat([u32(bits), ...(compressed ? [u32(fault === 'geometry' ? 1 : blockSize)] : []),
      word(fault === 'containment' ? 0x100000 : overlap ? 0 : offset), word(uncompressed),
      ...(compressed ? [word(size)] : []), ...(compressed && count > 1 ? encodedLengths : [])]);
    locations.push(encodedAt); encodedAt += entry.length; entries.push(entry); headers.push(header, payload); offset += header.length + size;
  }
  // Unselected authored entries need distinct encoded locations but no payload.
  const extras = Array.from({length: extraEntries}, (_, i) => [str(`Fixture${i}.unused`), u32(encodedAt + i)]);
  const encoded = Buffer.concat([...entries, Buffer.alloc(extraEntries)]), directory = Buffer.concat([u32(1), str(parent), u32(members.length + extraEntries),
    ...members.flatMap((m, i) => [str(m.slice(m.lastIndexOf('/') + 1)), u32(locations[i])]), ...extras.flat()]);
  const primary = directoryAt => Buffer.concat([str(mount), u32(members.length + extraEntries), u64(0), u32(0),
    u32(1), u64(directoryAt), u64(directory.length), hash(directory), u32(encoded.length), encoded, u32(0)]);
  const index = primary(offset + primary(0).length), footer = Buffer.alloc(FOOTER_SIZE);
  footer.writeUInt32LE(0x5a6f12e1, 17); footer.writeUInt32LE(11, 21); footer.writeBigUInt64LE(BigInt(offset), 25);
  footer.writeBigUInt64LE(BigInt(index.length), 33); hash(index).copy(footer, 41); footer.write(method, 61);
  return Buffer.concat([...headers, index, directory, footer]);
}
function input(t, bytes) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'controller-authored-'));
  t.after(() => fs.rmSync(dir, {recursive: true, force: true}));
  const file = path.join(dir, 'fixture.pak'); fs.writeFileSync(file, bytes);
  return {path: file, sha256: hash(bytes, 'sha256').toString('hex')};
}
test('exact two members yield metadata, declared Oodle and exclusive ranges without payload output', t => {
  const source = input(t, fixture()), result = inspect([source]);
  assert.equal(result.output_metadata_only, true); assert.equal(result.release_qualified, false);
  assert.equal(result.payload_checksums_verified, false);
  assert.deepEqual(result.members.map(m => m.member_path), MEMBERS);
  for (const m of result.members) {
    assert.equal(m.package_path, source.path); assert.equal(m.package_sha256, source.sha256);
    assert.equal(m.compression_method, 'Oodle'); assert.equal(m.compression_blocks.length, 1);
    assert.equal(m.payload_range.end - m.payload_range.start, m.declared_compressed_length);
  }
  assert(!JSON.stringify(result).includes('private-original'));
});
test('uncompressed and split explicit packages are supported', t => {
  const a = input(t, fixture({members: [MEMBERS[0]], compressed: false}));
  const b = input(t, fixture({members: [MEMBERS[1]]}));
  assert.deepEqual(inspect([a, b]).members.map(m => m.compression_method), ['None', 'Oodle']);
});
test('missing, duplicate, basename substitute and cross-package ambiguity fail closed', t => {
  for (const members of [[MEMBERS[0]], [MEMBERS[0], MEMBERS[0]], ['Other/BP_IcarusPlayerControllerSurvival.uasset']]) {
    assert.throws(() => inspect([input(t, fixture({members}))]));
  }
  const a = input(t, fixture()), b = input(t, fixture());
  assert.throws(() => inspect([a, b])); assert.throws(() => inspect([a, a]));
});
test('hash, malformed physical header and overlapping selected spans fail', t => {
  const a = input(t, fixture()); assert.throws(() => inspect([{...a, sha256: '0'.repeat(64)}]));
  for (const options of [{badHeader: true}, {overlap: true}, {method: ''}]) {
    assert.throws(() => inspect([input(t, fixture(options))]));
  }
  const bytes = fixture(); bytes[bytes.length - FOOTER_SIZE + 25] ^= 1;
  assert.throws(() => inspect([input(t, bytes)]));
});
test('bounds and nonregular inputs produce only a generic error', t => {
  const a = input(t, fixture()), link = a.path + '.link'; fs.symlinkSync(a.path, link);
  for (const args of [[], Array(LIMITS.packages + 1).fill(a), [{...a, path: link}],
    [{...a, path: path.dirname(a.path)}], [{...a, path: 'x'.repeat(LIMITS.path + 1)}]]) {
    assert.throws(() => inspect(args), /^Error: Unsupported or invalid controller package metadata$/);
  }
});

test('two and three blocks preserve exact ranges under both compact integer widths', t => {
  for (const count of [2, 3]) for (const wide of [false, true]) {
    const result = inspect([input(t, fixture({count, wide}))]);
    for (const member of result.members) {
      const blocks = member.compression_blocks;
      assert.equal(blocks.length, count);
      assert.equal(member.header_range.end - member.header_range.start, 57 + 16 * count);
      assert.equal(blocks[0].start, member.payload_range.start);
      assert.equal(blocks.at(-1).end, member.payload_range.end);
      for (let i = 1; i < count; i++) assert.equal(blocks[i - 1].end, blocks[i].start);
      assert.equal(member.declared_uncompressed_length, 16 * (count - 1) + 7);
    }
  }
});
test('malformed compact block lists, header ranges, geometry, bounds and encryption fail', t => {
  for (const fault of ['zero', 'overflow', 'sum', 'truncated', 'header-start', 'header-end',
    'geometry', 'count-cap', 'containment', 'encrypted']) {
    for (const wide of [false, true]) {
      assert.throws(() => inspect([input(t, fixture({count: 3, wide, fault}))]),
        /^Error: Unsupported or invalid controller package metadata$/, fault);
    }
  }
});

test('single unencrypted block omits its per-block length with either integer width', t => {
  for (const wide of [false, true]) {
    const result = inspect([input(t, fixture({wide}))]);
    assert.equal(result.members[0].compression_blocks.length, 1);
    const span = result.members[0].encoded_entry_range;
    assert.equal(span.end - span.start, wide ? 32 : 20);
  }
});

test('canonical mounted directories bind full logical identities without basename fallback', t => {
  const prefix = input(t, fixture({mount: '../../../Icarus/Content/', parent: 'BP/Player/'}));
  assert.deepEqual(inspect([prefix]).members.map(m => m.member_path), MEMBERS);
  const unrelated = input(t, fixture({mount: '../../../Icarus/Content/Maps/', parent: 'BP/Player/'}));
  assert.throws(() => inspect([unrelated]));
  assert.deepEqual(inspect([unrelated, prefix]).members.map(m => m.package_path), [prefix.path, prefix.path]);
  const base = input(t, fixture());
  assert.throws(() => inspect([base, prefix]));
  const a = input(t, fixture({members: [MEMBERS[0]]}));
  const b = input(t, fixture({members: [MEMBERS[1]], mount: '../../../Icarus/Content/', parent: 'BP/Player/'}));
  assert.deepEqual(inspect([a, b]).members.map(m => m.member_path), MEMBERS);
});
test('mount validation rejects malformed, absolute and traversal prefixes', t => {
  for (const mount of ['../../', '../../../../', '../../../Icarus//Content/', '../../../Icarus/./',
    '../../../Icarus/../', '../../../Icarus/Content', '../../../\\Icarus/',
    '../../../C:/Icarus/', '../../../ /../', '../../../ /./', '../../../../Icarus/',
    '../../../' + '/Icarus/', '/Icarus/']) {
    assert.throws(() => inspect([input(t, fixture({mount}))]),
      /^Error: Unsupported or invalid controller package metadata$/, mount);
  }
});
test('legacy primary parser still rejects mounted prefixes and preserves its result shape', () => {
  const {parseFooter, parsePrimaryIndex, parsePrimaryIndexWithMount} = require('../pak_protocol_metadata.cjs');
  for (const mount of ['../../../', '../../../Icarus/Content/']) {
    const bytes = fixture({mount}), footer = parseFooter(bytes.subarray(-FOOTER_SIZE), bytes.length);
    const index = bytes.subarray(footer.indexOffset, footer.indexOffset + footer.indexSize);
    assert.equal(parsePrimaryIndexWithMount(index, footer).mountPrefix, mount.slice(9));
    if (mount === '../../../') assert.deepEqual(Object.keys(parsePrimaryIndex(index, footer)), ['count', 'encoded', 'sections']);
    else assert.throws(() => parsePrimaryIndex(index, footer));
  }
});

test('selector entry budget accepts above legacy cap through its explicit boundary', t => {
  const {parseFooter, parsePrimaryIndex, parsePrimaryIndexWithMount, parseDirectoryIndex} = require('../pak_protocol_metadata.cjs');
  for (const count of [100001, LIMITS.entries]) {
    const bytes = fixture({extraEntries: count - MEMBERS.length});
    assert.equal(inspect([input(t, bytes)]).members.length, 2);
    const footer = parseFooter(bytes.subarray(-FOOTER_SIZE), bytes.length);
    const index = bytes.subarray(footer.indexOffset, footer.indexOffset + footer.indexSize);
    assert.throws(() => parsePrimaryIndex(index, footer));
    assert.throws(() => parsePrimaryIndexWithMount(index, footer));
    const primary = parsePrimaryIndexWithMount(index, footer, {maxEntries: LIMITS.entries});
    const section = primary.sections.directory;
    const directory = bytes.subarray(section.offset, section.offset + section.size);
    assert.throws(() => parseDirectoryIndex(directory, section, primary, new Set(MEMBERS)));
    assert.equal(parseDirectoryIndex(directory, section, primary, new Set(MEMBERS),
      {maxEntries: LIMITS.entries}).selected.size, 2);
    assert.throws(() => parsePrimaryIndexWithMount(index, footer, {maxEntries: LIMITS.entries + 1}));
    assert.throws(() => parseDirectoryIndex(directory, section, primary, new Set(MEMBERS),
      {maxEntries: LIMITS.entries + 1}));
  }
  assert.throws(() => inspect([input(t, fixture({extraEntries: LIMITS.entries + 1 - MEMBERS.length}))]));
});

// Independent minimal authored cooked metadata prefix; no official input.
function classBuffers({zero = false} = {}) {
  const names = [], ids = new Map();
  const name = text => {
    if (!ids.has(text)) { ids.set(text, names.length); names.push(text); }
    return Buffer.concat([u32(ids.get(text)), u32(0)]);
  };
  const imports = Buffer.concat([
    ['/Script/CoreUObject', 'Package', 0, '/Script/CoreUObject'],
    ['/Script/CoreUObject', 'Package', 0, '/Script/Engine'],
    ['/Script/CoreUObject', 'Class', -2, 'BlueprintGeneratedClass'],
    ['/Script/CoreUObject', 'Class', -1, 'Function'],
    ['/Script/CoreUObject', 'Package', 0, '/Script/Authored'],
    ['/Script/CoreUObject', 'Class', -5, 'AuthoredParent']
  ].map(([pkg, type, outer, text]) => Buffer.concat([name(pkg), name(type), u32(outer), name(text)])));
  const object = name('BP_IcarusPlayerControllerSurvival_C');
  const uexp = Buffer.concat([name('None'), u32(0), u32(-6), u32(0), u32(0), u32(0), u32(0)]);
  const table = Buffer.concat(names.map(text => Buffer.concat([str(text), u32(0)])));
  const summary = (size, nameAt, exportAt, importAt) => Buffer.concat([
    u32(0x9e2a83c1), u32(-7), u32(zero ? 0 : 864), u32(zero ? 0 : 522), u32(0), u32(0), u32(size), str(''),
    u32(zero ? 0x80000000 : 0x200), u32(names.length), u32(nameAt), u32(0), u32(0), u32(1), u32(exportAt), u32(6), u32(importAt),
    ...(zero ? [Buffer.alloc(20), Buffer.alloc(16), u32(0), Buffer.alloc(10), str(''),
      Buffer.alloc(10), str(''), u32(0), u32(0), u32(0), u32(0), u32(0), u64(0),
      u32(0), u32(0), u32(0), u32(0)] : [])]);
  const nameAt = summary(0, 0, 0, 0).length, importAt = nameAt + table.length;
  const exportAt = importAt + imports.length, size = exportAt + 104;
  const exp = Buffer.concat([u32(-3), u32(-6), u32(0), u32(0), object, u32(0), u64(uexp.length), u64(size),
    u32(0), u32(0), u32(0), Buffer.alloc(16), u32(0), u32(0), u32(0), u32(-1),
    u32(0), u32(0), u32(0), u32(0)]);
  assert.equal(exp.length, 104);
  return [Buffer.concat([summary(size, nameAt, exportAt, importAt), table, imports, exp]), uexp];
}

test('class reader supports uncompressed selected members and emits only metadata/counts/digests', t => {
  const payloads = classBuffers(), source = input(t, fixture({compressed: false, payloads}));
  const result = readClass([source]);
  assert.equal(result.output_metadata_only, true);
  assert.equal(result.payload_checksums_verified, true);
  assert.equal(result.release_qualified, false);
  assert.equal(result.local_net_property_count, 0);
  assert.equal(result.local_non_overriding_net_function_count, 0);
  assert.equal(result.buffer_digests.uasset_sha256, hash(payloads[0], 'sha256').toString('hex'));
  assert.equal(result.buffer_digests.uexp_sha256, hash(payloads[1], 'sha256').toString('hex'));
  assert.deepEqual(result.selected_identity, {package: '/Game/BP/Player/BP_IcarusPlayerControllerSurvival',
    object_path: ['BP_IcarusPlayerControllerSurvival_C']});
  assert(!JSON.stringify(result).includes('AuthoredParent\u0000'));
  assert(!Object.values(result).some(Buffer.isBuffer));
});

test('class reader forwards only explicitly selected parser profiles', t => {
  const versioned = classBuffers(), zero = classBuffers({zero: true});
  const source = input(t, fixture({compressed: false, payloads: zero}));
  const original = input(t, fixture({compressed: false, payloads: versioned}));
  assert.equal(readClass([original]).parser_profile, PROFILE);
  assert.equal(readClass([source], undefined, {profile: ZERO_VERSION_PROFILE}).parser_profile, ZERO_VERSION_PROFILE);
  for (const options of [{}, {profile: PROFILE}, {profile: null}, {profile: 'unknown'},
    {profile: ZERO_VERSION_PROFILE, limits: {}}, null, []])
    assert.throws(() => readClass([source], undefined, options),
      /^Error: Unsupported or invalid controller package metadata$/);
  assert.throws(() => readClass([original], undefined, {profile: ZERO_VERSION_PROFILE}));
  let calls = 0;
  assert.throws(() => readClass([input(t, fixture())], () => { calls++; }, {profile: 'unknown'}));
  assert.equal(calls, 0);
});

test('Oodle callbacks receive only selected block bytes and exact expected lengths', t => {
  const decoded = classBuffers(), payloads = [Buffer.from('SELECTED_UASSET'), Buffer.from('SELECTED_UEXP')];
  const source = input(t, fixture({payloads, outputLengths: decoded.map(b => b.length), extraEntries: 3}));
  let calls = 0;
  const result = readClass([source], (bytes, expected) => {
    assert.deepEqual(bytes, payloads[calls]); assert.equal(expected, decoded[calls].length);
    return decoded[calls++];
  });
  assert.equal(calls, 2); assert.equal(result.payload_checksums_verified, true);
  assert.equal(result.local_net_property_count, 0);
  assert(!JSON.stringify(result).includes('SELECTED_UASSET'));
});

test('payload checksums reject corruption before callbacks despite a matching package hash', t => {
  for (const offset of [73, 146 + Buffer.byteLength('private-original-payload-never-export')]) {
    const bytes = fixture(); bytes[offset] ^= 1;
    let calls = 0;
    assert.throws(() => readClass([input(t, bytes)], () => { calls++; return Buffer.alloc(7); }),
      /^Error: Unsupported or invalid controller package metadata$/);
    assert.equal(calls, 0);
    // Inspection deliberately does not verify payload SHA1.
    assert.equal(inspect([input(t, bytes)]).payload_checksums_verified, false);
  }
});

test('multiple Oodle blocks use exact final output sizes and concatenate privately', t => {
  const decoded = classBuffers(), count = 3, outputBlockSizes = decoded.map(b => Math.ceil(b.length / count));
  const payloads = [Buffer.alloc(12, 0x41), Buffer.alloc(12, 0x42)];
  const source = input(t, fixture({count, payloads, outputBlockSizes, outputLengths: decoded.map(b => b.length)}));
  let calls = 0;
  const result = readClass([source], (bytes, expected) => {
    const member = Math.floor(calls / count), block = calls++ % count, start = block * outputBlockSizes[member];
    assert.deepEqual(bytes, payloads[member].subarray(block * 4, block * 4 + 4));
    const output = decoded[member].subarray(start, start + outputBlockSizes[member]);
    assert.equal(expected, output.length); return output;
  });
  assert.equal(calls, 6);
  assert.equal(result.buffer_digests.uasset_sha256, hash(decoded[0], 'sha256').toString('hex'));
});

test('callback failures, wrong sizes, unsupported methods and profiles return no success prefix', t => {
  const source = input(t, fixture());
  for (const decoder of [undefined, () => { throw new Error('PRIVATE_DECODER_ERROR'); },
    () => Buffer.alloc(0), () => Buffer.alloc(8), () => new Uint8Array(7),
    () => Buffer.from(new SharedArrayBuffer(7)), () => Promise.resolve(Buffer.alloc(7)), () => Buffer.alloc(7)]) {
    let result;
    assert.throws(() => { result = readClass([source], decoder); },
      /^Error: Unsupported or invalid controller package metadata$/);
    assert.equal(result, undefined);
  }
  let calls = 0;
  assert.throws(() => readClass([input(t, fixture({method: 'Zlib'}))], () => { calls++; }));
  assert.equal(calls, 0);
});

test('global missing or ambiguous selection never invokes decoding', t => {
  let calls = 0; const decoder = () => { calls++; throw new Error('unexpected'); };
  const a = input(t, fixture()), b = input(t, fixture());
  for (const packages of [[a, b], [input(t, fixture({members: [MEMBERS[0]]}))], [a, a]])
    assert.throws(() => readClass(packages, decoder));
  assert.equal(calls, 0);
});

test('mutation during decoding rejects after processing with no result', t => {
  const decoded = classBuffers(), source = input(t, fixture({outputLengths: decoded.map(b => b.length)}));
  let calls = 0, result;
  assert.throws(() => { result = readClass([source], () => {
    const output = decoded[calls++];
    if (calls === 2) { const bytes = fs.readFileSync(source.path); bytes[0] ^= 1; fs.writeFileSync(source.path, bytes); }
    return output;
  }); }, /^Error: Unsupported or invalid controller package metadata$/);
  assert.equal(calls, 2); assert.equal(result, undefined);
});
