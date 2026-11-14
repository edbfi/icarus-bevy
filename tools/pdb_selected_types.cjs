'use strict';

// Independently authored supported subset from LLVM llvmorg-20.1.8:
// llvm/docs/PDB/{TpiStream,CodeViewTypes}.rst,
// llvm/lib/DebugInfo/CodeView/{TypeRecordMapping,RecordSerialization,CodeViewRecordIO}.cpp,
// llvm/include/llvm/DebugInfo/CodeView/{CodeView,TypeRecord}.h;
// and Microsoft's microsoft-pdb/include/cvinfo.h (MIT).
// No upstream implementation or original game code is copied.
// Pure API: reader.size and reader.read(offset,length) describe ONLY the TPI
// logical stream, already bound by the caller to a qualified MSF container.
// Caller must supply immutable exact reads and establish hashes, identity,
// authorization and provenance. This API performs no filesystem/network I/O.
// Complete outer framing is NOT complete semantic validation of unselected
// records/hash tables. Unknown selected/dependent forms fail atomically.
const {isSharedArrayBuffer} = require('node:util').types;

const PROFILE = 'tpi_v80_exact_udt_data_members_v1';
const PARAMETER_PROFILE = 'tpi_v80_exact_parameter_layouts_v1';
const ENUM_PROFILE = 'tpi_v80_exact_enum_values_v1';
const DECLARATION_PROFILE = 'tpi_v80_exact_udt_declarations_v1';
// Enum extension layout checked against the primary sources:
// https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-20.1.8/llvm/lib/DebugInfo/CodeView/TypeRecordMapping.cpp
// visitKnownRecord(EnumRecord), visitKnownMember(EnumeratorRecord): fixed enum
// header; enumerator attributes, encoded integer, modern zero-terminated name.
// https://raw.githubusercontent.com/microsoft/microsoft-pdb/master/include/cvinfo.h
// lfEnumerate independently identifies the attribute + variable numeric tail.
const CAPS = Object.freeze({
    stream_bytes: 512 * 1024 * 1024,
    records: 6000000,
    record_bytes: 65536,
    read_bytes: 65536,
    selectors: 8,
    name_bytes: 256,
    unique_name_bytes: 4096,
    candidates: 64,
    fields: 4096,
    continuations: 64,
    dependencies: 256,
    depth: 32,
    udt_bytes: 1024 * 1024,
    retained_bytes: 64 * 1024 * 1024,
    output_bytes: 65536,
});
const ENUM_CAPS = Object.freeze({...CAPS, enumerators: 4096});
const DECLARATION_CAPS = Object.freeze({...CAPS, declarations: 4096,
    weighted_members: 8192, methods: 8192, references: 4096,
    parameters: 256, argument_types: 4096});
// Opaque simple references use LLVM TypeIndex.h kinds/modes, independently
// of the deliberately smaller storage-width subset below. None is admitted
// only for a static signature's absent this; NotTranslated is unsupported.
const OPAQUE_SIMPLE = new Set([3, 8, 0x10, 0x20, 0x70, 0x71, 0x7a, 0x7b, 0x7c,
    0x68, 0x69, 0x11, 0x21, 0x72, 0x73, 0x12, 0x22, 0x74, 0x75,
    0x13, 0x23, 0x76, 0x77, 0x14, 0x24, 0x78, 0x79,
    0x46, 0x40, 0x45, 0x44, 0x41, 0x42, 0x43,
    0x56, 0x50, 0x55, 0x54, 0x51, 0x52, 0x53, 0x30, 0x31, 0x32, 0x33, 0x34]);
const LEAF = Object.freeze({
    MODIFIER: 0x1001, POINTER: 0x1002, FIELDLIST: 0x1203, BITFIELD: 0x1205,
    CLASS: 0x1504, STRUCTURE: 0x1505, ENUM: 0x1507, INTERFACE: 0x1519,
    MEMBER: 0x150d, INDEX: 0x1404, ENUMERATE: 0x1502,
    BCLASS: 0x1400, ONEMETHOD: 0x1511, METHOD: 0x150f, NESTTYPE: 0x1510,
    MFUNCTION: 0x1009, METHODLIST: 0x1206, ARGLIST: 0x1201,
});
const GENERIC_ERROR = 'invalid or unsupported selected TPI types';
const need = ok => { if (!ok) throw Error(GENERIC_ERROR); };
const plain = value => value !== null && typeof value === 'object'
    && !Array.isArray(value)
    && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);

function settings(options, enums = false) {
    need(plain(options));
    need(Reflect.ownKeys(options).every(key => typeof key === 'string'
        && ['profile', 'names', 'limits'].includes(key)));
    need(Object.hasOwn(options, 'profile') && (enums ? options.profile === ENUM_PROFILE : [PROFILE, PARAMETER_PROFILE, DECLARATION_PROFILE].includes(options.profile)) && Object.hasOwn(options, 'names'));
    const caps = enums ? ENUM_CAPS : options.profile === DECLARATION_PROFILE ? DECLARATION_CAPS : CAPS;
    const limits = {...caps};
    if (options.limits !== undefined) {
        need(plain(options.limits));
        for (const key of Reflect.ownKeys(options.limits)) {
            need(typeof key === 'string' && Object.hasOwn(caps, key));
            const value = options.limits[key];
            need(Number.isSafeInteger(value) && value >= 0 && value <= caps[key]);
            limits[key] = value;
        }
    }
    const selectedNames = options.names;
    need(Array.isArray(selectedNames));
    const selectorCount = selectedNames.length;
    need(Number.isSafeInteger(selectorCount) && selectorCount > 0 && selectorCount <= limits.selectors);
    const names = [], seen = new Set();
    for (let index = 0; index < selectorCount; index++) {
        need(Object.hasOwn(selectedNames, index));
        const name = selectedNames[index];
        need(typeof name === 'string' && /^[\x21-\x7e]+$/.test(name)
            && name.length <= limits.name_bytes && !seen.has(name));
        seen.add(name);
        names.push({name, bytes: Buffer.from(name, 'ascii'), declarations: [], definitions: []});
    }
    return {limits, names};
}

class Cursor {
    constructor(bytes) { this.bytes = bytes; this.at = 4; }
    take(length) {
        need(Number.isSafeInteger(length) && length >= 0 && this.at + length <= this.bytes.length);
        const start = this.at; this.at += length;
        return this.bytes.subarray(start, this.at);
    }
    u8() { return this.take(1)[0]; }
    u16() { return this.take(2).readUInt16LE(); }
    u32() { return this.take(4).readUInt32LE(); }
    integer() {
        const leaf = this.u16();
        if (leaf < 0x8000) return BigInt(leaf);
        let value;
        switch (leaf) {
        case 0x8000: value = BigInt(this.take(1).readInt8()); break;
        case 0x8001: value = BigInt(this.take(2).readInt16LE()); break;
        case 0x8002: value = BigInt(this.u16()); break;
        case 0x8003: value = BigInt(this.take(4).readInt32LE()); break;
        case 0x8004: value = BigInt(this.u32()); break;
        case 0x8009: value = this.take(8).readBigInt64LE(); break;
        case 0x800a: value = this.take(8).readBigUInt64LE(); break;
        default: need(false);
        }
        return value;
    }
    numeric() {
        const value = this.integer();
        need(value >= 0n && value <= BigInt(Number.MAX_SAFE_INTEGER));
        return Number(value);
    }
    name() {
        const end = this.bytes.indexOf(0, this.at);
        need(end >= this.at);
        const value = this.bytes.subarray(this.at, end);
        this.at = end + 1;
        return value;
    }
    pad() {
        const count = (4 - this.at % 4) % 4;
        const bytes = this.take(count);
        for (let index = 0; index < count; index++) need(bytes[index] === 0xf0 + count - index);
    }
    finish() { this.pad(); need(this.at === this.bytes.length); }
}

function text(bytes, cap, allowEmpty = false) {
    need(bytes.length <= cap && (allowEmpty || bytes.length > 0));
    need(bytes.every(byte => byte >= 0x21 && byte <= 0x7e));
    return bytes.toString('ascii');
}

function candidateName(record) {
    const c = new Cursor(record);
    c.take(16);
    // Name discovery necessarily requires the fixed prefix, a supported
    // integer numeric-leaf width, and a terminated name. Do not interpret the
    // size value, unique name or padding until the exact name is selected.
    const leaf = c.u16();
    if (leaf >= 0x8000) {
        const width = new Map([[0x8000, 1], [0x8001, 2], [0x8002, 2],
            [0x8003, 4], [0x8004, 4], [0x8009, 8], [0x800a, 8]]).get(leaf);
        need(width !== undefined);
        c.take(width);
    }
    return c.name();
}

function tag(record) {
    const c = new Cursor(record);
    const count = c.u16(), flags = c.u16(), fieldList = c.u32();
    const derived = c.u32(), vshape = c.u32(), size = c.numeric();
    const name = c.name(), unique = flags & 0x200 ? c.name() : null;
    c.finish();
    return {count, flags, fieldList, derived, vshape, size, name, unique};
}

// Explicit direct simple-type widths; no host C/C++ sizeof assumptions.
const SIMPLE = new Map();
for (const [width, signed, indices] of [
    [1, true, [0x10, 0x68]], [1, false, [0x20, 0x69]],
    [2, true, [0x11, 0x72]], [2, false, [0x21, 0x73]],
    [4, true, [0x12, 0x74]], [4, false, [0x22, 0x75]],
    [8, true, [0x13, 0x76]], [8, false, [0x23, 0x77]],
]) for (const index of indices) SIMPLE.set(index, {kind: 'primitive', storage_kind: 'integer', width_bytes: width, signed});
for (const [index, width] of [[0x30, 1], [0x31, 2], [0x32, 4], [0x33, 8]]) {
    SIMPLE.set(index, {kind: 'primitive', storage_kind: 'boolean', width_bytes: width, signed: false});
}
for (const [index, width] of [[0x70, 1], [0x71, 2], [0x7a, 2], [0x7b, 4], [0x7c, 1]]) {
    SIMPLE.set(index, {kind: 'primitive', storage_kind: 'character', width_bytes: width, signed: null});
}

function inspectSelectedTypes(reader, options) {
    try { return inspectImpl(reader, options); }
    catch { throw Error(GENERIC_ERROR); }
}
function inspectSelectedEnums(reader, options) {
    try { return inspectImpl(reader, options, true); }
    catch { throw Error(GENERIC_ERROR); }
}

function inspectImpl(reader, options, enums = false) {
    const {limits, names} = settings(options, enums);
    const parameters = !enums && options.profile === PARAMETER_PROFILE;
    const declarations = !enums && options.profile === DECLARATION_PROFILE;
    need(reader && Number.isSafeInteger(reader.size) && reader.size >= 56
        && reader.size <= limits.stream_bytes && typeof reader.read === 'function');
    const streamSize = reader.size;
    let charged = 0;
    const charge = bytes => {
        need(Number.isSafeInteger(bytes) && bytes >= 0 && charged + bytes <= limits.retained_bytes);
        charged += bytes;
    };
    // Fixed logical-byte charging bounds retained buffers and selected results;
    // it is not a measurement/guarantee of JavaScript object overhead or RSS.
    charge(limits.read_bytes * 2);
    for (const selection of names) charge(selection.bytes.length + 128);
    function exact(offset, length) {
        need(Number.isSafeInteger(offset) && Number.isSafeInteger(length)
            && offset >= 0 && length >= 0 && length <= limits.read_bytes
            && offset + length <= streamSize);
        const bytes = reader.read(offset, length);
        need(Buffer.isBuffer(bytes) && bytes.length === length && !isSharedArrayBuffer(bytes.buffer));
        return Buffer.from(bytes);
    }
    need(limits.read_bytes >= 56);
    const header = exact(0, 56);
    const begin = header.readUInt32LE(8), end = header.readUInt32LE(12);
    const recordBytes = header.readUInt32LE(16);
    need(header.readUInt32LE(0) === 20040203 && header.readUInt32LE(4) === 56);
    need(begin === 0x1000 && end >= begin && end < 0x80000000);
    const count = end - begin;
    need(count <= limits.records && count * 4 <= recordBytes && 56 + recordBytes === streamSize);
    need(header.readUInt32LE(24) === 4);
    const buckets = header.readUInt32LE(28);
    need(buckets >= 0x1000 && buckets <= 0x40000);
    const hashStream = header.readUInt16LE(20), auxiliary = header.readUInt16LE(22);
    need(auxiliary === 0xffff);
    const hashBytes = header.readUInt32LE(36), indexBytes = header.readUInt32LE(44);
    need(hashBytes === 0 || hashBytes === count * 4);
    need(indexBytes % 8 === 0 && header.readUInt32LE(52) === 0);
    for (const offset of [32, 40, 48]) need(header.readInt32LE(offset) >= 0);
    if (hashStream === 0xffff) need(hashBytes === 0 && indexBytes === 0);
    // Hash/auxiliary/index tables are never read or relied upon for lookup.
    // Nonempty hash-adjustment tables (incremental type replacement) are outside
    // this subset, rather than silently choosing an arbitrary prior definition.
    charge((count + 1) * 4);
    const offsets = new Uint32Array(count + 1);
    let cacheAt = -1, cache = null;
    function read(offset, length) {
        need(length <= limits.read_bytes);
        if (length === 0) return Buffer.alloc(0);
        if (cache !== null && offset >= cacheAt && offset + length <= cacheAt + cache.length) {
            return cache.subarray(offset - cacheAt, offset - cacheAt + length);
        }
        const start = Math.floor(offset / limits.read_bytes) * limits.read_bytes;
        cache = exact(start, Math.min(limits.read_bytes, streamSize - start));
        cacheAt = start;
        if (offset + length <= start + cache.length) return cache.subarray(offset - start, offset - start + length);
        return exact(offset, length);
    }
    let at = 56, candidates = 0;
    for (let ordinal = 0; ordinal < count; ordinal++) {
        offsets[ordinal] = at;
        const prefix = read(at, 4);
        const length = prefix.readUInt16LE() + 2, leaf = prefix.readUInt16LE(2);
        need(length >= 4 && length <= limits.record_bytes && length <= limits.read_bytes
            && length % 4 === 0 && at + length <= streamSize);
        if (enums ? leaf === LEAF.ENUM : [LEAF.CLASS, LEAF.STRUCTURE, LEAF.INTERFACE].includes(leaf)) {
            const record = read(at, length);
            const probe = enums ? new Cursor(record) : null;
            if (probe) probe.take(12);
            const name = probe ? probe.name() : candidateName(record);
            const selection = names.find(value => value.bytes.equals(name));
            if (selection) {
                let parsed;
                if (enums) {
                    const c = new Cursor(record);
                    const count = c.u16(), flags = c.u16(), underlying = c.u32(), fieldList = c.u32();
                    const enumName = c.name(), unique = flags & 0x200 ? c.name() : null;
                    c.finish();
                    parsed = {count, flags, underlying, fieldList, name: enumName, unique};
                } else parsed = tag(record);
                need(++candidates <= limits.candidates);
                need(leaf !== LEAF.INTERFACE);
                const unique = parsed.unique === null ? null : text(parsed.unique, limits.unique_name_bytes);
                charge((unique === null ? 0 : unique.length) + 128);
                const item = {
                    type_index: begin + ordinal, leaf, flags: parsed.flags,
                    ...(declarations ? {record_offset: at, record_bytes: length} : {}),
                    ...(enums ? {declared_enumerator_count: parsed.count} : {declared_member_count: parsed.count}), field_list: parsed.fieldList,
                    ...(enums ? {underlying_type_index: parsed.underlying} : {size_bytes: parsed.size}), unique_name: unique,
                };
                if (!enums) need(parsed.derived === 0 && parsed.vshape === 0);
                if (parsed.flags & 0x80) {
                    need(parsed.count === 0 && parsed.fieldList === 0 && (enums || parsed.size === 0));
                    selection.declarations.push(item);
                } else {
                    if (!enums) need(parsed.size > 0 && parsed.size <= limits.udt_bytes);
                    selection.definitions.push(item);
                }
            }
        }
        at += length;
    }
    offsets[count] = at;
    need(at === streamSize);
    function validIndex(index) { need(Number.isInteger(index) && index >= begin && index < end); }
    function record(index) {
        validIndex(index);
        const ordinal = index - begin;
        const bytes = read(offsets[ordinal], offsets[ordinal + 1] - offsets[ordinal]);
        return {bytes, leaf: bytes.readUInt16LE(2)};
    }
    function validReference(index) {
        if (index >= begin) validIndex(index);
        else {
            const simple = index & 0xff, mode = index >>> 8;
            need(SIMPLE.has(index) || index === 3 || (mode === 6 && (simple === 3 || SIMPLE.has(simple))));
        }
    }
    let dependencyCount = 0;
    const memo = new Map(), visiting = new Set();
    function width(index, depth = 0) {
        need(depth <= limits.depth);
        if (SIMPLE.has(index)) return {...SIMPLE.get(index)};
        if (index < begin) {
            const simple = index & 0xff;
            need(index >>> 8 === 6 && (simple === 3 || SIMPLE.has(simple)));
            return {kind: 'pointer', storage_kind: 'pointer', width_bytes: 8, referent_type_index: simple};
        }
        validIndex(index);
        if (memo.has(index)) return memo.get(index);
        need(!visiting.has(index) && ++dependencyCount <= limits.dependencies);
        visiting.add(index);
        const entry = record(index), c = new Cursor(entry.bytes);
        let result;
        if (entry.leaf === LEAF.POINTER) {
            const referent = c.u32(), attributes = c.u32();
            validReference(referent);
            need((attributes & 0x1f) === 0x0c && ((attributes >>> 5) & 7) === 0);
            need((attributes >>> 19) === 0 && ((attributes >>> 13) & 0x3f) === 8);
            result = {kind: 'pointer', storage_kind: 'pointer', width_bytes: 8, referent_type_index: referent, attributes};
        } else if (entry.leaf === LEAF.MODIFIER) {
            const modified = c.u32(), modifiers = c.u16();
            need((modifiers & ~7) === 0);
            const base = width(modified, depth + 1);
            need(base.kind !== 'bitfield');
            result = {kind: 'modifier', storage_kind: base.storage_kind, width_bytes: base.width_bytes,
                modified_type_index: modified, modifiers, signed: base.signed ?? null};
        } else if (entry.leaf === LEAF.BITFIELD) {
            const storage = c.u32(), bits = c.u8(), position = c.u8();
            const base = width(storage, depth + 1);
            need(['integer', 'boolean'].includes(base.storage_kind) && base.kind !== 'bitfield');
            need(bits > 0 && position + bits <= base.width_bytes * 8);
            result = {kind: 'bitfield', storage_kind: base.storage_kind, width_bytes: base.width_bytes,
                storage_type_index: storage, bit_size: bits, bit_offset: position, signed: base.signed};
        } else if (entry.leaf === LEAF.ENUM) {
            const members = c.u16(), flags = c.u16(), underlying = c.u32(), fieldList = c.u32();
            need((flags & 0x80) === 0);
            const enumName = c.name();
            need(enumName.length > 0);
            if (flags & 0x200) c.name();
            if (fieldList !== 0) {
                validIndex(fieldList);
                need(record(fieldList).leaf === LEAF.FIELDLIST);
            } else need(members === 0);
            const base = width(underlying, depth + 1);
            need(base.storage_kind === 'integer' && base.kind !== 'bitfield');
            result = {kind: 'enum', storage_kind: 'integer', width_bytes: base.width_bytes,
                underlying_type_index: underlying, signed: base.signed, enumerators_interpreted: false};
        } else if (parameters && entry.leaf === 0x1503) {
            const element = c.u32(), indexType = c.u32(), size = c.numeric();
            validReference(indexType);
            need(SIMPLE.has(indexType) && SIMPLE.get(indexType).storage_kind === 'integer');
            const name = text(c.name(), limits.name_bytes, true), base = width(element, depth + 1);
            need(size > 0 && size <= limits.udt_bytes && size % base.width_bytes === 0);
            result = {kind: 'array', storage_kind: 'array', width_bytes: size,
                element_type_index: element, index_type_index: indexType, name,
                element_count: size / base.width_bytes};
        } else if (parameters && [LEAF.CLASS, LEAF.STRUCTURE].includes(entry.leaf)) {
            const parsed = tag(entry.bytes);
            let resolved = index;
            if (parsed.flags & 0x80) {
                need(parsed.count === 0 && parsed.fieldList === 0 && parsed.size === 0
                    && parsed.derived === 0 && parsed.vshape === 0);
                if (parsed.unique !== null) text(parsed.unique, limits.unique_name_bytes);
                const matches = [];
                let matchingCandidates = 0;
                for (let target = begin; target < end; target++) {
                    const candidate = record(target);
                    if (candidate.leaf !== entry.leaf) continue;
                    if (!candidateName(candidate.bytes).equals(parsed.name)) continue;
                    need(++matchingCandidates <= limits.candidates);
                    const value = tag(candidate.bytes);
                    if (!(value.flags & 0x80)) {
                        need((value.unique === null) === (parsed.unique === null)
                            && (value.unique === null || value.unique.equals(parsed.unique)));
                        matches.push(target);
                    }
                }
                need(matches.length === 1); resolved = matches[0];
                result = {...width(resolved, depth + 1), resolved_type_index: resolved};
            } else {
                need(parsed.derived === 0 && parsed.vshape === 0 && parsed.size > 0 && parsed.size <= limits.udt_bytes);
                const definition = {field_list: parsed.fieldList, declared_member_count: parsed.count, size_bytes: parsed.size};
                result = {kind: 'udt', storage_kind: 'udt', width_bytes: parsed.size,
                    name: text(parsed.name, limits.name_bytes), fields: fields(definition, depth + 1)};
                if (parsed.unique !== null) text(parsed.unique, limits.unique_name_bytes);
            }
            c.at = c.bytes.length;
        } else need(false);
        c.finish();
        visiting.delete(index);
        charge(128);
        memo.set(index, result);
        return result;
    }

    let fieldsTotal = 0;
    function fields(definition, depth = 0) {
        need(depth <= limits.depth);
        const result = [], seenLists = new Set();
        let next = definition.field_list;
        if (next === 0) { need(definition.declared_member_count === 0); return result; }
        while (next !== 0) {
            need(!seenLists.has(next) && seenLists.size < limits.continuations);
            seenLists.add(next);
            const entry = record(next);
            need(entry.leaf === LEAF.FIELDLIST);
            const c = new Cursor(entry.bytes);
            next = 0;
            while (c.at < c.bytes.length) {
                const kind = c.u16();
                if (kind === LEAF.INDEX) {
                    need(c.u16() === 0);
                    next = c.u32(); validIndex(next);
                    c.finish(); // continuation must be terminal in this list
                    break;
                }
                need((kind === LEAF.MEMBER || (parameters && kind === 0x1400)) && ++fieldsTotal <= limits.fields);
                const attributes = c.u16(), typeIndex = c.u32(), offset = c.numeric();
                need((attributes & ~3) === 0);
                const isBase = kind === 0x1400;
                const name = isBase ? null : text(c.name(), limits.name_bytes, true);
                const layout = width(typeIndex, depth);
                if (isBase) need(layout.kind === 'udt');
                need(offset + layout.width_bytes <= definition.size_bytes);
                charge((name === null ? 0 : name.length) + 192);
                result.push(parameters
                    ? {entry_kind: isBase ? 'base' : 'member', ...layout,
                        ...(layout.kind === 'pointer' && Object.hasOwn(layout, 'attributes')
                            ? {pointer_attributes: layout.attributes} : {}),
                        name, attributes, type_index: typeIndex, offset_bytes: offset}
                    : {name, attributes, type_index: typeIndex, offset_bytes: offset, ...layout});
                c.pad();
            }
        }
        // This profile admits only data members plus continuation metadata.
        // No guessed overload/base/member-count relationship is necessary.
        need(result.length === definition.declared_member_count);
        if (parameters) {
            for (let i = 0; i < result.length; i++) for (let j = 0; j < i; j++) {
                const a = result[i], b = result[j];
                const disjoint = a.offset_bytes >= b.offset_bytes + b.width_bytes || b.offset_bytes >= a.offset_bytes + a.width_bytes;
                const bits = a.kind === 'bitfield' && b.kind === 'bitfield' && a.offset_bytes === b.offset_bytes
                    && a.width_bytes === b.width_bytes && (a.bit_offset >= b.bit_offset + b.bit_size || b.bit_offset >= a.bit_offset + a.bit_size);
                need(disjoint || bits);
            }
        }
        return result;
    }
    // Enum values are declaration metadata, not runtime flag meanings. Only
    // integer simple types through bounded modifier chains are supported.
    function integerUnderlying(index, seen = new Set()) {
        need(seen.size <= limits.depth && !seen.has(index));
        if (SIMPLE.has(index)) {
            const base = SIMPLE.get(index);
            need(base.storage_kind === 'integer');
            return;
        }
        const entry = record(index), c = new Cursor(entry.bytes);
        need(entry.leaf === LEAF.MODIFIER);
        seen.add(index);
        const modified = c.u32(), modifiers = c.u16();
        need((modifiers & ~7) === 0); c.finish();
        integerUnderlying(modified, seen);
    }
    let enumeratorsTotal = 0;
    function enumValues(definition) {
        integerUnderlying(definition.underlying_type_index);
        const base = width(definition.underlying_type_index);
        const bits = BigInt(base.width_bytes * 8);
        const minimum = base.signed ? -(1n << (bits - 1n)) : 0n;
        const maximum = (1n << (base.signed ? bits - 1n : bits)) - 1n;
        const result = [], seenLists = new Set(), seenNames = new Set();
        let next = definition.field_list;
        if (!next) need(definition.declared_enumerator_count === 0);
        while (next !== 0) {
            need(!seenLists.has(next) && seenLists.size < limits.continuations);
            seenLists.add(next);
            const entry = record(next); need(entry.leaf === LEAF.FIELDLIST);
            const c = new Cursor(entry.bytes); next = 0;
            while (c.at < c.bytes.length) {
                const kind = c.u16();
                if (kind === LEAF.INDEX) {
                    need(c.u16() === 0); next = c.u32(); validIndex(next);
                    c.finish(); break;
                }
                need(kind === LEAF.ENUMERATE && ++enumeratorsTotal <= limits.enumerators);
                const attributes = c.u16(); need((attributes & ~3) === 0);
                const value = c.integer(), name = text(c.name(), limits.name_bytes);
                need(value >= minimum && value <= maximum && !seenNames.has(name));
                seenNames.add(name); charge(name.length + 160);
                result.push({name, attributes, value_decimal: value.toString(10)});
                c.pad();
            }
        }
        need(result.length === definition.declared_enumerator_count);
        return {width_bytes: base.width_bytes, signed: base.signed, enumerators: result};
    }
    // Declaration-only path. Numeric references are not requests to derive a
    // storage layout. In particular, method return/class/this/argument records
    // remain opaque even when their leaf would be unsupported by width().
    // Grammar: LLVM 20.1.8 TypeRecordMapping.cpp (MapOneMethodRecord,
    // MemberFunctionRecord, ArgListRecord) and CodeView.h MethodOptions.
    const directReferences = new Map(), referenceMemo = new Map(), referenceHeights = new Map(), referenceVisiting = new Set();
    let declarationsTotal = 0, weightedTotal = 0, methodsTotal = 0, methodEntries = 0, argumentTypes = 0;
    function reference(index, allowNone = false) {
        need(Number.isInteger(index) && index >= 0);
        if (index === 0) need(allowNone);
        else if (index >= begin) validIndex(index);
        else need(index < 0x800 && OPAQUE_SIMPLE.has(index & 0xff));
        if (!directReferences.has(index)) {
            need(directReferences.size < limits.references);
            const entry = index >= begin ? record(index) : null;
            const ordinal = index - begin;
            charge(160);
            directReferences.set(index, {type_index: index, leaf: entry?.leaf ?? null,
                ...(entry ? {record_offset: offsets[ordinal], record_bytes: offsets[ordinal + 1] - offsets[ordinal]} : {}),
                interpretation: 'opaque'});
        }
        return directReferences.get(index);
    }
    function methodAttributes(attributes) {
        const methodKind = (attributes >>> 2) & 7;
        // Only access, vanilla/static and CompilerGenerated are admitted.
        need((attributes & ~0x11f) === 0 && (attributes & 3) !== 0
            && (methodKind === 0 || methodKind === 2));
        return methodKind;
    }
    function methodName(c) {
        const bytes = c.name();
        need(bytes.length > 0 && bytes.length <= limits.name_bytes
            && bytes.every(byte => byte >= 0x20 && byte <= 0x7e));
        return bytes.toString('ascii');
    }
    function signature(index, attributes) {
        const methodKind = methodAttributes(attributes);
        const value = declarationRecord(index, LEAF.MFUNCTION);
        need(methodKind === 2 ? value.this_type_index === 0 : value.this_type_index !== 0);
        return value;
    }
    function declarationRecord(index, expectedLeaf, depth = 0) {
        need(depth <= limits.depth);
        validIndex(index);
        const ref = reference(index);
        need(ref.leaf === expectedLeaf);
        if (referenceMemo.has(index)) {
            need(depth + referenceHeights.get(index) <= limits.depth);
            return referenceMemo.get(index);
        }
        need(!referenceVisiting.has(index) && referenceMemo.size + referenceVisiting.size < limits.dependencies);
        referenceVisiting.add(index);
        const entry = record(index), c = new Cursor(entry.bytes);
        let value, height = 0;
        if (expectedLeaf === LEAF.MFUNCTION) {
            const returnType = c.u32(), classType = c.u32(), thisType = c.u32();
            validIndex(classType);
            if (thisType !== 0) validIndex(thisType);
            reference(returnType); reference(classType); reference(thisType, true);
            const callingConvention = c.u8(), options = c.u8(), parameterCount = c.u16();
            const argList = c.u32(), adjustment = c.take(4).readInt32LE();
            need(callingConvention === 0 && [0, 1, 2].includes(options)
                && adjustment === 0 && parameterCount <= limits.parameters);
            c.finish();
            const args = declarationRecord(argList, LEAF.ARGLIST, depth + 1);
            height = 1;
            need(args.argument_type_indices.length === parameterCount);
            value = {return_type_index: returnType, class_type_index: classType, this_type_index: thisType,
                calling_convention: callingConvention, options, parameter_count: parameterCount,
                argument_list_type_index: argList, this_adjustment: adjustment};
        } else if (expectedLeaf === LEAF.ARGLIST) {
            const count = c.u32();
            need(count <= limits.parameters && argumentTypes + count <= limits.argument_types
                && c.bytes.length - c.at === count * 4);
            argumentTypes += count;
            const args = [];
            for (let i = 0; i < count; i++) { const type = c.u32(); reference(type); args.push(type); }
            value = {argument_type_indices: args};
            charge(count * 8);
        } else if (expectedLeaf === LEAF.METHODLIST) {
            const methods = [];
            // Every admitted entry is exactly eight bytes: no virtual offset.
            need((c.bytes.length - c.at) % 8 === 0);
            while (c.at < c.bytes.length) {
                need(++methodEntries <= limits.methods);
                const attributes = c.u16(); methodAttributes(attributes);
                need(c.u16() === 0);
                const type = c.u32();
                const method = declarationRecord(type, LEAF.MFUNCTION, depth + 1);
                height = Math.max(height, 1 + referenceHeights.get(type));
                need(((attributes >>> 2) & 7) === 2 ? method.this_type_index === 0 : method.this_type_index !== 0);
                charge(96); methods.push({attributes, type_index: type});
            }
            need(methods.length > 0);
            value = {methods};
        } else need(false);
        c.finish();
        referenceVisiting.delete(index);
        referenceMemo.set(index, value);
        referenceHeights.set(index, height);
        ref.interpretation = 'declaration';
        Object.assign(ref, value);
        charge(192);
        return value;
    }
    function declarationValues(definition) {
        const result = [], fieldLists = [], seenLists = new Set();
        let next = definition.field_list, weighted = 0, methodCount = 0;
        if (next === 0) need(definition.declared_member_count === 0);
        while (next !== 0) {
            need(!seenLists.has(next) && seenLists.size < limits.continuations);
            seenLists.add(next);
            const listIndex = next, ref = reference(next), entry = record(next);
            need(entry.leaf === LEAF.FIELDLIST);
            const c = new Cursor(entry.bytes);
            ref.interpretation = 'selected_field_list';
            fieldLists.push({type_index: listIndex, record_offset: ref.record_offset}); next = 0;
            while (c.at < c.bytes.length) {
                const entryOffset = c.at, leaf = c.u16();
                if (leaf === LEAF.INDEX) {
                    need(c.u16() === 0); next = c.u32(); validIndex(next); reference(next);
                    fieldLists[fieldLists.length - 1].continuation_type_index = next;
                    fieldLists[fieldLists.length - 1].continuation_entry_offset = entryOffset;
                    c.finish(); break;
                }
                need(++declarationsTotal <= limits.declarations);
                let value, weight = 1, methods = 0;
                if (leaf === LEAF.MEMBER || leaf === LEAF.BCLASS) {
                    const attributes = c.u16(), type = c.u32(), offset = c.numeric();
                    need((attributes & ~3) === 0 && offset <= definition.size_bytes);
                    reference(type);
                    const isBase = leaf === LEAF.BCLASS;
                    if (isBase) need([LEAF.CLASS, LEAF.STRUCTURE].includes(record(type).leaf));
                    value = {entry_kind: isBase ? 'base' : 'member', attributes, type_index: type,
                        offset_bytes: offset, ...(isBase ? {} : {name: text(c.name(), limits.name_bytes, true)})};
                } else if (leaf === LEAF.NESTTYPE) {
                    need(c.u16() === 0); const type = c.u32(); reference(type);
                    value = {entry_kind: 'nested_type', type_index: type, name: text(c.name(), limits.name_bytes)};
                } else if (leaf === LEAF.ONEMETHOD) {
                    const attributes = c.u16(), type = c.u32();
                    signature(type, attributes);
                    value = {entry_kind: 'method', attributes, type_index: type, name: methodName(c)};
                    methods = 1;
                } else if (leaf === LEAF.METHOD) {
                    const count = c.u16(), type = c.u32();
                    need(count > 0 && count <= limits.methods);
                    const overloads = declarationRecord(type, LEAF.METHODLIST);
                    need(overloads.methods.length === count);
                    value = {entry_kind: 'overload_group', overload_count: count, type_index: type, name: methodName(c)};
                    weight = count; methods = count;
                } else need(false);
                weighted += weight; weightedTotal += weight; methodCount += methods; methodsTotal += methods;
                need(weightedTotal <= limits.weighted_members && methodsTotal <= limits.methods);
                c.pad();
                charge(256 + (value.name?.length ?? 0));
                result.push({leaf, field_list_type_index: listIndex, entry_offset: entryOffset, ...value});
            }
        }
        need(weighted === definition.declared_member_count);
        return {declarations: result, physical_member_count: result.length,
            weighted_member_count: weighted, method_count: methodCount, field_lists: fieldLists,
            fields: result.filter(value => value.entry_kind === 'member'), storage_layout_complete: false};
    }
    const selected = names.map(selection => {
        need(selection.definitions.length === 1);
        const definition = selection.definitions[0];
        if (declarations) need(selection.declarations.length <= 1);
        for (const declaration of selection.declarations) {
            need(declaration.leaf === definition.leaf);
            if (enums) need(declaration.underlying_type_index === definition.underlying_type_index
                && declaration.unique_name === definition.unique_name);
            if (declarations) need(declaration.unique_name === definition.unique_name);
            if (declaration.unique_name !== null && definition.unique_name !== null) {
                need(declaration.unique_name === definition.unique_name);
            }
        }
        return {name: selection.name, ...definition, forward_declarations: selection.declarations,
            ...(enums ? enumValues(definition) : declarations ? declarationValues(definition) : {fields: fields(definition)})};
    });
    if (declarations) {
        // Only separately selected by-value relationships are traversed.
        // Opaque bases have no inferred emptiness, occupied extent or layout.
        const byIndex = new Map();
        for (const value of selected) {
            byIndex.set(value.type_index, value);
            for (const forward of value.forward_declarations) byIndex.set(forward.type_index, value);
        }
        const checkedRelationships = new Map();
        function relationships(value, seen = new Set()) {
            need(!seen.has(value.type_index) && seen.size <= limits.depth);
            if (checkedRelationships.has(value.type_index)) {
                need(seen.size + checkedRelationships.get(value.type_index) <= limits.depth);
                return checkedRelationships.get(value.type_index);
            }
            const path = new Set(seen); path.add(value.type_index);
            let height = 0;
            for (const item of value.declarations.filter(item => ['base', 'member'].includes(item.entry_kind))) {
                need(item.type_index !== value.type_index);
                if (byIndex.has(item.type_index)) height = Math.max(height, 1 + relationships(byIndex.get(item.type_index), path));
            }
            checkedRelationships.set(value.type_index, height);
            return height;
        }
        for (const value of selected) relationships(value);
    }
    if (parameters) {
        let expanded = 0;
        function flatten(layout, path, offset, output, depth = 0) {
            need(depth <= limits.depth && ++expanded <= limits.fields);
            if (layout.kind === 'modifier') {
                flatten(width(layout.modified_type_index), path, offset, output, depth + 1);
            } else if (layout.kind === 'udt') {
                for (const field of layout.fields) flatten(field,
                    [...path, field.entry_kind === 'base' ? {base_type_index: field.type_index} : field.name],
                    offset + field.offset_bytes, output, depth + 1);
            } else if (layout.kind === 'array') {
                const element = width(layout.element_type_index);
                for (let i = 0; i < layout.element_count; i++) flatten(element, [...path, i],
                    offset + i * element.width_bytes, output, depth + 1);
            } else {
                charge(192 + JSON.stringify(path).length);
                output.push({path, offset_bytes: offset, width_bytes: layout.width_bytes});
            }
        }
        for (const definition of selected) {
            definition.flattened_fields = [];
            flatten({kind: 'udt', fields: definition.fields}, [], 0, definition.flattened_fields);
        }
    }
    const result = {
        profile: enums ? ENUM_PROFILE : parameters ? PARAMETER_PROFILE : declarations ? DECLARATION_PROFILE : PROFILE,
        version: 20040203, header_bytes: 56,
        type_index_begin: begin, type_index_end: end, records_framed: count,
        record_bytes: recordBytes, selected,
        dependent_types: [...memo].sort(([left], [right]) => left - right)
            .map(([type_index, layout]) => ({type_index, ...layout})),
        ...(declarations ? {direct_references: [...directReferences.values()].sort((a, b) => a.type_index - b.type_index),
            storage_layout_complete: false} : {}),
        complete_outer_record_framing: true,
        hash_tables_validated: false, unselected_payloads_validated: false,
        caller_provenance_verified: false, original_layout_qualified: false,
        release_qualified: false, runtime_behavior_proven: false,
        charged_retained_bytes: charged,
        limitations: enums ? [
            'Caller must bind immutable stream reads, input hashes and matching provenance.',
            'Only exact selected LF_ENUM declarations, integer underlying widths and complete supported enumerators are interpreted.',
            'Enum declaration values do not establish runtime flag meanings, native consumers or release compatibility.',
            'Unselected payloads, external type servers and incremental hash adjustments remain unqualified.',
        ] : declarations ? [
            'Caller must bind immutable stream reads, input hashes and matching provenance.',
            'Only exact selected class/struct declarations and direct supported method signatures/lists are interpreted.',
            'Offsets and declared size do not establish occupied storage, empty-base policy or a complete storage layout.',
            'Return/class/this/argument types and unselected base/member/alias referents remain opaque; no runtime semantics are established.',
            'Virtual/friend methods, virtual bases, external type servers and incremental hash adjustments are unsupported.',
        ] : [
            'Caller must bind immutable stream reads, input hashes and matching provenance.',
            'Only selected modern class/struct data members and supported width dependencies are interpreted.',
            parameters
                ? 'Only nonvirtual bases and fixed arrays are expanded; methods, virtual bases, unions and external type servers are unsupported.'
                : 'Inheritance, methods, external type servers, unions and incremental hash adjustments are unsupported.',
            'Pointer referent semantics and enum enumerators remain opaque; declarations do not prove native consumer behavior.',
        ],
    };
    // Parameter layouts share memoized subtrees. Bound serialization visits before
    // a repeated dependency graph can expand into an enormous temporary string.
    let serializationUnits = 0;
    const boundedReplacer = parameters || declarations ? (key, value) => {
        serializationUnits += 1 + Buffer.byteLength(key)
            + (typeof value === 'string' ? Buffer.byteLength(value) : 0);
        need(serializationUnits <= limits.output_bytes);
        return value;
    } : undefined;
    need(Buffer.byteLength(JSON.stringify(result, boundedReplacer)) <= limits.output_bytes);
    return result;
}

module.exports = {PARAMETER_PROFILE, inspectSelectedTypes, PROFILE, CAPS, inspectSelectedEnums, ENUM_PROFILE, ENUM_CAPS,
    DECLARATION_PROFILE, DECLARATION_CAPS};
