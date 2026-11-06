'use strict';
// Independently authored envelopes and names only; no target inputs.
const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const vm = require('node:vm');
const {image} = require('./pe_candidates_fixtures.cjs');
const {inspectNativeFieldNames: inspect, PROFILE, CAPS} = require('../native_field_names.cjs');
const ERROR = 'invalid or unsupported native field names';
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const va = (f, rva) => '0x' + (f.base + BigInt(rva)).toString(16);
function fixture() {
    const f = image();
    f.bytes.write('AuthoredName\0OtherName\0', 0x400, 'ascii');
    return f;
}
const options = (f, rvas = [0x2000], limits) => ({sha256: hash(f.bytes), profile: PROFILE,
    addresses: rvas.map(rva => va(f, rva)), ...(limits === undefined ? {} : {limits})});
function reject(f, opts = options(f), bytes = f.bytes) {
    assert.throws(() => inspect(bytes, opts), error => error instanceof Error
        && error.message === ERROR && String(error) === 'Error: ' + ERROR);
}

test('selected names preserve order, duplicate selectors and immutable inputs', () => {
    const f = fixture(), opts = options(f, [0x200d, 0x2000, 0x200d]);
    const before = Buffer.from(f.bytes), saved = structuredClone(opts);
    const r = inspect(f.bytes, opts);
    assert.deepEqual(r.selected, [
        {ordinal: 0, address: va(f, 0x200d), name: 'OtherName', section_id: 'SEC02', rva: 0x200d, file_offset: 0x40d, bytes_including_nul: 10},
        {ordinal: 1, address: va(f, 0x2000), name: 'AuthoredName', section_id: 'SEC02', rva: 0x2000, file_offset: 0x400, bytes_including_nul: 13},
        {ordinal: 2, address: va(f, 0x200d), name: 'OtherName', section_id: 'SEC02', rva: 0x200d, file_offset: 0x40d, bytes_including_nul: 10},
    ]);
    assert.equal(r.unique_addresses, 2); assert.equal(r.examined_name_bytes, 23);
    for (const key of ['caller_provenance_verified', 'original_encoding_qualified', 'release_qualified',
        'runtime_behavior_proven', 'net_field_counts_qualified', 'net_field_order_qualified']) assert.equal(r[key], false);
    assert.deepEqual(f.bytes, before); assert.deepEqual(opts, saved);
});

test('exact name and aggregate bounds include terminators and memoize duplicates', () => {
    const f = fixture();
    assert.equal(inspect(f.bytes, options(f, [0x2000, 0x2000], {name_bytes: 12, total_name_bytes: 13})).examined_name_bytes, 13);
    reject(f, options(f, [0x2000], {name_bytes: 11}));
    reject(f, options(f, [0x2000], {total_name_bytes: 12}));
    reject(f, options(f, [0x2000, 0x200d], {total_name_bytes: 22}));
    assert.equal(inspect(f.bytes, options(f, [0x2000, 0x200d], {total_name_bytes: 23})).examined_name_bytes, 23);
    // Distinct overlapping selectors are separate scans.
    assert.equal(inspect(f.bytes, options(f, [0x2000, 0x2001])).examined_name_bytes, 25);
});

test('only nonempty printable ASCII bytes through a bounded NUL are admitted', () => {
    for (const value of [0, 0x20, 0x7f, 0x80, 0xff, 0x0a]) {
        const f = fixture(); f.bytes[0x400] = value; reject(f);
    }
    const f = fixture(); f.bytes.fill(0x41, 0x400, 0x580); reject(f);
    const ends = fixture(); ends.bytes[0x57e] = 0x41; ends.bytes[0x57f] = 0;
    assert.equal(inspect(ends.bytes, options(ends, [0x217e])).selected[0].name, 'A');
    ends.bytes[0x57f] = 0x41; ends.bytes[0x580] = 0;
    reject(ends, options(ends, [0x217e])); // raw tail cannot terminate the name
});

test('code, resources, writable/unreadable data, headers and virtual/raw tails reject', () => {
    for (const rva of [0x1000, 0x3000, 0x2180, 0x21ff, 0x2200, 0x200, 0x4000]) {
        const f = fixture(); reject(f, options(f, [rva]));
    }
    for (const flags of [0xc0000040, 0x40, 0x40000000, 0x60000040, 0x40000060]) {
        const f = fixture(); f.bytes.writeUInt32LE(flags, f.table + 40 + 36); reject(f);
    }
    const f = fixture(); f.bytes.writeUInt32LE(0x300, f.table + 40 + 8);
    reject(f, options(f, [0x2200])); // virtual tail beyond raw bytes
});

test('options, selectors and lower-only limits have strict shapes', () => {
    const f = fixture(), opts = options(f);
    for (const invalid of [null, [], Object.create(opts), {...opts, extra: 1}, {...opts, profile: 'wrong'},
        {...opts, addresses: []}, {...opts, addresses: new Array(1)}, {...opts, addresses: [f.base + 0x2000n]},
        {...opts, addresses: ['0x0140002000']}, {...opts, addresses: ['0x14000200A']},
        {...opts, addresses: ['0x0']}, {...opts, addresses: ['0x800000000000']},
        {...opts, [Symbol('private')]: true}]) reject(f, invalid);
    for (const limits of [{selectors: 0}, {selectors: CAPS.selectors + 1}, {name_bytes: 0},
        {total_name_bytes: 0}, {name_bytes: -1}, {name_bytes: 1.5}, {name_bytes: NaN},
        {name_bytes: '12'}, {unknown: 1}, []]) reject(f, {...opts, limits});
    assert.deepEqual(inspect(f.bytes, Object.assign(Object.create(null), opts)), inspect(f.bytes, opts));
});

test('exact output cap admits whole result; one lower fails', () => {
    const f = fixture(), expected = inspect(f.bytes, options(f));
    const size = Buffer.byteLength(JSON.stringify(expected));
    assert.deepEqual(inspect(f.bytes, options(f, [0x2000], {output_bytes: size})), expected);
    reject(f, options(f, [0x2000], {output_bytes: size - 1}));
});

test('changing option getters cannot replace admitted hash or selected addresses', () => {
    const f = fixture(), admittedHash = hash(f.bytes);
    let hashReads = 0, addressReads = 0;
    const opts = {profile: PROFILE,
        get sha256() { return ++hashReads === 1 ? admittedHash : '0'.repeat(64); },
        get addresses() { return ++addressReads === 1 ? [va(f, 0x2000)] : [va(f, 0x3000)]; },
    };
    const result = inspect(f.bytes, opts);
    assert.equal(result.input_sha256, admittedHash);
    assert.equal(result.selected[0].address, va(f, 0x2000));
    assert.equal(hashReads, 1); assert.equal(addressReads, 1);
});

test('selector iteration is bounded by admitted length without invoking custom iterators', () => {
    const f = fixture(), addresses = [va(f, 0x2000)];
    let iteratorCalls = 0;
    addresses[Symbol.iterator] = function* () {
        iteratorCalls++;
        while (true) yield va(f, 0x3000);
    };
    const result = inspect(f.bytes, {...options(f), addresses, limits: {selectors: 1}});
    assert.equal(result.selected.length, 1);
    assert.equal(result.selected[0].address, va(f, 0x2000));
    assert.equal(iteratorCalls, 0);
    const inherited = new Array(1);
    Object.setPrototypeOf(inherited, Object.assign(Object.create(Array.prototype), {0: va(f, 0x2000)}));
    reject(f, {...options(f), addresses: inherited});
});

test('late failure never returns selected prefix and errors contain no names', () => {
    const f = fixture(); f.bytes.write('PRIVATE_SENTINEL\0', 0x400, 'ascii');
    let returned = false;
    assert.throws(() => { inspect(f.bytes, options(f, [0x2000, 0x3000])); returned = true; }, {message: ERROR});
    assert.equal(returned, false);
});

test('wrong hashes fail before PE interpretation and shared backing rejects across realms', () => {
    const f = fixture(); let reads = 0;
    f.bytes.readUInt16LE = () => { reads++; throw Error('PRIVATE_PARSE_SENTINEL'); };
    reject(f, {...options(f), sha256: '0'.repeat(64)}); assert.equal(reads, 0);
    for (const backing of [new SharedArrayBuffer(f.bytes.length), vm.runInNewContext(`new SharedArrayBuffer(${f.bytes.length})`)]) {
        const shared = Buffer.from(backing); f.bytes.copy(shared); reject(f, options(f), shared);
    }
});
