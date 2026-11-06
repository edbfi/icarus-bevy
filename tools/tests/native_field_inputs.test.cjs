'use strict';
// Authored PE envelopes only. No target files or game values.
const test = require('node:test');
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const vm = require('node:vm');
const {image} = require('./pe_candidates_fixtures.cjs');
const {inspectNativeFieldInputs: inspect, PROFILE, CAPS} = require('../native_field_inputs.cjs');
const ERROR = 'invalid or unsupported native field inputs';
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const va = (f, rva) => '0x' + (f.base + BigInt(rva)).toString(16);
const raw = rva => 0x400 + rva - 0x2000;

function header(f, rva = 0x2000) {
    const at = raw(rva);
    f.bytes.fill(0, at, at + 76);
    f.bytes.writeBigUInt64LE(f.target, at);
    f.bytes.writeBigUInt64LE(f.base + 0x2120n, at + 32);
    f.bytes.writeBigUInt64LE(f.base + 0x2100n, at + 40);
    f.bytes.writeUInt32LE(1, at + 60);
    f.bytes.writeInt32LE(1, at + 64);
}
function fixture() {
    const f = image();
    const bytes = Buffer.alloc(0xe00); f.bytes.copy(bytes); f.bytes = bytes;
    f.bytes.writeUInt32LE(0x600, f.table + 40 + 8);
    f.bytes.writeUInt32LE(0x600, f.table + 40 + 16);
    f.bytes.writeUInt32LE(0xa00, f.table + 80 + 20);
    f.bytes.fill(0, 0x400, 0xa00);
    header(f);
    f.bytes.writeBigUInt64LE(f.base + 0x2141n, raw(0x2100));
    f.bytes.writeBigUInt64LE(f.target, raw(0x2120));
    f.bytes.writeBigUInt64LE(f.base + 0x2181n, raw(0x2120) + 8);
    property(f, 0x2141);
    f.bytes.write('PRIVATE_SENTINEL', raw(0x2180), 'ascii');
    return f;
}
function property(f, rva) {
    const at = raw(rva);
    f.bytes.writeBigUInt64LE(f.base + 0x2180n, at);
    f.bytes.writeBigUInt64LE(0n, at + 8);
    f.bytes.writeBigUInt64LE(0xfffffffffffffffen, at + 16);
    f.bytes.writeUInt32LE(0xfedcba98, at + 24);
    f.bytes.writeUInt32LE(0x89abcdef, at + 28);
    f.bytes.writeInt32LE(1, at + 32);
}
const options = (f, rvas = [0x2000], limits) => ({
    sha256: hash(f.bytes), addresses: rvas.map(rva => va(f, rva)), profile: PROFILE,
    ...(limits === undefined ? {} : {limits}),
});
function reject(f, opts = options(f), bytes = f.bytes) {
    assert.throws(() => inspect(bytes, opts), error => error instanceof Error
        && error.message === ERROR && String(error) === 'Error: ' + ERROR);
}

test('exact base inputs and function links preserve 64-bit flags without name contents', () => {
    const f = fixture(), before = Buffer.from(f.bytes), opts = options(f), saved = structuredClone(opts);
    const result = inspect(f.bytes, opts);
    assert.equal(result.total_property_inputs, 1);
    assert.equal(result.total_function_links, 1);
    assert.deepEqual(result.selected[0].property_inputs, [{
        ordinal: 0, address: '0x140002141', name_pointer: '0x140002180', rep_notify_pointer: '0x0',
        property_flags: '0xfffffffffffffffe', gen_flags: 0xfedcba98, object_flags: 0x89abcdef, array_dim: 1,
    }]);
    assert.deepEqual(result.selected[0].function_links, [{
        ordinal: 0, factory_pointer: '0x140001100', name_pointer: '0x140002181',
    }]);
    assert.ok(!JSON.stringify(result).includes('PRIVATE_SENTINEL'));
    for (const key of ['caller_provenance_verified', 'release_qualified', 'net_field_counts_qualified',
        'net_field_order_qualified', 'instruction_boundaries_confirmed']) assert.equal(result[key], false);
    assert.deepEqual(f.bytes, before); assert.deepEqual(opts, saved);
});

test('unaligned opaque strings, descriptors and byte-readable function links are admitted', () => {
    const f = fixture();
    f.bytes.writeBigUInt64LE(f.base + 0x2181n, raw(0x2141) + 8);
    f.bytes.copy(f.bytes, raw(0x2121), raw(0x2120), raw(0x2120) + 16);
    f.bytes.writeBigUInt64LE(f.base + 0x2121n, raw(0x2000) + 32);
    const result = inspect(f.bytes, options(f)).selected[0];
    assert.equal(result.property_inputs[0].rep_notify_pointer, '0x140002181');
    assert.equal(result.property_inputs[0].address, '0x140002141');
    assert.equal(result.function_links[0].factory_pointer, '0x140001100');
});

test('signed ArrayDim input is retained without invented runtime policy', () => {
    for (const dim of [0, -1, -2147483648, 2147483647]) {
        const f = fixture(); f.bytes.writeInt32LE(dim, raw(0x2141) + 32);
        assert.equal(inspect(f.bytes, options(f)).selected[0].property_inputs[0].array_dim, dim);
    }
});

test('every required property and function name/code pointer must be nonnull', () => {
    for (const offset of [raw(0x2100), raw(0x2141), raw(0x2120), raw(0x2120) + 8]) {
        const f = fixture(); f.bytes.writeBigUInt64LE(0n, offset); reject(f);
    }
    const f = fixture(); f.bytes.writeBigUInt64LE(0n, raw(0x2000) + 32); reject(f);
});

test('whole descriptor and array spans reject virtual/raw tails', () => {
    const end = fixture(); property(end, 0x25dc);
    end.bytes.writeBigUInt64LE(end.base + 0x25dcn, raw(0x2100));
    assert.equal(inspect(end.bytes, options(end)).selected[0].property_inputs[0].address, '0x1400025dc');
    const badProperty = fixture(); badProperty.bytes.writeBigUInt64LE(badProperty.base + 0x25ddn, raw(0x2100)); reject(badProperty);
    const badLinks = fixture(); badLinks.bytes.writeBigUInt64LE(badLinks.base + 0x25f8n, raw(0x2000) + 32); reject(badLinks);
    const badPointerArray = fixture(); badPointerArray.bytes.writeBigUInt64LE(badPointerArray.base + 0x25fcn, raw(0x2000) + 40); reject(badPointerArray);
});

test('opaque names reject code/resource/unmapped locations and code targets reject data', () => {
    for (const rva of [0x1100, 0x3000, 0x2600, 0x4000]) {
        const f = fixture(); f.bytes.writeBigUInt64LE(f.base + BigInt(rva), raw(0x2141)); reject(f);
    }
    for (const rva of [0x2180, 0x3000, 0x1180]) {
        const f = fixture(); f.bytes.writeBigUInt64LE(f.base + BigInt(rva), raw(0x2120)); reject(f);
    }
    const writable = fixture(); writable.bytes.writeUInt32LE(0xc0000040, writable.table + 40 + 36); reject(writable);
});

test('function link order and duplicate targets remain explicit records', () => {
    const f = fixture();
    f.bytes.copy(f.bytes, raw(0x2130), raw(0x2120), raw(0x2120) + 16);
    f.bytes.writeUInt32LE(2, raw(0x2000) + 60);
    const links = inspect(f.bytes, options(f)).selected[0].function_links;
    assert.equal(links.length, 2);
    assert.deepEqual(links.map(value => value.ordinal), [0, 1]);
    assert.equal(links[0].factory_pointer, links[1].factory_pointer);
});

test('aggregate limits reject two individually admitted class inputs', () => {
    const f = fixture(); header(f, 0x2050);
    const opts = options(f, [0x2050, 0x2000], {properties: 1, functions: 1, total_properties: 2, total_functions: 2});
    const result = inspect(f.bytes, opts);
    assert.deepEqual(result.selected.map(value => value.address), ['0x140002050', '0x140002000']);
    assert.equal(result.total_function_links, 2); assert.equal(result.total_property_inputs, 2);
    reject(f, {...opts, limits: {...opts.limits, total_properties: 1}});
    reject(f, {...opts, limits: {...opts.limits, total_functions: 1}});
});

test('zero counts preserve optional opaque pointers without dereferencing arrays', () => {
    const f = fixture();
    f.bytes.writeInt32LE(0, raw(0x2000) + 64); f.bytes.writeUInt32LE(0, raw(0x2000) + 60);
    for (const offset of [32, 40]) f.bytes.writeBigUInt64LE(f.base + 0x25ffn, raw(0x2000) + offset);
    const result = inspect(f.bytes, options(f, [0x2000], {properties: 0, functions: 0, total_properties: 0, total_functions: 0}));
    assert.deepEqual(result.selected[0].property_inputs, []); assert.deepEqual(result.selected[0].function_links, []);
});

test('per-class count failures and whole-link-array errors reject atomically', () => {
    for (const [offset, value] of [[64, -1], [64, 4097], [60, 4097], [60, 300]]) {
        const f = fixture(); f.bytes.writeInt32LE(value, raw(0x2000) + offset); reject(f);
    }
    const f = fixture(); header(f, 0x2050);
    f.bytes.writeUInt32LE(300, raw(0x2050) + 60);
    let returned = false;
    assert.throws(() => { inspect(f.bytes, options(f, [0x2000, 0x2050])); returned = true; }, {message: ERROR});
    assert.equal(returned, false);
});

test('exact output budget passes; one below and raised/invalid caps reject', () => {
    const f = fixture(), expected = inspect(f.bytes, options(f));
    const size = Buffer.byteLength(JSON.stringify(expected));
    assert.deepEqual(inspect(f.bytes, options(f, [0x2000], {output_bytes: size})), expected);
    reject(f, options(f, [0x2000], {output_bytes: size - 1}));
    for (const limits of [{properties: CAPS.properties + 1}, {total_functions: CAPS.total_functions + 1},
        {functions: -1}, {functions: 0.5}, {functions: NaN}, {functions: '1'}, {unknown: 1}, []]) {
        reject(f, {...options(f), limits});
    }
});

test('options and selectors retain strict ownership and canonical shape', () => {
    const f = fixture(), opts = options(f);
    for (const invalid of [null, [], Object.create(opts), {...opts, unknown: 1}, {...opts, profile: 'wrong'},
        {...opts, addresses: [f.base + 0x2000n]}, {...opts, addresses: ['0x0140002000']},
        {...opts, addresses: [opts.addresses[0], opts.addresses[0]]}]) reject(f, invalid);
    const withSymbol = {...opts, [Symbol('private')]: true}; reject(f, withSymbol);
    assert.deepEqual(inspect(f.bytes, Object.assign(Object.create(null), opts)), inspect(f.bytes, opts));
});

test('hash mismatches precede PE access and all errors stay generic', () => {
    const f = fixture(); let reads = 0;
    f.bytes.readUInt16LE = () => { reads++; throw Error('PRIVATE_PARSE_SENTINEL'); };
    reject(f, {...options(f), sha256: '0'.repeat(64)});
    assert.equal(reads, 0);
});

test('shared backing from either realm remains refused', () => {
    const f = fixture();
    for (const backing of [new SharedArrayBuffer(f.bytes.length), vm.runInNewContext(`new SharedArrayBuffer(${f.bytes.length})`)]) {
        const shared = Buffer.from(backing); f.bytes.copy(shared); reject(f, options(f), shared);
    }
});

test('reported input hash is the admitted pin even when caller options change on read', () => {
    const f = fixture(), opts = options(f), admitted = opts.sha256;
    let reads = 0;
    Object.defineProperty(opts, 'sha256', {get() { return reads++ === 0 ? admitted : '0'.repeat(64); }});
    assert.equal(inspect(f.bytes, opts).input_sha256, admitted);
    assert.equal(reads, 1);
});

test('address options are captured once and custom iterators cannot expand selectors', () => {
    const f = fixture(), opts = options(f);
    let iteratorCalls = 0, reads = 0;
    const addresses = [va(f, 0x2000)];
    addresses[Symbol.iterator] = function* () {
        iteratorCalls++;
        for (let index = 0; index < 9; index++) yield va(f, 0x2000 + index * 8);
    };
    Object.defineProperty(opts, 'addresses', {get() { return ++reads === 1 ? addresses : []; }});
    const result = inspect(f.bytes, opts);
    assert.deepEqual(result.selected.map(value => value.address), [va(f, 0x2000)]);
    assert.equal(reads, 1);
    assert.equal(iteratorCalls, 0);
});

test('address ceiling and own indexed selectors are enforced before forwarding', () => {
    const f = fixture();
    const oversized = Array(9).fill(va(f, 0x2000));
    oversized[Symbol.iterator] = function* () { yield va(f, 0x2000); };
    reject(f, {...options(f), addresses: oversized});
    reject(f, {...options(f), addresses: new Array(1)});
    const inherited = new Array(1), prototype = Object.create(Array.prototype);
    prototype[0] = va(f, 0x2000);
    Object.setPrototypeOf(inherited, prototype);
    reject(f, {...options(f), addresses: inherited});
});

test('property pointer order and duplicate descriptors remain explicit inputs', () => {
    const f = fixture();
    property(f, 0x21a0);
    f.bytes.writeInt32LE(-7, raw(0x21a0) + 32);
    for (const [index, rva] of [0x21a0, 0x2141, 0x21a0].entries()) {
        f.bytes.writeBigUInt64LE(f.base + BigInt(rva), raw(0x2100) + index * 8);
    }
    f.bytes.writeInt32LE(3, raw(0x2000) + 64);
    const result = inspect(f.bytes, options(f));
    assert.equal(result.total_property_inputs, 3);
    assert.deepEqual(result.selected[0].property_inputs.map(value => [value.ordinal, value.address, value.array_dim]), [
        [0, '0x1400021a0', -7], [1, '0x140002141', 1], [2, '0x1400021a0', -7],
    ]);
});

test('bytes after the exact property prefix have no descriptor meaning', () => {
    const f = fixture(), before = inspect(f.bytes, options(f));
    // Bytes 36 onward could be padding or an unsupported property-kind tail.
    // Deliberately include an invalid pointer without granting it meaning.
    f.bytes.fill(0xff, raw(0x2141) + 36, raw(0x2141) + 48);
    const after = inspect(f.bytes, options(f));
    assert.deepEqual(after.selected, before.selected);
    assert.notEqual(after.input_sha256, before.input_sha256);
});

test('every opaque pointer role rejects invalid nonnull mappings', () => {
    for (const offset of [raw(0x2141), raw(0x2141) + 8, raw(0x2120) + 8]) {
        for (const pointer of [0x140001100n, 0x140003000n, 0x140002600n, 0x800000000000n, 0xffffffffffffffffn]) {
            const f = fixture();
            f.bytes.writeBigUInt64LE(pointer, offset);
            reject(f);
        }
    }
});

test('an invalid final function link prevents any partial result', () => {
    const f = fixture();
    f.bytes.copy(f.bytes, raw(0x2130), raw(0x2120), raw(0x2120) + 16);
    f.bytes.writeUInt32LE(2, raw(0x2000) + 60);
    f.bytes.writeBigUInt64LE(f.base + 0x2180n, raw(0x2130));
    const before = Buffer.from(f.bytes), opts = options(f), saved = structuredClone(opts);
    reject(f, opts);
    assert.deepEqual(f.bytes, before);
    assert.deepEqual(opts, saved);
});
