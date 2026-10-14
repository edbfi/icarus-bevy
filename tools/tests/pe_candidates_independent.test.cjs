'use strict';
// Independent synthetic PE envelopes. No original artifact is read.
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const {parse, scan} = require('../pe_candidates.cjs');
const base = 0x180000000n, target = base + 0x1080n;
function fixture() {
  const bytes = Buffer.alloc(0x1000), pe = 0x80, op = pe + 24, table = op + 240;
  const u16 = (at, v) => bytes.writeUInt16LE(v, at);
  const u32 = (at, v) => bytes.writeUInt32LE(v, at);
  u16(0, 0x5a4d); u32(60, pe); u32(pe, 0x4550); u16(pe + 4, 0x8664);
  u16(pe + 6, 4); u16(pe + 20, 240); u16(pe + 22, 2);
  u16(op, 0x20b); u32(op + 16, 0x1000); bytes.writeBigUInt64LE(base, op + 24);
  u32(op + 32, 0x1000); u32(op + 36, 0x200); u32(op + 56, 0x5000);
  u32(op + 60, 0x400); u32(op + 108, 16); u32(op + 128, 0x3000); u32(op + 132, 0x80);
  [0x60000020, 0x40000040, 0x40000040, 0xc0000040].forEach((flags, i) => {
    const at = table + 40 * i;
    bytes.write('SECR' + i, at); u32(at + 8, 0x180); u32(at + 12, (i + 1) * 0x1000);
    u32(at + 16, 0x200); u32(at + 20, 0x400 + i * 0x200); u32(at + 36, flags);
  });
  return {bytes, op, table, u32};
}
const run = f => scan(f.bytes, {target, sha256: crypto.createHash('sha256').update(f.bytes).digest('hex')});
function rel(f, at, opcode = 0xe8) { f.bytes[at] = opcode; f.bytes.writeInt32LE(Number(target - base) - (0x1000 + at - 0x400 + 5), at + 1); }
const checks = [];
function check(name, fn) { try { fn(); checks.push({name, pass: true}); } catch (error) { checks.push({name, pass: false, reason: error.message.split('\n')[0]}); } }
check('eligible exact boundary and unaligned pointer offsets', () => {
  const f = fixture(); rel(f, 0x400); rel(f, 0x57b, 0xe9); f.bytes.writeBigUInt64LE(target, 0x611);
  const r = run(f); assert.deepEqual(r.candidates.map(c => c.file_offset), [0x400, 0x57b, 0x611]);
  assert.equal(r.examined_offsets, (0x180 - 4) + (0x180 - 7));
  assert.equal(r.complete_within_declared_section_patterns, true); assert.equal(r.confirmed_callers, false);
  assert(!JSON.stringify(r).includes('SECR')); assert(Buffer.byteLength(JSON.stringify(r)) < 65536);
});
check('resource writable overlay and raw padding excluded', () => {
  const f = fixture(); rel(f, 0x580); [0x780, 0x810, 0xa10, 0xc10].forEach(at => f.bytes.writeBigUInt64LE(target, at));
  assert.equal(run(f).candidate_count, 0);
});
check('virtual zero fill cannot serve as target', () => {
  const f = fixture(); f.u32(f.table + 8, 0x400);
  assert.throws(() => scan(f.bytes, {target: base + 0x1300n, sha256: crypto.createHash('sha256').update(f.bytes).digest('hex')}));
});
check('combined candidate limit rejects complete result', () => {
  const f = fixture(); for (let i = 0; i < 8; i++) rel(f, 0x400 + i * 8);
  for (let i = 0; i < 8; i++) f.bytes.writeBigUInt64LE(target, 0x600 + i * 8);
  assert.equal(run(f).candidate_count, 16); f.bytes.writeBigUInt64LE(target, 0x650); assert.throws(() => run(f), /candidate cap/);
});
check('real oversized buffer rejected before pin or hash inspection', () => {
  const bytes = Buffer.allocUnsafe(128 * 1024 * 1024 + 1);
  const pin = { get sha256() { throw new Error('pin inspected too early'); }, target };
  assert.throws(() => scan(bytes, pin), /file bound before hash/);
});
check('96 section ceiling rejects 97 before table access', () => {
  const f = fixture(); f.bytes.writeUInt16LE(97, 0x86);
  assert.throws(() => parse(f.bytes), /executable sections/);
});
for (const [name, edit] of [
  ['misaligned headers rejected', f => f.u32(f.op + 60, 0x3ff)],
  ['misaligned image rejected', f => f.u32(f.op + 56, 0x5001)],
  ['misaligned image base rejected', f => f.bytes.writeBigUInt64LE(base + 1n, f.op + 24)],
  ['illegal ordinary file alignment rejected', f => f.u32(f.op + 36, 1)],
  ['unmapped resource directory rejected', f => f.u32(f.op + 128, 0x2500)],
  ['resource directory in headers rejected', f => f.u32(f.op + 128, 0x100)],
]) check(name, () => { const f = fixture(); edit(f); assert.throws(() => parse(f.bytes)); });
check('executable data excluded from code scan', () => {
  const f = fixture(); f.u32(f.table + 40 + 36, 0x60000040);
  f.bytes[0x610] = 0xe8; f.bytes.writeInt32LE(0x1080 - 0x2015, 0x611);
  assert.equal(run(f).candidate_count, 0);
});
console.log(JSON.stringify({synthetic_only: true, checks}, null, 2));
if (checks.some(c => !c.pass)) process.exitCode = 1;
