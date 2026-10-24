'use strict';

// Independently authored synthetic PE buffers only. Never reads an input file.
// Acceptance: node --test tools/tests/pe_function_ranges.test.cjs
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const {test} = require('node:test');
const vm = require('node:vm');
const {inspect, LIMITS} = require('../pe_function_ranges.cjs');

function fixture() {
  const bytes = Buffer.alloc(0xe00), pe = 0x80, op = pe + 24, table = op + 240;
  const base = 0x140000000n;
  bytes.writeUInt16LE(0x5a4d, 0);
  bytes.writeUInt32LE(pe, 60);
  bytes.writeUInt32LE(0x4550, pe);
  bytes.writeUInt16LE(0x8664, pe + 4);
  bytes.writeUInt16LE(3, pe + 6);
  bytes.writeUInt16LE(240, pe + 20);
  bytes.writeUInt16LE(2, pe + 22);
  bytes.writeUInt16LE(0x20b, op);
  bytes.writeUInt32LE(0x1000, op + 16);
  bytes.writeBigUInt64LE(base, op + 24);
  bytes.writeUInt32LE(0x1000, op + 32);
  bytes.writeUInt32LE(0x200, op + 36);
  bytes.writeUInt32LE(0x4000, op + 56);
  bytes.writeUInt32LE(0x200, op + 60);
  bytes.writeUInt32LE(16, op + 108);
  bytes.writeUInt32LE(0x2020, op + 136);
  bytes.writeUInt32LE(36, op + 140);
  for (const [index, name, rva, raw, virtual, rawBytes, flags] of [
    [0, '.text', 0x1000, 0x800, 0x180, 0x200, 0x60000020],
    [1, '.pdata', 0x2000, 0x200, 0x180, 0x200, 0x40000040],
    [2, '.xdata', 0x3000, 0x600, 0x180, 0x200, 0x40000040],
  ]) {
    const at = table + index * 40;
    bytes.write(name, at, 'ascii');
    bytes.writeUInt32LE(virtual, at + 8);
    bytes.writeUInt32LE(rva, at + 12);
    bytes.writeUInt32LE(rawBytes, at + 16);
    bytes.writeUInt32LE(raw, at + 20);
    bytes.writeUInt32LE(flags, at + 36);
  }
  // Adjacent entries, then a gap. Physical offsets deliberately differ from RVAs.
  const entries = [[0x1010, 0x1020, 0x3010], [0x1020, 0x1040, 0x3020], [0x1080, 0x1100, 0x3030]];
  entries.forEach((entry, index) => entry.forEach((value, field) =>
    bytes.writeUInt32LE(value, 0x220 + index * 12 + field * 4)));
  // No unwind contents are required or interpreted, including chain-looking flags.
  bytes[0x610] = 0x21;
  bytes.write('SyntheticPrivateMarker', 0xc00, 'ascii');
  return {bytes, pe, op, table, base};
}
const sha = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const va = (f, rva) => '0x' + (f.base + BigInt(rva)).toString(16);
const read = (f, rvas = [0x1010]) => inspect(f.bytes, {sha256: sha(f.bytes), addresses: rvas.map(rva => va(f, rva))});
const rejects = (fn, code) => assert.throws(fn, error => error instanceof Error && error.code === code);
const entry = (f, index, field, value) => f.bytes.writeUInt32LE(value, 0x220 + index * 12 + field * 4);

test('inclusive starts, exclusive ends, original selector order and exact physical spans', () => {
  const f = fixture(), result = read(f, [0x10ff, 0x1020, 0x101f, 0x1010]);
  assert.deepEqual(result.matches.map(m => m.entry_index), [2, 1, 0, 0]);
  assert.deepEqual(result.matches[1], {
    address: '0x140001020', rva: 0x1020, entry_index: 1,
    start_rva: 0x1020, end_rva: 0x1040, unwind_rva: 0x3020,
    start_address: '0x140001020', end_address: '0x140001040',
    entry_file_span: {start: 0x22c, end: 0x238},
    function_file_span: {start: 0x820, end: 0x840},
    unwind_header_file_span: {start: 0x620, end: 0x624},
  });
  assert.deepEqual(result.exception_directory_file_span, {start: 0x220, end: 0x244});
  assert.equal(result.runtime_function_entry_count, 3);
  assert.equal(result.complete_exception_table_framing, true);
  assert.equal(result.input_sha256, sha(f.bytes));
  assert.equal(result.interval_convention, 'start_inclusive_end_exclusive');
  for (const field of ['public_symbol_identity_verified', 'instruction_boundaries_confirmed',
    'function_identity_confirmed', 'unwind_chains_expanded', 'unwind_headers_interpreted']) {
    assert.equal(result[field], false);
  }
  assert(!JSON.stringify(result).includes('SyntheticPrivateMarker'));
  assert(Buffer.byteLength(JSON.stringify(result) + '\n') <= LIMITS.output_bytes);
});

test('selectors export only selected records and leave the caller Buffer intact', () => {
  const f = fixture(), original = Buffer.from(f.bytes), result = read(f);
  assert.deepEqual(result.matches.map(m => m.entry_index), [0]);
  assert.deepEqual(f.bytes, original);
  // Section headers need not be in RVA order. Their physical ranges are disjoint.
  const first = Buffer.from(f.bytes.subarray(f.table, f.table + 40));
  f.bytes.copy(f.bytes, f.table, f.table + 80, f.table + 120);
  first.copy(f.bytes, f.table + 80);
  assert.deepEqual(read(f).matches, result.matches);
});

test('high canonical VAs retain exact integer precision', () => {
  const f = fixture();
  f.base = 0xffff800000000000n;
  f.bytes.writeBigUInt64LE(f.base, f.op + 24);
  assert.equal(read(f).matches[0].start_address, '0xffff800000001010');
  assert.equal(read(f).matches[0].end_address, '0xffff800000001020');
  rejects(() => inspect(f.bytes, {sha256: sha(f.bytes), addresses: ['0x0']}), 'ERR_SELECTOR_RANGE');
});

test('gaps and exclusive ends report explicit absence, never neighboring ownership', () => {
  for (const rva of [0x1000, 0x1040, 0x107f, 0x1100, 0x3000]) {
    rejects(() => read(fixture(), [rva]), 'ERR_FUNCTION_ABSENT');
  }
  rejects(() => read(fixture(), [0x1010, 0x1040]), 'ERR_FUNCTION_ABSENT');
  rejects(() => read(fixture(), [0x4000]), 'ERR_SELECTOR_RANGE');
  rejects(() => read(fixture(), [-1]), 'ERR_SELECTOR_RANGE');
});

test('hash mismatch precedes interpretation of even malformed PE bytes', () => {
  const f = fixture(), expected = sha(f.bytes);
  f.bytes.writeUInt16LE(0, 0);
  rejects(() => inspect(f.bytes, {sha256: expected, addresses: [va(f, 0x1010)]}), 'ERR_SHA_MISMATCH');
  rejects(() => read(f), 'ERR_PE_PROFILE');
});

test('strict DOS, PE32+, x64 and executable header bounds', () => {
  for (const [kind, at, value] of [
    ['u16', 0, 0], ['u32', 60, 0xffffff00], ['u32', 0x80, 0],
    ['u16', 0x84, 0x14c], ['u16', 0x86, 97], ['u16', 0x94, 2],
    ['u16', 0x96, 0x2002], ['u16', 0x98, 0x10b],
  ]) {
    const f = fixture();
    if (kind === 'u16') f.bytes.writeUInt16LE(value, at);
    else f.bytes.writeUInt32LE(value, at);
    rejects(() => read(f), 'ERR_PE_PROFILE');
  }
  for (const size of [64, 0x80, 0x98, 0x180, 0x9ff]) {
    const f = fixture();
    f.bytes = f.bytes.subarray(0, size);
    rejects(() => read(f), 'ERR_PE_PROFILE');
  }
});

test('missing and malformed exception directories fail explicitly', () => {
  const absent = fixture();
  absent.bytes.writeUInt32LE(3, absent.op + 108);
  rejects(() => read(absent), 'ERR_EXCEPTION_ABSENT');
  for (const [rva, size, code] of [
    [0, 0, 'ERR_EXCEPTION_ABSENT'], [0, 12, 'ERR_EXCEPTION_DIRECTORY'],
    [0x2020, 0, 'ERR_EXCEPTION_DIRECTORY'], [0x2021, 36, 'ERR_EXCEPTION_DIRECTORY'],
    [0x2020, 35, 'ERR_EXCEPTION_DIRECTORY'], [0xfffffff0, 36, 'ERR_EXCEPTION_DIRECTORY'],
    [0x2020, (LIMITS.entries + 1) * 12, 'ERR_EXCEPTION_DIRECTORY'],
    [0x2178, 12, 'ERR_MAPPING_ABSENT'], [0x2180, 12, 'ERR_MAPPING_ABSENT'],
    [0x1000, 36, 'ERR_EXCEPTION_DIRECTORY'],
  ]) {
    const f = fixture();
    f.bytes.writeUInt32LE(rva, f.op + 136);
    f.bytes.writeUInt32LE(size, f.op + 140);
    rejects(() => read(f), code);
  }
  const renamed = fixture();
  renamed.bytes.fill(0, renamed.table + 40, renamed.table + 48);
  renamed.bytes.write('.rdata', renamed.table + 40, 'ascii');
  rejects(() => read(renamed), 'ERR_EXCEPTION_DIRECTORY');
  const capped = fixture();
  capped.bytes.writeUInt32LE(0x1000000, capped.op + 56);
  capped.bytes.writeUInt32LE((LIMITS.entries + 1) * 12, capped.op + 140);
  rejects(() => read(capped), 'ERR_EXCEPTION_DIRECTORY');
});

test('exception directory and four-byte unwind header require their complete physical extents', () => {
  const f = fixture();
  f.bytes.writeUInt32LE(0x44, f.table + 40 + 8);
  assert.equal(read(f).matches.length, 1);
  f.bytes.writeUInt32LE(0x43, f.table + 40 + 8);
  rejects(() => read(f), 'ERR_MAPPING_ABSENT');
  const header = fixture();
  header.bytes.writeUInt32LE(0x34, header.table + 80 + 8);
  assert.equal(read(header).matches.length, 1);
  header.bytes.writeUInt32LE(0x33, header.table + 80 + 8);
  rejects(() => read(header), 'ERR_MAPPING_ABSENT');
});

test('complete framing rejects malformed unselected records and ambiguous intervals', () => {
  for (const [index, field, value, code] of [
    [2, 0, 0, 'ERR_RUNTIME_FUNCTION_RANGE'], [2, 1, 0x1080, 'ERR_RUNTIME_FUNCTION_RANGE'],
    [2, 1, 0x107f, 'ERR_RUNTIME_FUNCTION_RANGE'], [2, 1, 0xffffffff, 'ERR_RUNTIME_FUNCTION_RANGE'],
    [1, 0, 0x1010, 'ERR_FUNCTION_AMBIGUOUS'], [1, 0, 0x1018, 'ERR_FUNCTION_AMBIGUOUS'],
    [1, 0, 0x1008, 'ERR_RUNTIME_FUNCTION_ORDER'], [2, 2, 0, 'ERR_UNWIND_POINTER'],
    [2, 2, 0x3031, 'ERR_UNWIND_POINTER'], [2, 2, 0xfffffffc, 'ERR_MAPPING_ABSENT'],
    [2, 2, 0x1000, 'ERR_UNWIND_POINTER'], [2, 2, 0x317c, null],
    [2, 2, 0x3180, 'ERR_MAPPING_ABSENT'],
  ]) {
    const f = fixture();
    entry(f, index, field, value);
    if (code) rejects(() => read(f), code);
    else assert.equal(read(f).matches.length, 1);
  }
  const duplicate = fixture();
  for (let field = 0; field < 3; field++) {
    entry(duplicate, 1, field, duplicate.bytes.readUInt32LE(0x220 + field * 4));
  }
  rejects(() => read(duplicate), 'ERR_FUNCTION_AMBIGUOUS');
  const padded = fixture();
  padded.bytes.writeUInt32LE(48, padded.op + 140);
  rejects(() => read(padded), 'ERR_RUNTIME_FUNCTION_RANGE');
});

test('overlapping virtual or physical mappings are rejected even outside selectors', () => {
  for (const [offset, value] of [[12, 0x1000], [20, 0x800]]) {
    const f = fixture();
    f.bytes.writeUInt32LE(value, f.table + 80 + offset);
    rejects(() => read(f), 'ERR_MAPPING_AMBIGUOUS');
  }
  const crossing = fixture();
  entry(crossing, 2, 1, 0x2001);
  rejects(() => read(crossing), 'ERR_MAPPING_ABSENT');
});

test('physical code spans exclude raw padding, virtual tails and unmapped gaps', () => {
  const exact = fixture();
  entry(exact, 2, 1, 0x1180);
  assert.deepEqual(read(exact, [0x117f]).matches[0].function_file_span, {start: 0x880, end: 0x980});
  for (const end of [0x1181, 0x1200]) {
    const f = fixture();
    entry(f, 2, 1, end);
    rejects(() => read(f), 'ERR_MAPPING_ABSENT');
  }
  const tail = fixture();
  tail.bytes.writeUInt32LE(0x300, tail.table + 8);
  entry(tail, 2, 1, 0x1201);
  rejects(() => read(tail), 'ERR_MAPPING_ABSENT');
  const gap = fixture();
  entry(gap, 2, 0, 0x1800);
  entry(gap, 2, 1, 0x1810);
  rejects(() => read(gap), 'ERR_MAPPING_ABSENT');
});

test('strict section flags, bounds and arithmetic do not wrap', () => {
  for (const [section, field, value, code] of [
    [0, 36, 0xe0000020, 'ERR_RUNTIME_FUNCTION_RANGE'],
    [1, 36, 0xc0000040, 'ERR_EXCEPTION_DIRECTORY'],
    [2, 36, 0xc0000040, 'ERR_UNWIND_POINTER'],
    [0, 12, 0xfffff000, 'ERR_PE_PROFILE'],
    [1, 20, 0xfffffe00, 'ERR_PE_PROFILE'],
    [1, 16, 0xfffffe00, 'ERR_PE_PROFILE'],
    [2, 16, 0, 'ERR_MAPPING_ABSENT'],
  ]) {
    const f = fixture();
    f.bytes.writeUInt32LE(value, f.table + section * 40 + field);
    rejects(() => read(f), code);
  }
  const overflow = fixture();
  overflow.bytes.writeBigUInt64LE(0xffffffffffff0000n, overflow.op + 24);
  overflow.bytes.writeUInt32LE(0x20000, overflow.op + 56);
  rejects(() => read(overflow), 'ERR_PE_PROFILE');
  const noncanonical = fixture();
  noncanonical.bytes.writeBigUInt64LE(0x800000000000n, noncanonical.op + 24);
  rejects(() => read(noncanonical), 'ERR_PE_PROFILE');
});

test('selector syntax, uniqueness and pin shapes are bounded', () => {
  const f = fixture(), expected = sha(f.bytes), valid = va(f, 0x1010);
  for (const addresses of [[], new Array(25).fill(valid), [valid, valid],
    ['0x0140001010'], ['0X140001010'], ['0x14000101A'], ['140001010'],
    ['0x140001010\n'], ['0x800000000000'], ['0x10000000000000000'], [1], [0x140001010n],
    [null], '0x140001010', new Array(1)]) {
    rejects(() => inspect(f.bytes, {sha256: expected, addresses}), 'ERR_SELECTORS');
  }
  for (const pin of [undefined, null, {}, [], {sha256: expected},
    {sha256: expected.toUpperCase(), addresses: [valid]},
    {sha256: expected + '\n', addresses: [valid]},
    {sha256: expected, addresses: [valid], extra: true}]) {
    rejects(() => inspect(f.bytes, pin), 'ERR_INPUT_PIN');
  }
  const addresses = Array.from({length: 24}, (_, index) => va(f, 0x1020 + index));
  assert.equal(inspect(f.bytes, {sha256: expected, addresses}).matches.length, 24);
});

test('Buffer byte cap and shared backing storage fail before hashing', () => {
  assert.equal(LIMITS.file_bytes, 128 * 1024 * 1024);
  assert.equal(LIMITS.output_bytes, 65536);
  for (const bytes of [null, {length: LIMITS.file_bytes + 1}, new Uint8Array(64), Buffer.alloc(63)]) {
    rejects(() => inspect(bytes, null), 'ERR_INPUT_BOUND');
  }
  // Exercise the real oversized-Buffer boundary without allocating 128 MiB.
  const oversized = Buffer.alloc(64);
  Object.defineProperty(oversized, 'length', {value: LIMITS.file_bytes + 1});
  rejects(() => inspect(oversized, null), 'ERR_INPUT_BOUND');
  if (typeof SharedArrayBuffer !== 'undefined') {
    rejects(() => inspect(Buffer.from(new SharedArrayBuffer(64)), null), 'ERR_INPUT_BOUND');
    const foreign = vm.runInNewContext('new SharedArrayBuffer(64)');
    assert.equal(foreign instanceof SharedArrayBuffer, false);
    rejects(() => inspect(Buffer.from(foreign), null), 'ERR_INPUT_BOUND');
  }
});
