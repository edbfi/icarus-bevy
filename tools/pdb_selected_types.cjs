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

// Separate pure metadata APIs: legacy decoding and its generic errors above are
// deliberately unchanged. Sources (grammar, not implementation):
// https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-20.1.8/llvm/lib/DebugInfo/CodeView/TypeRecordMapping.cpp
// https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-20.1.8/llvm/include/llvm/DebugInfo/CodeView/CodeView.h
// https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-20.1.8/llvm/lib/DebugInfo/CodeView/CodeViewRecordIO.cpp
// https://raw.githubusercontent.com/microsoft/microsoft-pdb/master/include/cvinfo.h
// https://llvm.org/docs/PDB/TpiStream.html
const UDT_HEADER_PROFILE = 'tpi_v80_exact_udt_headers_v1';
const MFUNCTION_PROFILE = 'tpi_v80_exact_mfunction_signatures_v1';
const UDT_HEADER_CAPS = Object.freeze({
    stream_bytes: 512 * 1024 * 1024, records: 6000000,
    record_bytes: 65536, read_bytes: 65536,
    logical_read_bytes: 1024 * 1024 * 1024, read_calls: 12000001,
    selectors: 8, candidates: 16, name_bytes: 256, unique_name_bytes: 4096,
    retained_bytes: 1024 * 1024, output_bytes: 65536,
});
const MFUNCTION_CAPS = Object.freeze({
    stream_bytes: 512 * 1024 * 1024, records: 6000000,
    record_bytes: 65536, read_bytes: 65536,
    logical_read_bytes: 32 * 1024 * 1024, read_calls: 6000009,
    selectors: 8, candidates: 8, retained_bytes: 256 * 1024, output_bytes: 65536,
});

// Names and type_indices are mutually exclusive; MFUNCTION accepts indices
// only. Both modes frame EVERY outer record in one pass, without hash tables,
// retained offset lookup or a second pass. Numeric selection returns exactly that record,
// including a forward UDT; it neither follows nor associates it with a complete
// definition. Name mode requires one complete definition and at most one
// forward, matching leaf AND unique_name (including null). This is header
// identity metadata only; no member/storage/hierarchy or native owner inference.
// Error codes describe bounded reader phases, never a target-specific diagnosis.
function inspectSelectedUdtHeaders(reader, options) {
    return inspectMetadata(reader, options, false);
}
function inspectSelectedMemberFunctions(reader, options) {
    return inspectMetadata(reader, options, true);
}
function inspectMetadata(reader, options, signatures) {
    const state = {phase: 'OPTIONS'};
    try { return metadataImpl(reader, options, signatures, state); }
    catch {
        const error = Error('invalid or unsupported TPI metadata');
        error.code = 'TPI_METADATA_' + state.phase;
        throw error;
    }
}
function metadataImpl(reader, options, signatures, state) {
    const {createHash} = require('node:crypto');
    const {isProxy} = require('node:util').types;
    // Snapshot descriptor values only. This pure API admits ordinary bounded
    // data, not executable options, proxies, sparse arrays or custom iterators.
    function data(value, array = false) {
        need(value && typeof value === 'object' && !isProxy(value));
        need(array ? Array.isArray(value) && Object.getPrototypeOf(value) === Array.prototype : plain(value));
        const keys = Reflect.ownKeys(value);
        need(keys.length <= 32 && keys.every(key => typeof key === 'string'));
        const copy = Object.create(null);
        for (const key of keys) {
            const d = Object.getOwnPropertyDescriptor(value, key);
            need(Object.hasOwn(d, 'value') && (d.enumerable || (array && key === 'length')));
            copy[key] = d.value;
        }
        return copy;
    }
    options = data(options);
    need(Object.keys(options).every(key => ['profile', 'names', 'type_indices', 'limits'].includes(key)));
    const profile = signatures ? MFUNCTION_PROFILE : UDT_HEADER_PROFILE;
    need(options.profile === profile);
    const caps = signatures ? MFUNCTION_CAPS : UDT_HEADER_CAPS, limits = {...caps};
    if (Object.hasOwn(options, 'limits')) {
        const values = data(options.limits);
        for (const key of Object.keys(values)) {
            need(Object.hasOwn(caps, key) && Number.isSafeInteger(values[key])
                && values[key] >= 0 && values[key] <= caps[key]);
            limits[key] = values[key];
        }
    }
    const byName = Object.hasOwn(options, 'names');
    need(byName !== Object.hasOwn(options, 'type_indices') && (!signatures || !byName));
    const selectors = data(byName ? options.names : options.type_indices, true);
    need(Number.isSafeInteger(selectors.length) && selectors.length > 0 && selectors.length <= limits.selectors
        && Object.keys(selectors).length === selectors.length + 1);
    const selections = [], seen = new Set();
    for (let i = 0; i < selectors.length; i++) {
        need(Object.hasOwn(selectors, String(i)));
        const value = selectors[i];
        need(!seen.has(value)); seen.add(value);
        if (byName) need(typeof value === 'string' && /^[\x20-\x7e]+$/.test(value) && value.length <= limits.name_bytes);
        else need(Number.isSafeInteger(value) && value >= 0x1000 && value < 0x80000000);
        selections.push({selector: value, records: []});
    }
    state.phase = 'HEADER';
    const streamSize = reader?.size;
    need(Number.isSafeInteger(streamSize) && streamSize >= 56 && streamSize <= limits.stream_bytes
        && typeof reader.read === 'function' && limits.read_bytes >= 56);
    let charged = 0, readCalls = 0, logicalReadBytes = 0;
    function charge(bytes) {
        need(Number.isSafeInteger(bytes) && bytes >= 0 && charged + bytes <= limits.retained_bytes);
        charged += bytes;
    }
    // Logical accounting, not RSS: reserve two transient read buffers, charge
    // retained selector/record hex/text plus fixed object allowances. No offsets
    // array, cache, referent objects or recursive serialization graph is retained.
    charge(limits.read_bytes * 2);
    for (const selection of selections) charge(128 + (byName ? selection.selector.length : 4));
    function exact(offset, length) {
        need(Number.isSafeInteger(offset) && Number.isSafeInteger(length) && offset >= 0 && length >= 0
            && length <= limits.read_bytes && offset + length <= streamSize
            && readCalls + 1 <= limits.read_calls && logicalReadBytes + length <= limits.logical_read_bytes);
        readCalls++; logicalReadBytes += length;
        const bytes = reader.read(offset, length);
        need(Buffer.isBuffer(bytes) && bytes.length === length && !isSharedArrayBuffer(bytes.buffer));
        return Buffer.from(bytes); // snapshot borrowed/reused buffers before the next callback
    }
    const h = exact(0, 56), begin = h.readUInt32LE(8), end = h.readUInt32LE(12);
    const recordBytes = h.readUInt32LE(16), count = end - begin;
    need(h.readUInt32LE(0) === 20040203 && h.readUInt32LE(4) === 56
        && begin === 0x1000 && end >= begin && end < 0x80000000 && count <= limits.records
        && count * 4 <= recordBytes && 56 + recordBytes === streamSize);
    need(h.readUInt32LE(24) === 4 && h.readUInt32LE(28) >= 0x1000 && h.readUInt32LE(28) <= 0x40000
        && h.readUInt16LE(22) === 0xffff);
    const hashBytes = h.readUInt32LE(36), indexBytes = h.readUInt32LE(44);
    need((hashBytes === 0 || hashBytes === count * 4) && indexBytes % 8 === 0 && h.readUInt32LE(52) === 0);
    for (const offset of [32, 40, 48]) need(h.readInt32LE(offset) >= 0);
    if (h.readUInt16LE(20) === 0xffff) need(hashBytes === 0 && indexBytes === 0);
    state.phase = 'SELECTOR_RANGE';
    if (!byName) for (const selection of selections) need(selection.selector >= begin && selection.selector < end);
    function tpi(index, allowNone = false) { need((allowNone && index === 0) || (index >= begin && index < end)); }
    function opaqueReturn(index) {
        if (index >= begin) tpi(index);
        else need(index > 0 && index < 0x800 && OPAQUE_SIMPLE.has(index & 0xff));
    }
    function name(bytes, cap) {
        need(bytes.length > 0 && bytes.length <= cap && bytes.every(byte => byte >= 0x20 && byte <= 0x7e));
        return bytes.toString('ascii');
    }
    let at = 56, candidates = 0;
    for (let ordinal = 0; ordinal < count; ordinal++) {
        state.phase = 'FRAMING';
        const prefix = exact(at, 4), length = prefix.readUInt16LE() + 2, leaf = prefix.readUInt16LE(2);
        need(length >= 4 && length <= limits.record_bytes && length <= limits.read_bytes
            && length % 4 === 0 && at + length <= streamSize);
        const index = begin + ordinal;
        let selection = byName ? null : selections.find(value => value.selector === index), bytes;
        if (byName && [LEAF.CLASS, LEAF.STRUCTURE, LEAF.INTERFACE].includes(leaf)) {
            // Full name lookup needs every candidate's minimal name framing;
            // malformed discovery can refuse even when its name is unselected.
            bytes = exact(at, length);
            need(bytes.subarray(0, 4).equals(prefix));
            const probe = candidateName(bytes);
            selection = selections.find(value => probe.equals(Buffer.from(value.selector, 'ascii')));
        }
        if (selection) {
            state.phase = 'SELECTED_RECORD';
            need(++candidates <= limits.candidates);
            bytes ??= exact(at, length);
            need(bytes.subarray(0, 4).equals(prefix));
            const c = new Cursor(bytes);
            let value;
            if (signatures) {
                need(leaf === LEAF.MFUNCTION);
                const returns = c.u32(), owner = c.u32(), thisType = c.u32();
                const cc = c.u8(), attributes = c.u8(), parameterCount = c.u16();
                const args = c.u32(), adjustment = c.take(4).readInt32LE();
                // Conservative admitted signature subset matches legacy NearC
                // and options 0/1/2, but permits all signed 32-bit adjustments.
                // No argument count/list equality, this-pointer interpretation,
                // referent leaf validation, class association or IPI lookup.
                need(cc === 0 && [0, 1, 2].includes(attributes));
                opaqueReturn(returns); tpi(owner); tpi(thisType, true); tpi(args);
                c.finish(); // exactly 28 bytes; extra "padding" is not admitted
                value = {return_type_index: returns, class_type_index: owner, this_type_index: thisType,
                    calling_convention: cc, attributes, declared_argument_count: parameterCount,
                    argument_list_type_index: args, this_adjustment: adjustment};
            } else {
                need(leaf === LEAF.CLASS || leaf === LEAF.STRUCTURE);
                const parsed = tag(bytes);
                // LLVM ClassOptions named bits only; HFA/MOCOM encodings are
                // explicitly unsupported. Integer size uses Cursor's admitted
                // immediate/CHAR/SHORT/USHORT/LONG/ULONG/QUAD/UQUAD forms,
                // nonnegative and <= MAX_SAFE_INTEGER. No storage-size policy.
                need((parsed.flags & ~0x27ff) === 0);
                tpi(parsed.fieldList, true); tpi(parsed.derived, true); tpi(parsed.vshape, true);
                const forward = (parsed.flags & 0x80) !== 0;
                if (forward) need(parsed.count === 0 && parsed.fieldList === 0 && parsed.size === 0);
                const exactName = name(parsed.name, limits.name_bytes);
                const uniqueName = parsed.unique === null ? null : name(parsed.unique, limits.unique_name_bytes);
                charge(exactName.length + (uniqueName?.length ?? 0));
                value = {flags: parsed.flags, name: exactName, unique_name: uniqueName,
                    forward_reference: forward, declared_size_bytes: parsed.size, declared_member_count: parsed.count,
                    field_list_type_index: parsed.fieldList, derived_type_index: parsed.derived,
                    vshape_type_index: parsed.vshape};
            }
            charge(length * 2 + 768);
            selection.records.push({type_index: index, record_offset: at, record_bytes: length,
                record_sha256: createHash('sha256').update(bytes).digest('hex'), record_hex: bytes.toString('hex'), leaf, ...value});
        }
        at += length;
    }
    state.phase = 'FRAMING'; need(at === streamSize);
    state.phase = 'SELECTION';
    const selected = selections.map(selection => {
        if (!byName) { need(selection.records.length === 1); return selection.records[0]; }
        const definitions = selection.records.filter(value => !value.forward_reference);
        const forwards = selection.records.filter(value => value.forward_reference);
        need(definitions.length === 1 && forwards.length <= 1);
        const definition = definitions[0];
        for (const forward of forwards) need(forward.leaf === definition.leaf && forward.unique_name === definition.unique_name);
        return {...definition, forward_declarations: forwards};
    });
    const result = {profile, selection_mode: byName ? 'exact_names' : 'exact_type_indices',
        version: 20040203, header_bytes: 56, type_index_begin: begin, type_index_end: end,
        records_framed: count, record_bytes: recordBytes, selected,
        complete_outer_record_framing: true, hash_tables_validated: false, unselected_payloads_validated: false,
        references_resolved: false, storage_layout_complete: false, native_owner_qualified: false,
        caller_provenance_verified: false, original_layout_qualified: false,
        release_qualified: false, runtime_behavior_proven: false,
        logical_read_bytes: logicalReadBytes, read_calls: readCalls, charged_retained_bytes: charged,
        limitations: [
            'Caller must bind immutable TPI logical reads, container identity, hashes, authorization and provenance.',
            'All outer records are framed even for numeric selection; unselected payloads and hash tables are not validated.',
            'Selected references are opaque numeric TPI words; no referent, member, argument list or IPI record is read.',
            byName ? 'Exact name identity requires one definition and at most one matching forward declaration.'
                : 'Numeric selection returns exactly the requested record, without following or associating forward declarations.',
            'Header and signature metadata establish no storage layout, hierarchy, native owner/consumer or compatibility.',
        ]};
    state.phase = 'OUTPUT';
    // Flat, bounded objects and <=16 candidates bound traversal. Stop before
    // serializing an oversized string; exact serialized bytes are checked too.
    let outputUnits = 0;
    const serialized = JSON.stringify(result, (key, value) => {
        outputUnits += 1 + Buffer.byteLength(key)
            + (typeof value === 'string' ? Buffer.byteLength(value) : 0);
        need(outputUnits <= limits.output_bytes);
        return value;
    });
    need(Buffer.byteLength(serialized) <= limits.output_bytes);
    return result;
}
Object.assign(module.exports, {UDT_HEADER_PROFILE, UDT_HEADER_CAPS, MFUNCTION_PROFILE, MFUNCTION_CAPS,
    inspectSelectedUdtHeaders, inspectSelectedMemberFunctions});

// Separate research-only declaration-entry contract. Keep the legacy region
// above byte-for-byte unchanged. This API has no layout/signature resolver.
// Independently authored grammar, qualified against:
// https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-20.1.8/llvm/lib/DebugInfo/CodeView/TypeRecordMapping.cpp
// (MapOneMethodRecord and visitKnownMember), CodeView.h (attributes), and
// https://raw.githubusercontent.com/microsoft/microsoft-pdb/master/include/cvinfo.h
// (lfClass, lfIndex, lfVBClass, lfVFuncTab, lfOneMethod, lfNestType).
// No upstream implementation is copied. Modern zero-terminated names only.
// Caller binds ONLY a TPI stream to its immutable container/hash/provenance.
// Selectors pin exact numeric complete class/struct records, raw bytes/hash,
// and fieldlist identity; no name search or original-input I/O takes place.
// Duplicate selectors/selected identities and all shared FIELDLISTs refuse.
// Unselected identities/payloads are not inspected for global uniqueness.
const DECLARATION_ENTRY_PROFILE = 'tpi_v80_exact_udt_declaration_entries_v1';
const DECLARATION_ENTRY_CAPS = Object.freeze({
    stream_bytes: 512 * 1024 * 1024,
    records: 6000000,
    record_bytes: 65536,
    read_bytes: 65536,
    logical_read_bytes: 64 * 1024 * 1024,
    read_calls: 6100000,
    selectors: 8,
    selected_record_bytes: 65536,
    selected_bytes: 4 * 1024 * 1024,
    retained_bytes: 64 * 1024 * 1024,
    output_bytes: 4 * 1024 * 1024,
    name_bytes: 256,
    unique_name_bytes: 4096,
    field_lists: 128,
    continuations: 64,
    depth: 32,
    declarations: 4096,
    weighted_members: 8192,
    direct_references: 8192,
});
const DECLARATION_ENTRY_ERROR = 'invalid or unsupported TPI declaration entries';

function inspectSelectedDeclarationEntries(reader, options) {
    try { return declarationEntriesImpl(reader, options); }
    catch { throw Error(DECLARATION_ENTRY_ERROR); }
}

function declarationEntriesImpl(reader, options) {
    const crypto = require('node:crypto');
    const {isProxy} = require('node:util').types;
    const sha = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
    // Reject getters and unknown keys, snapshot every option before the first
    // callback, and capture reader size/read once. Callback buffers are copied.
    function object(value, keys) {
        need(!isProxy(value) && plain(value));
        const copy = Object.create(null);
        for (const key of Reflect.ownKeys(value)) {
            need(typeof key === 'string' && keys.includes(key));
            const descriptor = Object.getOwnPropertyDescriptor(value, key);
            need(Object.hasOwn(descriptor, 'value'));
            copy[key] = descriptor.value;
        }
        return copy;
    }
    const opt = object(options, ['profile', 'owners', 'limits']);
    need(opt.profile === DECLARATION_ENTRY_PROFILE);
    const limits = {...DECLARATION_ENTRY_CAPS};
    if (opt.limits !== undefined) {
        const supplied = object(opt.limits, Object.keys(limits));
        for (const key of Object.keys(supplied)) {
            const value = supplied[key];
            need(Number.isSafeInteger(value) && value >= 0 && value <= limits[key]);
            limits[key] = value;
        }
    }
    let retained = 0;
    function charge(bytes) {
        need(Number.isSafeInteger(bytes) && bytes >= 0 && retained + bytes <= limits.retained_bytes);
        retained += bytes;
    }
    const ownersInput = opt.owners;
    need(!isProxy(ownersInput) && Array.isArray(ownersInput));
    const ownerCount = ownersInput.length;
    need(ownerCount > 0 && ownerCount <= limits.selectors);
    need(Reflect.ownKeys(ownersInput).every(key => key === 'length'
        || (typeof key === 'string' && /^(0|[1-9][0-9]*)$/.test(key) && Number(key) < ownerCount)));
    const owners = [], indices = new Set();
    const ownerKeys = ['type_index', 'record_sha256', 'record_hex', 'field_list_type_index'];
    for (let i = 0; i < ownerCount; i++) {
        const descriptor = Object.getOwnPropertyDescriptor(ownersInput, String(i));
        need(descriptor && Object.hasOwn(descriptor, 'value'));
        const owner = object(descriptor.value, ownerKeys);
        need(ownerKeys.every(key => Object.hasOwn(owner, key)));
        need(Number.isSafeInteger(owner.type_index) && owner.type_index >= 0x1000
            && owner.type_index < 0x80000000 && !indices.has(owner.type_index));
        need(Number.isSafeInteger(owner.field_list_type_index) && owner.field_list_type_index >= 0
            && owner.field_list_type_index < 0x80000000);
        need(typeof owner.record_sha256 === 'string' && /^[0-9a-f]{64}$/.test(owner.record_sha256));
        need(typeof owner.record_hex === 'string' && owner.record_hex.length >= 8
            && owner.record_hex.length % 8 === 0
            && owner.record_hex.length <= limits.selected_record_bytes * 2
            && /^[0-9a-f]+$/.test(owner.record_hex));
        charge(owner.record_hex.length + 256);
        owners.push({...owner}); indices.add(owner.type_index);
    }
    need(reader && !isProxy(reader) && typeof reader === 'object');
    const sizeDescriptor = Object.getOwnPropertyDescriptor(reader, 'size');
    const readDescriptor = Object.getOwnPropertyDescriptor(reader, 'read');
    need(sizeDescriptor && Object.hasOwn(sizeDescriptor, 'value')
        && readDescriptor && Object.hasOwn(readDescriptor, 'value'));
    const streamSize = sizeDescriptor.value, readCallback = readDescriptor.value;
    need(Number.isSafeInteger(streamSize) && streamSize >= 56 && streamSize <= limits.stream_bytes
        && typeof readCallback === 'function');
    let logicalBytes = 0, calls = 0, selectedBytes = 0;
    // Logical charging is a conservative budget, not an RSS guarantee. All
    // individual reads are bounded, including prefix-only unselected reads.
    charge(limits.read_bytes * 2);
    function exact(offset, length) {
        need(Number.isSafeInteger(offset) && offset >= 0 && Number.isSafeInteger(length)
            && length >= 0 && length <= limits.read_bytes && offset + length <= streamSize);
        need(calls + 1 <= limits.read_calls && logicalBytes + length <= limits.logical_read_bytes);
        calls++; logicalBytes += length;
        const bytes = readCallback.call(reader, offset, length);
        need(!isProxy(bytes) && Buffer.isBuffer(bytes) && bytes.length === length && !isSharedArrayBuffer(bytes.buffer));
        return Buffer.from(bytes);
    }
    const header = exact(0, 56);
    const begin = header.readUInt32LE(8), end = header.readUInt32LE(12);
    const recordBytes = header.readUInt32LE(16), count = end - begin;
    need(header.readUInt32LE(0) === 20040203 && header.readUInt32LE(4) === 56
        && begin === 0x1000 && end >= begin && end < 0x80000000);
    need(count <= limits.records && count * 4 <= recordBytes && 56 + recordBytes === streamSize);
    need(header.readUInt32LE(24) === 4 && header.readUInt16LE(22) === 0xffff);
    const buckets = header.readUInt32LE(28);
    need(buckets >= 0x1000 && buckets <= 0x40000);
    const hashBytes = header.readUInt32LE(36), indexBytes = header.readUInt32LE(44);
    need((hashBytes === 0 || hashBytes === count * 4) && indexBytes % 8 === 0
        && header.readUInt32LE(52) === 0);
    for (const offset of [32, 40, 48]) need(header.readInt32LE(offset) >= 0);
    if (header.readUInt16LE(20) === 0xffff) need(hashBytes === 0 && indexBytes === 0);
    function tpi(index) { need(Number.isSafeInteger(index) && index >= begin && index < end); }
    for (const owner of owners) tpi(owner.type_index);
    charge((count + 1) * 4 + count * 2);
    const offsets = new Uint32Array(count + 1), leaves = new Uint16Array(count);
    let at = 56;
    // No cache/read-ahead: only each outer prefix is read for opaque records.
    for (let i = 0; i < count; i++) {
        offsets[i] = at;
        const prefix = exact(at, 4), length = prefix.readUInt16LE() + 2;
        need(length >= 4 && length % 4 === 0 && length <= limits.record_bytes && at + length <= streamSize);
        leaves[i] = prefix.readUInt16LE(2);
        at += length;
    }
    offsets[count] = at; need(at === streamSize);
    const records = [];
    function selectedRecord(index, expected) {
        tpi(index);
        const ordinal = index - begin, offset = offsets[ordinal], length = offsets[ordinal + 1] - offset;
        need(expected.includes(leaves[ordinal]) && length <= limits.selected_record_bytes
            && selectedBytes + length <= limits.selected_bytes);
        selectedBytes += length;
        charge(length * 3 + 256);
        const bytes = exact(offset, length);
        need(bytes.readUInt16LE() + 2 === length && bytes.readUInt16LE(2) === leaves[ordinal]);
        const provenance = {type_index: index, leaf: leaves[ordinal], record_offset: offset,
            record_bytes: length, record_sha256: sha(bytes), record_hex: bytes.toString('hex')};
        records.push(provenance);
        return {bytes, provenance};
    }
    const directReferences = [];
    function reference(index, role, source, domain = 'type', allowNone = false) {
        need(Number.isSafeInteger(index) && index >= 0 && index < 0x80000000);
        let actual;
        if (index === 0) { need(allowNone); actual = 'none'; }
        else if (index >= begin) { tpi(index); actual = 'tpi'; }
        else {
            need(domain === 'type' && index < 0x800 && OPAQUE_SIMPLE.has(index & 0xff));
            actual = 'simple';
        }
        need(directReferences.length < limits.direct_references);
        charge(192);
        const value = {type_index: index, domain: actual, role,
            source_type_index: source.type_index, source_record_offset: source.record_offset,
            source_entry_offset: source.entry_offset ?? null,
            interpretation: 'opaque', followed: false};
        directReferences.push(value);
        return index;
    }
    function name(c, unique = false) {
        const bytes = c.name();
        need(bytes.length > 0 && bytes.length <= (unique ? limits.unique_name_bytes : limits.name_bytes)
            && bytes.every(byte => byte >= 0x20 && byte <= 0x7e));
        charge(bytes.length * 2);
        return bytes.toString('ascii');
    }
    const selected = [], identities = new Set();
    for (const owner of owners) {
        const entry = selectedRecord(owner.type_index, [LEAF.CLASS, LEAF.STRUCTURE]);
        need(entry.provenance.record_sha256 === owner.record_sha256
            && entry.provenance.record_hex === owner.record_hex);
        const c = new Cursor(entry.bytes);
        const memberCount = c.u16(), flags = c.u16(), fieldList = c.u32();
        const derived = c.u32(), vshape = c.u32(), declaredSize = c.numeric();
        // Only the individually named LLVM ClassOptions bits are qualified;
        // HFA/managed/other header variants remain unsupported.
        need((flags & ~0x27ff) === 0 && (flags & 0x80) === 0);
        const ownerName = name(c), uniqueName = flags & 0x200 ? name(c, true) : null;
        c.finish();
        need(fieldList === owner.field_list_type_index && memberCount <= limits.weighted_members);
        if (fieldList !== 0) tpi(fieldList); else need(memberCount === 0);
        // Exact selection is not a global name census. Refuse duplicate names
        // or unique identities among requested complete owners only.
        need(!identities.has('name:' + ownerName)
            && (uniqueName === null || !identities.has('unique:' + uniqueName)));
        identities.add('name:' + ownerName);
        if (uniqueName !== null) identities.add('unique:' + uniqueName);
        reference(derived, 'derived', entry.provenance, 'tpi', true);
        reference(vshape, 'vshape', entry.provenance, 'tpi', true);
        charge(384);
        selected.push({...entry.provenance, name: ownerName, unique_name: uniqueName, flags,
            forward_reference: false, declared_size_bytes: declaredSize,
            declared_member_count: memberCount, field_list_type_index: fieldList,
            derived_type_index: derived, vshape_type_index: vshape,
            declarations: [], field_list_type_indices: [], continuations: []});
    }
    const usedLists = new Set();
    let declarationsTotal = 0, weightedTotal = 0, continuationTotal = 0;
    function access(c) { const value = c.u16(); need(value >= 1 && value <= 3); return value; }
    function framing(c, start, provenance) {
        const contentEnd = c.at; c.pad();
        const raw = c.bytes.subarray(start, c.at);
        charge(raw.length * 2 + 256);
        return {field_list_type_index: provenance.type_index, entry_offset: start,
            stream_offset: provenance.record_offset + start, content_bytes: contentEnd - start,
            padding_bytes: c.at - contentEnd, framed_bytes: c.at - start,
            entry_sha256: sha(raw), entry_hex: raw.toString('hex')};
    }
    for (const owner of selected) {
        let next = owner.field_list_type_index, depth = 0, physical = 0, weighted = 0;
        while (next !== 0) {
            // Global no-reuse policy also rejects cycles, self-links and
            // shared tails across owners. Depth counts edges, root is zero.
            need(depth <= limits.depth && !usedLists.has(next) && usedLists.size < limits.field_lists);
            usedLists.add(next); owner.field_list_type_indices.push(next);
            const entry = selectedRecord(next, [LEAF.FIELDLIST]);
            const c = new Cursor(entry.bytes); next = 0;
            while (c.at < c.bytes.length) {
                const start = c.at, leaf = c.u16();
                const source = {...entry.provenance, entry_offset: start};
                if (leaf === LEAF.INDEX) {
                    need(c.u16() === 0);
                    next = c.u32(); tpi(next);
                    need(++continuationTotal <= limits.continuations);
                    const frame = framing(c, start, entry.provenance);
                    need(c.at === c.bytes.length);
                    owner.continuations.push({leaf, ...frame, target_type_index: next,
                        interpretation: 'field_list_continuation', followed: true});
                    break;
                }
                need(++declarationsTotal <= limits.declarations);
                let value, weight = 1;
                switch (leaf) {
                case LEAF.MEMBER: {
                    const attributes = access(c), type = reference(c.u32(), 'member_type', source);
                    value = {entry_kind: 'member', attributes, type_index: type,
                        declared_offset_bytes: c.numeric(), name: name(c)}; break;
                }
                case LEAF.BCLASS: {
                    const attributes = access(c), type = reference(c.u32(), 'base_type', source, 'tpi');
                    value = {entry_kind: 'base', attributes, type_index: type,
                        declared_offset_bytes: c.numeric()}; break;
                }
                case 0x1401: case 0x1402: {
                    const attributes = access(c), base = reference(c.u32(), 'virtual_base_type', source, 'tpi');
                    const pointer = reference(c.u32(), 'vbptr_type', source);
                    value = {entry_kind: leaf === 0x1401 ? 'virtual_base' : 'indirect_virtual_base',
                        attributes, type_index: base, vbptr_type_index: pointer,
                        declared_vbptr_offset_bytes: c.numeric(), declared_vbtable_index: c.numeric()}; break;
                }
                case 0x150e: {
                    const attributes = access(c), type = reference(c.u32(), 'static_member_type', source);
                    value = {entry_kind: 'static_member', attributes, type_index: type, name: name(c)}; break;
                }
                case LEAF.NESTTYPE: {
                    need(c.u16() === 0);
                    const type = reference(c.u32(), 'nested_type', source);
                    value = {entry_kind: 'nested_type', type_index: type, name: name(c)}; break;
                }
                case LEAF.ONEMETHOD: {
                    const attributes = c.u16(), kind = (attributes >>> 2) & 7;
                    need((attributes & ~0x3ff) === 0 && (attributes & 3) !== 0
                        && [0, 1, 2, 4, 5, 6].includes(kind));
                    const type = reference(c.u32(), 'method_type', source, 'tpi');
                    const offset = kind === 4 || kind === 6 ? c.take(4).readInt32LE() : null;
                    value = {entry_kind: 'one_method', attributes, method_kind: kind, type_index: type,
                        declared_vftable_offset: offset, name: name(c), signature_followed: false}; break;
                }
                case LEAF.METHOD: {
                    weight = c.u16(); need(weight > 0);
                    const type = reference(c.u32(), 'method_list', source, 'tpi');
                    value = {entry_kind: 'method_group', declared_overload_count: weight,
                        method_list_type_index: type, name: name(c), method_list_followed: false,
                        overload_signatures_complete: false}; break;
                }
                case 0x1409: {
                    need(c.u16() === 0);
                    const type = reference(c.u32(), 'vfptr_type', source);
                    value = {entry_kind: 'vfptr_marker', type_index: type}; break;
                }
                default: need(false);
                }
                need(weightedTotal + weight <= limits.weighted_members);
                weightedTotal += weight; weighted += weight; physical++;
                owner.declarations.push({leaf, ...framing(c, start, entry.provenance),
                    declared_weight: weight, ...value});
            }
            depth++;
        }
        need(weighted === owner.declared_member_count);
        owner.physical_declaration_count = physical;
        owner.weighted_declaration_count = weighted;
    }
    const result = {
        profile: DECLARATION_ENTRY_PROFILE, version: 1, tpi_version: 20040203,
        type_index_begin: begin, type_index_end: end, records_framed: count, record_bytes: recordBytes,
        tpi_header_sha256: sha(header), tpi_header_hex: header.toString('hex'),
        selected, selected_records: records, direct_references: directReferences,
        physical_declaration_count: declarationsTotal, weighted_declaration_count: weightedTotal,
        continuation_count: continuationTotal, field_list_reuse_policy: 'refuse_all_reuse',
        complete_outer_record_framing: true, selected_entry_framing_complete: true,
        hash_tables_validated: false, unselected_payloads_validated: false,
        unselected_identity_uniqueness_validated: false, caller_provenance_verified: false,
        references_resolved: false, method_lists_followed: false, signatures_complete: false,
        storage_layout_complete: false, occupied_storage_qualified: false,
        native_owner_qualified: false, native_receiver_qualified: false,
        compatibility_qualified: false, save_import_qualified: false, wire_qualified: false,
        logical_read_bytes: logicalBytes, read_calls: calls, selected_bytes: selectedBytes,
        charged_retained_bytes: retained,
        limitations: [
            'Research declaration metadata only; declared sizes/offsets and count closure establish no storage layout or runtime semantics.',
            'Each METHOD contributes its declared overload weight; METHODLIST payloads and overload/signature closure remain unqualified.',
            'All direct type/simple/derived/vshape references are opaque and explicitly unfollowed; only FIELDLIST continuations are traversed.',
            'Caller supplies trusted synchronous immutable exact reads, whole-input identity, authorization and provenance; no callback runtime guarantee.',
            'Only selected complete class/struct identities are checked; unselected payloads and global identity uniqueness remain unqualified.',
            'Unknown leaves, friend methods, HFA/managed headers, external type servers and incremental hash adjustments refuse.',
        ],
    };
    let units = 0;
    const serialized = JSON.stringify(result, (key, value) => {
        units += 1 + Buffer.byteLength(key) + (typeof value === 'string' ? Buffer.byteLength(value) : 0);
        need(units <= limits.output_bytes);
        return value;
    });
    need(Buffer.byteLength(serialized) <= limits.output_bytes);
    return result;
}

Object.assign(module.exports, {DECLARATION_ENTRY_PROFILE, DECLARATION_ENTRY_CAPS,
    inspectSelectedDeclarationEntries});
