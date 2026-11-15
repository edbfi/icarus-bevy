'use strict';
// Authored in-memory envelopes only. No target constants, files or snapshots.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const {createHash} = require('node:crypto');
const api = require('../pdb_selected_types.cjs');
const {inspectSelectedRawTypeRecords: inspect, RAW_TYPE_RECORD_PROFILE: profile,
    RAW_TYPE_RECORD_CAPS: caps} = api;
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const options = (limits, type_indices = [0x1000]) => ({profile, type_indices, ...(limits ? {limits} : {})});
function record(leaf = 0x7777, size = 12) {
    const b = Buffer.alloc(size, 0xa5);
    b.writeUInt16LE(size - 2); b.writeUInt16LE(leaf, 2);
    return b;
}
function fixture(records = [record()]) {
    const header = Buffer.alloc(56), body = Buffer.concat(records);
    for (const [at, value] of [[0, 20040203], [4, 56], [8, 0x1000],
        [12, 0x1000 + records.length], [16, body.length], [24, 4], [28, 4096]]) header.writeUInt32LE(value, at);
    header.writeUInt16LE(0xffff, 20); header.writeUInt16LE(0xffff, 22);
    const bytes = Buffer.concat([header, body]), reads = [], offsets = [];
    let at = 56;
    for (const row of records) { offsets.push(at); at += row.length; }
    const reader = {size: bytes.length, read(offset, length) {
        reads.push([offset, length]); return bytes.subarray(offset, offset + length);
    }};
    return {bytes, reader, reads, offsets, records};
}
function refuses(fn, phase) {
    assert.throws(fn, error => {
        assert.equal(error.constructor, Error);
        assert.equal(error.message, 'invalid or unsupported raw TPI records');
        assert.match(error.code, /^TPI_RAW_/);
        if (phase) assert.equal(error.code, 'TPI_RAW_' + phase);
        assert.deepEqual(Object.keys(error), ['code']);
        assert.equal(error.selected, undefined); assert.equal(error.cause, undefined);
        return true;
    });
}
function noReads(opt, transform = x => x.reader) {
    const x = fixture(); refuses(() => inspect(transform(x), opt), 'OPTIONS');
    assert.deepEqual(x.reads, []);
}

test('frozen 79744-byte prefix and all existing exports/profiles survive', () => {
    const source = fs.readFileSync(require.resolve('../pdb_selected_types.cjs'));
    assert.equal(sha(source.subarray(0, 79744)), '7e6aa46d7e8af7720c93db97c74dc5af3eea7fcce69071d76beae51d7f417272');
    assert.equal(profile, 'tpi_v80_exact_raw_type_records_v1');
    assert.ok(Object.isFrozen(caps));
    assert.deepEqual(caps, {stream_bytes: 512 * 1024 * 1024, records: 6000000, selectors: 1,
        record_bytes: 65536, selected_record_bytes: 65536, read_bytes: 65536,
        logical_read_bytes: 32 * 1024 * 1024, read_calls: 6000002,
        retained_bytes: 1024 * 1024, transient_bytes: 131136, hex_bytes: 262144,
        serialization_bytes: 524288, output_bytes: 262144});
    for (const [fn, oldProfile] of [[api.inspectSelectedUdtHeaders, api.UDT_HEADER_PROFILE],
        [api.inspectSelectedMemberFunctions, api.MFUNCTION_PROFILE]]) {
        for (const leaf of [0x7777, 0x1507, 0x1001]) {
            const x = fixture([record(leaf)]);
            assert.throws(() => fn(x.reader, {profile: oldProfile, type_indices: [0x1000]}),
                error => error.code === 'TPI_METADATA_SELECTED_RECORD');
            assert.deepEqual(x.reads, [[0, 56], [56, 4], [56, 12]]);
        }
    }
    const x = fixture();
    assert.throws(() => api.inspectSelectedTypes(x.reader, options()));
    assert.deepEqual(x.reads, []);
});

for (const leaf of [0, 0x1001, 0x1002, 0x1009, 0x1203, 0x1504, 0x1507, 0x7777, 0xffff]) {
    test(`u16 leaf ${leaf}: exact envelope, hash and logical offsets; no semantic following`, () => {
        const records = [record(0xffff, 20), record(leaf, 16), record(0x1504, 24)];
        // Deliberately invalid payload, including apparent out-of-domain words
        // and meaningless padding. The API must leave every payload undecoded.
        records[1].writeUInt32LE(0xffffffff, 4);
        const x = fixture(records), result = inspect(x.reader, options(undefined, [0x1001]));
        assert.deepEqual(x.reads, [[0, 56], [56, 4], [76, 4], [76, 16], [92, 4]]);
        assert.deepEqual(result.selected, [{type_index: 0x1001, record_offset: 76, record_bytes: 16,
            leaf, record_hex: records[1].toString('hex'), record_sha256: sha(records[1])}]);
        assert.equal(result.header_hex, x.bytes.subarray(0, 56).toString('hex'));
        assert.equal(result.header_sha256, sha(x.bytes.subarray(0, 56)));
        assert.equal(result.records_framed, 3); assert.equal(result.record_bytes, 60);
        assert.equal(result.logical_read_bytes, 84); assert.equal(result.read_calls, 5);
        assert.equal(result.prefix_reads, 3); assert.equal(result.selected_body_reads, 1);
        assert.equal(result.unselected_body_reads, 0); assert.equal(result.complete_outer_record_framing, true);
        for (const key of ['selected_payload_validated', 'unselected_payloads_validated',
            'hash_tables_validated', 'references_resolved', 'storage_layout_complete',
            'occupied_storage_qualified', 'native_owner_qualified', 'native_receiver_qualified',
            'original_layout_qualified', 'runtime_behavior_proven', 'compatibility_qualified',
            'save_import_qualified', 'wire_qualified', 'caller_provenance_verified', 'release_qualified']) {
            assert.equal(result[key], false, key);
        }
        assert.ok(Buffer.byteLength(JSON.stringify(result)) <= result.output_bytes_upper_bound);
        assert.ok(!JSON.stringify(result.selected).includes('reference'));
    });
}

test('first/middle/last/minimal selection; body reads only for exact selected ordinal', () => {
    const records = [record(0x7777, 4), record(0, 8), record(0xffff, 12)];
    for (let i = 0; i < records.length; i++) {
        const x = fixture(records), r = inspect(x.reader, options(undefined, [0x1000 + i]));
        const expected = [[0, 56]];
        records.forEach((rec, ordinal) => {
            expected.push([x.offsets[ordinal], 4]);
            if (i === ordinal) expected.push([x.offsets[ordinal], rec.length]);
        });
        assert.deepEqual(x.reads, expected);
        assert.equal(r.selected[0].record_sha256, sha(records[i]));
    }
    const x = fixture([record(0xffff, 4)]);
    assert.equal(inspect(x.reader, options()).selected[0].record_bytes, 4);
});

test('selector and unknown-option refusals precede callbacks', () => {
    for (const bad of [undefined, null, [], {}, {profile}, {type_indices: [0x1000]},
        {profile: api.UDT_HEADER_PROFILE, type_indices: [0x1000]},
        {...options(), names: ['Authored']}, {...options(), domain: 'IPI'},
        {...options(), type_indices: []}, {...options(), type_indices: [0x1000, 0x1000]},
        {...options(), type_indices: [0x1000, 0x1001]},
        {...options(), type_indices: new Array(1)}, {...options(), type_indices: '0x1000'},
        {...options(), limits: undefined}, {...options(), limits: {unknown: 0}},
        {...options(), [Symbol('extra')]: 1}]) noReads(bad);
    for (const index of [0, 0x75, 0xfff, 0x80000000, 0xffffffff, -1, 1.5, NaN, Infinity,
        '0x1000', 'Authored', {domain: 'TPI', index: 0x1000}, 0x1000n, undefined]) {
        noReads(options(undefined, [index]));
    }
    const extra = [0x1000]; extra.foo = 1; noReads({...options(), type_indices: extra});
    for (const [key, cap] of Object.entries(caps)) {
        for (const value of [-1, 0.5, NaN, Infinity, cap + 1, '1']) noReads(options({[key]: value}));
    }
});

test('accessors/proxies/custom structures rejected without executing traps/getters/callbacks', () => {
    let effects = 0;
    const getter = () => { effects++; throw Error('must not execute'); };
    const proxy = value => new Proxy(value, {ownKeys: getter, get: getter, getPrototypeOf: getter,
        getOwnPropertyDescriptor: getter, apply: getter});
    noReads(proxy(options())); noReads({...options(), limits: proxy({})});
    noReads({...options(), type_indices: proxy([0x1000])});
    noReads(options(), x => proxy(x.reader));
    noReads(options(), x => ({size: x.bytes.length, read: proxy(x.reader.read)}));
    for (const key of ['profile', 'type_indices', 'limits']) {
        const opt = options(); Object.defineProperty(opt, key, {enumerable: true, get: getter}); noReads(opt);
    }
    const indices = [0x1000]; Object.defineProperty(indices, '0', {enumerable: true, get: getter});
    noReads({...options(), type_indices: indices});
    const limits = {}; Object.defineProperty(limits, 'records', {enumerable: true, get: getter});
    noReads({...options(), limits});
    for (const key of ['size', 'read']) noReads(options(), x => {
        Object.defineProperty(x.reader, key, {enumerable: true, get: getter}); return x.reader;
    });
    noReads(options(), x => Object.create(x.reader));
    noReads(options(), x => ({...x.reader, extra: 0}));
    noReads(Object.assign(Object.create({}), options()));
    const hidden = options(); Object.defineProperty(hidden, 'profile', {enumerable: false}); noReads(hidden);
    const customArray = [0x1000]; Object.setPrototypeOf(customArray, null);
    noReads({...options(), type_indices: customArray});
    assert.equal(effects, 0);
});

test('reader and option data descriptors snapshotted before first callback', () => {
    const x = fixture([record(), record(0xffff, 8)]), opt = options({records: 2}, [0x1001]);
    const originalRead = x.reader.read;
    let calls = 0;
    x.reader.read = function(offset, length) {
        assert.equal(this, x.reader); calls++;
        opt.type_indices[0] = 0x75; opt.limits.records = 0; opt.profile = 'bad';
        x.reader.size = 0; x.reader.read = () => { throw Error('replacement must not run'); };
        return originalRead(offset, length);
    };
    const result = inspect(x.reader, opt);
    assert.equal(result.selected[0].type_index, 0x1001); assert.equal(calls, 4);
    assert.deepEqual(x.reads, [[0, 56], [56, 4], [68, 4], [68, 8]]);
    const nullOptions = Object.assign(Object.create(null), options());
    const y = fixture(); Object.setPrototypeOf(y.reader, null);
    assert.equal(inspect(y.reader, nullOptions).selected.length, 1);
});

test('reader size/read validation and all impossible minimum caps are pre-callback', () => {
    for (const size of [0, 55, 56, 59, -1, 60.5, NaN, Infinity, caps.stream_bytes + 1])
        noReads(options(), x => ({size, read: x.reader.read}));
    for (const read of [undefined, null, 1, {}]) noReads(options(), x => ({size: x.bytes.length, read}));
    for (const key of Object.keys(caps)) noReads(options({[key]: 0}));
    noReads(options({read_bytes: 55})); noReads(options({read_calls: 2}));
    noReads(options({logical_read_bytes: 63})); noReads(options({record_bytes: 3}));
    noReads(options({selected_record_bytes: 3})); noReads(options({transient_bytes: 131135}));
    noReads(options({retained_bytes: 8211})); noReads(options({hex_bytes: 15}));
    noReads(options({output_bytes: 8199})); noReads(options({serialization_bytes: 16399}));
});

const headerMutations = [
    [0, 0], [4, 52], [8, 0x1001], [12, 0xfff], [12, 0x80000000],
    [12, 0x1004], [16, 0], [16, 11], [16, 16], [24, 0], [28, 4095], [28, 0x40001],
    [32, 0xffffffff], [36, 1], [36, 4], [40, 0xffffffff], [44, 4], [44, 8],
    [48, 0xffffffff], [52, 1],
];
for (const [at, value] of headerMutations) {
    test(`unsupported/malformed header u32 ${at}=${value} refuses after header only`, () => {
        const x = fixture(); x.bytes.writeUInt32LE(value, at);
        refuses(() => inspect(x.reader, options()), 'HEADER'); assert.deepEqual(x.reads, [[0, 56]]);
    });
}

test('header hash profile, counts and selector range; no hash-table reads', () => {
    const x = fixture(); x.bytes.writeUInt16LE(0, 22);
    refuses(() => inspect(x.reader, options()), 'HEADER');
    for (const buckets of [0x1000, 0x40000]) {
        const y = fixture(); y.bytes.writeUInt32LE(buckets, 28);
        y.bytes.writeUInt16LE(1, 20); y.bytes.writeUInt32LE(4, 36); y.bytes.writeUInt32LE(8, 44);
        assert.equal(inspect(y.reader, options()).hash_tables_validated, false);
        assert.deepEqual(y.reads, [[0, 56], [56, 4], [56, 12]]);
    }
    const y = fixture([record(), record()]);
    refuses(() => inspect(y.reader, options({records: 1})), 'HEADER');
    assert.deepEqual(y.reads, [[0, 56]]);
    const empty = fixture(); empty.bytes.writeUInt32LE(0x1000, 12);
    refuses(() => inspect(empty.reader, options()), 'SELECTOR_RANGE');
    assert.deepEqual(empty.reads, [[0, 56]]);
    for (const index of [0x1001, 0x7fffffff]) {
        const z = fixture(); refuses(() => inspect(z.reader, options(undefined, [index])), 'SELECTOR_RANGE');
        assert.deepEqual(z.reads, [[0, 56]]);
    }
});

test('complete framing: short/misaligned/overrun prefixes, count mismatch and later errors are atomic', () => {
    for (const ordinal of [0, 1, 2]) for (const encodedLength of [0, 1, 3, 7, 0xffff]) {
        const x = fixture([record(), record(), record()]);
        x.bytes.writeUInt16LE(encodedLength, x.offsets[ordinal]);
        refuses(() => inspect(x.reader, options()), 'FRAMING');
        assert.ok(x.reads.some(([at, length]) => at === x.offsets[ordinal] && length === 4));
        assert.ok(!x.reads.some(([at, length]) => at === x.offsets[ordinal] && length !== 4));
    }
    const x = fixture([record(), record()]); x.bytes.writeUInt32LE(0x1001, 12);
    refuses(() => inspect(x.reader, options()), 'FRAMING');
    assert.deepEqual(x.reads, [[0, 56], [56, 4], [56, 12]]);
    const y = fixture([record()]); y.bytes.writeUInt32LE(0x1002, 12);
    refuses(() => inspect(y.reader, options()), 'FRAMING');
    assert.deepEqual(y.reads, [[0, 56], [56, 4], [56, 12]]);
});

test('prefix/body consistency and thrown callbacks refuse atomically', () => {
    for (const changeAt of [0, 2]) {
        const x = fixture(); const original = x.reader.read;
        x.reader.read = (at, length) => {
            const raw = original(at, length);
            if (at === 56 && length === 12) { const b = Buffer.from(raw); b[changeAt] ^= 1; return b; }
            return raw;
        };
        refuses(() => inspect(x.reader, options()), 'SELECTED_RECORD');
    }
    for (const call of [1, 2, 3, 4]) {
        const x = fixture([record(), record()]), original = x.reader.read; let seen = 0;
        x.reader.read = (at, length) => {
            if (++seen === call) throw Error('private callback message');
            return original(at, length);
        };
        refuses(() => inspect(x.reader, options())); assert.equal(seen, call);
    }
});

test('borrowed reused buffers copied before next callback; shadow byte getters never run', () => {
    const x = fixture([record(), record(0xffff)]), borrowed = Buffer.alloc(65536);
    const original = x.reader.read; let effects = 0;
    x.reader.read = (at, length) => {
        borrowed.fill(0xff); original(at, length).copy(borrowed);
        const view = borrowed.subarray(0, length);
        for (const key of ['length', 'byteLength', 'buffer']) Object.defineProperty(view, key, {
            get() { effects++; throw Error('shadow getter'); },
        });
        return view;
    };
    const result = inspect(x.reader, options()); borrowed.fill(0);
    assert.equal(result.header_sha256, sha(x.bytes.subarray(0, 56)));
    assert.equal(result.selected[0].record_hex, x.records[0].toString('hex'));
    assert.equal(result.selected[0].record_sha256, sha(x.records[0])); assert.equal(effects, 0);
});

test('short/long/nonBuffer/shared/proxy buffers rejected at every read phase', () => {
    let effects = 0;
    const badOutputs = [length => Buffer.alloc(length - 1), length => Buffer.alloc(length + 1),
        length => new Uint8Array(length), () => null, () => Promise.resolve(Buffer.alloc(56)),
        length => Buffer.from(new SharedArrayBuffer(length)),
        length => new Proxy(Buffer.alloc(length), {get() { effects++; throw Error('proxy trap'); }})];
    for (const bad of badOutputs) for (const failCall of [1, 2, 3, 4]) {
        const x = fixture([record(), record()]), original = x.reader.read; let calls = 0;
        x.reader.read = (at, length) => ++calls === failCall ? bad(length) : original(at, length);
        refuses(() => inspect(x.reader, options())); assert.equal(calls, failCall);
    }
    assert.equal(effects, 0);
});

test('count-dependent read budgets refused before prefixes; exact budget accepted', () => {
    for (const limits of [{read_calls: 3}, {logical_read_bytes: 67}]) {
        const x = fixture([record(), record()]);
        refuses(() => inspect(x.reader, options(limits)), 'FRAMING'); assert.deepEqual(x.reads, [[0, 56]]);
    }
    const x = fixture([record(), record()]);
    const r = inspect(x.reader, options({read_calls: 4, logical_read_bytes: 76}));
    assert.equal(r.read_calls, 4); assert.equal(r.logical_read_bytes, 76);
});

test('all selected-size/read/retained/hex/serialization/output caps checked before full body', () => {
    const length = 64;
    const limits = [{selected_record_bytes: length - 1}, {read_bytes: length - 1},
        {logical_read_bytes: 56 + 4 + length - 1}, {retained_bytes: 8192 + 5 * length - 1},
        {hex_bytes: 4 * length - 1}, {serialization_bytes: 2 * (8192 + 2 * length) - 1},
        {output_bytes: 8192 + 2 * length - 1}];
    for (const limit of limits) {
        const x = fixture([record(0xffff, length)]);
        refuses(() => inspect(x.reader, options(limit)), 'SELECTED_RECORD');
        assert.deepEqual(x.reads, [[0, 56], [56, 4]]);
    }
    const x = fixture([record(0xffff, length)]);
    refuses(() => inspect(x.reader, options({record_bytes: length - 1})), 'FRAMING');
    assert.deepEqual(x.reads, [[0, 56], [56, 4]]);
    const y = fixture([record(0, 4), record(0xffff, 64)]);
    refuses(() => inspect(y.reader, options({record_bytes: 63})), 'FRAMING');
    assert.deepEqual(y.reads, [[0, 56], [56, 4], [56, 4], [60, 4]]);
});

test('one maximum 65536-byte frame fits default output; exact lower resource bounds work', () => {
    const length = 65536, x = fixture([record(0xffff, length)]);
    const limits = {stream_bytes: x.bytes.length, records: 1, record_bytes: length,
        selected_record_bytes: length, read_bytes: length, selectors: 1,
        read_calls: 3, logical_read_bytes: 56 + 4 + length,
        retained_bytes: 8192 + 5 * length, transient_bytes: 2 * length + 64,
        hex_bytes: 4 * length, serialization_bytes: 2 * (8192 + 2 * length),
        output_bytes: 8192 + 2 * length};
    for (const config of [undefined, limits]) {
        x.reads.length = 0;
        const r = inspect(x.reader, options(config));
        assert.equal(r.selected[0].record_bytes, length);
        assert.equal(r.selected[0].record_sha256, sha(x.records[0]));
        assert.equal(r.selected[0].record_hex, x.records[0].toString('hex'));
        assert.deepEqual(x.reads, [[0, 56], [56, 4], [56, length]]);
        assert.ok(Buffer.byteLength(JSON.stringify(r)) > 65536);
        assert.ok(Buffer.byteLength(JSON.stringify(r)) <= caps.output_bytes);
        assert.equal(r.charged_retained_bytes, limits.retained_bytes);
        assert.equal(r.charged_transient_bytes, limits.transient_bytes);
        assert.equal(r.charged_hex_bytes, limits.hex_bytes);
        assert.equal(r.charged_serialization_bytes, limits.serialization_bytes);
    }
});

test('default six-million-record ceiling refuses from synthetic header without prefix reads', () => {
    const count = caps.records + 1, x = fixture();
    const header = Buffer.from(x.bytes.subarray(0, 56));
    header.writeUInt32LE(0x1000 + count, 12); header.writeUInt32LE(count * 4, 16);
    const reads = [];
    const reader = {size: 56 + count * 4, read(at, length) {
        reads.push([at, length]); assert.equal(at, 0); assert.equal(length, 56); return header;
    }};
    refuses(() => inspect(reader, options()), 'HEADER'); assert.deepEqual(reads, [[0, 56]]);
});

test('existing selected-types hook runs and selects the new fixture and previous inputs', () => {
    const config = fs.readFileSync(require.resolve('../../.pre-commit-config.yaml'), 'utf8');
    const block = config.split('      - id: pdb-selected-types-tests\n')[1].split('      - id:')[0];
    const entry = block.split('\n').find(line => line.trim().startsWith('entry: ')).trim().slice(7);
    const pattern = block.split('\n').find(line => line.trim().startsWith('files: ')).trim().slice(7);
    assert.ok(entry.split(' ').includes('tools/tests/pdb_raw_type_records.test.cjs'));
    const matches = new RegExp(pattern);
    for (const input of ['tools/pdb_selected_types.cjs', 'tools/pdb_selected_metadata.cjs',
        'tools/pdb_public_symbols.cjs', 'tools/tests/pdb_selected_types.test.cjs',
        'tools/tests/pdb_selected_metadata.test.cjs', 'tools/tests/pdb_declaration_entries.test.cjs',
        'tools/tests/pdb_raw_type_records.test.cjs', 'tools/tests/pdb_public_symbols_fixtures.cjs',
        'tools/tests/pe_candidates_fixtures.cjs']) assert.ok(matches.test(input), input);
    assert.equal(matches.test('tools/tests/unrelated.test.cjs'), false);
});
