'use strict';

// Authored synthetic bytes only. This suite establishes parser behavior, not
// compatibility with any official package or release. It performs no file I/O.
const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const {PROFILE, ZERO_VERSION_PROFILE, LIMITS, countCookedClassFields} = require('../cooked_class_field_counts.cjs');
const join = parts => Buffer.concat(parts);
const i32 = n => { const b = Buffer.alloc(4); b.writeInt32LE(n); return b; };
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const i64 = n => { const b = Buffer.alloc(8); b.writeBigInt64LE(BigInt(n)); return b; };
const flags64 = n => { const b = Buffer.alloc(8); b.writeBigUInt64LE(BigInt(n)); return b; };
const u8 = n => Buffer.from([n]);
const str = s => { const b = Buffer.from(s + '\0'); return join([i32(b.length), b]); };
const identity = {package: '/Game/Authored/Fixture', object_path: ['BP_Target_C']};
const digest = b => crypto.createHash('sha256').update(b).digest('hex');
const fail = fn => assert.throws(fn, {message: 'Unsupported or invalid cooked class metadata'});

function fixture({classes, extraExports = [], wideNames = false, parentName = 'AuthoredParent'} = {}) {
  const names = [], nameIds = new Map();
  const fname = (s, number = 0) => {
    if (!nameIds.has(s)) { nameIds.set(s, names.length); names.push(s); }
    return join([i32(nameIds.get(s)), i32(number)]);
  };
  const imports = [
    ['/Script/CoreUObject', 'Package', 0, '/Script/CoreUObject'],
    ['/Script/CoreUObject', 'Package', 0, '/Script/Engine'],
    ['/Script/CoreUObject', 'Class', -2, 'BlueprintGeneratedClass'],
    ['/Script/CoreUObject', 'Class', -1, 'Function'],
    ['/Script/CoreUObject', 'Package', 0, '/Script/Authored'],
    ['/Script/CoreUObject', 'Class', -5, parentName],
    ['/Script/CoreUObject', 'Function', -6, 'InheritedCall']
  ];
  const importBytes = join(imports.map(([pkg, type, outer, name]) => join([fname(pkg), fname(type), i32(outer), fname(name)])));
  const property = (p = {}) => {
    const type = p.type ?? 'IntProperty';
    let tail = Buffer.alloc(0);
    if (['StructProperty', 'ObjectProperty', 'SoftObjectProperty', 'WeakObjectProperty',
      'LazyObjectProperty', 'InterfaceProperty', 'ByteProperty', 'DelegateProperty',
      'MulticastInlineDelegateProperty', 'MulticastSparseDelegateProperty'].includes(type)) tail = i32(p.ref ?? 0);
    else if (type === 'ClassProperty' || type === 'SoftClassProperty') tail = join([i32(p.ref ?? 0), i32(p.meta ?? 0)]);
    else if (type === 'EnumProperty') tail = join([i32(p.ref ?? 0), property(p.inner)]);
    else if (type === 'BoolProperty') tail = Buffer.from([1, 0, 1, 1, 1, p.native ?? 0]);
    else if (type === 'ArrayProperty' || type === 'SetProperty') tail = property(p.inner);
    else if (type === 'MapProperty') tail = join([property(p.key), property(p.inner)]);
    return join([fname(type, p.typeNumber ?? 0), fname(p.name ?? 'PrivateField', p.nameNumber ?? 0),
      u32(0), i32(p.dim ?? 1), i32(p.size ?? 4), flags64(p.flags ?? 0), Buffer.alloc(2),
      fname('None'), u8(p.condition ?? 0), tail]);
  };
  const tagged = (tag = {}) => {
    const type = tag.type ?? 'StrProperty', payload = Buffer.from(tag.payload ?? 'PRIVATE_DEFAULT_NEVER_EMIT');
    let tail = [];
    if (type === 'StructProperty') tail = [fname('PrivateStruct'), Buffer.alloc(16)];
    else if (type === 'BoolProperty') tail = [u8(tag.bool ?? 1)];
    else if (type === 'ByteProperty' || type === 'EnumProperty') tail = [fname('PrivateEnum')];
    else if (type === 'ArrayProperty' || type === 'SetProperty') tail = [fname('StrProperty')];
    else if (type === 'MapProperty') tail = [fname('NameProperty'), fname('StrProperty')];
    return join([fname('PrivateDefault'), fname(type), i32(tag.size ?? payload.length), i32(tag.index ?? 0),
      ...tail, u8(tag.guid ?? 0), ...(tag.guid === 1 ? [Buffer.alloc(16)] : []), payload]);
  };
  const body = (spec = {}, children = [], superIndex = -6) => {
    const script = Buffer.from(spec.script ?? ''), props = spec.props ?? [];
    return join([...(spec.tags ?? []).map(tagged), fname('None'), i32(spec.objectGuid ?? 0),
      i32(spec.structSuper ?? superIndex), i32(spec.childCount ?? children.length), ...children.map(i32),
      i32(spec.fieldCount ?? props.length), ...props.map(property),
      i32(spec.bytecodeSize ?? script.length), i32(spec.storageSize ?? script.length), script,
      ...(spec.function ? [u32(spec.flags ?? 0)] : []), Buffer.from(spec.tail ?? 'PRIVATE_OPAQUE_TAIL')]);
  };
  const exports = [];
  for (const cls of classes ?? [{name: 'BP_Target_C'}]) {
    const ci = exports.length + 1, functions = cls.functions ?? [];
    exports.push({name: cls.name, number: cls.number, classIndex: cls.classIndex ?? -3,
      superIndex: cls.superIndex ?? -6, outer: cls.outer ?? 0,
      body: body(cls, cls.children ?? functions.map((_, i) => ci + i + 1), cls.superIndex ?? -6)});
    for (const fn of functions) exports.push({name: fn.name ?? 'PrivateFunction', classIndex: fn.classIndex ?? -4,
      superIndex: fn.superIndex ?? 0, outer: fn.outer ?? ci,
      body: body({...fn, function: true}, fn.children ?? [], fn.structSuper ?? 0)});
  }
  for (const e of extraExports) exports.push({...e, body: Buffer.from(e.body ?? 'UNRELATED_PRIVATE_DEFAULT')});
  // Populate export object names before writing the name table.
  const exportNames = exports.map(e => fname(e.name, e.number ?? 0));
  const nameBytes = join(names.map(s => {
    const b = wideNames ? join([i32(-(s.length + 1)), Buffer.from(s + '\0', 'utf16le')]) : str(s);
    return join([b, Buffer.alloc(4)]);
  }));
  const summary = (headerSize, nameAt, exportAt, importAt) => join([
    u32(0x9e2a83c1), i32(-7), i32(864), i32(522), i32(0), i32(0), i32(headerSize), str(''),
    u32(0x200), i32(names.length), i32(nameAt), i32(0), i32(0),
    i32(exports.length), i32(exportAt), i32(imports.length), i32(importAt)]);
  const prefixLength = summary(0, 0, 0, 0).length;
  const nameAt = prefixLength, importAt = nameAt + nameBytes.length, exportAt = importAt + importBytes.length;
  const headerSize = exportAt + exports.length * 104;
  let serialAt = headerSize;
  const offsets = [];
  const exportBytes = join(exports.map((e, i) => {
    offsets.push(serialAt - headerSize);
    const b = join([i32(e.classIndex), i32(e.superIndex), i32(e.template ?? 0), i32(e.outer), exportNames[i],
      u32(0), i64(e.body.length), i64(serialAt), i32(0), i32(0), i32(0), Buffer.alloc(16),
      u32(0), i32(0), i32(0), i32(-1), i32(0), i32(0), i32(0), i32(0)]);
    assert.equal(b.length, 104); serialAt += e.body.length; return b;
  }));
  return {uasset: join([summary(headerSize, nameAt, exportAt, importAt), nameBytes, importBytes, exportBytes]),
    uexp: join(exports.map(e => e.body)), exports, offsets, exportAt, importAt, nameAt, names, nameIds, prefixLength};
}
const count = (f, selected = identity, options) => countCookedClassFields(f.uasset, f.uexp, selected, options);
const changed = (f, mutate) => {
  const copy = {...f, uasset: Buffer.from(f.uasset), uexp: Buffer.from(f.uexp)}; mutate(copy); return copy;
};
function zeroFixture({dependencies = [], generations = [], chunks = [], ...options} = {}) {
  const f = fixture(options), parts = [], summaryOffsets = {}; let tailLength = 0;
  const add = (key, bytes) => { summaryOffsets[key] = f.prefixLength + tailLength; parts.push(bytes); tailLength += bytes.length; };
  add('otherOffsets', Buffer.alloc(20)); add('guid', Buffer.alloc(16));
  add('generations', i32(generations.length));
  for (const [exports, names] of generations) add('generationEntry', join([i32(exports), i32(names)]));
  add('engine', join([Buffer.alloc(10), str('')])); add('compatibleEngine', join([Buffer.alloc(10), str('')]));
  add('compressionFlags', u32(0)); add('compressedChunks', i32(0)); add('packageSource', u32(0));
  add('additionalPackages', i32(0)); add('assetRegistry', i32(0)); add('bulkStart', i64(0));
  add('worldTile', i32(0)); add('chunkCount', i32(chunks.length));
  for (const chunk of chunks) add('chunkEntry', i32(chunk));
  add('preloadCount', i32(dependencies.length)); add('preloadAt', i32(0));
  const preloadAt = f.uasset.length + tailLength, headerLength = preloadAt + dependencies.length * 4;
  const uasset = join([f.uasset.subarray(0, f.prefixLength), ...parts, f.uasset.subarray(f.prefixLength), ...dependencies.map(i32)]);
  uasset.writeInt32LE(0, 8); uasset.writeInt32LE(0, 12); uasset.writeUInt32LE(0x80000000, 33);
  uasset.writeInt32LE(headerLength, 24);
  for (const at of [41, 57, 65]) uasset.writeInt32LE(f.uasset.readInt32LE(at) + tailLength, at);
  uasset.writeInt32LE(dependencies.length ? preloadAt : 0, summaryOffsets.preloadAt);
  const exportAt = f.exportAt + tailLength;
  for (let i = 0; i < f.exports.length; i++) uasset.writeBigInt64LE(BigInt(headerLength + f.offsets[i]), exportAt + i * 104 + 36);
  return {...f, uasset, nameAt: f.nameAt + tailLength, importAt: f.importAt + tailLength,
    exportAt, preloadAt, summaryOffsets};
}
const fields = result => [result.local_net_property_count, result.local_non_overriding_net_function_count];

test('exact class selection among multiple exports preserves superclass and only emits metadata', () => {
  const f = fixture({classes: [
    {name: 'BP_Unrelated_C', props: [{name: 'UNRELATED_PRIVATE_NAME', flags: 0x20}]},
    {name: 'BP_Target_C', tags: [{}], props: [
      {name: 'PRIVATE_PROPERTY_NAME', flags: 0x20n | (1n << 63n), dim: 9},
      {name: 'Array', type: 'ArrayProperty', flags: 0x20, inner: {flags: 0x20}},
      {name: 'Map', type: 'MapProperty', flags: 0, key: {flags: 0x20}, inner: {flags: 0x20}},
      {name: 'Scalar', type: 'Int64Property', flags: 0x40}],
    functions: [{name: 'PRIVATE_RPC_NAME', flags: 0x40, props: [{name: 'PRIVATE_PARAMETER', flags: 0xa0}], script: 'PRIVATE_BYTECODE'},
      {name: 'Unreliable', flags: 0x80}, {name: 'Ordinary', flags: 0}]}
  ]});
  const result = count(f);
  assert.deepEqual(result, {selected_identity: identity,
    superclass: {package: '/Script/Authored', object_path: ['AuthoredParent']},
    local_net_property_count: 2, local_non_overriding_net_function_count: 1,
    parser_profile: PROFILE, buffer_digests: {uasset_sha256: digest(f.uasset), uexp_sha256: digest(f.uexp)}});
  assert.deepEqual(fields(count(f, {...identity, object_path: ['BP_Unrelated_C']})), [1, 0]);
  const output = JSON.stringify(result);
  for (const secret of ['PRIVATE', 'UNRELATED', 'Private', 'Unreliable', 'Ordinary', 'flags', 'params', 'defaults', 'bytecode'])
    assert(!output.includes(secret));
});

test('valid empty metadata returns zero; missing, ambiguous and wrong-kind selections reject', () => {
  assert.deepEqual(fields(count(fixture())), [0, 0]);
  for (const classes of [[{name: 'BP_Absent_C'}], [{name: 'BP_Target_C'}, {name: 'BP_Target_C'}],
    [{name: 'BP_Target_C', classIndex: -4}], [{name: 'BP_Target_C'}, {name: 'bp_target_c'}]]) fail(() => count(fixture({classes})));
  // FName rendering collisions cannot select an arbitrary first export.
  fail(() => count(fixture({classes: [{name: 'BP_Target_C_0'}, {name: 'BP_Target_C', number: 1}]}),
    {...identity, object_path: ['BP_Target_C_0']}));
});

test('zero-version metadata requires its explicit isolated prefix profile', () => {
  const versioned = fixture();
  const zero = zeroFixture();
  assert.equal(count(versioned).parser_profile, PROFILE);
  assert.equal(count(versioned, identity, {profile: PROFILE}).parser_profile, PROFILE);
  const result = count(zero, identity, {profile: ZERO_VERSION_PROFILE});
  assert.equal(result.parser_profile, ZERO_VERSION_PROFILE);
  assert.deepEqual(fields(result), [0, 0]);
  fail(() => count(zero));
  fail(() => count(zero, identity, {profile: PROFILE}));
  fail(() => count(versioned, identity, {profile: ZERO_VERSION_PROFILE}));
  for (const profile of [null, '', 'unknown', 522, {}, []])
    fail(() => count(zero, identity, {profile}));
  for (const [offset, value] of [[4, -8], [8, 864], [8, 1], [12, 522], [12, 1], [16, 1], [20, 1]])
    fail(() => count(changed(zero, b => b.uasset.writeInt32LE(value, offset)), identity, {profile: ZERO_VERSION_PROFILE}));
  for (const flags of [0, 0x200, 0x80002000])
    fail(() => count(changed(zero, b => b.uasset.writeUInt32LE(flags, 33)), identity, {profile: ZERO_VERSION_PROFILE}));
  // Existing structural limits and validation still apply to the zero tuple.
  fail(() => count(zero, identity, {profile: ZERO_VERSION_PROFILE, limits: {names: 1}}));
});

test('zero profile validates bounded full summary and preload dependency slices', () => {
  const options = {profile: ZERO_VERSION_PROFILE};
  const f = zeroFixture({dependencies: [-6, 1], generations: [[1, 1]], chunks: [0, 7]});
  f.uasset.writeInt32LE(0, f.exportAt + 84); f.uasset.writeInt32LE(1, f.exportAt + 88);
  f.uasset.writeInt32LE(1, f.exportAt + 100);
  assert.deepEqual(fields(count(f, identity, options)), [0, 0]);
  const empty = zeroFixture(); empty.uasset.writeInt32LE(0, empty.exportAt + 84);
  assert.deepEqual(fields(count(empty, identity, options)), [0, 0]);
  const unrelated = zeroFixture({dependencies: [-6], extraExports: [
    {name: 'OpaqueDefault', classIndex: 1, outer: 0, superIndex: 0}]});
  unrelated.uasset.writeInt32LE(0, unrelated.exportAt + 104 + 84);
  unrelated.uasset.writeInt32LE(1, unrelated.exportAt + 104 + 88);
  assert.deepEqual(fields(count(unrelated, identity, options)), [0, 0]);
  fail(() => count(changed(unrelated, b => b.uasset.writeInt32LE(2, unrelated.exportAt + 104 + 88)), identity, options));
  const {summaryOffsets: s} = f;
  for (const mutate of [
    b => b.uasset.writeInt32LE(-1, s.generations),
    b => b.uasset.writeInt32LE(LIMITS.generations + 1, s.generations),
    b => b.uasset.writeInt32LE(-1, s.generationEntry),
    b => b.uasset.writeInt32LE(1, s.compressionFlags),
    b => b.uasset.writeInt32LE(1, s.compressedChunks),
    b => b.uasset.writeInt32LE(-1, s.compressedChunks),
    b => b.uasset.writeInt32LE(1, s.additionalPackages),
    b => b.uasset.writeInt32LE(LIMITS.chunkIDs + 1, s.chunkCount),
    b => b.uasset.writeInt32LE(-1, s.chunkEntry),
    b => b.uasset.writeInt32LE(-1, s.preloadCount),
    b => b.uasset.writeInt32LE(LIMITS.preloadDependencies + 1, s.preloadCount),
    b => b.uasset.writeInt32LE(0x7fffffff, s.preloadCount),
    b => b.uasset.writeInt32LE(f.uasset.length, s.preloadAt),
    b => b.uasset.writeInt32LE(f.nameAt, s.preloadAt),
    b => b.uasset.writeInt32LE(f.importAt, s.preloadAt),
    b => b.uasset.writeInt32LE(f.exportAt, s.preloadAt),
    b => b.uasset.writeInt32LE(1, s.preloadAt),
    b => b.uasset.writeInt32LE(-999, f.preloadAt),
    b => b.uasset.writeInt32LE(999, f.preloadAt),
    b => b.uasset.writeInt32LE(0, f.preloadAt),
    b => b.uasset.writeInt32LE(-2, f.exportAt + 84),
    b => b.uasset.writeInt32LE(-1, f.exportAt + 84),
    b => b.uasset.writeInt32LE(1, f.exportAt + 84),
    b => b.uasset.writeInt32LE(-1, f.exportAt + 88),
    b => b.uasset.writeInt32LE(0x7fffffff, f.exportAt + 88)
  ]) fail(() => count(changed(f, mutate), identity, options));
  for (const limits of [{generations: 1}, {chunkIDs: 1}, {preloadDependencies: 1}]) {
    const source = limits.generations ? zeroFixture({generations: [[1, 1], [1, 1]]}) : f;
    fail(() => count(source, identity, {...options, limits}));
  }
  for (const length of [s.engine + 9, s.compatibleEngine + 12, s.preloadAt + 3, f.uasset.length - 1]) {
    const truncated = {...f, uasset: Buffer.from(f.uasset.subarray(0, length))};
    truncated.uasset.writeInt32LE(length, 24);
    fail(() => count(truncated, identity, options));
  }
  // Default profile still rejects preload slices even on its existing prefix.
  const versioned = fixture(); versioned.uasset.writeInt32LE(0, versioned.exportAt + 84);
  fail(() => count(versioned));
});

test('outer path and FName number participate in selection; package identity is a caller binding', () => {
  const f = fixture({classes: [{name: 'BP_Target_C'}, {name: 'BP_Target_C', outer: 1}]});
  assert.deepEqual(count(f, {...identity, object_path: ['BP_Target_C', 'BP_Target_C']}).selected_identity.object_path,
    ['BP_Target_C', 'BP_Target_C']);
  fail(() => count(f, {...identity, object_path: ['Other', 'BP_Target_C']}));
  const labeled = {...identity, package: '/Game/CallerBoundPackage'};
  assert.deepEqual(count(f, labeled).selected_identity, labeled);
  const numbered = fixture({classes: [{name: 'BP_Target_C', number: 2}]});
  fail(() => count(numbered));
  assert.deepEqual(count(numbered, {...identity, object_path: ['BP_Target_C_1']}).selected_identity.object_path,
    ['BP_Target_C_1']);
});

test('ordinary spaces are exact object-name characters; controls and separators reject', () => {
  const selected = {...identity, object_path: ['BP Target C']};
  const f = fixture({parentName: 'Authored Parent', classes: [{name: 'BP Target C'}],
    extraExports: [{name: 'Unrelated Object', classIndex: 1, outer: 0, superIndex: 0}]});
  const result = count(f, selected);
  assert.deepEqual(result.selected_identity, selected);
  assert.deepEqual(result.superclass, {package: '/Script/Authored', object_path: ['Authored Parent']});
  assert.deepEqual(fields(result), [0, 0]);
  fail(() => count(f, {...identity, object_path: ['BP_Target_C']}));
  for (const character of ['\x00', '\x01', '\t', '\n', '\r', '\x1f', '\x7f', '.', '/', '\\', ':']) {
    const name = 'Object' + character + 'Name';
    fail(() => count(fixture({parentName: name})));
    fail(() => count(fixture({extraExports: [{name, classIndex: 1, outer: 0, superIndex: 0}]})));
    fail(() => count(fixture(), {...identity, object_path: [name]}));
  }
});

test('local superclass identity is preserved without adding its fields to local counts', () => {
  const f = fixture({classes: [{name: 'BP_Parent_C', props: [{flags: 0x20}]},
    {name: 'BP_Target_C', superIndex: 1}]});
  assert.deepEqual(count(f).superclass, {...identity, object_path: ['BP_Parent_C']});
  assert.deepEqual(fields(count(f)), [0, 0]);
  fail(() => count(fixture({classes: [{name: 'BP_Target_C', superIndex: 2}, {name: 'BP_Parent_C', superIndex: 1}]})));
});

test('unrelated exports and default payloads remain opaque and never appear in the result', () => {
  const f = fixture({classes: [{name: 'BP_Target_C', tags: [{payload: '\xff\x00PRIVATE_VALUE'}]}],
    extraExports: [{name: 'PRIVATE_UNRELATED', classIndex: 1, outer: 0, superIndex: 0}]});
  const result = count(f); assert.deepEqual(fields(result), [0, 0]);
  assert(!JSON.stringify(result).includes('PRIVATE'));
  const changedValues = changed(f, b => {
    // The second export deliberately has no recognizable struct prefix.
    b.uexp.fill(0xff, b.offsets[1]);
  });
  assert.deepEqual(fields(count(changedValues)), [0, 0]);
  assert.notEqual(result.buffer_digests.uexp_sha256, count(changedValues).buffer_digests.uexp_sha256);
});

test('local superclass chains must terminate in imports of the correct kind', () => {
  const classes = parentSuper => [{name: 'BP_Parent_C', superIndex: parentSuper},
    {name: 'BP_Target_C', superIndex: 1}];
  assert.deepEqual(fields(count(fixture({classes: classes(-6)}))), [0, 0]);
  // The direct superclass is a class, but its terminal import is a Function.
  fail(() => count(fixture({classes: classes(-7)})));

  const functions = terminal => fixture({classes: [{name: 'BP_Target_C', functions: [
    {name: 'Override', flags: 0x40, superIndex: 3, structSuper: 3}]}], extraExports: [
    {name: 'AncestorFunction', classIndex: -4, outer: 0, superIndex: terminal}]});
  assert.deepEqual(fields(count(functions(-7))), [0, 0]);
  // The ancestor function is outside the selected class; its body stays opaque.
  fail(() => count(functions(-6)));
});

test('all modeled field tails and tag headers have explicit bounded layouts', () => {
  const props = ['Int8Property', 'Int16Property', 'IntProperty', 'Int64Property', 'UInt16Property', 'UInt32Property',
    'UInt64Property', 'FloatProperty', 'DoubleProperty', 'NameProperty', 'StrProperty', 'TextProperty',
    'StructProperty', 'ObjectProperty', 'SoftObjectProperty', 'WeakObjectProperty', 'LazyObjectProperty',
    'InterfaceProperty', 'DelegateProperty', 'MulticastInlineDelegateProperty', 'MulticastSparseDelegateProperty',
    'ClassProperty', 'SoftClassProperty', 'ByteProperty', 'BoolProperty', 'EnumProperty', 'ArrayProperty',
    'SetProperty', 'MapProperty'].map((type, i) => ({name: 'Field' + i, type, flags: 0x20,
      ref: 0, inner: {name: 'Inner'}, key: {name: 'Key'}}));
  const tags = ['StructProperty', 'BoolProperty', 'ByteProperty', 'EnumProperty', 'ArrayProperty',
    'SetProperty', 'MapProperty', 'StrProperty'].map(type => ({type, guid: 1}));
  assert.deepEqual(fields(count(fixture({classes: [{name: 'BP_Target_C', props, tags}], wideNames: true}))), [props.length, 0]);
});

test('net functions require agreeing super links and do not count parameter net flags', () => {
  for (const [structSuper, superIndex] of [[0, 0], [-7, 0], [0, -7], [-7, -7]]) {
    const f = fixture({classes: [{name: 'BP_Target_C', functions: [{name: 'Call', flags: 0x40,
      structSuper, superIndex, props: [{flags: 0x20}]}]}]});
    if (structSuper !== superIndex) fail(() => count(f));
    else assert.deepEqual(fields(count(f)), [0, structSuper ? 0 : 1]);
  }
  // Both refer to valid functions, but disagree on which function is overridden.
  const mismatched = fixture({classes: [{name: 'BP_Target_C', functions: [
    {name: 'Call', flags: 0x40, structSuper: -7, superIndex: 3}]}], extraExports: [
    {name: 'OtherFunction', classIndex: -4, outer: 0, superIndex: -7}]});
  fail(() => count(mismatched));
});

test('serialized script length, not bytecode memory length or flag-like bytes, locates function flags', () => {
  const f = fixture({classes: [{name: 'BP_Target_C', script: 'ClassCode', bytecodeSize: 99,
    functions: [{name: 'Call', flags: 0x40, script: '\x00\x40\x00\x00\x00garbage', bytecodeSize: 1000}]}]});
  assert.deepEqual(fields(count(f)), [0, 1]);
  const other = changed(f, b => b.uexp.fill(0xff, b.offsets[1] + 32, b.offsets[1] + 44));
  // Only the storage bytes change; both metadata results remain identical.
  assert.deepEqual(fields(count(other)), [0, 1]);
  assert.notEqual(count(other).buffer_digests.uexp_sha256, count(f).buffer_digests.uexp_sha256);
});

test('unsupported versions, unversioned properties, malformed summary and table overlaps reject', () => {
  const f = fixture();
  for (const mutate of [b => b.uasset.writeUInt32LE(0, 0), b => b.uasset.writeInt32LE(-8, 4),
    b => b.uasset.writeInt32LE(0, 8), b => b.uasset.writeInt32LE(521, 12), b => b.uasset.writeInt32LE(1, 16),
    b => b.uasset.writeInt32LE(-1, 20), b => b.uasset.writeInt32LE(1, 20),
    b => b.uasset.writeInt32LE(LIMITS.customVersions + 1, 20),
    b => b.uasset.writeInt32LE(f.uasset.length - 1, 24),
    b => b.uasset.writeUInt32LE(0x2200, 33), b => b.uasset.writeUInt32LE(0, 33),
    b => b.uasset.writeInt32LE(-1, 37), b => b.uasset.writeInt32LE(LIMITS.names + 1, 37),
    b => b.uasset.writeInt32LE(1, 41), b => b.uasset.writeInt32LE(1, 45),
    b => b.uasset.writeInt32LE(1, 49), b => b.uasset.writeInt32LE(-1, 53),
    b => b.uasset.writeInt32LE(f.uasset.length, 57), b => b.uasset.writeInt32LE(-1, 61),
    b => b.uasset.writeInt32LE(f.nameAt, 65)]) fail(() => count(changed(f, mutate)));
});

test('all import/export indices, outer cycles, names and serial ranges are validated', () => {
  const f = fixture({classes: [{name: 'BP_Target_C'}, {name: 'BP_Unrelated_C'}]});
  for (const mutate of [b => b.uasset.writeInt32LE(-999, b.exportAt), b => b.uasset.writeInt32LE(999, b.exportAt + 4),
    b => b.uasset.writeInt32LE(999, b.exportAt + 8), b => b.uasset.writeInt32LE(1, b.exportAt + 12),
    b => b.uasset.writeInt32LE(999, b.importAt + 16), b => b.uasset.writeInt32LE(-1, b.importAt + 16),
    b => b.uasset.writeInt32LE(-999, b.exportAt + 16), b => b.uasset.writeInt32LE(-1, b.exportAt + 20),
    b => b.uasset.writeBigInt64LE(-1n, b.exportAt + 28), b => b.uasset.writeBigInt64LE(2n ** 53n, b.exportAt + 28),
    b => b.uasset.writeBigInt64LE(0n, b.exportAt + 36), b => b.uasset.writeBigInt64LE(BigInt(f.uasset.length + f.uexp.length), b.exportAt + 36),
    b => b.uasset.writeBigInt64LE(BigInt(f.uasset.length), b.exportAt + 104 + 36),
    b => b.uasset.writeInt32LE(2, b.exportAt + 44), b => b.uasset.writeInt32LE(-2, b.exportAt + 84),
    b => b.uasset.writeInt32LE(0, b.exportAt + 84), b => b.uasset.writeInt32LE(1, b.exportAt + 88)])
    fail(() => count(changed(f, mutate)));
  // Unrelated exports are range checked even though their bodies remain opaque.
  fail(() => count(changed(f, b => b.uasset.writeBigInt64LE(-1n, b.exportAt + 104 + 28))));
});

test('truncation never turns malformed structures into a zero count', () => {
  const f = fixture({classes: [{name: 'BP_Target_C', props: [{flags: 0x20}], functions: [{name: 'Call', flags: 0x40}]}]});
  for (const length of [0, 1, 23, 68, f.uasset.length - 1]) fail(() => count({...f, uasset: f.uasset.subarray(0, length)}));
  for (const length of [0, 1, 8, 16, 32, f.uexp.length - 1]) fail(() => count({...f, uexp: f.uexp.subarray(0, length)}));
  // A forged smaller serial slice still cannot borrow its missing metadata from the next export.
  fail(() => count(changed(f, b => b.uasset.writeBigInt64LE(20n, b.exportAt + 28))));
});

test('invalid tags, counts, nested field structures and bytecode lengths reject', () => {
  const invalid = [
    {childCount: -1}, {childCount: LIMITS.children + 1}, {fieldCount: -1}, {fieldCount: LIMITS.fields + 1},
    {children: [0]}, {children: [-7]}, {children: [999]}, {children: [1]},
    {objectGuid: 1}, {structSuper: 0}, {structSuper: 999},
    {props: [{type: 'UnknownProperty'}]}, {props: [{type: 'FieldPathProperty'}]},
    {props: [{typeNumber: 1}]}, {props: [{dim: 0}]}, {props: [{dim: -1}]},
    {props: [{size: 0}]}, {props: [{condition: 16}]}, {props: [{condition: 255}]},
    {props: [{type: 'ObjectProperty', ref: -999}]},
    {props: [{type: 'ClassProperty', meta: 999}]}, {props: [{type: 'BoolProperty', native: 2}]},
    {props: [{name: 'Same'}, {name: 'same'}]}, {props: [{name: 'None'}]},
    {props: [{type: 'MapProperty', key: {}, inner: {type: 'UnknownProperty'}}]},
    {tags: [{type: 'UnknownProperty'}]}, {tags: [{size: -1}]}, {tags: [{size: LIMITS.uexpBytes + 1}]},
    {tags: [{index: -1}]}, {tags: [{guid: 2}]}, {tags: [{type: 'BoolProperty', bool: 2}]},
    {bytecodeSize: -1}, {storageSize: -1}, {bytecodeSize: LIMITS.scriptBytes + 1},
    {storageSize: LIMITS.scriptBytes + 1}, {bytecodeSize: 1}, {script: 'a', bytecodeSize: 0}
  ];
  for (const spec of invalid) fail(() => count(fixture({classes: [{name: 'BP_Target_C', ...spec}]})));
});

test('children must be local unique functions and complete; malformed function metadata rejects', () => {
  for (const cls of [
    {children: [2, 2], functions: [{name: 'Call'}]},
    {children: [], functions: [{name: 'Call'}]},
    {functions: [{name: 'Call', outer: 0}]},
    {functions: [{name: 'Call', classIndex: -3}]},
    {functions: [{name: 'Call'}, {name: 'call'}]},
    {functions: [{name: 'Call', structSuper: 2}]},
    {functions: [{name: 'Call', superIndex: 1}]},
    {functions: [{name: 'Call', superIndex: -6}]},
    {functions: [{name: 'Call', childCount: -1}]},
    {functions: [{name: 'Call', fieldCount: -1}]},
    {functions: [{name: 'Call', storageSize: LIMITS.scriptBytes + 1}]}
  ]) fail(() => count(fixture({classes: [{name: 'BP_Target_C', ...cls}]})));
});

test('lowered global budgets cover siblings, nested fields, tags and function parameters', () => {
  const nested = {type: 'ArrayProperty', flags: 0x20, inner: {type: 'MapProperty', key: {}, inner: {}}};
  const f = fixture({classes: [{name: 'BP_Target_C', props: [nested], tags: [{}], functions: [
    {name: 'Call', flags: 0x40, tags: [{}], props: [{name: 'Parameter'}]}]}]});
  assert.deepEqual(fields(count(f, identity, {limits: {fields: 5, depth: 3, tags: 2, children: 1}})), [1, 1]);
  for (const limits of [{fields: 4}, {depth: 2}, {tags: 1}, {objectDepth: 1}, {uassetBytes: f.uasset.length - 1},
    {uexpBytes: f.uexp.length - 1}, {names: 1}, {imports: 1}, {exports: 1}, {stringUnits: 1}])
    fail(() => count(f, identity, {limits}));
  let deep = {};
  for (let i = 0; i < LIMITS.depth; i++) deep = {type: 'ArrayProperty', inner: deep};
  fail(() => count(fixture({classes: [{name: 'BP_Target_C', props: [deep]}]})));
  const siblings = fixture({classes: [{name: 'BP_Target_C', props: [{name: 'A'}, {name: 'B'}]}]});
  fail(() => count(siblings, identity, {limits: {fields: 1}}));
  const children = fixture({classes: [{name: 'BP_Target_C', functions: [{name: 'A'}, {name: 'B'}]}]});
  fail(() => count(children, identity, {limits: {children: 1}}));
});

test('invalid public inputs and attempts to increase or disable budgets reject without leaking inputs', () => {
  const f = fixture();
  for (const selected of [null, {}, 'BP_Target_C', {...identity, object_path: []}, {...identity, object_path: ['../BP_Target_C']},
    {...identity, object_path: ['Other']}, {...identity, package: '../Fixture'}, {...identity, extra: 'PRIVATE'}])
    fail(() => count(f, selected));
  for (const limits of [null, [], {depth: 0}, {fields: Infinity}, {exports: -1}, {depth: LIMITS.depth + 1}, {unknown: 1}])
    fail(() => count(f, identity, {limits}));
  fail(() => countCookedClassFields(new Uint8Array(f.uasset), f.uexp, identity));
  fail(() => countCookedClassFields(f.uasset, null, identity));
  fail(() => count(f, identity, null));
  fail(() => count(f, identity, {unknown: 'PRIVATE'}));
  fail(() => countCookedClassFields(Buffer.from(new SharedArrayBuffer(f.uasset.length)), f.uexp, identity));
  const beforeAsset = Buffer.from(f.uasset), beforeExports = Buffer.from(f.uexp);
  count(f); assert.deepEqual(f.uasset, beforeAsset); assert.deepEqual(f.uexp, beforeExports);
  const result = count(f); result.selected_identity.object_path.push('Changed');
  assert.deepEqual(count(f).selected_identity, identity);
});
