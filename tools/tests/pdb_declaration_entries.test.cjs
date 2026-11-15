'use strict';
// Small independently authored TPI records only. No original inputs or target
// field-list assumptions. All runtime fixtures are in memory.
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const {createHash} = require('node:crypto');
const api = require('../pdb_selected_types.cjs');
const PROFILE = api.DECLARATION_ENTRY_PROFILE;
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const u16 = n => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const i32 = n => { const b = Buffer.alloc(4); b.writeInt32LE(n); return b; };
const z = s => Buffer.from(s + '\0', 'ascii');
function padded(bytes) {
    const n = (4 - bytes.length % 4) % 4;
    return Buffer.concat([bytes, Buffer.from(Array.from({length: n}, (_, i) => 0xf0 + n - i))]);
}
function record(leaf, ...parts) {
    const bytes = padded(Buffer.concat([Buffer.alloc(2), u16(leaf), ...parts]));
    bytes.writeUInt16LE(bytes.length - 2); return bytes;
}
const field = (leaf, ...parts) => padded(Buffer.concat([u16(leaf), ...parts]));
const member = (type = 0x74, offset = 0, name = 'x', attributes = 3) =>
    field(0x150d, u16(attributes), u32(type), u16(offset), z(name));
const method = (kind, offset = 0, attributes = 3 | kind << 2) =>
    field(0x1511, u16(attributes), u32(0x1001),
        ...([4, 6].includes(kind) ? [i32(offset)] : []), z('call'));
const group = count => field(0x150f, u16(count), u32(0x1001), z('overloaded'));
const continuation = index => field(0x1404, u16(0), u32(index));
const list = (...entries) => record(0x1203, ...entries);
const opaque = () => record(0x7777, u32(0xdeadbeef));
function udt({count = 1, flags = 566, fields = 0x1000, derived = 0, vshape = 0x1001,
    size = 968, sizeEncoding, name = 'Authored::Small', unique = '.?AVSmall@Authored@@', leaf = 0x1504} = {}) {
    return record(leaf, u16(count), u16(flags), u32(fields), u32(derived), u32(vshape),
        sizeEncoding ?? u16(size), z(name), ...(flags & 0x200 ? [z(unique)] : []));
}
function fixture(records = [list(member()), opaque(), udt()], selectedIndices = [0x1002]) {
    const body = Buffer.concat(records), header = Buffer.alloc(56);
    for (const [at, value] of [[0, 20040203], [4, 56], [8, 0x1000], [12, 0x1000 + records.length],
        [16, body.length], [24, 4], [28, 4096]]) header.writeUInt32LE(value, at);
    header.writeUInt16LE(0xffff, 20); header.writeUInt16LE(0xffff, 22);
    const bytes = Buffer.concat([header, body]), reads = [], offsets = [];
    let at = 56;
    for (const rec of records) { offsets.push(at); at += rec.length; }
    const owners = selectedIndices.map(type_index => {
        const raw = records[type_index - 0x1000];
        return {type_index, record_sha256: sha(raw), record_hex: raw.toString('hex'),
            field_list_type_index: raw.readUInt32LE(8)};
    });
    const reader = {size: bytes.length, read(offset, length) {
        reads.push({offset, length}); return bytes.subarray(offset, offset + length);
    }};
    return {bytes, reads, reader, records, offsets, options: {profile: PROFILE, owners}};
}
function inspect(x = fixture(), limits) {
    return api.inspectSelectedDeclarationEntries(x.reader, limits ? {...x.options, limits} : x.options);
}
function refuses(fn) {
    assert.throws(fn, error => {
        assert.equal(error.constructor, Error);
        assert.equal(error.message, 'invalid or unsupported TPI declaration entries');
        assert.deepEqual(Object.keys(error), []);
        assert.equal(error.cause, undefined);
        assert.equal(error.selected, undefined);
        return true;
    });
}
const refuse = (x, limits) => refuses(() => inspect(x, limits));

function chained() {
    return fixture([list(member(), continuation(0x1003)), opaque(), udt({count: 4}), list(group(3))]);
}
function twoOwners(shared = false) {
    return fixture([list(member()), opaque(), udt(), list(member(0x75, 0, 'y')),
        udt({fields: shared ? 0x1000 : 0x1003, name: 'Authored::Other', unique: '.?AUOther@@'})], [0x1004, 0x1002]);
}

test('new numeric/hash/raw binding admits known flags and opaque nonzero vshape; legacy source and refusal survive', () => {
    const source = fs.readFileSync(require.resolve('../pdb_selected_types.cjs'));
    assert.equal(sha(source.subarray(0, 43976)), 'c565fb4c0a9346e3887d38521dc144cf5ef94b627ad87ae7c6153f53e8d13714');
    const x = fixture(), result = inspect(x), owner = result.selected[0];
    assert.equal(result.profile, 'tpi_v80_exact_udt_declaration_entries_v1');
    assert.equal(owner.flags, 566); assert.equal(owner.declared_size_bytes, 968);
    assert.equal(owner.vshape_type_index, 0x1001);
    assert.equal(owner.record_sha256, x.options.owners[0].record_sha256);
    assert.equal(owner.record_hex, x.options.owners[0].record_hex);
    assert.equal(owner.record_offset, x.offsets[2]);
    assert.equal(result.selected_records.length, 2);
    for (const row of result.selected_records) {
        assert.equal(row.record_sha256, sha(Buffer.from(row.record_hex, 'hex')));
        assert.equal(row.record_bytes, row.record_hex.length / 2);
    }
    assert.throws(() => api.inspectSelectedTypes(x.reader, {profile: api.DECLARATION_PROFILE,
        names: ['Authored::Small']}), /^Error: invalid or unsupported selected TPI types$/);
    assert.equal(api.inspectSelectedTypes(fixture([list(member()), opaque(), udt({vshape: 0})]).reader,
        {profile: api.DECLARATION_PROFILE, names: ['Authored::Small']}).selected.length, 1);
    for (const key of ['references_resolved', 'method_lists_followed', 'signatures_complete',
        'storage_layout_complete', 'occupied_storage_qualified', 'native_owner_qualified',
        'native_receiver_qualified', 'compatibility_qualified', 'save_import_qualified', 'wire_qualified']) {
        assert.equal(result[key], false);
    }
});

test('member/base/static/nested/virtual-base/table-marker metadata has exact entry framing and opaque refs', () => {
    const entries = [member(0x1001, 600, 'overlap'), member(0x74, 600, 'overlap'),
        field(0x1400, u16(1), u32(0x1001), u16(0)),
        field(0x150e, u16(2), u32(0x675), z('static')),
        field(0x1510, u16(0), u32(0x1001), z('Nested')),
        field(0x1401, u16(3), u32(0x1001), u32(0x603), u16(12), u16(2)),
        field(0x1402, u16(2), u32(0x1001), u32(0x1001), u16(16), u16(3)),
        field(0x1409, u16(0), u32(0x1001))];
    const x = fixture([list(...entries), opaque(), udt({count: entries.length, derived: 0x1001, size: 1})]);
    const r = inspect(x), declarations = r.selected[0].declarations;
    assert.deepEqual(declarations.map(d => d.entry_kind), ['member', 'member', 'base', 'static_member',
        'nested_type', 'virtual_base', 'indirect_virtual_base', 'vfptr_marker']);
    assert.equal(declarations[0].declared_offset_bytes, 600);
    assert.equal(declarations[5].declared_vbptr_offset_bytes, 12);
    assert.equal(declarations[5].declared_vbtable_index, 2);
    let entryAt = 4;
    for (let i = 0; i < entries.length; i++) {
        const d = declarations[i];
        assert.equal(d.entry_offset, entryAt);
        assert.equal(d.stream_offset, x.offsets[0] + entryAt);
        assert.equal(d.framed_bytes, entries[i].length);
        assert.equal(d.content_bytes + d.padding_bytes, d.framed_bytes);
        assert.equal(d.entry_hex, entries[i].toString('hex'));
        assert.equal(d.entry_sha256, sha(entries[i])); entryAt += entries[i].length;
        assert.equal(Object.hasOwn(d, 'width_bytes'), false);
    }
    assert.equal(r.direct_references.length, 12);
    for (const ref of r.direct_references) {
        assert.equal(ref.followed, false); assert.equal(ref.interpretation, 'opaque');
        assert.equal(ref.domain, ref.type_index >= 0x1000 ? 'tpi' : 'simple');
    }
});

for (const kind of [0, 1, 2, 4, 5, 6]) {
    for (const offset of ([4, 6].includes(kind) ? [-2147483648, -1, 0, 2147483647] : [null])) {
        test(`ONEMETHOD kind ${kind} offset ${offset}: optional offset only for introducing virtual`, () => {
            const r = inspect(fixture([list(method(kind, offset)), opaque(), udt()]));
            const d = r.selected[0].declarations[0];
            assert.equal(d.method_kind, kind); assert.equal(d.declared_vftable_offset, offset);
            assert.equal(d.signature_followed, false);
        });
    }
}
for (const option of [0x20, 0x40, 0x80, 0x100, 0x200, 0x3e0]) {
    test(`named method options 0x${option.toString(16)} retain attributes without following a signature`, () => {
        const attributes = option | 3 | 4 << 2;
        const r = inspect(fixture([list(method(4, 4, attributes)), opaque(), udt()]));
        assert.equal(r.selected[0].declarations[0].attributes, attributes);
    });
}

test('METHOD declared overload weight and continuation counts do not claim METHODLIST closure', () => {
    const x = chained(), r = inspect(x), owner = r.selected[0];
    assert.equal(owner.physical_declaration_count, 2); assert.equal(owner.weighted_declaration_count, 4);
    assert.equal(r.physical_declaration_count, 2); assert.equal(r.weighted_declaration_count, 4);
    assert.equal(r.continuation_count, 1);
    assert.deepEqual(owner.field_list_type_indices, [0x1000, 0x1003]);
    assert.equal(owner.continuations[0].target_type_index, 0x1003);
    assert.equal(owner.continuations[0].followed, true);
    assert.equal(owner.declarations[1].method_list_followed, false);
    assert.equal(owner.declarations[1].overload_signatures_complete, false);
    assert.equal(r.selected_records.length, 3);
    assert.equal(r.selected_records[2].record_offset, x.offsets[3]);
    assert.equal(owner.continuations[0].entry_sha256, sha(continuation(0x1003)));
    refuse(fixture([list(group(0)), opaque(), udt({count: 0})]));
    for (const count of [1, 3, 5]) refuse(fixture([list(group(4)), opaque(), udt({count})]));
});

test('callback trace reads only all outer prefixes, selected header and followed lists, never opaque payloads', () => {
    const x = chained(), r = inspect(x);
    const expected = [{offset: 0, length: 56}, ...x.offsets.map(offset => ({offset, length: 4})),
        ...[2, 0, 3].map(i => ({offset: x.offsets[i], length: x.records[i].length}))];
    assert.deepEqual(x.reads, expected);
    assert.equal(r.read_calls, expected.length);
    assert.equal(r.logical_read_bytes, expected.reduce((n, v) => n + v.length, 0));
    assert.equal(x.reads.filter(v => v.offset === x.offsets[1]).length, 1);
    const one = fixture([list(method(4, 10)), record(0x1009, u32(0xffffffff)), udt({derived: 0x1001})]);
    inspect(one);
    assert.deepEqual(one.reads.filter(v => v.offset === one.offsets[1]), [{offset: one.offsets[1], length: 4}]);
});

test('exact owner index/hash/raw/fieldlist binding, missing/forward/duplicate owner refusal', () => {
    for (const mutate of [x => x.options.owners[0].type_index++, x => x.options.owners[0].type_index--,
        x => x.options.owners[0].record_sha256 = '0'.repeat(64),
        x => x.options.owners[0].record_hex = x.options.owners[0].record_hex.replace(/^../, '00'),
        x => x.options.owners[0].field_list_type_index = 0x1001,
        x => x.options.owners.push({...x.options.owners[0]})]) {
        const x = fixture(); mutate(x); refuse(x);
    }
    refuse(fixture([list(), opaque(), udt({count: 0, flags: 0x280, size: 0, fields: 0, vshape: 0})]));
    refuse(fixture([list(member()), opaque(), udt(), udt()], [0x1002, 0x1003]));
    refuse(fixture([list(member()), opaque(), udt(), udt({name: 'Other'})], [0x1002, 0x1003]));
    const r = inspect(twoOwners()); assert.deepEqual(r.selected.map(x => x.type_index), [0x1004, 0x1002]);
    // A malformed unselected semantic tail stays opaque, even on another UDT.
    const unrelated = record(0x1504, u32(0xffffffff));
    assert.equal(inspect(fixture([list(member()), unrelated, udt()])).selected.length, 1);
});

test('complete empty class/struct metadata permits zero size without applying storage or empty-base rules', () => {
    for (const leaf of [0x1504, 0x1505]) {
        const r = inspect(fixture([opaque(), opaque(), udt({leaf, count: 0, fields: 0, size: 0, flags: 0, vshape: 0})]));
        assert.equal(r.physical_declaration_count, 0); assert.equal(r.selected_records.length, 1);
        assert.equal(r.direct_references[0].domain, 'none');
    }
    refuse(fixture([list(), opaque(), udt({fields: 0})]));
    assert.equal(inspect(fixture([list(), opaque(), udt({count: 0})])).selected_records.length, 2);
});

test('terminal continuations refuse cycles, missing/wrong leaves, depth exhaustion and any shared list', () => {
    for (const records of [
        [list(continuation(0x1000)), opaque(), udt({count: 0})],
        [list(continuation(0x1003)), opaque(), udt({count: 0}), list(continuation(0x1000))],
        [list(continuation(0x1001)), opaque(), udt({count: 0})],
        [list(continuation(0x1004)), opaque(), udt({count: 0})],
        [list(continuation(0x1003), member()), opaque(), udt(), list()],
    ]) refuse(fixture(records));
    refuse(chained(), {depth: 0}); assert.equal(inspect(chained(), {depth: 1}).continuation_count, 1);
    refuse(twoOwners(true));
    const sharedTail = fixture([list(continuation(0x1005)), opaque(), udt({count: 0}),
        list(continuation(0x1005)), udt({fields: 0x1003, count: 0, name: 'Other', unique: 'Other'}), list()], [0x1002, 0x1004]);
    refuse(sharedTail);
});

test('unknown/late/truncated entries and unqualified header, leaf, attribute and optional forms refuse atomically', () => {
    const invalid = [field(0x7777, u16(0)), field(0x1512, u16(3), u32(0x1001), z('extended')),
        field(0x140a, u16(0), u32(0x1001), i32(0)), method(3), method(7), method(0, 0, 0),
        method(0, 0, 0x403), member(0x74, 0, 'x', 0), member(0x74, 0, 'x', 0x103),
        field(0x1510, u16(1), u32(0x1001), z('Nested')),
        field(0x1409, u16(1), u32(0x1001)), field(0x1404, u16(1), u32(0x1000)),
        field(0x1511, u16(3 | 4 << 2), u32(0x1001)),
        field(0x1511, u16(3), u32(0x1001), i32(0), z('extra')),
        field(0x1401, u16(3), u32(0x1001), u32(0x603), u16(0)),
        Buffer.concat([u16(0x150d), u16(3)]),
        field(0x150d, u16(3), u32(0x74), u16(0), Buffer.from('unterminated'))];
    for (const entry of invalid) {
        refuse(fixture([list(entry), opaque(), udt()]));
        refuse(fixture([list(member(), entry), opaque(), udt({count: 2})]));
    }
    for (const flags of [0x800, 0x1000, 0x4000, 0x8000]) refuse(fixture([list(member()), opaque(), udt({flags})]));
    for (const leaf of [0x1503, 0x1506, 0x1519]) refuse(fixture([list(member()), opaque(), udt({leaf})]));
});

test('strict descending alignment padding, reserved internal padding and all outer framing validate', () => {
    for (const padding of [1, 2, 3]) {
        // Name byte lengths 4,3,2 yield respectively 1,2,3 pad bytes.
        const entry = member(0x74, 0, 'x'.repeat(5 - padding));
        assert.equal(entry[entry.length - padding], 0xf0 + padding);
        inspect(fixture([list(entry), opaque(), udt()]));
        for (let i = 1; i <= padding; i++) {
            const bad = Buffer.from(entry); bad[bad.length - i] = 0;
            refuse(fixture([list(bad), opaque(), udt()]));
        }
    }
    const extraPad = list(member(), Buffer.from([0xf4, 0xf3, 0xf2, 0xf1]));
    refuse(fixture([extraPad, opaque(), udt()]));
    const malformedHeader = udt(); malformedHeader[malformedHeader.length - 1] = 0;
    refuse(fixture([list(member()), opaque(), malformedHeader]));
    for (const mutate of [x => x.bytes.writeUInt32LE(1, 0), x => x.bytes.writeUInt32LE(52, 4),
        x => x.bytes.writeUInt32LE(0x1001, 8), x => x.bytes.writeUInt32LE(0x80000000, 12),
        x => x.bytes.writeUInt32LE(x.records.length + 0x1001, 12), x => x.bytes.writeUInt32LE(0, 24),
        x => x.bytes.writeUInt32LE(1, 28), x => x.bytes.writeUInt16LE(0, 22),
        x => x.bytes.writeUInt32LE(4, 52), x => x.bytes.writeInt32LE(-1, 32),
        x => x.bytes.writeUInt32LE(3, 36), x => x.bytes.writeUInt32LE(3, 44),
        x => x.bytes.writeUInt16LE(3, x.offsets[1]), x => x.bytes.writeUInt16LE(0xffff, x.offsets[1]),
        x => x.reader.size--, x => { x.bytes = Buffer.concat([x.bytes, Buffer.alloc(4)]); x.reader.size += 4;
            x.reader.read = (o, n) => x.bytes.subarray(o, o + n); }]) {
        const x = fixture(); mutate(x); refuse(x);
    }
});

test('integer numeric leaves retain only safe nonnegative declared values, with no size/offset relation', () => {
    const encodings = [[0x8000, Buffer.from([12])], [0x8001, Buffer.from([12, 0])], [0x8002, u16(12)],
        [0x8003, i32(12)], [0x8004, u32(12)], [0x8009, Buffer.from('0c00000000000000', 'hex')],
        [0x800a, Buffer.from('0c00000000000000', 'hex')]];
    for (const [leaf, bytes] of encodings) {
        const r = inspect(fixture([list(field(0x150d, u16(3), u32(0x74), u16(leaf), bytes, z('x'))),
            opaque(), udt({sizeEncoding: Buffer.concat([u16(leaf), bytes])})]));
        assert.equal(r.selected[0].declared_size_bytes, 12);
        assert.equal(r.selected[0].declarations[0].declared_offset_bytes, 12);
    }
    for (const encoding of [Buffer.concat([u16(0x8000), Buffer.from([255])]),
        Buffer.concat([u16(0x800a), Buffer.from('0000000000002000', 'hex')]), u16(0x8005)]) {
        refuse(fixture([list(member()), opaque(), udt({sizeEncoding: encoding})]));
        refuse(fixture([list(field(0x150d, u16(3), u32(0x74), encoding, z('x'))), opaque(), udt()]));
    }
});

test('direct reference domain/range checks are opaque and never traverse or validate referent kind', () => {
    for (const index of [0, 1, 0x7ff, 0x800, 0xfff, 0x1003, 0x80001001, 0xffffffff]) {
        refuse(fixture([list(member(index)), opaque(), udt()]));
    }
    for (const index of [3, 0x74, 0x603, 0x775, 0x1001, 0x1002]) {
        assert.equal(inspect(fixture([list(member(index)), opaque(), udt()])).selected.length, 1);
    }
    for (const key of ['derived', 'vshape']) for (const index of [3, 0xfff, 0x1003, 0x80000000]) {
        refuse(fixture([list(member()), opaque(), udt({[key]: index})]));
    }
    for (const entry of [field(0x1400, u16(3), u32(0x74), u16(0)),
        field(0x1511, u16(3), u32(0x74), z('x')), field(0x150f, u16(1), u32(0x74), z('x'))]) {
        refuse(fixture([list(entry), opaque(), udt()]));
    }
});

test('all independently parameterized caps refuse below the required budget and accept at the measured boundary', () => {
    const r = inspect(chained());
    const required = {stream_bytes: chained().bytes.length, records: 4,
        record_bytes: Math.max(...chained().records.map(x => x.length)),
        read_bytes: Math.max(56, ...chained().records.map(x => x.length)),
        logical_read_bytes: r.logical_read_bytes, read_calls: r.read_calls, selectors: 1,
        selected_record_bytes: Math.max(...chained().records.map(x => x.length)),
        selected_bytes: r.selected_bytes, retained_bytes: r.charged_retained_bytes,
        output_bytes: Buffer.byteLength(JSON.stringify(r)), name_bytes: 'Authored::Small'.length,
        unique_name_bytes: '.?AVSmall@Authored@@'.length, field_lists: 2, continuations: 1, depth: 1,
        declarations: 2, weighted_members: 4, direct_references: 4};
    assert.deepEqual(Object.keys(required).sort(), Object.keys(api.DECLARATION_ENTRY_CAPS).sort());
    for (const [key, value] of Object.entries(required)) {
        // Reducing read_bytes changes conservative retained charging but the
        // output and all other accounting stay independently bounded.
        assert.equal(inspect(chained(), {[key]: value}).selected.length, 1, key);
        refuse(chained(), {[key]: value - 1});
    }
    refuse(twoOwners(), {selectors: 1});
    for (const [key, max] of Object.entries(api.DECLARATION_ENTRY_CAPS)) {
        for (const value of [-1, 0.5, NaN, Infinity, '1', max + 1]) refuse(fixture(), {[key]: value});
    }
    refuse(fixture(), {not_a_cap: 1});
});

test('immutable selector/limits/read snapshots survive callback mutation and returned buffer reuse', () => {
    const x = fixture(), originalRead = x.reader.read;
    const limits = {declarations: 1, read_calls: 10}; x.options.limits = limits;
    let mutated = false;
    x.reader.read = function(offset, length) {
        if (!mutated) {
            mutated = true;
            x.options.profile = 'mutated'; limits.declarations = 0;
            x.options.owners[0].type_index = 0x1001; x.options.owners[0].record_hex = '00000000';
            x.options.owners.length = 0; x.reader.size = 0; x.reader.read = () => { throw Error('wrong callback'); };
        }
        return originalRead(offset, length);
    };
    assert.equal(inspect(x).selected[0].type_index, 0x1002);
    const y = fixture(), scratch = Buffer.alloc(65536); let previous = 0;
    y.reader.read = (o, n) => {
        scratch.fill(0, 0, previous); y.bytes.copy(scratch, 0, o, o + n); previous = n;
        return scratch.subarray(0, n);
    };
    const r = inspect(y), frozen = JSON.stringify(r); scratch.fill(0); y.bytes.fill(0);
    assert.equal(JSON.stringify(r), frozen);
});

test('read calls and bytes are charged before each callback, including refusal paths', () => {
    for (const [limits, expected] of [[{read_calls: 0}, []],
        [{logical_read_bytes: 55}, []], [{read_bytes: 55}, []],
        [{read_calls: 1}, [{offset: 0, length: 56}]],
        [{logical_read_bytes: 59}, [{offset: 0, length: 56}]]]) {
        const x = fixture(); refuse(x, limits); assert.deepEqual(x.reads, expected);
    }
    const x = fixture(); refuse(x, {selected_bytes: x.records[2].length - 1});
    assert.deepEqual(x.reads, [{offset: 0, length: 56},
        ...x.offsets.map(offset => ({offset, length: 4}))]);
});

test('invalid options/getters/readers and late callback exceptions expose one atomic public error only', () => {
    for (const mutate of [x => x.options.profile = api.DECLARATION_PROFILE,
        x => x.options.extra = 'no', x => x.options.owners = [], x => x.options.owners = [null],
        x => x.options.owners = Array(1), x => x.options.owners[0].extra = true,
        x => x.options.owners.extra = true, x => x.options[Symbol('unexpected')] = true,
        x => x.options.owners[Symbol('unexpected')] = true,
        x => x.options.owners[0][Symbol('unexpected')] = true,
        x => Object.defineProperty(x.options.owners, '0', {get() { throw Error('private getter'); }}),
        x => x.options = new Proxy(x.options, {ownKeys() { throw Error('private proxy'); }}),
        x => x.options.owners = new Proxy(x.options.owners, {get() { throw Error('private proxy'); }}),
        x => x.options.owners[0] = new Proxy(x.options.owners[0], {ownKeys() { throw Error('private proxy'); }}),
        x => x.options.limits = Object.defineProperty({}, 'records', {get() { throw Error('private getter'); }}),
        x => x.options.owners[0].record_sha256 = 'X'.repeat(64),
        x => x.options.owners[0].record_hex = 'ff', x => x.options.owners[0].record_hex = 'zzzzzzzz',
        x => x.options.owners[0].type_index = Number.MAX_SAFE_INTEGER + 1,
        x => x.options.owners[0].field_list_type_index = -1,
        x => { delete x.options.owners[0].record_hex; },
        x => Object.defineProperty(x.options, 'profile', {get() { throw Error('secret getter'); }}),
        x => Object.defineProperty(x.options.owners[0], 'record_hex', {get() { throw Error('secret'); }}),
        x => x.reader.size = NaN, x => x.reader.size = 55,
        x => x.reader.read = () => new Uint8Array(56), x => x.reader.read = () => Buffer.alloc(0),
        x => x.reader.read = () => Buffer.from(new SharedArrayBuffer(56)),
        x => x.reader = new Proxy(x.reader, {getOwnPropertyDescriptor() { throw Error('private proxy'); }}),
        x => Object.defineProperty(x.reader, 'size', {get() { throw Error('secret'); }})]) {
        const x = fixture(); mutate(x); refuse(x);
    }
    const x = chained(), originalRead = x.reader.read;
    x.reader.read = (o, n) => { if (o === x.offsets[3] && n > 4) throw Error('private callback details'); return originalRead(o, n); };
    refuse(x);
    assert.ok(x.reads.some(row => row.offset === x.offsets[0] && row.length > 4));
    const y = fixture(), original = y.reader.read;
    y.reader.read = (o, n) => {
        const bytes = original(o, n);
        if (o === y.offsets[0] && n > 4) { const changed = Buffer.from(bytes); changed.writeUInt16LE(0x7777, 2); return changed; }
        return bytes;
    };
    refuse(y);
});
