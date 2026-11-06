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
const ENUM_PROFILE = 'tpi_v80_exact_enum_values_v1';
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
const LEAF = Object.freeze({
    MODIFIER: 0x1001, POINTER: 0x1002, FIELDLIST: 0x1203, BITFIELD: 0x1205,
    CLASS: 0x1504, STRUCTURE: 0x1505, ENUM: 0x1507, INTERFACE: 0x1519,
    MEMBER: 0x150d, INDEX: 0x1404, ENUMERATE: 0x1502,
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
    need(Object.hasOwn(options, 'profile') && options.profile === (enums ? ENUM_PROFILE : PROFILE) && Object.hasOwn(options, 'names'));
    const caps = enums ? ENUM_CAPS : CAPS;
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
        } else need(false);
        c.finish();
        visiting.delete(index);
        charge(128);
        memo.set(index, result);
        return result;
    }

    let fieldsTotal = 0;
    function fields(definition) {
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
                need(kind === LEAF.MEMBER && ++fieldsTotal <= limits.fields);
                const attributes = c.u16(), typeIndex = c.u32(), offset = c.numeric();
                need((attributes & ~3) === 0);
                const name = text(c.name(), limits.name_bytes, true);
                const layout = width(typeIndex);
                need(offset + layout.width_bytes <= definition.size_bytes);
                charge(name.length + 192);
                result.push({name, attributes, type_index: typeIndex, offset_bytes: offset, ...layout});
                c.pad();
            }
        }
        // This profile admits only data members plus continuation metadata.
        // No guessed overload/base/member-count relationship is necessary.
        need(result.length === definition.declared_member_count);
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
    const selected = names.map(selection => {
        need(selection.definitions.length === 1);
        const definition = selection.definitions[0];
        for (const declaration of selection.declarations) {
            need(declaration.leaf === definition.leaf);
            if (enums) need(declaration.underlying_type_index === definition.underlying_type_index
                && declaration.unique_name === definition.unique_name);
            if (declaration.unique_name !== null && definition.unique_name !== null) {
                need(declaration.unique_name === definition.unique_name);
            }
        }
        return {name: selection.name, ...definition, forward_declarations: selection.declarations,
            ...(enums ? enumValues(definition) : {fields: fields(definition)})};
    });
    const result = {
        profile: enums ? ENUM_PROFILE : PROFILE, version: 20040203, header_bytes: 56,
        type_index_begin: begin, type_index_end: end, records_framed: count,
        record_bytes: recordBytes, selected,
        dependent_types: [...memo].sort(([left], [right]) => left - right)
            .map(([type_index, layout]) => ({type_index, ...layout})),
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
        ] : [
            'Caller must bind immutable stream reads, input hashes and matching provenance.',
            'Only selected modern class/struct data members and supported width dependencies are interpreted.',
            'Inheritance, methods, external type servers, unions and incremental hash adjustments are unsupported.',
            'Pointer referent semantics and enum enumerators remain opaque; declarations do not prove native consumer behavior.',
        ],
    };
    need(Buffer.byteLength(JSON.stringify(result)) <= limits.output_bytes);
    return result;
}

module.exports = {inspectSelectedTypes, PROFILE, CAPS, inspectSelectedEnums, ENUM_PROFILE, ENUM_CAPS};
