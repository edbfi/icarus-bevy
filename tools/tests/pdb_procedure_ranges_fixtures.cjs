'use strict';
// Independently authored synthetic envelopes; no target data or production
// serializer. Layout references and their licenses are preserved in
// ../pdb_procedure_ranges.cjs (LLVM llvmorg-20.1.8, Apache-2.0 WITH LLVM-exception;
// Microsoft 805655a28bd8198004be2ac27e6e0290121a5e89, MIT). In particular:
// https://github.com/llvm/llvm-project/blob/llvmorg-20.1.8/llvm/include/llvm/DebugInfo/PDB/Native/RawTypes.h
// https://github.com/microsoft/microsoft-pdb/blob/805655a28bd8198004be2ac27e6e0290121a5e89/PDB/include/dbicommon.h
const GUID = '0123456789abcdef0123456789abcdef';
const SC60 = 0xeffe0000 + 19970605, SC2 = 0xeffe0000 + 20140516;
const VA = '0x140001020';

function pe() {
    const exe = Buffer.alloc(0xa00), peAt = 0x80, op = peAt + 24, table = op + 240;
    exe.writeUInt16LE(0x5a4d); exe.writeUInt32LE(peAt, 60);
    exe.writeUInt32LE(0x4550, peAt); exe.writeUInt16LE(0x8664, peAt + 4);
    exe.writeUInt16LE(3, peAt + 6); exe.writeUInt16LE(240, peAt + 20); exe.writeUInt16LE(2, peAt + 22);
    exe.writeUInt16LE(0x20b, op); exe.writeUInt32LE(0x1000, op + 16);
    exe.writeBigUInt64LE(0x140000000n, op + 24);
    exe.writeUInt32LE(0x1000, op + 32); exe.writeUInt32LE(0x200, op + 36);
    exe.writeUInt32LE(0x4000, op + 56); exe.writeUInt32LE(0x200, op + 60); exe.writeUInt32LE(16, op + 108);
    [[0x1000, 0x200, 0x280, 0x400, 0x60000020],
        [0x2000, 0x600, 0x180, 0x200, 0x60000020],
        [0x3000, 0x800, 0x180, 0x200, 0x40000040]].forEach(([rva, raw, virtual, bytes, flags], i) => {
        const at = table + i * 40;
        exe.writeUInt32LE(virtual, at + 8); exe.writeUInt32LE(rva, at + 12);
        exe.writeUInt32LE(bytes, at + 16); exe.writeUInt32LE(raw, at + 20); exe.writeUInt32LE(flags, at + 36);
    });
    exe.fill(0xa5, 0x200, 0x800); // Code is deliberately unrelated to records.
    exe.writeUInt32LE(0x3080, op + 112 + 6 * 8); exe.writeUInt32LE(28, op + 116 + 6 * 8);
    const debug = 0x880, rsds = 0x8a0;
    exe.writeUInt32LE(2, debug + 12); exe.writeUInt32LE(32, debug + 16);
    exe.writeUInt32LE(0x30a0, debug + 20); exe.writeUInt32LE(rsds, debug + 24);
    exe.write('RSDS', rsds); Buffer.from(GUID, 'hex').copy(exe, rsds + 4);
    exe.writeUInt32LE(1, rsds + 20); exe.write('opaque', rsds + 24);
    return {exe, op, table, debug, rsds};
}

function proc({kind = 0x110f, offset = 0x20, size = 0x30, segment = 1,
    name = 'private display name', type = 0x9999, padding = 'zero'} = {}) {
    const text = Buffer.isBuffer(name) ? name : Buffer.from(name), n = Math.ceil((40 + text.length) / 4) * 4;
    const b = Buffer.alloc(n);
    b.writeUInt16LE(n - 2); b.writeUInt16LE(kind, 2); b.writeUInt32LE(size, 16);
    b.writeUInt32LE(type, 28); b.writeUInt32LE(offset, 32); b.writeUInt16LE(segment, 36); text.copy(b, 39);
    if (padding === 'cv') for (let at = 40 + text.length; at < n; at++) b[at] = 0xf0 + n - at;
    return b;
}

function record(kind = 0x7777, bytes = 4) {
    const b = Buffer.alloc(bytes); b.writeUInt16LE(bytes - 2); b.writeUInt16LE(kind, 2); return b;
}

function info({names = [], features = [20140508]} = {}) {
    const strings = Buffer.concat(names.map(n => Buffer.concat([Buffer.from(n.name), Buffer.alloc(1)])));
    const capacity = Math.max(1, names.length * 2), words = Math.ceil(names.length / 32);
    const b = Buffer.alloc(28 + 4 + strings.length + 8 + 4 + words * 4 + 4 + names.length * 8 + features.length * 4);
    b.writeUInt32LE(20000404); b.writeUInt32LE(1, 8); Buffer.from(GUID, 'hex').copy(b, 12);
    b.writeUInt32LE(strings.length, 28); strings.copy(b, 32);
    let at = 32 + strings.length;
    b.writeUInt32LE(names.length, at); b.writeUInt32LE(capacity, at + 4); at += 8;
    b.writeUInt32LE(words, at); at += 4;
    for (let i = 0; i < names.length; i++) {
        const pos = at + Math.floor(i / 32) * 4;
        b.writeUInt32LE((b.readUInt32LE(pos) | (1 << (i % 32))) >>> 0, pos);
    }
    at += words * 4; b.writeUInt32LE(0, at); at += 4;
    let key = 0;
    for (const n of names) {
        b.writeUInt32LE(key, at); b.writeUInt32LE(n.stream, at + 4); at += 8;
        key += Buffer.byteLength(n.name) + 1;
    }
    for (const feature of features) { b.writeUInt32LE(feature, at); at += 4; }
    return b;
}

function contribution({module_index = 0, segment = 1, offset = 0, size = 0x180, width = 28,
    coff_section = 0xabcdef01} = {}) {
    const b = Buffer.alloc(width);
    b.writeUInt16LE(segment); b.writeInt32LE(offset, 4); b.writeInt32LE(size, 8);
    b.writeUInt32LE(0x60000020, 12); b.writeUInt16LE(module_index, 16);
    if (width === 32) b.writeUInt32LE(coff_section, 28);
    return b;
}

function segmentMap(includeAbsolute = true) {
    const count = includeAbsolute ? 4 : 3, b = Buffer.alloc(4 + count * 20);
    b.writeUInt16LE(count); b.writeUInt16LE(count, 2);
    for (let i = 0; i < count; i++) {
        const at = 4 + i * 20;
        b.writeUInt16LE(i < 2 ? 0x10d : i === 2 ? 0x109 : 0x208, at);
        b.writeUInt16LE(i + 1, at + 6); b.writeUInt16LE(0xffff, at + 8); b.writeUInt16LE(0xffff, at + 10);
        b.writeUInt32LE(i === 0 ? 0x280 : i < 3 ? 0x180 : 0xffffffff, at + 16);
    }
    return b;
}

function fixture(options = {}) {
    const image = pe(), streams = Array(9).fill(null);
    streams[0] = Buffer.alloc(0); streams[1] = options.info ?? info();
    streams[5] = Buffer.alloc(0); streams[6] = Buffer.alloc(0); streams[7] = Buffer.alloc(0);
    streams[8] = Buffer.from(image.exe.subarray(image.table, image.table + 120));
    const specs = options.modules ?? [{records: [proc()]}], descriptors = [];
    const normalized = specs.map((s, i) => {
        const stream = s.stream ?? 9 + i * 3, records = s.records ?? [proc()];
        const symbols = Buffer.concat([Buffer.alloc(4), ...records]); symbols.writeUInt32LE(s.signature ?? 4);
        const c11 = s.c11 ?? Buffer.alloc(0), c13 = s.c13 ?? Buffer.alloc(0), refs = s.refs ?? Buffer.alloc(0);
        const trailer = Buffer.alloc(4); trailer.writeUInt32LE(refs.length);
        const body = s.body ?? Buffer.concat([symbols, c11, c13, trailer, refs]);
        while (streams.length <= stream) streams.push(null);
        streams[stream] = body;
        const names = Buffer.concat([Buffer.from(s.module_name ?? '/opaque/module'), Buffer.alloc(1),
            Buffer.from(s.object_name ?? '/never/open/private.obj'), Buffer.alloc(1)]);
        const d = Buffer.alloc(Math.ceil((64 + names.length) / 4) * 4);
        d.writeUInt32LE(0xdeadbeef); // Neither a stream ID nor an ordinal.
        contribution({module_index: i, segment: 2, size: 8}).copy(d, 4); // Decoy FIRST contribution.
        d.writeUInt16LE(s.flags ?? 0, 32); d.writeUInt16LE(stream, 34);
        d.writeUInt32LE(s.sym ?? symbols.length, 36); d.writeUInt32LE(c11.length, 40); d.writeUInt32LE(c13.length, 44);
        names.copy(d, 64); descriptors.push(d);
        return {stream, symbols, descriptor: d, body};
    });
    const moduleInfo = options.moduleInfo ?? Buffer.concat(descriptors), version = options.version ?? SC60;
    const contributions = options.contributions ?? specs.map((_, i) => ({module_index: i}));
    const head = Buffer.alloc(4); head.writeUInt32LE(version);
    const sc = options.sc ?? Buffer.concat([head, ...contributions.map(c => Buffer.isBuffer(c) ? c : contribution({
        ...c, width: version === SC2 ? 32 : 28}))]);
    const sectionMap = options.sectionMap ?? Buffer.alloc(0), h = Buffer.alloc(64), optional = Buffer.alloc(22, 0xff);
    h.writeInt32LE(-1); h.writeUInt32LE(19990903, 4); h.writeUInt32LE(1, 8);
    h.writeUInt16LE(5, 12); h.writeUInt16LE(7, 16); h.writeUInt16LE(6, 20);
    h.writeInt32LE(moduleInfo.length, 24); h.writeInt32LE(sc.length, 28);
    h.writeInt32LE(sectionMap.length, 32);
    h.writeInt32LE(optional.length, 48); h.writeUInt16LE(0x8664, 58); optional.writeUInt16LE(8, 10);
    streams[3] = Buffer.concat([h, moduleInfo, sc, sectionMap, optional]);
    return {...image, streams, modules: normalized, moduleInfo, sc, sectionMap, scAt: 64 + moduleInfo.length,
        optionalAt: 64 + moduleInfo.length + sc.length + sectionMap.length, identity: {guid_bytes: GUID, age: 1}};
}

// A sparse synthetic MSF reader permits exact cap tests without retaining a
// 256 MiB byte array. Stream sources are Buffers or {size, read(offset,count)}.
// Physical page assignments and directory serialization are independent of msf().
function pack(f) {
    const bs = 4096, entries = f.streams.map(s => ({size: s === null ? 0xffffffff : Buffer.isBuffer(s) ? s.length : s.size,
        pages: []}));
    const directoryBytes = 4 + entries.length * 4 + entries.reduce((n, e) => n +
        (e.size === 0xffffffff ? 0 : Math.ceil(e.size / bs) * 4), 0);
    const dirPages = Math.ceil(directoryBytes / bs), directory = Buffer.alloc(directoryBytes);
    const superblock = Buffer.alloc(bs), blockmap = Buffer.alloc(bs), physical = new Map();
    Buffer.from('Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0', 'binary').copy(superblock);
    directory.writeUInt32LE(entries.length); let page = 4 + dirPages, cursor = 4 + entries.length * 4;
    entries.forEach((e, index) => {
        directory.writeUInt32LE(e.size, 4 + index * 4);
        if (e.size === 0xffffffff) return;
        for (let off = 0; off < e.size; off += bs) {
            while (page % bs === 1 || page % bs === 2) page++;
            e.pages.push(page); directory.writeUInt32LE(page, cursor); cursor += 4;
            physical.set(page++, {index, off});
        }
    });
    for (let i = 0; i < dirPages; i++) blockmap.writeUInt32LE(4 + i, i * 4);
    for (const [at, value] of [[32, bs], [36, 1], [40, page], [44, directoryBytes], [52, 3]]) superblock.writeUInt32LE(value, at);
    const reads = [];
    const reader = {size: page * bs, read(offset, count) {
        reads.push({offset, count});
        const out = Buffer.alloc(count); let done = 0;
        while (done < count) {
            const pos = offset + done, p = Math.floor(pos / bs), within = pos % bs, take = Math.min(count - done, bs - within);
            let source;
            if (p === 0) source = superblock.subarray(within, within + take);
            else if (p === 3) source = blockmap.subarray(within, within + take);
            else if (p >= 4 && p < 4 + dirPages) source = directory.subarray((p - 4) * bs + within, (p - 4) * bs + within + take);
            else if (physical.has(p)) {
                const location = physical.get(p), stream = f.streams[location.index], at = location.off + within;
                const amount = Math.max(0, Math.min(take, entries[location.index].size - at));
                source = Buffer.isBuffer(stream) ? stream.subarray(at, at + amount) : stream.read(at, amount);
            } else source = Buffer.alloc(take);
            source.copy(out, done); done += take;
        }
        return out;
    }};
    return {...f, reader, entries, directory, superblock, blockmap, reads};
}

// Repeated framed records with one leading supported procedure; trailing bytes
// are supplied lazily. totalSymbols includes signature. No code bytes involved.
function repeatedModule(totalSymbols, repeated = record(), signature = 4, first = proc({name: ''})) {
    const trailer = Buffer.alloc(4), prefix = Buffer.alloc(4);
    prefix.writeUInt32LE(signature);
    return {size: totalSymbols + 4, read(offset, count) {
        const out = Buffer.alloc(count);
        for (let i = 0; i < count; i++) {
            const p = offset + i;
            out[i] = p < 4 ? prefix[p] : p < 4 + first.length ? first[p - 4] :
                p < totalSymbols ? repeated[(p - 4 - first.length) % repeated.length] : trailer[p - totalSymbols];
        }
        return out;
    }};
}

// Replace only the contribution table with a lazy repeated table. Every table
// record is still consumed by the production reader in automatic mode.
function repeatedContributions(f, count, {width = 28, declaredBytes = 4 + count * width} = {}) {
    const old = f.streams[3], header = Buffer.from(old.subarray(0, 64));
    header.writeInt32LE(declaredBytes, 28);
    const tableHead = Buffer.alloc(4); tableHead.writeUInt32LE(width === 32 ? SC2 : SC60);
    const entry = contribution({width}), suffix = Buffer.concat([f.sectionMap, old.subarray(f.optionalAt)]), start = 64 + f.moduleInfo.length;
    f.streams[3] = {size: start + declaredBytes + suffix.length, read(offset, bytes) {
        const b = Buffer.alloc(bytes);
        for (let i = 0; i < bytes; i++) {
            const p = offset + i;
            b[i] = p < 64 ? header[p] : p < start ? f.moduleInfo[p - 64] : p < start + 4 ? tableHead[p - start] :
                p < start + declaredBytes ? entry[(p - start - 4) % width] : suffix[p - start - declaredBytes];
        }
        return b;
    }};
    return f;
}

module.exports = {GUID, SC60, SC2, VA, pe, proc, record, info, contribution, segmentMap, fixture, pack, repeatedModule, repeatedContributions};
