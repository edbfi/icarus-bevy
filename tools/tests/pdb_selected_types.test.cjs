'use strict';
// Independently authored synthetic TPI records only. No original inputs.
const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const {inspectSelectedTypes, PROFILE, CAPS} = require('../pdb_selected_types.cjs');

const TARGET = 'Authored::Parameters';
const u8 = n => Buffer.from([n]);
const u16 = n => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const z = name => Buffer.from(name + '\0', 'ascii');
function padded(bytes) {
    const n = (4 - bytes.length % 4) % 4;
    return Buffer.concat([bytes, Buffer.from(Array.from({length: n}, (_, index) => 0xf0 + n - index))]);
}
function record(leaf, ...parts) {
    const b = padded(Buffer.concat([Buffer.alloc(2), u16(leaf), ...parts]));
    b.writeUInt16LE(b.length - 2); return b;
}
function member(name, type = 0x75, offset = 0) {
    return padded(Buffer.concat([u16(0x150d), u16(3), u32(type), u16(offset), z(name)]));
}
const continuation = index => Buffer.concat([u16(0x1404), u16(0), u32(index)]);
const list = (...members) => record(0x1203, ...members);
function udt({name = TARGET, fields = 0x1000, count = 1, size = 4, flags = 0,
    unique = null, leaf = 0x1505, sizeEncoding = null} = {}) {
    const options = flags | (unique === null ? 0 : 0x200);
    return record(leaf, u16(count), u16(options), u32(fields), u32(0), u32(0),
        sizeEncoding === null ? u16(size) : sizeEncoding, z(name),
        unique === null ? Buffer.alloc(0) : z(unique));
}
function stream(records) {
    const data = Buffer.concat(records), h = Buffer.alloc(56);
    h.writeUInt32LE(20040203, 0); h.writeUInt32LE(56, 4);
    h.writeUInt32LE(0x1000, 8); h.writeUInt32LE(0x1000 + records.length, 12);
    h.writeUInt32LE(data.length, 16); h.writeUInt16LE(0xffff, 20); h.writeUInt16LE(0xffff, 22);
    h.writeUInt32LE(4, 24); h.writeUInt32LE(4096, 28);
    return Buffer.concat([h, data]);
}
const reader = bytes => ({size: bytes.length, read: (offset, length) => bytes.subarray(offset, offset + length)});
const options = (limits = {}) => ({profile: PROFILE, names: [TARGET], limits});
const fixture = () => stream([list(member('Count')), udt()]);
const inspect = (bytes, limits = {}) => inspectSelectedTypes(reader(bytes), options(limits));
function fails(fn) {
    assert.throws(fn, error => error instanceof Error
        && error.message === 'invalid or unsupported selected TPI types');
}

test('exact selected layout reports offsets and widths without qualification claims', () => {
    const result = inspect(fixture());
    assert.equal(result.records_framed, 2);
    assert.equal(result.selected[0].size_bytes, 4);
    assert.deepEqual(result.selected[0].fields, [{name: 'Count', attributes: 3,
        type_index: 0x75, offset_bytes: 0, kind: 'primitive', storage_kind: 'integer', width_bytes: 4, signed: false}]);
    for (const key of ['caller_provenance_verified', 'original_layout_qualified',
        'release_qualified', 'runtime_behavior_proven', 'hash_tables_validated']) assert.equal(result[key], false);
});

test('selection is exact and ordered; unrelated record payloads remain opaque', () => {
    const bytes = stream([list(member('A')), udt(), udt({name: TARGET + 'Extra'}),
        udt({name: 'Other::Parameters'}), record(0x7777, Buffer.from([0xde, 0xad, 0xbe, 0xef]))]);
    const result = inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: ['Other::Parameters', TARGET]});
    assert.deepEqual(result.selected.map(value => value.name), ['Other::Parameters', TARGET]);
    assert.equal(result.unselected_payloads_validated, false);
    fails(() => inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: ['Parameters']}));
});

test('unselected UDT semantic tails and size values remain opaque', () => {
    const name = 'Other::Malformed';
    const trailing = udt({name});
    const malformed = Buffer.concat([trailing, Buffer.from([1, 2, 3, 4])]);
    malformed.writeUInt16LE(malformed.length - 2);
    const negative = udt({name, sizeEncoding: Buffer.from([0, 0x80, 0xff])});
    for (const unrelated of [malformed, negative]) {
        const bytes = stream([list(member('Count')), udt(), unrelated]);
        assert.equal(inspect(bytes).selected[0].name, TARGET);
        fails(() => inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: [TARGET, name]}));
    }
});

test('forward declarations are retained and bind one complete definition', () => {
    const unique = '.?AUParameters@Authored@@';
    const bytes = stream([list(member('Count')), udt({fields: 0, count: 0, size: 0, flags: 0x80, unique}), udt({unique})]);
    const result = inspect(bytes).selected[0];
    assert.equal(result.type_index, 0x1002);
    assert.equal(result.forward_declarations.length, 1);
    assert.equal(result.forward_declarations[0].type_index, 0x1001);
    assert.equal(result.unique_name, unique);
});

test('missing, forward-only, duplicate and unique-name-conflicting definitions fail', () => {
    fails(() => inspect(stream([udt({name: 'PrivateSentinel', fields: 0, count: 0})])));
    fails(() => inspect(stream([udt({fields: 0, count: 0, size: 0, flags: 0x80})])));
    fails(() => inspect(stream([list(member('Count')), udt(), udt()])));
    fails(() => inspect(stream([list(member('Count')), udt({fields: 0, count: 0, size: 0, flags: 0x80, unique: 'Wrong'}), udt({unique: 'Right'})])));
});

test('ordinary64 pointer, modifier, enum and bitfield dependencies resolve widths', () => {
    const bytes = stream([
        record(0x1002, u32(3), u32(0x1000c)),
        record(0x1001, u32(0x1000), u16(1)),
        record(0x1205, u32(0x75), u8(3), u8(5)),
        record(0x1507, u16(0), u16(0), u32(0x75), u32(0), z('AuthoredEnum')),
        list(member('Pointer', 0x1001, 0), member('Bits', 0x1002, 8), member('Enum', 0x1003, 12)),
        udt({fields: 0x1004, count: 3, size: 16}),
    ]);
    const fields = inspect(bytes).selected[0].fields;
    assert.deepEqual(fields.map(value => value.width_bytes), [8, 4, 4]);
    assert.equal(fields[0].kind, 'modifier');
    assert.equal(fields[1].bit_size, 3); assert.equal(fields[1].bit_offset, 5);
    assert.equal(fields[2].enumerators_interpreted, false);
});

test('primitive64 pointers do not require standalone pointer records', () => {
    const result = inspect(stream([list(member('Pointer', 0x603)), udt({size: 8})]));
    assert.equal(result.selected[0].fields[0].width_bytes, 8);
    assert.equal(result.selected[0].fields[0].referent_type_index, 3);
});

test('fieldlist continuation preserves field order across lists', () => {
    const bytes = stream([list(member('A'), continuation(0x1001)), list(member('B', 0x75, 4)),
        udt({fields: 0x1000, count: 2, size: 8})]);
    assert.deepEqual(inspect(bytes).selected[0].fields.map(value => [value.name, value.offset_bytes]), [['A', 0], ['B', 4]]);
    fails(() => inspect(bytes, {continuations: 1}));
});

test('continuation cycles, nonterminal continuation and wrong target kinds reject', () => {
    fails(() => inspect(stream([list(continuation(0x1001)), list(continuation(0x1000)), udt({count: 0})])));
    fails(() => inspect(stream([list(continuation(0x1001), member('A')), list(), udt()])));
    fails(() => inspect(stream([list(continuation(0x1001)), udt({count: 0})])));
    const bad = continuation(0x1001); bad.writeUInt16LE(1, 2);
    fails(() => inspect(stream([list(bad), list(), udt({count: 0})])));
});

test('selected unsupported member kind, type kind, inheritance and bad field extent reject', () => {
    fails(() => inspect(stream([list(Buffer.concat([u16(0x150f), Buffer.alloc(6)])), udt()])));
    fails(() => inspect(stream([list(member('Opaque', 0x1001)), record(0x7777), udt({fields: 0x1000})])));
    fails(() => inspect(stream([list(member('TooFar', 0x75, 1)), udt()])));
    const inherited = udt(); inherited.writeUInt32LE(0x1000, 12);
    fails(() => inspect(stream([list(member('Count')), inherited])));
    fails(() => inspect(stream([list(member('Count')), udt({count: 2})])));
});

test('invalid pointer modes, bitfield extents and dependent type cycles reject', () => {
    fails(() => inspect(stream([record(0x1002, u32(3), u32(0x1004c)), list(member('P', 0x1000)), udt({fields: 0x1001, size: 8})])));
    fails(() => inspect(stream([record(0x1205, u32(0x75), u8(32), u8(1)), list(member('B', 0x1000)), udt({fields: 0x1001})])));
    fails(() => inspect(stream([record(0x1001, u32(0x1001), u16(1)), record(0x1001, u32(0x1000), u16(1)), list(member('C', 0x1000)), udt({fields: 0x1002})])));
    fails(() => inspect(stream([list(member('C', 0x80001000)), udt()])));
});

test('supported numeric encodings are explicit and negative or oversized values reject', () => {
    for (const encoded of [Buffer.concat([u16(0x8002), u16(4)]), Buffer.concat([u16(0x8004), u32(4)]),
        Buffer.concat([u16(0x800a), Buffer.from([4, 0, 0, 0, 0, 0, 0, 0])])]) {
        assert.equal(inspect(stream([list(member('Count')), udt({sizeEncoding: encoded})])).selected[0].size_bytes, 4);
    }
    fails(() => inspect(stream([list(member('Count')), udt({sizeEncoding: Buffer.concat([u16(0x8001), Buffer.from([0xff, 0xff])])})])));
    fails(() => inspect(stream([list(member('Count')), udt({sizeEncoding: Buffer.concat([u16(0x800a), Buffer.alloc(8, 0xff)])})])));
    fails(() => inspect(stream([list(member('Count')), udt({sizeEncoding: u16(0x8005)})])));
});

test('header profile, count, record and exact extent mutations fail closed', () => {
    for (const [offset, value] of [[0, 19990903], [4, 52], [8, 4095], [12, 0x1003],
        [16, 0], [24, 8], [28, 0], [28, 4095], [36, 4], [44, 1], [52, 1]]) {
        const bytes = fixture(); bytes.writeUInt32LE(value, offset);
        fails(() => inspect(bytes));
    }
    const bytes = fixture(); bytes.writeUInt16LE(1, 56);
    fails(() => inspect(bytes));
    fails(() => inspect(Buffer.concat([fixture(), Buffer.from([0])])));
    fails(() => inspect(fixture().subarray(0, fixture().length - 1)));
});

test('selected padding, names, unknown options and limits are strictly bounded', () => {
    const badPadding = udt({name: TARGET + 'X'});
    // Append another complete record byte instead of relying on a NUL scan to
    // silently consume any tail: exact semantic consumption rejects it.
    const overlong = Buffer.concat([badPadding, Buffer.from([0xf0, 0xf0, 0xf0, 0xf0])]);
    overlong.writeUInt16LE(overlong.length - 2);
    fails(() => inspectSelectedTypes(reader(stream([list(member('Count')), overlong])), {profile: PROFILE, names: [TARGET + 'X']}));
    for (const limits of [{records: 1}, {record_bytes: 4}, {read_bytes: 55}, {fields: 0},
        {retained_bytes: 0}, {output_bytes: 1}, {candidates: 0}, {udt_bytes: 3},
        {fields: CAPS.fields + 1}, {unknown: 1}]) fails(() => inspect(fixture(), limits));
    fails(() => inspectSelectedTypes(reader(fixture()), {...options(), extra: 'PRIVATE'}));
    fails(() => inspectSelectedTypes(reader(fixture()), {...options(), names: [TARGET, TARGET]}));
    fails(() => inspectSelectedTypes(reader(fixture()), {...options(), names: ['Not\u0000Allowed']}));
});

test('cache crossing stays within lowered read cap and preserves exact result', () => {
    const bytes = fixture(), reads = [];
    const input = {size: bytes.length, read(offset, length) {
        reads.push([offset, length]); return bytes.subarray(offset, offset + length);
    }};
    assert.equal(inspectSelectedTypes(input, options({read_bytes: 64})).selected[0].fields[0].name, 'Count');
    assert.ok(reads.every(([offset, length]) => length <= 64 && offset + length <= bytes.length));
});

test('short reads and same/cross-realm shared backing are refused', () => {
    const bytes = fixture();
    fails(() => inspectSelectedTypes({size: bytes.length, read: () => Buffer.alloc(1)}, options()));
    for (const backing of [new SharedArrayBuffer(bytes.length), vm.runInNewContext(`new SharedArrayBuffer(${bytes.length})`)]) {
        const shared = Buffer.from(backing); bytes.copy(shared);
        fails(() => inspectSelectedTypes(reader(shared), options()));
    }
});

test('late unsupported selected result is never returned as a successful prefix', () => {
    const bytes = stream([list(member('Count')), udt(), list(Buffer.concat([u16(0x150f), Buffer.alloc(6)])),
        udt({name: 'PrivateSentinel', fields: 0x1002})]);
    let returned = false;
    fails(() => { inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: [TARGET, 'PrivateSentinel']}); returned = true; });
    assert.equal(returned, false);
    try { inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: ['MissingPrivateSentinel']}); }
    catch (error) { assert.equal(error.message, 'invalid or unsupported selected TPI types'); }
});

test('dependency count and recursion caps lower without changing successful layouts', () => {
    const bytes = stream([record(0x1002, u32(3), u32(0x1000c)), record(0x1001, u32(0x1000), u16(1)),
        list(member('P', 0x1001)), udt({fields: 0x1002, size: 8})]);
    const result = inspect(bytes, {dependencies: 2, depth: 1});
    assert.deepEqual(result.dependent_types.map(value => [value.type_index, value.width_bytes]), [[0x1000, 8], [0x1001, 8]]);
    fails(() => inspect(bytes, {dependencies: 1}));
    fails(() => inspect(bytes, {depth: 0}));
});

test('exact output and retained-byte budgets pass while one lower rejects atomically', () => {
    const bytes = fixture(), expected = inspect(bytes);
    const outputBytes = Buffer.byteLength(JSON.stringify(expected));
    assert.deepEqual(inspect(bytes, {output_bytes: outputBytes}), expected);
    fails(() => inspect(bytes, {output_bytes: outputBytes - 1}));
    assert.deepEqual(inspect(bytes, {retained_bytes: expected.charged_retained_bytes}), expected);
    fails(() => inspect(bytes, {retained_bytes: expected.charged_retained_bytes - 1}));
});
