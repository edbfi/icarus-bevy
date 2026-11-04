'use strict';

// Pure metadata reader. No package acquisition, decompression or release qualification.
// The package label is caller-supplied: the integrating tool must bind it to its
// package member. This profile covers only the serialized metadata prefix used
// below, not arbitrary UE assets, class tails, function tails or object defaults.
const crypto = require('node:crypto');
const {TextDecoder} = require('node:util');

const PROFILE = 'ue4-522-cooked-ffield-104-prefix-v1';
const ZERO_VERSION_PROFILE = 'zero-version-cooked-ffield-104-prefix-v1';
const LIMITS = Object.freeze({uassetBytes: 16 * 1024 * 1024, uexpBytes: 64 * 1024 * 1024,
  names: 65536, imports: 65536, exports: 65536, customVersions: 256, stringUnits: 4096,
  fields: 65536, children: 65536, tags: 65536, depth: 32, objectDepth: 64,
  generations: 256, chunkIDs: 4096, preloadDependencies: 1048576,
  arrayDim: 65536, elementSize: 16 * 1024 * 1024, scriptBytes: 16 * 1024 * 1024});
const MESSAGE = 'Unsupported or invalid cooked class metadata';
const utf8 = new TextDecoder('utf-8', {fatal: true, ignoreBOM: true});
const utf16 = new TextDecoder('utf-16le', {fatal: true, ignoreBOM: true});
const SCALARS = new Set(['Int8Property', 'Int16Property', 'IntProperty', 'Int64Property',
  'UInt16Property', 'UInt32Property', 'UInt64Property', 'FloatProperty', 'DoubleProperty',
  'NameProperty', 'StrProperty', 'TextProperty']);
const REFS = new Set(['StructProperty', 'ObjectProperty', 'SoftObjectProperty',
  'WeakObjectProperty', 'LazyObjectProperty', 'InterfaceProperty', 'DelegateProperty',
  'MulticastInlineDelegateProperty', 'MulticastSparseDelegateProperty']);
const TYPES = new Set([...SCALARS, ...REFS, 'ClassProperty', 'SoftClassProperty',
  'ByteProperty', 'EnumProperty', 'BoolProperty', 'ArrayProperty', 'SetProperty', 'MapProperty']);

function check(ok) { if (!ok) throw new Error(MESSAGE); }
function integer(n, max = Number.MAX_SAFE_INTEGER) {
  check(Number.isSafeInteger(n) && n >= 0 && n <= max); return n;
}
function range(at, size, end) {
  integer(at); integer(size); integer(end); check(at <= end && size <= end - at);
}
class Cursor {
  constructor(bytes, limits, at = 0, end = bytes.length) {
    range(at, end - at, bytes.length); this.bytes = bytes; this.limits = limits; this.at = at; this.end = end;
  }
  take(n) { range(this.at, n, this.end); const b = this.bytes.subarray(this.at, this.at + n); this.at += n; return b; }
  skip(n) { range(this.at, n, this.end); this.at += n; }
  u8() { return this.take(1)[0]; }
  u16() { return this.take(2).readUInt16LE(); }
  i32() { return this.take(4).readInt32LE(); }
  u32() { return this.take(4).readUInt32LE(); }
  flags64() { return this.take(8).readBigUInt64LE(); }
  i64() { const n = this.take(8).readBigInt64LE(); check(n >= 0n && n <= BigInt(Number.MAX_SAFE_INTEGER)); return Number(n); }
  bool32() { const n = this.i32(); check(n === 0 || n === 1); return n; }
  str() {
    const n = this.i32(); if (n === 0) return '';
    const units = integer(Math.abs(n), this.limits.stringUnits);
    const b = this.take(units * (n < 0 ? 2 : 1));
    if (n > 0) {
      check(b[b.length - 1] === 0 && !b.subarray(0, -1).includes(0)); return utf8.decode(b.subarray(0, -1));
    }
    check(b.readUInt16LE(b.length - 2) === 0);
    for (let i = 0; i < b.length - 2; i += 2) check(b.readUInt16LE(i) !== 0);
    return utf16.decode(b.subarray(0, -2));
  }
}
function limitsFor(overrides) {
  check(overrides && typeof overrides === 'object' && !Array.isArray(overrides));
  const result = {...LIMITS};
  for (const key of Object.keys(overrides)) {
    check(Object.hasOwn(LIMITS, key)); result[key] = integer(overrides[key], LIMITS[key]); check(result[key] > 0);
  }
  return result;
}
function packageName(s, l) {
  check(typeof s === 'string' && s.length <= l.stringUnits && /^\/(?:[A-Za-z0-9_]+\/)*[A-Za-z0-9_]+$/.test(s)); return s;
}
function objectName(s, l) {
  check(typeof s === 'string' && s.length > 0 && s.length <= l.stringUnits && !/[\x00-\x1f\x7f./\\:]/.test(s)); return s;
}
function requestedIdentity(identity, l) {
  check(identity && typeof identity === 'object' && !Array.isArray(identity));
  check(Object.keys(identity).length === 2 && Object.hasOwn(identity, 'package') && Object.hasOwn(identity, 'object_path'));
  const pkg = packageName(identity.package, l), path = identity.object_path;
  check(Array.isArray(path) && path.length > 0 && path.length <= l.objectDepth);
  const objectPath = Array.from(path, s => objectName(s, l));
  check(objectPath.reduce((n, s) => n + s.length, 0) <= l.stringUnits);
  return {package: pkg, object_path: objectPath};
}
function sameIdentity(a, b) {
  return a.package === b.package && a.object_path.length === b.object_path.length &&
    a.object_path.every((s, i) => s === b.object_path[i]);
}

function readPackage(ua, ue, selected, l, profile) {
  const c = new Cursor(ua, l);
  // The explicit zero-version profile models the same bounded prefix without
  // treating absent version declarations as evidence of an engine version.
  const zeroVersion = profile === ZERO_VERSION_PROFILE;
  check(c.u32() === 0x9e2a83c1 && c.i32() === -7 &&
    c.i32() === (zeroVersion ? 0 : 864) && c.i32() === (zeroVersion ? 0 : 522) && c.i32() === 0);
  // No custom-version declarations have been qualified for this source-only
  // profile. A versioned variant needs an explicit profile extension, not an
  // assumption that an arbitrary custom version preserves these layouts.
  check(integer(c.i32(), l.customVersions) === 0);
  check(c.i32() === ua.length); c.str();
  const flags = c.u32();
  // This narrow absent-version profile requires FilterEditorOnly exactly;
  // that flag does not establish PKG_Cooked or an engine version.
  if (zeroVersion) check(flags === 0x80000000);
  else check((flags & 0x200) !== 0 && (flags & 0x2000) === 0);
  const nameCount = integer(c.i32(), l.names), nameAt = integer(c.i32()); check(nameCount > 0);
  // Gatherable editor text is outside this cooked profile.
  check(c.i32() === 0 && c.i32() === 0);
  const exportCount = integer(c.i32(), l.exports), exportAt = integer(c.i32());
  const importCount = integer(c.i32(), l.imports), importAt = integer(c.i32());
  check(exportCount > 0 && importCount > 0);
  let preloadCount = 0, preloadAt = 0;
  if (zeroVersion) {
    // Explicit summary layout, independently implemented from UAssetAPI:
    // https://github.com/atenfyr/UAssetAPI/blob/b4e8a012939295a6fd890ddc64e57ea78b709b95/UAssetAPI/UAsset.cs#L1681-L1776
    // Other pointed-to tables remain opaque; their contents are not counted.
    integer(c.i32(), ua.length); // DependsOffset
    integer(c.i32(), l.imports); integer(c.i32(), ua.length); // soft package references
    integer(c.i32(), ua.length); integer(c.i32(), ua.length); // searchable names, thumbnails
    c.skip(16); // PackageGuid; FilterEditorOnly omits PersistentGuid.
    const generations = integer(c.i32(), l.generations);
    for (let i = 0; i < generations; i++) { integer(c.i32(), l.exports); integer(c.i32(), l.names); }
    for (let i = 0; i < 2; i++) { c.u16(); c.u16(); c.u16(); c.u32(); c.str(); }
    check(c.u32() === 0 && c.i32() === 0); // no package-level compression
    c.u32(); check(c.i32() === 0); // PackageSource; additional packages unsupported
    integer(c.i32(), ua.length); // asset registry offset
    integer(c.i64(), ua.length + ue.length); // bulk start; no bulk payload is read
    integer(c.i32(), ua.length); // world tile offset
    const chunks = integer(c.i32(), l.chunkIDs);
    for (let i = 0; i < chunks; i++) integer(c.i32(), 0x7fffffff);
    preloadCount = integer(c.i32(), l.preloadDependencies); preloadAt = integer(c.i32(), ua.length);
    range(preloadAt, preloadCount * 4, ua.length);
  }
  const summaryPrefixEnd = c.at;
  const nc = new Cursor(ua, l, nameAt), names = [];
  check(nameAt >= summaryPrefixEnd);
  for (let i = 0; i < nameCount; i++) { const s = nc.str(); check(s.length > 0); names.push(s); nc.skip(4); }
  const sections = [[nameAt, nc.at], [importAt, importAt + importCount * 28], [exportAt, exportAt + exportCount * 104]];
  if (preloadCount > 0) sections.push([preloadAt, preloadAt + preloadCount * 4]);
  for (const [start, end] of sections) { check(start >= summaryPrefixEnd); range(start, end - start, ua.length); }
  sections.sort((a, b) => a[0] - b[0]);
  for (let i = 1; i < sections.length; i++) check(sections[i - 1][1] <= sections[i][0]);
  const fname = cursor => {
    const index = integer(cursor.i32(), names.length - 1), number = integer(cursor.i32(), 0x7fffffff);
    return {base: names[index], number, text: names[index] + (number ? '_' + (number - 1) : '')};
  };
  const imports = [], exports = [];
  for (let i = 0; i < importCount; i++) {
    const ic = new Cursor(ua, l, importAt + i * 28, importAt + (i + 1) * 28);
    imports.push({classPackage: fname(ic), className: fname(ic), outer: ic.i32(), name: fname(ic)});
  }
  for (let i = 0; i < exportCount; i++) {
    const ec = new Cursor(ua, l, exportAt + i * 104, exportAt + (i + 1) * 104);
    const e = {classIndex: ec.i32(), superIndex: ec.i32(), template: ec.i32(), outer: ec.i32(), name: fname(ec)};
    ec.u32(); e.size = ec.i64(); e.offset = ec.i64();
    ec.bool32(); ec.bool32(); ec.bool32(); ec.skip(16); ec.u32(); ec.bool32(); ec.bool32();
    const dependencyStart = ec.i32(); let dependencyCount = 0;
    for (let j = 0; j < 4; j++) {
      const count = integer(ec.i32(), l.preloadDependencies);
      dependencyCount += count; integer(dependencyCount, l.preloadDependencies);
    }
    if (!zeroVersion) check(dependencyStart === -1 && dependencyCount === 0);
    else if (dependencyStart === -1) check(dependencyCount === 0);
    else range(integer(dependencyStart), dependencyCount, preloadCount);
    // Every serial range is checked, including unrelated exports. No defaults are read.
    check(e.offset >= ua.length); range(e.offset - ua.length, e.size, ue.length);
    exports.push(e);
  }
  const validIndex = (index, nullable = true) => {
    check(Number.isInteger(index) && index >= -imports.length && index <= exports.length && (nullable || index !== 0)); return index;
  };
  for (const i of imports) { validIndex(i.outer); check(i.outer <= 0); }
  for (const e of exports) { validIndex(e.classIndex, false); validIndex(e.superIndex); validIndex(e.template); validIndex(e.outer); }
  if (preloadCount > 0) {
    const dependencies = new Cursor(ua, l, preloadAt, preloadAt + preloadCount * 4);
    for (let i = 0; i < preloadCount; i++) validIndex(dependencies.i32(), false);
  }
  const spans = exports.filter(e => e.size > 0).map(e => [e.offset, e.offset + e.size]).sort((a, b) => a[0] - b[0]);
  for (let i = 1; i < spans.length; i++) check(spans[i - 1][1] <= spans[i][0]);
  const entry = index => index < 0 ? imports[-index - 1] : exports[index - 1];
  const plain = (name, text) => name.number === 0 && name.base === text;
  const identities = new Map();
  // Iterative bounded traversal: cyclic or over-deep outers never reach recursion.
  const identity = index => {
    validIndex(index, false);
    if (identities.has(index)) return identities.get(index);
    let at = index; const seen = new Set(), chain = [];
    while (at !== 0) {
      check(!seen.has(at) && chain.length < l.objectDepth); seen.add(at); chain.push(at); at = entry(at).outer;
    }
    const root = entry(chain.at(-1)); let pkg = selected.package;
    if (chain.at(-1) < 0) {
      check(plain(root.classPackage, '/Script/CoreUObject') && plain(root.className, 'Package') && root.name.number === 0);
      pkg = packageName(root.name.text, l); chain.pop();
    }
    const path = chain.reverse().map(i => objectName(entry(i).name.text, l));
    check(path.reduce((n, s) => n + s.length, 0) <= l.stringUnits);
    const result = {package: pkg, object_path: path}; identities.set(index, result); return result;
  };
  for (let i = 0; i < imports.length; i++) identity(-i - 1);
  for (let i = 0; i < exports.length; i++) identity(i + 1);
  const importedType = (index, pkg, name) => {
    if (index >= 0) return false;
    const e = entry(index), id = identity(index);
    return plain(e.classPackage, '/Script/CoreUObject') && plain(e.className, 'Class') &&
      id.package === pkg && id.object_path.length === 1 && id.object_path[0] === name && e.name.number === 0;
  };
  const isClass = index => importedType(entry(index).classIndex, '/Script/Engine', 'BlueprintGeneratedClass');
  const isFunction = index => importedType(entry(index).classIndex, '/Script/CoreUObject', 'Function');
  const matches = [];
  for (let i = 0; i < exports.length; i++) if (sameIdentity(identity(i + 1), selected)) matches.push(i + 1);
  check(matches.length === 1 && isClass(matches[0]));
  const selectedIndex = matches[0];
  const folded = id => JSON.stringify([id.package.toLowerCase(), ...id.object_path.map(s => s.toLowerCase())]);
  const selectedKey = folded(selected);
  check(exports.filter((_, i) => folded(identity(i + 1)) === selectedKey).length === 1);
  const serial = index => { const e = entry(index); check(index > 0 && e.size > 0); return new Cursor(ue, l, e.offset - ua.length, e.offset - ua.length + e.size); };
  const budget = {fields: 0, children: 0, tags: 0};
  const spend = (key, n = 1) => { integer(n, l[key]); check(n <= l[key] - budget[key]); budget[key] += n; };
  const typeName = cursor => { const t = fname(cursor); check(t.number === 0 && TYPES.has(t.base)); return t.base; };
  const ref = cursor => validIndex(cursor.i32());
  const field = (cursor, depth) => {
    check(depth <= l.depth); spend('fields'); const type = typeName(cursor), name = fname(cursor);
    check(!plain(name, 'None')); objectName(name.text, l);
    cursor.u32(); const dim = integer(cursor.i32(), l.arrayDim), size = integer(cursor.i32(), l.elementSize);
    check(dim > 0 && size > 0); const flags = cursor.flags64(); cursor.u16(); fname(cursor);
    check(cursor.u8() <= 15); // ELifetimeCondition through COND_Never; sentinels unsupported.
    if (REFS.has(type) || type === 'ByteProperty') ref(cursor);
    else if (type === 'ClassProperty' || type === 'SoftClassProperty') { ref(cursor); ref(cursor); }
    else if (type === 'EnumProperty') { ref(cursor); field(cursor, depth + 1); }
    else if (type === 'BoolProperty') { cursor.skip(5); check(cursor.u8() <= 1); }
    else if (type === 'ArrayProperty' || type === 'SetProperty') field(cursor, depth + 1);
    else if (type === 'MapProperty') { field(cursor, depth + 1); field(cursor, depth + 1); }
    return {net: (flags & 0x20n) !== 0n, name: name.text.toLowerCase()};
  };
  const skipTags = cursor => {
    while (true) {
      const name = fname(cursor); if (plain(name, 'None')) break;
      objectName(name.text, l); spend('tags'); const type = typeName(cursor);
      const size = integer(cursor.i32(), l.uexpBytes); integer(cursor.i32(), l.arrayDim);
      if (type === 'StructProperty') { fname(cursor); cursor.skip(16); }
      else if (type === 'BoolProperty') check(cursor.u8() <= 1);
      else if (type === 'ByteProperty' || type === 'EnumProperty') fname(cursor);
      else if (type === 'ArrayProperty' || type === 'SetProperty') typeName(cursor);
      else if (type === 'MapProperty') { typeName(cursor); typeName(cursor); }
      const guid = cursor.u8(); check(guid === 0 || guid === 1); if (guid) cursor.skip(16);
      cursor.skip(size);
    }
    // This prefix profile does not cover an object GUID payload.
    check(cursor.i32() === 0);
  };
  const structPrefix = index => {
    const cursor = serial(index); skipTags(cursor); const superIndex = ref(cursor), childCount = integer(cursor.i32(), l.children);
    spend('children', childCount); range(cursor.at, childCount * 4, cursor.end);
    const children = [], seen = new Set();
    for (let i = 0; i < childCount; i++) {
      const child = ref(cursor); check(child > 0 && child !== index && !seen.has(child) && entry(child).outer === index);
      seen.add(child); children.push(child);
    }
    const fieldCount = integer(cursor.i32(), l.fields); check(fieldCount <= l.fields - budget.fields);
    let netProperties = 0; const fieldNames = new Set();
    for (let i = 0; i < fieldCount; i++) {
      const property = field(cursor, 1); check(!fieldNames.has(property.name)); fieldNames.add(property.name);
      if (property.net) netProperties++;
    }
    const bytecodeSize = integer(cursor.i32(), l.scriptBytes), storageSize = integer(cursor.i32(), l.scriptBytes);
    check((bytecodeSize === 0) === (storageSize === 0)); cursor.skip(storageSize);
    return {cursor, superIndex, children, netProperties};
  };
  const ownClass = entry(selectedIndex), own = structPrefix(selectedIndex);
  check(own.superIndex === ownClass.superIndex && own.superIndex !== selectedIndex && own.superIndex !== 0);
  // A superclass must denote a class, not merely any valid package index.
  const superEntry = entry(own.superIndex);
  if (own.superIndex > 0) check(isClass(own.superIndex));
  else check((plain(superEntry.classPackage, '/Script/CoreUObject') && plain(superEntry.className, 'Class')) ||
    (plain(superEntry.classPackage, '/Script/Engine') && plain(superEntry.className, 'BlueprintGeneratedClass')));
  const superChain = new Set([selectedIndex]); let ancestor = own.superIndex;
  while (ancestor > 0) {
    check(superChain.size < l.objectDepth && !superChain.has(ancestor) && isClass(ancestor));
    superChain.add(ancestor); ancestor = entry(ancestor).superIndex;
  }
  if (ancestor < 0) {
    const terminal = entry(ancestor);
    check((plain(terminal.classPackage, '/Script/CoreUObject') && plain(terminal.className, 'Class')) ||
      (plain(terminal.classPackage, '/Script/Engine') && plain(terminal.className, 'BlueprintGeneratedClass')));
  }
  const childSet = new Set(own.children);
  // Do not silently undercount a function export omitted from the class children.
  for (let i = 0; i < exports.length; i++) if (entry(i + 1).outer === selectedIndex && isFunction(i + 1)) check(childSet.has(i + 1));
  let netFunctions = 0;
  const functionNames = new Set();
  for (const child of own.children) {
    check(isFunction(child)); const name = entry(child).name.text.toLowerCase(); check(!functionNames.has(name)); functionNames.add(name);
    const f = structPrefix(child), exportedSuper = entry(child).superIndex;
    check(f.superIndex === exportedSuper);
    check(f.children.length === 0 && f.superIndex !== child && exportedSuper !== child);
    for (const superIndex of [f.superIndex, exportedSuper]) {
      if (superIndex > 0) check(isFunction(superIndex));
      else if (superIndex < 0) check(plain(entry(superIndex).classPackage, '/Script/CoreUObject') &&
        plain(entry(superIndex).className, 'Function'));
      const supers = new Set([child]); let next = superIndex;
      while (next > 0) {
        check(supers.size < l.objectDepth && !supers.has(next) && isFunction(next));
        supers.add(next); next = entry(next).superIndex;
      }
      if (next < 0) check(plain(entry(next).classPackage, '/Script/CoreUObject') &&
        plain(entry(next).className, 'Function'));
    }
    const flags = f.cursor.u32();
    if ((flags & 0x40) !== 0 && f.superIndex === 0 && exportedSuper === 0) netFunctions++;
  }
  return {selected_identity: identity(selectedIndex), superclass: identity(own.superIndex),
    local_net_property_count: own.netProperties, local_non_overriding_net_function_count: netFunctions};
}

/**
 * Count local fields in exactly one generated class. Identity is
 * {package: '/Game/Path/Asset', object_path: ['Outer', 'BP_Name_C']}.
 * Limits may only be lowered. Results contain identities, counts, profile and
 * SHA-256 of the supplied buffers; no values, names of fields or code escape.
 * Unparsed export tails are intentionally opaque, not claimed as validated.
 */
function countCookedClassFields(uasset, uexp, identity, options = {}) {
  try {
    check(options && typeof options === 'object' && !Array.isArray(options));
    check(Object.keys(options).every(key => key === 'limits' || key === 'profile'));
    const profile = options.profile === undefined ? PROFILE : options.profile;
    check(profile === PROFILE || profile === ZERO_VERSION_PROFILE);
    const l = limitsFor(options.limits === undefined ? {} : options.limits), selected = requestedIdentity(identity, l);
    check(Buffer.isBuffer(uasset) && Buffer.isBuffer(uexp));
    integer(uasset.length, l.uassetBytes); integer(uexp.length, l.uexpBytes); check(uasset.length > 0 && uexp.length > 0);
    // Shared backing stores could change concurrently between parsing and digesting.
    check(!(uasset.buffer instanceof SharedArrayBuffer) && !(uexp.buffer instanceof SharedArrayBuffer));
    const result = readPackage(uasset, uexp, selected, l, profile);
    return {...result, parser_profile: profile, buffer_digests: {
      uasset_sha256: crypto.createHash('sha256').update(uasset).digest('hex'),
      uexp_sha256: crypto.createHash('sha256').update(uexp).digest('hex')}};
  } catch { throw new Error(MESSAGE); }
}

module.exports = {PROFILE, ZERO_VERSION_PROFILE, LIMITS, countCookedClassFields};
