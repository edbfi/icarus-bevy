'use strict';
// Independently authored, metadata-only reader. No upstream implementation copied.
// Public primary format sources, pinned to LLVM llvmorg-20.1.8:
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/include/llvm/DebugInfo/PDB/Native/RawTypes.h
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/include/llvm/DebugInfo/PDB/Native/RawConstants.h
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/include/llvm/DebugInfo/PDB/Native/HashTable.h
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/lib/DebugInfo/PDB/Native/NamedStreamMap.cpp
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/lib/DebugInfo/PDB/Native/InfoStream.cpp
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/lib/DebugInfo/PDB/Native/InfoStreamBuilder.cpp
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/lib/DebugInfo/PDB/Native/ModuleDebugStream.cpp
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/lib/DebugInfo/PDB/Native/DbiStreamBuilder.cpp
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/lld/COFF/PDB.cpp
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/include/llvm/DebugInfo/CodeView/CodeViewSymbols.def
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/lib/DebugInfo/CodeView/SymbolRecordMapping.cpp
// LLVM sources: Apache-2.0 WITH LLVM-exception; preserved license reference:
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/LICENSE.TXT
// Microsoft primary sources at 805655a28bd8198004be2ac27e6e0290121a5e89:
// https://github.com/microsoft/microsoft-pdb/blob/805655a28bd8198004be2ac27e6e0290121a5e89/PDB/include/dbicommon.h
// https://github.com/microsoft/microsoft-pdb/blob/805655a28bd8198004be2ac27e6e0290121a5e89/PDB/include/nmtni.h
// https://github.com/microsoft/microsoft-pdb/blob/805655a28bd8198004be2ac27e6e0290121a5e89/include/cvinfo.h
// Microsoft sources: Copyright (c) 2015 Microsoft Corporation, MIT license:
// https://github.com/microsoft/microsoft-pdb/blob/805655a28bd8198004be2ac27e6e0290121a5e89/LICENSE
// dbicommon.h's SC.imod is INTERNAL ZERO based; its enumeration API converts to
// external ONE based. lld serializes getModuleIndex() directly into SC.Imod.
// SC.ISect and ProcSym.Segment use PE section ordinal 1..N in this direct-map
// profile (matching section headers, no OMAP); SC2.ISectCoff is NOT that ordinal.
// A supplied DBI segment map must also describe that identity mapping (the
// documented final absolute-symbol descriptor is allowed but never routed).
// The descriptor's first contribution is deliberately never used for routing.
//
// Contract: reader.size, reader.read results, EXE and identity must remain
// immutable throughout the call. This reader does not hash/authorize files or
// certify .pdata absence; that gate and input pinning belong to its caller.
// Reads are exact and copied, SharedArrayBuffer backing is refused. Selected
// symbol records are fully framed; C13 lines and trailing global refs are opaque
// extents only. No external object/type-server lookup or S_PROCREF joining.
// PE debug metadata must reside in readable initialized noncode, nonexecutable,
// nonwritable, nonresource storage. Nonidentity DBI segment maps are refused.
// Unknown record payloads are opaque counters, NEVER semantic coverage. Known
// separate-code, thunk, managed, legacy procedure and FASTLINK forms
// refuse the selected-module profile. Success is supported declaration evidence
// within the reported scope, never a complete function/owner/alias census.
// Inline-site annotation records stay opaque: they do not supply standalone
// procedure declarations, and their annotation payloads/scopes are unvalidated.
// Only the explicit inlinees profile frames exact counted-u32 auxiliary lists.
// Their values remain opaque: no references, ownership or declarations follow.
// Digests cover complete matching raw records (including names/padding), in
// ascending module ordinal then symbol offset order; bytes are never exported.

const {createHash} = require('node:crypto');
const {isSharedArrayBuffer} = require('node:util').types;
const {msf} = require('./pdb_public_symbols.cjs');
const {parse} = require('./pe_candidates.cjs');

const LIMITS = Object.freeze({
    pdb: 1280 * 1024 * 1024, exe: 128 * 1024 * 1024,
    directory: 4 * 1024 * 1024, read: 4 * 1024 * 1024,
    streams: 4096, sections: 96, info: 65536, module_info: 4 * 1024 * 1024,
    descriptors: 65535, modules: 32, selectors: 32,
    symbols: 256 * 1024 * 1024, records: 2500000, record: 65537,
    contributions: 256 * 1024 * 1024, contribution_records: 2500000,
    cache: 65536, output: 65536,
});
class MetadataError extends Error {
    constructor(code) { super(code); this.code = code; }
}
function need(ok, code) { if (!ok) throw new MetadataError(code); }
// Only the explicit diagnostic supplies this per-call context. Ordinary
// exceptions keep their code-only shape; successful guards never set a site.
function mappingNeed(ok, mappingSite, stage, guard) {
    if (ok) return;
    if (mappingSite !== null && mappingSite.first === null) mappingSite.first = {stage, guard};
    throw new MetadataError('ERR_SOURCE_MAPPING');
}
function segmentFrameMatches(sm, at, expected, mappingSite, tailCandidate) {
    const frame = sm.readUInt16LE(at + 6), matches = frame === expected;
    // Only the exact-tail diagnostic owns this field. Retain the comparison's
    // value only at a failing positional tail, never in either older API.
    if (!matches && tailCandidate && mappingSite !== null &&
        Object.hasOwn(mappingSite, 'actualFrame') && mappingSite.first === null) mappingSite.actualFrame = frame;
    return matches;
}
function legalMappingSite(site) {
    if (site === null) return false;
    const {stage, guard} = site;
    switch (stage) {
    case 'pe': return guard === 'parse' || guard === 'base_canonical';
    case 'section_binding': return guard === 'headers';
    case 'segment_map': return ['bytes', 'counts', 'identity', 'absolute', 'section'].includes(guard);
    case 'selector': return ['image_range', 'mapped_section', 'extent', 'extent_segment'].includes(guard);
    case 'procedure': return ['segment', 'start_rva', 'extent', 'extent_segment'].includes(guard);
    default: return false;
    }
}
const shared = b => isSharedArrayBuffer(b.buffer);
const canonical = v => v >= 0n && v <= 0xffffffffffffffffn &&
    (v <= 0x7fffffffffffn || v >= 0xffff800000000000n);
const hex = v => '0x' + v.toString(16);
const NI_MAC_ZERO_GRAMMAR = 'named_stream_ni_mac_zero_v1';
// An explicit bounded variant, not a producer/ABI or ownership assertion.
const ABSOLUTE_TAIL_FRAME_ZERO_GRAMMAR = 'identity_sections_absolute_tail_frame_zero_v1';
const INLINEES_COUNTED_U32_EXACT_GRAMMAR = 'inlinees_counted_u32_exact_v1';

function selection(query) {
    need(query && typeof query === 'object' && !Array.isArray(query) &&
        (Object.getPrototypeOf(query) === Object.prototype || Object.getPrototypeOf(query) === null), 'ERR_SELECTORS');
    need(Reflect.ownKeys(query).every(k => k === 'addresses' || k === 'module_indices') &&
        Object.hasOwn(query, 'addresses'), 'ERR_SELECTORS');
    const boundedArray = (v, max) => Array.isArray(v) && v.length >= 1 && v.length <= max &&
        Array.from({length: v.length}, (_, i) => Object.hasOwn(v, i)).every(Boolean);
    need(boundedArray(query.addresses, LIMITS.selectors), 'ERR_SELECTORS');
    const seen = new Set();
    const addresses = query.addresses.map(a => {
        need(typeof a === 'string' && /^0x(?:0|[1-9a-f][0-9a-f]{0,15})$/.test(a), 'ERR_SELECTORS');
        const value = BigInt(a);
        need(canonical(value) && !seen.has(a), 'ERR_SELECTORS'); seen.add(a);
        return {address: a, value};
    });
    let modules = null;
    if (Object.hasOwn(query, 'module_indices')) {
        need(boundedArray(query.module_indices, LIMITS.modules), 'ERR_SELECTORS');
        modules = query.module_indices.slice();
        need(modules.every(i => Number.isInteger(i) && i >= 0 && i < LIMITS.descriptors) &&
            new Set(modules).size === modules.length, 'ERR_SELECTORS');
        modules.sort((a, b) => a - b);
    }
    return {addresses, modules};
}

function infoFraming(m, identity) {
    const code = 'ERR_PDB_FRAMING', size = m.streams[1].size;
    need(size >= 48 && size <= LIMITS.info, code);
    const b = m.stream(1, 0, size);
    need(b.readUInt32LE() === 20000404 && b.readUInt32LE(8) === identity.age &&
        b.subarray(12, 28).toString('hex') === identity.guid_bytes, 'ERR_IDENTITY');
    let at = 28;
    const word = () => { need(at + 4 <= b.length, code); const v = b.readUInt32LE(at); at += 4; return v; };
    const namesSize = word(), namesAt = at;
    need(namesSize <= b.length - at, code); at += namesSize;
    const starts = new Set();
    for (let p = namesAt; p < at;) {
        starts.add(p - namesAt);
        const end = b.indexOf(0, p);
        need(end >= p && end < at, code); p = end + 1;
    }
    const count = word(), capacity = word();
    // A design bound independent of sparse hash capacity allocations.
    need(capacity > 0 && capacity <= LIMITS.info * 8 && count <= capacity &&
        count <= Math.floor(capacity * 2 / 3) + 1, code);
    const bits = () => {
        const words = word(); need(words <= Math.ceil(capacity / 32) && words * 4 <= b.length - at, code);
        const indices = new Set();
        for (let w = 0; w < words; w++) {
            const mask = word();
            for (let bit = 0; bit < 32; bit++) if ((mask >>> bit) & 1) {
                const index = w * 32 + bit; need(index < capacity, code); indices.add(index);
            }
        }
        return indices;
    };
    const present = bits(), deleted = bits();
    need(present.size === count && [...present].every(i => !deleted.has(i)), code);
    const keys = new Set(), names = new Set();
    for (let i = 0; i < count; i++) {
        const key = word(), stream = word();
        need(starts.has(key) && !keys.has(key) && stream < m.count && m.streams[stream].size !== 0xffffffff, code);
        keys.add(key);
        const end = b.indexOf(0, namesAt + key);
        const name = b.subarray(namesAt + key, end).toString('hex');
        need(!names.has(name), code); names.add(name);
    }
    need((b.length - at) % 4 === 0, code);
    return {b, count, featureStart: at};
}

function infoFeatures(b, featureStart) {
    const features = new Set();
    let at = featureStart;
    const refuse = (code, reason, feature) => ({features: [...features], refusal: {
        code, reason, word_index: (at - featureStart) / 4 - 1, word_offset: at - 4,
        word_u32: feature, remaining_words: (b.length - at) / 4,
    }});
    while (at < b.length) {
        const feature = b.readUInt32LE(at); at += 4;
        if (feature === 0x494e494d) return refuse('ERR_FASTLINK_UNSUPPORTED', 'minimal_debug_info', feature);
        if (feature === 0x4d544f4e) return refuse('ERR_FASTLINK_UNSUPPORTED', 'no_type_merge', feature);
        if (feature !== 20091201 && feature !== 20140508) return refuse('ERR_FEATURE_UNSUPPORTED', 'unknown_signature', feature);
        if (features.has(feature)) return refuse('ERR_FEATURE_UNSUPPORTED', 'duplicate_vc140', feature);
        features.add(feature);
        // VC110 terminates LLVM's feature parser. Do not hide an unchecked tail.
        if (feature === 20091201 && at !== b.length) return refuse('ERR_FEATURE_UNSUPPORTED', 'vc110_nonterminal', feature);
    }
    return {features: [...features], refusal: null};
}

function infoLayout(m, identity, niMacZero) {
    const framed = infoFraming(m, identity);
    if (!niMacZero) return framed;
    const namedMapEnd = framed.featureStart;
    // The opt-in writer grammar has exactly one structural NI after the map.
    // Whole-tail u32 framing above precedes interpreting this slot or features.
    need(namedMapEnd + 4 <= framed.b.length, 'ERR_PDB_FRAMING');
    need(framed.b.readUInt32LE(namedMapEnd) === 0, 'ERR_NI_MAC_UNSUPPORTED');
    return {...framed, namedMapEnd, featureStart: namedMapEnd + 4};
}

function infoProfile(m, identity, niMacZero) {
    const {b, count, featureStart} = infoLayout(m, identity, niMacZero);
    const {features, refusal} = infoFeatures(b, featureStart);
    if (refusal !== null) throw new MetadataError(refusal.code);
    if (niMacZero) return {grammar: NI_MAC_ZERO_GRAMMAR, named_stream_ni_mac: 0, named_streams: count, features};
    return {named_streams: count, features};
}

function identityInput(identity) {
    need(identity && typeof identity.guid_bytes === 'string' && /^[0-9a-f]{32}$/.test(identity.guid_bytes) &&
        Number.isInteger(identity.age) && identity.age > 0 && identity.age <= 0xffffffff, 'ERR_IDENTITY');
}

function boundedReader(reader) {
    need(reader && Number.isSafeInteger(reader.size) && reader.size >= 56 && reader.size <= LIMITS.pdb &&
        typeof reader.read === 'function', 'ERR_INPUT_BOUND');
    const size = reader.size;
    return {size, read(o, n) {
        need(Number.isSafeInteger(o) && Number.isSafeInteger(n) && o >= 0 && n >= 0 &&
            n <= LIMITS.read && o + n <= size, 'ERR_INPUT_BOUND');
        let b;
        try { b = reader.read(o, n); } catch { throw new MetadataError('ERR_PDB_FRAMING'); }
        need(Buffer.isBuffer(b) && b.length === n, 'ERR_SHORT_READ');
        need(!shared(b), 'ERR_INPUT_BOUND');
        return Buffer.from(b);
    }};
}

// Explicit info-only diagnostic: accepted_profile grants no procedure, body,
// ownership or complete input-identity qualification. Only stream 1 is opened.
function diagnoseInfo(reader, identity, niMacZero) {
    try {
        identityInput(identity);
        const {b, namedMapEnd, featureStart} = infoLayout(msf(boundedReader(reader)), identity, niMacZero);
        const {refusal} = infoFeatures(b, featureStart);
        const result = {
            profile: niMacZero ? 'MSFPDB7_Info_named_stream_ni_mac_zero_diagnostic_v1' :
                'MSFPDB7_Info_feature_guard_diagnostic_v1', info_bytes: b.length,
            ...(niMacZero ? {named_map_end_offset: namedMapEnd, ni_mac_offset: namedMapEnd,
                named_stream_ni_mac: 0} : {}),
            feature_start_offset: featureStart, feature_word_count: (b.length - featureStart) / 4,
            status: refusal === null ? 'accepted_profile' : 'refused_profile', refusal,
        };
        need(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024, 'ERR_OUTPUT_BOUND');
        return result;
    } catch (error) {
        if (error instanceof MetadataError) throw error;
        throw new MetadataError('ERR_PDB_FRAMING');
    }
}

function diagnoseInfoFeatures(reader, identity) {
    return diagnoseInfo(reader, identity, false);
}

function diagnoseInfoNamedStreamNiMacZero(reader, identity) {
    return diagnoseInfo(reader, identity, true);
}

function context(reader, exe, identity, niMacZero, mappingSite, segmentMapGrammar) {
    need(Buffer.isBuffer(exe) && exe.length >= 64 && exe.length <= LIMITS.exe && !shared(exe), 'ERR_INPUT_BOUND');
    identityInput(identity);
    const input = boundedReader(reader);
    let image;
    try { image = parse(exe); } catch { mappingNeed(false, mappingSite, 'pe', 'parse'); }
    mappingNeed(canonical(image.base), mappingSite, 'pe', 'base_canonical');
    const m = msf(input);
    const info = infoProfile(m, identity, niMacZero);
    need(m.streams[3].size >= 64 && m.streams[3].size !== 0xffffffff, 'ERR_PDB_FRAMING');
    const h = m.stream(3, 0, 64);
    need(h.readInt32LE() === -1 && h.readUInt32LE(4) === 19990903 &&
        h.readUInt32LE(8) === identity.age && h.readUInt16LE(58) === 0x8664 &&
        h.readUInt32LE(60) === 0, 'ERR_IDENTITY');
    const sizes = [24, 28, 32, 36, 40, 52].map(p => h.readInt32LE(p)), optional = h.readInt32LE(48);
    need(sizes.every(n => n >= 0) && sizes.slice(0, 5).every(n => n % 4 === 0) &&
        optional >= 12 && optional <= 22 && optional % 2 === 0 &&
        64 + sizes.reduce((a, b) => a + b, 0) + optional === m.streams[3].size, 'ERR_PDB_FRAMING');
    need(sizes[0] <= LIMITS.module_info, 'ERR_DBI_MODULE_INFO');
    need(sizes[1] <= LIMITS.contributions, 'ERR_ROUTING_BOUND');
    need(sizes[4] === 0 && h.readUInt32LE(44) === 0, 'ERR_REFERENCE_UNSUPPORTED');
    need((h.readUInt16LE(56) & ~7) === 0, 'ERR_PDB_FRAMING');
    need((h.readUInt16LE(56) & 4) === 0, 'ERR_REFERENCE_UNSUPPORTED');
    const optAt = 64 + sizes.reduce((a, b) => a + b, 0), dbg = m.stream(3, optAt, optional);
    need(dbg.readUInt16LE(6) === 0xffff && dbg.readUInt16LE(8) === 0xffff, 'ERR_OMAP_UNSUPPORTED');
    const reserved = new Set([0, 1, 2, 3, 4]);
    const live = i => i < m.count && m.streams[i].size !== 0xffffffff;
    for (const p of [12, 16, 20]) {
        const i = h.readUInt16LE(p);
        if (i !== 0xffff) { need(i > 4 && live(i), 'ERR_PDB_FRAMING'); reserved.add(i); }
    }
    for (let p = 0; p < optional; p += 2) {
        const i = dbg.readUInt16LE(p);
        if (i !== 0xffff) { need(i > 4 && live(i), 'ERR_PDB_FRAMING'); reserved.add(i); }
    }
    const sectionStream = dbg.readUInt16LE(10), pe = exe.readUInt32LE(60), op = pe + 24;
    const table = op + exe.readUInt16LE(pe + 20), sectionBytes = image.sections.length * 40;
    mappingNeed(live(sectionStream) && m.streams[sectionStream].size === sectionBytes &&
        m.stream(sectionStream, 0, sectionBytes).equals(exe.subarray(table, table + sectionBytes)),
        mappingSite, 'section_binding', 'headers');
    const absoluteTailFrameZero = segmentMapGrammar === ABSOLUTE_TAIL_FRAME_ZERO_GRAMMAR;
    let segmentMapProfile = null;
    if (absoluteTailFrameZero) mappingNeed(sizes[2] !== 0, mappingSite, 'segment_map', 'bytes');
    if (sizes[2] !== 0) {
        // OMFSegMapHeader + SecCount fixed 20-byte OMFSegMapDesc entries.
        mappingNeed(sizes[2] >= 4 && sizes[2] <= 4 + (image.sections.length + 1) * 20,
            mappingSite, 'segment_map', 'bytes');
        const sm = m.stream(3, 64 + sizes[0] + sizes[1], sizes[2]);
        const count = sm.readUInt16LE(), logical = sm.readUInt16LE(2);
        mappingNeed(logical === count && (absoluteTailFrameZero ? count === image.sections.length + 1 :
            (count === image.sections.length || count === image.sections.length + 1)) &&
            sizes[2] === 4 + count * 20, mappingSite, 'segment_map', 'counts');
        for (let i = 0; i < count; i++) {
            const at = 4 + i * 20, flags = sm.readUInt16LE(at), absolute = i === image.sections.length;
            // Preserve the identity predicate's left-to-right short circuit.
            // The optional last slot is positional only, since its flags/length
            // guard has not yet been reached. The opt-in frame helper reads once.
            const member = sm.readUInt16LE(at + 2) !== 0 ? 'overlay' :
                sm.readUInt16LE(at + 4) !== 0 ? 'group' :
                !segmentFrameMatches(sm, at, absolute && absoluteTailFrameZero ? 0 : i + 1,
                    mappingSite, absolute) ? 'frame' :
                sm.readUInt32LE(at + 12) !== 0 ? 'offset' : null;
            if (member !== null && mappingSite !== null && mappingSite.first === null &&
                mappingSite.identityDetail === null) {
                mappingSite.identityDetail = {descriptor_index: i,
                    descriptor_slot: absolute ? 'tail_candidate' : 'section', identity_member: member};
            }
            mappingNeed(member === null, mappingSite, 'segment_map', 'identity');
            if (absolute) mappingNeed(flags === 0x208 && sm.readUInt32LE(at + 16) === 0xffffffff,
                mappingSite, 'segment_map', 'absolute');
            else {
                const s = image.sections[i], expected = 0x108 | (s.readable ? 1 : 0) | (s.writable ? 2 : 0) | (s.executable ? 4 : 0);
                mappingNeed(flags === expected && sm.readUInt32LE(at + 16) === exe.readUInt32LE(table + i * 40 + 8),
                    mappingSite, 'segment_map', 'section');
            }
        }
        // Construct only after the exact presence/count/identity/full-tuple
        // checks. These facts never enter physical routing or extent arithmetic.
        if (absoluteTailFrameZero) segmentMapProfile = {
            grammar: ABSOLUTE_TAIL_FRAME_ZERO_GRAMMAR,
            physical_section_count: image.sections.length, descriptor_count: count,
            tail_descriptor_index: image.sections.length, tail_overlay: 0, tail_group: 0,
            tail_frame: 0, tail_offset: 0, tail_flags: 520, tail_length: 4294967295,
            tail_routable: false,
        };
    }
    const mapped = (rva, n) => {
        const sections = image.sections.filter(s => rva >= s.rva && rva + n <= s.rva + s.mappedBytes);
        need(sections.length === 1, 'ERR_IDENTITY');
        const s = sections[0];
        // Metadata pointers must not cause instruction/code-region reads.
        need(s.readable && s.initialized && !s.code && !s.executable && !s.writable && !s.resourceOverlap, 'ERR_IDENTITY');
        return s.raw + rva - s.rva;
    };
    need(exe.readUInt32LE(op + 108) > 6, 'ERR_IDENTITY');
    const drva = exe.readUInt32LE(op + 112 + 6 * 8), bytes = exe.readUInt32LE(op + 116 + 6 * 8);
    need(bytes > 0 && bytes <= 4096 && bytes % 28 === 0, 'ERR_IDENTITY');
    const dd = mapped(drva, bytes); let cv = 0;
    for (let p = dd; p < dd + bytes; p += 28) {
        if (exe.readUInt32LE(p + 12) !== 2) continue;
        cv++;
        const n = exe.readUInt32LE(p + 16), rva = exe.readUInt32LE(p + 20), raw = exe.readUInt32LE(p + 24);
        need(n >= 25 && n <= 4096 && mapped(rva, n) === raw &&
            exe.subarray(raw, raw + 4).equals(Buffer.from('RSDS')) &&
            exe.subarray(raw + 4, raw + 20).toString('hex') === identity.guid_bytes &&
            exe.readUInt32LE(raw + 20) === identity.age && exe[raw + n - 1] === 0, 'ERR_IDENTITY');
    }
    need(cv === 1, 'ERR_IDENTITY');
    return {m, image, info, sizes, reserved, mappingSite, segmentMapProfile};
}

function descriptors(m, bytes) {
    const code = 'ERR_DBI_MODULE_INFO', b = m.stream(3, 64, bytes), modules = [];
    let at = 0;
    while (at < b.length) {
        need(at + 64 <= b.length && modules.length < LIMITS.descriptors, code);
        const start = at;
        at += 64;
        for (let i = 0; i < 2; i++) {
            const end = b.indexOf(0, at); need(end >= at && end < b.length, code); at = end + 1;
        }
        const aligned = Math.ceil(at / 4) * 4;
        need(aligned <= b.length, code);
        while (at < aligned) { need(b[at] === 0, code); at++; }
        modules.push({module_index: modules.length, stream_id: b.readUInt16LE(start + 34),
            sym: b.readUInt32LE(start + 36), c11: b.readUInt32LE(start + 40),
            c13: b.readUInt32LE(start + 44), flags: b.readUInt16LE(start + 32)});
    }
    return modules;
}

function route(ctx, modules, addresses, explicit) {
    const {m, image, sizes} = ctx;
    if (explicit) {
        need(explicit.every(i => i < modules.length), 'ERR_MODULE_BOUND');
        return {indices: explicit, scope: 'explicit_modules_only', complete_table_framing: false,
            contribution_bytes: sizes[1], contribution_records: null, contribution_version: null,
            address_routes: null};
    }
    const code = 'ERR_ROUTING_FRAMING', bytes = sizes[1], start = 64 + sizes[0];
    need(bytes >= 4, 'ERR_ROUTING_ABSENT');
    const version = m.stream(3, start, 4).readUInt32LE();
    const width = version === 0xeffe0000 + 19970605 ? 28 : version === 0xeffe0000 + 20140516 ? 32 : 0;
    need(width !== 0, 'ERR_ROUTING_UNSUPPORTED');
    need((bytes - 4) % width === 0, code);
    const records = (bytes - 4) / width;
    need(records <= LIMITS.contribution_records, 'ERR_ROUTING_BOUND');
    const selected = new Set(), perAddress = addresses.map(() => ({modules: new Set(), records: 0}));
    const chunkBytes = Math.floor(LIMITS.read / width) * width;
    for (let off = 4; off < bytes;) {
        const take = Math.min(chunkBytes, bytes - off), b = m.stream(3, start + off, take);
        for (let p = 0; p < take; p += width) {
            const segment = b.readUInt16LE(p), offset = b.readInt32LE(p + 4), size = b.readInt32LE(p + 8);
            const index = b.readUInt16LE(p + 16);
            need(segment >= 1 && segment <= image.sections.length && offset >= 0 && size >= 0 &&
                index < modules.length, code);
            const section = image.sections[segment - 1], begin = section.rva + offset, end = begin + size;
            need(offset + size <= section.memoryBytes && end <= image.imageBytes && end <= 0x100000000, code);
            for (let q = 0; q < addresses.length; q++) if (addresses[q].rva >= begin && addresses[q].rva < end) {
                selected.add(index); perAddress[q].modules.add(index); perAddress[q].records++;
                need(selected.size <= LIMITS.modules, 'ERR_MODULE_BOUND');
            }
        }
        off += take;
    }
    need(perAddress.every(a => a.modules.size > 0), 'ERR_ROUTING_ABSENT');
    return {indices: [...selected].sort((a, b) => a - b), scope: 'all_matching_section_contribution_modules',
        complete_table_framing: true, contribution_bytes: bytes, contribution_records: records,
        contribution_version: version, address_routes: perAddress.map((a, i) => ({address: addresses[i].address,
            module_indices: [...a.modules].sort((a, b) => a - b), matching_contribution_records: a.records}))};
}

const PROCEDURES = new Set([0x110f, 0x1110, 0x1146, 0x1147]);
const EXTERNAL = new Set([0x115f, 0x1160, 0x1161, 0x1167]);
const UNSUPPORTED = new Set([
    0x0104, 0x0105, 0x0106, 0x0204, 0x0205, 0x0206, 0x0300, 0x0301,
    0x0400, 0x0403, 0x100a, 0x100b, 0x1010, 0x1011, 0x1015, 0x1016,
    0x101a, 0x101b, 0x1020, 0x1021, 0x1022, 0x1023, 0x1024, 0x1025, 0x1026, 0x1027, 0x1028,
    0x1102, 0x1114, 0x1115, 0x1118, 0x1119, 0x111c, 0x111d, 0x111e, 0x111f,
    0x1120, 0x1121, 0x1122, 0x1123, 0x112a, 0x112b, 0x112c, 0x112d, 0x1132,
    0x1148, 0x1149, 0x114a, 0x114b, 0x1154, 0x1155, 0x1156, 0x1157, 0x1158, 0x1168,
]);

function legalUnsupportedKind(kind) {
    return Number.isInteger(kind) && !Object.is(kind, -0) && kind >= 0 && kind <= 0xffff && UNSUPPORTED.has(kind);
}

function legalInlineesUnsupportedKind(kind) {
    return legalUnsupportedKind(kind) && kind !== 0x1168;
}

function extent(image, segment, offset, size, mappingSite, stage) {
    need(size > 0, 'ERR_PROCEDURE_SIZE');
    // Defensive only under the immutable-input contract: selectors have just
    // found an in-range section; supported records have passed procedure/segment.
    // Neither public caller can independently reach extent_segment today.
    mappingNeed(segment >= 1 && segment <= image.sections.length, mappingSite, stage, 'extent_segment');
    const s = image.sections[segment - 1], start = s.rva + offset, end = start + size;
    const va = image.base + BigInt(start), exclusive = image.base + BigInt(end);
    mappingNeed(offset + size <= s.mappedBytes && start < end && end <= image.imageBytes &&
        end <= 0x100000000 && canonical(va) && canonical(exclusive - 1n) && canonical(exclusive) &&
        s.code && s.readable && s.executable && !s.writable && !s.resourceOverlap, mappingSite, stage, 'extent');
    return {segment, section_id: s.id, section_offset: offset, start_rva: start, end_rva: end,
        size, start: hex(va), end_inclusive: hex(exclusive - 1n), end_exclusive: hex(exclusive),
        file_span: {start: s.raw + offset, end_exclusive: s.raw + offset + size}};
}

function scanModule(ctx, mod, aggregates, totals, unsupportedSite, inlineesGrammar) {
    const {m, image} = ctx, code = 'ERR_MODULE_FRAMING', stream = mod.stream_id;
    const tail = mod.sym + mod.c11 + mod.c13;
    need(mod.sym >= 4 && mod.sym % 4 === 0 && mod.c11 === 0 && tail + 4 <= m.streams[stream].size, code);
    need(m.stream(stream, 0, 4).readUInt32LE() === 4, code);
    const refs = m.stream(stream, tail, 4).readUInt32LE();
    need(tail + 4 + refs === m.streams[stream].size, code);
    let cache = null, cacheAt = -1;
    const read = (o, n) => {
        need(o >= 4 && n >= 0 && n <= LIMITS.record && o + n <= mod.sym, code);
        const out = Buffer.alloc(n); let done = 0;
        while (done < n) {
            const pos = o + done, base = Math.floor(pos / LIMITS.cache) * LIMITS.cache;
            if (cacheAt !== base) { cacheAt = base; cache = m.stream(stream, base, Math.min(LIMITS.cache, mod.sym - base)); }
            const part = Math.min(n - done, cache.length - (pos - base));
            cache.copy(out, done, pos - base, pos - base + part); done += part;
        }
        return out;
    };
    let records = 0, procedures = 0, opaque = 0, matches = 0;
    for (let at = 4; at < mod.sym;) {
        need(at + 4 <= mod.sym, code);
        const h = read(at, 4), n = h.readUInt16LE() + 2, kind = h.readUInt16LE(2);
        need(n >= 4 && n <= LIMITS.record && n % 4 === 0 && at + n <= mod.sym, code);
        need(++totals.records <= LIMITS.records, 'ERR_MODULE_BOUND'); records++;
        need(!EXTERNAL.has(kind), 'ERR_FASTLINK_UNSUPPORTED');
        if (inlineesGrammar === INLINEES_COUNTED_U32_EXACT_GRAMMAR && kind === 0x1168) {
            // Exact LLVM-shaped subset only. The whole prefix-inclusive record
            // is bounded before reading the count; no count-driven allocation,
            // entry traversal, padding guesses or semantic value predicates.
            need(n >= 8 && n <= 0xff00, code);
            const b = read(at, n), count = b.readUInt32LE(4);
            need(count === (n - 8) / 4, code);
            const records = totals.inlinees.records + 1, entries = totals.inlinees.entries + count;
            need(Number.isSafeInteger(records) && records > 0 && records <= totals.records &&
                Number.isSafeInteger(entries) && entries >= 0 && entries <= Math.floor(LIMITS.symbols / 4), code);
            totals.inlinees.records = records; totals.inlinees.entries = entries;
        } else if (UNSUPPORTED.has(kind)) {
            // Retain only the already-read discriminator at this exact guard.
            // The private opt-in site adds no payload/header read or traversal.
            if (unsupportedSite !== null && unsupportedSite.first === null) unsupportedSite.first = kind;
            throw new MetadataError('ERR_SYMBOL_UNSUPPORTED');
        }
        if (PROCEDURES.has(kind)) {
            procedures++;
            const b = read(at, n);
            need(n >= 40, code);
            const nul = b.indexOf(0, 39); need(nul >= 39 && n - nul - 1 <= 3, code);
            const padding = n - nul - 1;
            need(b.subarray(nul + 1).every(v => v === 0) ||
                b.subarray(nul + 1).every((v, i) => v === 0xf0 + padding - i), code);
            const segment = b.readUInt16LE(36), offset = b.readUInt32LE(32), size = b.readUInt32LE(16);
            mappingNeed(segment >= 1 && segment <= image.sections.length, ctx.mappingSite, 'procedure', 'segment');
            const rva = image.sections[segment - 1].rva + offset;
            mappingNeed(rva <= 0xffffffff, ctx.mappingSite, 'procedure', 'start_rva');
            const selected = aggregates.get(rva);
            if (selected) {
                const mapping = extent(image, segment, offset, size, ctx.mappingSite, 'procedure');
                need(selected.extent === null || JSON.stringify(selected.extent) === JSON.stringify(mapping), 'ERR_PROCEDURE_CONFLICT');
                selected.extent = mapping; selected.count++; selected.bytes += n; selected.hash.update(b); matches++;
            }
        } else opaque++;
        at += n;
    }
    totals.procedures += procedures; totals.opaque += opaque;
    return {module_index: mod.module_index, stream_id: stream, symbol_bytes: mod.sym,
        records, supported_procedure_records: procedures, opaque_records: opaque, matching_records: matches};
}

function inspectProfile(reader, exe, identity, query, niMacZero, mappingSite = null, segmentMapGrammar = null,
    unsupportedSite = null, inlineesGrammar = null) {
    try {
        // Selector validation is deliberately before any PE parsing or reader I/O.
        const selected = selection(query),
            ctx = context(reader, exe, identity, niMacZero, mappingSite, segmentMapGrammar), {image, m} = ctx;
        for (const a of selected.addresses) {
            mappingNeed(a.value >= image.base && a.value < image.base + BigInt(image.imageBytes),
                mappingSite, 'selector', 'image_range');
            a.rva = Number(a.value - image.base);
            const segment = image.sections.findIndex(s => a.rva >= s.rva && a.rva < s.rva + s.mappedBytes) + 1;
            mappingNeed(segment > 0, mappingSite, 'selector', 'mapped_section');
            extent(image, segment, a.rva - image.sections[segment - 1].rva, 1, mappingSite, 'selector');
        }
        const modules = descriptors(m, ctx.sizes[0]);
        const routing = route(ctx, modules, selected.addresses, selected.modules);
        let symbolBytes = 0; const streams = new Set();
        for (const index of routing.indices) {
            const mod = modules[index], stream = mod.stream_id;
            need(stream < m.count && m.streams[stream].size !== 0xffffffff &&
                !ctx.reserved.has(stream) && !streams.has(stream), 'ERR_MODULE_BOUND'); streams.add(stream);
            need((mod.flags & 0xff00) === 0, 'ERR_REFERENCE_UNSUPPORTED');
            symbolBytes += mod.sym; need(symbolBytes <= LIMITS.symbols, 'ERR_MODULE_BOUND');
        }
        const aggregates = new Map(selected.addresses.map(a => [a.rva,
            {address: a.address, count: 0, bytes: 0, hash: createHash('sha256'), extent: null}]));
        const totals = {records: 0, procedures: 0, opaque: 0};
        const inlineesExact = inlineesGrammar === INLINEES_COUNTED_U32_EXACT_GRAMMAR;
        if (inlineesExact) totals.inlinees = {grammar: INLINEES_COUNTED_U32_EXACT_GRAMMAR, records: 0, entries: 0};
        const scanned = routing.indices.map(i => scanModule(ctx, modules[i], aggregates, totals, unsupportedSite,
            inlineesGrammar));
        need([...aggregates.values()].every(a => a.count > 0), 'ERR_PROCEDURE_ABSENT');
        const result = {
            profile: 'MSF7_DBI_C13_scoped_supported_procedure_declarations' +
                (niMacZero ? '_' + NI_MAC_ZERO_GRAMMAR : '') +
                (ctx.segmentMapProfile === null ? '' : '_absolute_tail_frame_zero_v1') +
                (inlineesExact ? '_' + INLINEES_COUNTED_U32_EXACT_GRAMMAR : ''),
            guid_bytes: identity.guid_bytes, age: identity.age, image_base: hex(image.base),
            routing, selected_modules: scanned, symbol_bytes: symbolBytes, records: totals.records,
            supported_procedure_records: totals.procedures, opaque_records: totals.opaque,
            info_profile: ctx.info, omap_absent: true, pdb_pe_section_headers_equal: true,
            complete_selected_symbol_record_framing: true, unsupported_semantic_coverage: false,
            declaration_extent_only: true, all_pdb_alias_coverage_confirmed: false,
            whole_pdb_coverage_confirmed: false, reference_linkage_confirmed: false,
            instruction_boundaries_confirmed: false, function_completeness_confirmed: false,
            unique_owner_confirmed: false, purity_confirmed: false, unwind_interpretation_confirmed: false,
            runtime_behavior_confirmed: false, names_exported: false, code_bytes_read: false,
            addresses: [...aggregates.values()].map(a => ({address: a.address,
                procedure_record_count: a.count, procedure_record_bytes: a.bytes,
                procedure_records_sha256: a.hash.digest('hex'), extent: a.extent})),
        };
        if (ctx.segmentMapProfile !== null) result.segment_map_profile = ctx.segmentMapProfile;
        if (inlineesExact) {
            need(totals.inlinees.records <= totals.opaque && totals.records === totals.procedures + totals.opaque,
                'ERR_MODULE_FRAMING');
            result.inlinees_profile = totals.inlinees;
        }
        need(Buffer.byteLength(JSON.stringify(result) + '\n') <= LIMITS.output, 'ERR_OUTPUT_BOUND');
        return result;
    } catch (error) {
        if (error instanceof MetadataError) throw error;
        // Never propagate reader errors, buffers, paths, names, or parser text.
        throw new MetadataError('ERR_PDB_FRAMING');
    }
}

function inspect(reader, exe, identity, query) {
    return inspectProfile(reader, exe, identity, query, false);
}

function inspectNamedStreamNiMacZero(reader, exe, identity, query) {
    return inspectProfile(reader, exe, identity, query, true);
}

// Exact NI-zero + present N+1 zero-frame absolute tail only. No retry, union
// with older map shapes, new diagnostic, or tail-derived physical section.
function inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(reader, exe, identity, query) {
    return inspectProfile(reader, exe, identity, query, true, null, ABSOLUTE_TAIL_FRAME_ZERO_GRAMMAR);
}

// Same exact NI-zero / zero-frame-tail envelope, with one closed auxiliary
// grammar. This per-call selector is disabled for every older API/diagnostic.
function inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(reader, exe, identity, query) {
    return inspectProfile(reader, exe, identity, query, true, null, ABSOLUTE_TAIL_FRAME_ZERO_GRAMMAR,
        null, INLINEES_COUNTED_U32_EXACT_GRAMMAR);
}

// Exact zero-tail grammar only. Refusal is a prefix observation, while acceptance
// requires the entire ordinary scan, final checks and internal digest work.
// Discard all accumulated ordinary metadata in either diagnostic branch.
function diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(reader, exe, identity, query) {
    const unsupportedSite = {first: null};
    try {
        let status = 'accepted_profile';
        try {
            inspectProfile(reader, exe, identity, query, true, null, ABSOLUTE_TAIL_FRAME_ZERO_GRAMMAR,
                unsupportedSite);
        } catch (error) {
            if (!(error instanceof MetadataError) || error.code !== 'ERR_SYMBOL_UNSUPPORTED' ||
                !legalUnsupportedKind(unsupportedSite.first)) throw error;
            status = 'refused_symbol';
        }
        const refused = status === 'refused_symbol';
        need(refused ? legalUnsupportedKind(unsupportedSite.first) : unsupportedSite.first === null,
            'ERR_SYMBOL_UNSUPPORTED');
        const result = {
            profile: 'MSF7_procedure_unsupported_kind_zero_tail_diagnostic_v1', status,
            stage: refused ? 'procedure_metadata' : 'complete',
            guard: refused ? 'unsupported_kind' : null,
            kind_u16: refused ? unsupportedSite.first : null,
        };
        need(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024, 'ERR_OUTPUT_BOUND');
        return result;
    } catch (error) {
        if (error instanceof MetadataError) throw error;
        throw new MetadataError('ERR_PDB_FRAMING');
    }
}

// Separate opt-in diagnostic over the full exact-inlinees traversal. Only the
// remaining 54 guard discriminators may be reported, after the ordinary record
// guards. Exact lists still require validation; no retry or second query occurs.
// Acceptance requires every full metadata/final/output check before discarding
// that metadata. A refused discriminator describes only the first failing guard.
function diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(reader, exe, identity, query) {
    const unsupportedSite = {first: null};
    try {
        let status = 'accepted_profile';
        try {
            inspectProfile(reader, exe, identity, query, true, null, ABSOLUTE_TAIL_FRAME_ZERO_GRAMMAR,
                unsupportedSite, INLINEES_COUNTED_U32_EXACT_GRAMMAR);
        } catch (error) {
            if (!(error instanceof MetadataError) || error.code !== 'ERR_SYMBOL_UNSUPPORTED' ||
                !legalInlineesUnsupportedKind(unsupportedSite.first)) throw error;
            status = 'refused_symbol';
        }
        const refused = status === 'refused_symbol';
        need(refused ? legalInlineesUnsupportedKind(unsupportedSite.first) : unsupportedSite.first === null,
            'ERR_SYMBOL_UNSUPPORTED');
        const result = {
            profile: 'MSF7_procedure_unsupported_kind_inlinees_exact_diagnostic_v1', status,
            stage: refused ? 'procedure_metadata' : 'complete',
            guard: refused ? 'unsupported_kind' : null,
            kind_u16: refused ? unsupportedSite.first : null,
        };
        need(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024, 'ERR_OUTPUT_BOUND');
        return result;
    } catch (error) {
        if (error instanceof MetadataError) throw error;
        throw new MetadataError('ERR_PDB_FRAMING');
    }
}

// Full strict NI profile traversal, including the ordinary output/conflict
// checks and existing internal digests. Discard that result; export only a
// closed first-refusal site or complete acceptance, never partial coverage.
function diagnoseSourceMappingNamedStreamNiMacZero(reader, exe, identity, query) {
    const mappingSite = {first: null};
    try {
        let status = 'accepted_profile';
        try { inspectProfile(reader, exe, identity, query, true, mappingSite); }
        catch (error) {
            if (!(error instanceof MetadataError) || error.code !== 'ERR_SOURCE_MAPPING' ||
                !legalMappingSite(mappingSite.first)) throw error;
            status = 'refused_mapping';
        }
        const result = {
            profile: 'MSF7_procedure_source_mapping_diagnostic_v1', status,
            stage: status === 'accepted_profile' ? 'complete' : mappingSite.first.stage,
            guard: status === 'accepted_profile' ? null : mappingSite.first.guard,
        };
        need(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024, 'ERR_OUTPUT_BOUND');
        return result;
    } catch (error) {
        if (error instanceof MetadataError) throw error;
        throw new MetadataError('ERR_PDB_FRAMING');
    }
}

// Opt-in identity-site detail only. Full acceptance retains all ordinary
// checks; the older four-field diagnostic and code-only errors stay unchanged.
function diagnoseMappingIdentityProfile(reader, exe, identity, query, tailFrame) {
    const mappingSite = {first: null, identityDetail: null};
    if (tailFrame) mappingSite.actualFrame = null;
    try {
        let status = 'accepted_profile';
        try { inspectProfile(reader, exe, identity, query, true, mappingSite); }
        catch (error) {
            if (!(error instanceof MetadataError) || error.code !== 'ERR_SOURCE_MAPPING' ||
                !legalMappingSite(mappingSite.first)) throw error;
            status = 'refused_mapping';
        }
        const site = mappingSite.first, detail = mappingSite.identityDetail;
        const identityRefusal = status === 'refused_mapping' && site.stage === 'segment_map' && site.guard === 'identity';
        need(identityRefusal ? detail !== null && Number.isInteger(detail.descriptor_index) &&
            detail.descriptor_index >= 0 && detail.descriptor_index <= LIMITS.sections &&
            (detail.descriptor_slot === 'section' || detail.descriptor_slot === 'tail_candidate') &&
            ['overlay', 'group', 'frame', 'offset'].includes(detail.identity_member) : detail === null,
            'ERR_SOURCE_MAPPING');
        const result = {
            profile: tailFrame ? 'MSF7_procedure_tail_frame_diagnostic_v1' :
                'MSF7_procedure_source_mapping_identity_diagnostic_v1', status,
            stage: status === 'accepted_profile' ? 'complete' : site.stage,
            guard: status === 'accepted_profile' ? null : site.guard,
            descriptor_index: identityRefusal ? detail.descriptor_index : null,
            descriptor_slot: identityRefusal ? detail.descriptor_slot : null,
            identity_member: identityRefusal ? detail.identity_member : null,
        };
        if (tailFrame) {
            const frameRefusal = identityRefusal && detail.descriptor_slot === 'tail_candidate' &&
                detail.identity_member === 'frame';
            need(frameRefusal ? Number.isInteger(mappingSite.actualFrame) && mappingSite.actualFrame >= 0 &&
                mappingSite.actualFrame <= 0xffff && mappingSite.actualFrame !== detail.descriptor_index + 1 :
                mappingSite.actualFrame === null, 'ERR_SOURCE_MAPPING');
            result.actual_frame = mappingSite.actualFrame;
        }
        need(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024, 'ERR_OUTPUT_BOUND');
        return result;
    } catch (error) {
        if (error instanceof MetadataError) throw error;
        throw new MetadataError('ERR_PDB_FRAMING');
    }
}

function diagnoseSourceMappingIdentityNamedStreamNiMacZero(reader, exe, identity, query) {
    return diagnoseMappingIdentityProfile(reader, exe, identity, query, false);
}

// Separate opt-in scalar diagnostic. A mismatch remains refused; this grants
// no alternate mapping, tail routing, partial coverage or fallback traversal.
function diagnoseTailFrameNamedStreamNiMacZero(reader, exe, identity, query) {
    return diagnoseMappingIdentityProfile(reader, exe, identity, query, true);
}

module.exports = {inspect, diagnoseInfoFeatures, inspectNamedStreamNiMacZero, diagnoseInfoNamedStreamNiMacZero,
    inspectNamedStreamNiMacZeroAbsoluteTailFrameZero,
    inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
    diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero,
    diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
    diagnoseSourceMappingNamedStreamNiMacZero, diagnoseSourceMappingIdentityNamedStreamNiMacZero,
    diagnoseTailFrameNamedStreamNiMacZero, LIMITS};
