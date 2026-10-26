'use strict';
// Synthetic source-only acceptance cases. Primary source pins/license references
// are preserved in ../pdb_procedure_ranges.cjs and this suite's fixture module.
// Run only by root: node --test tools/tests/pdb_procedure_ranges.test.cjs
const test = require('node:test');
const a = require('node:assert/strict');
const {createHash} = require('node:crypto');
const {inspect, LIMITS} = require('../pdb_procedure_ranges.cjs');
const {fixture, pack, proc, record, info, segmentMap, repeatedModule, repeatedContributions,
    SC60, SC2, VA} = require('./pdb_procedure_ranges_fixtures.cjs');

const query = (extra = {}) => ({addresses: [VA], ...extra});
const run = (f = fixture(), q = query()) => {
    const p = pack(f); return inspect(p.reader, p.exe, p.identity, q);
};
function fails(f, code, q = query()) {
    a.throws(() => run(f, q), error => {
        a.equal(error.code, code); a.equal(error.message, code);
        a.deepEqual(Object.keys(error), ['code']);
        a.equal(JSON.stringify(error), JSON.stringify({code}));
        return true;
    });
}
function sectionChange(f, change) {
    change(f.exe, f.table);
    f.streams[8] = Buffer.from(f.exe.subarray(f.table, f.table + 120));
    return f;
}
const dbiModuleAt = (f, i = 0) => 64 + f.modules.slice(0, i).reduce((n, m) => n + m.descriptor.length, 0);

test('four modern native kinds bind exact starts, signature offset four, with display names opaque', () => {
    for (const kind of [0x110f, 0x1110, 0x1146, 0x1147]) {
        const p = proc({kind, name: 'not a decorated public name', type: 0x12345678});
        const f = fixture({modules: [{records: [p]}]}), r = run(f);
        a.equal(r.addresses[0].procedure_record_count, 1);
        a.equal(r.addresses[0].procedure_records_sha256, createHash('sha256').update(p).digest('hex'));
        a.deepEqual(r.addresses[0].extent, {segment: 1, section_id: 'SEC01', section_offset: 0x20,
            start_rva: 0x1020, end_rva: 0x1050, size: 0x30, start: VA,
            end_inclusive: '0x14000104f', end_exclusive: '0x140001050',
            file_span: {start: 0x220, end_exclusive: 0x250}});
        a.equal(r.selected_modules[0].module_index, 0);
        a.equal(r.selected_modules[0].stream_id, 9);
        a.equal(r.symbol_bytes, 4 + p.length);
        a.equal(r.complete_selected_symbol_record_framing, true);
    }
});

test('nonfirst record, LF_PAD padding, empty name and page/cache crossing names are framed', () => {
    for (const name of ['', 'ab', Buffer.alloc(65496, 0x41)]) {
        const p = proc({name, padding: 'cv'});
        const prefix = Array.from({length: 1015}, () => record(0x0006));
        const r = run(fixture({modules: [{records: [...prefix, p]}]}));
        a.equal(r.addresses[0].procedure_record_count, 1);
        a.equal(r.records, prefix.length + 1);
        a.equal(r.opaque_records, prefix.length);
        a.equal(r.unsupported_semantic_coverage, false);
    }
});

test('automatic routing consumes full SC table, not first descriptor contribution', () => {
    const f = fixture({modules: [
        {records: [proc({offset: 0x60})]}, {records: [proc()]},
    ], contributions: [
        {module_index: 0, offset: 0x60, size: 0x10},
        {module_index: 1, offset: 0x20, size: 0x30},
    ]});
    const r = run(f);
    a.deepEqual(r.routing.indices, [1]);
    a.deepEqual(r.selected_modules.map(m => [m.module_index, m.stream_id]), [[1, 12]]);
    a.equal(r.routing.scope, 'all_matching_section_contribution_modules');
    a.equal(r.routing.complete_table_framing, true);
    a.equal(r.routing.contribution_records, 2);
    a.equal(r.routing.address_routes[0].matching_contribution_records, 1);
});

test('serialized SC imod zero means descriptor zero; one means descriptor one, never MSF stream one', () => {
    for (const index of [0, 1]) {
        const f = fixture({modules: [{records: [proc()]}, {records: [proc()]}],
            contributions: [{module_index: index}]});
        const r = run(f);
        a.deepEqual(r.routing.indices, [index]);
        a.equal(r.selected_modules[0].stream_id, index === 0 ? 9 : 12);
    }
    const f = fixture(); f.streams[3].writeUInt16LE(1, f.scAt + 4 + 16);
    fails(f, 'ERR_ROUTING_FRAMING');
});

test('both documented contribution versions use physical one-based PE section mapping', () => {
    for (const version of [SC60, SC2]) {
        const f = fixture({version, modules: [{records: [proc({segment: 2, offset: 0x40})]}],
            contributions: [{segment: 2, offset: 0x30, size: 0x30, coff_section: 999999}]});
        const r = run(f, query({addresses: ['0x140002040']}));
        a.equal(r.routing.contribution_version, version);
        a.equal(r.addresses[0].extent.section_id, 'SEC02');
        a.equal(r.addresses[0].extent.file_span.start, 0x640);
    }
    fails(fixture({contributions: [{segment: 0}]}), 'ERR_ROUTING_FRAMING');
    fails(fixture({contributions: [{segment: 4}]}), 'ERR_ROUTING_FRAMING');
});

test('optional DBI segment map must match the direct PE section profile', () => {
    for (const absolute of [false, true]) a.equal(run(fixture({sectionMap: segmentMap(absolute)})).addresses[0].procedure_record_count, 1);
    for (const mutate of [b => b.writeUInt16LE(2, 4 + 6), b => b.writeUInt32LE(1, 4 + 12),
        b => b.writeUInt16LE(0x50d, 4), b => b.writeUInt16LE(0, 2), b => b.writeUInt32LE(0x281, 4 + 16),
        b => b.writeUInt16LE(0, 4 + 3 * 20)]) {
        const sectionMap = segmentMap(); mutate(sectionMap);
        fails(fixture({sectionMap}), 'ERR_SOURCE_MAPPING');
    }
    fails(fixture({sectionMap: Buffer.concat([segmentMap(), Buffer.alloc(4)])}), 'ERR_SOURCE_MAPPING');
});

test('overlapping and duplicate folded contributions route every module and preserve contribution counts', () => {
    const f = fixture({modules: [{records: [proc({name: 'one'})]}, {records: [proc({name: 'two'})]}],
        contributions: [{module_index: 0}, {module_index: 1, offset: 0x10}, {module_index: 0}]});
    const r = run(f);
    a.deepEqual(r.routing.indices, [0, 1]);
    a.equal(r.routing.address_routes[0].matching_contribution_records, 3);
    a.equal(r.addresses[0].procedure_record_count, 2);
    a.equal(r.all_pdb_alias_coverage_confirmed, false);
    a.equal(r.unique_owner_confirmed, false);
});

test('a later conflicting declaration in any overlapping routed module fails the whole call', () => {
    const f = fixture({modules: [{records: [proc(), proc()]}, {records: [proc({size: 0x31})]}]});
    fails(f, 'ERR_PROCEDURE_CONFLICT');
    const scoped = run(f, query({module_indices: [0]}));
    a.equal(scoped.addresses[0].procedure_record_count, 2);
    a.equal(scoped.routing.scope, 'explicit_modules_only');
    a.equal(scoped.routing.complete_table_framing, false);
    a.equal(scoped.whole_pdb_coverage_confirmed, false);
});

test('thousands of agreeing declarations aggregate independently of names and type IDs', () => {
    const records = Array.from({length: 3000}, (_, i) => proc({name: 'folded alias ' + i, type: i}));
    const r = run(fixture({modules: [{records}]}));
    a.equal(r.addresses[0].procedure_record_count, 3000);
    a.equal(r.addresses[0].procedure_record_bytes, records.reduce((n, b) => n + b.length, 0));
    a.equal(r.addresses[0].procedure_records_sha256,
        createHash('sha256').update(Buffer.concat(records)).digest('hex'));
    a.ok(Buffer.byteLength(JSON.stringify(r) + '\n') < 4096);
});

test('contribution containment alone never supplies an extent; interiors and exclusive ends are absent', () => {
    fails(fixture({modules: [{records: [record()]}]}), 'ERR_PROCEDURE_ABSENT');
    for (const address of ['0x140001021', '0x14000104f', '0x140001050']) {
        fails(fixture(), 'ERR_PROCEDURE_ABSENT', query({addresses: [address]}));
    }
    fails(fixture({contributions: [{offset: 0x21, size: 0x30}]}), 'ERR_ROUTING_ABSENT');
    fails(fixture({contributions: [{size: 0}]}), 'ERR_ROUTING_ABSENT');
    fails(fixture({modules: [{records: []}]}), 'ERR_PROCEDURE_ABSENT');
});

test('explicit modules can bind with absent/unsupported routing and do not claim global coverage', () => {
    for (const f of [fixture({sc: Buffer.alloc(0)}), fixture({version: 1})]) {
        const r = run(f, query({module_indices: [0]}));
        a.equal(r.addresses[0].procedure_record_count, 1);
        a.equal(r.routing.contribution_records, null);
        a.equal(r.reference_linkage_confirmed, false);
        a.equal(r.all_pdb_alias_coverage_confirmed, false);
    }
    fails(fixture({sc: Buffer.alloc(0)}), 'ERR_ROUTING_ABSENT');
    fails(fixture({version: 1}), 'ERR_ROUTING_UNSUPPORTED');
});

test('all SC records are framed even after a hit; malformed tail and ordinal/range errors refuse', () => {
    for (const sc of [Buffer.alloc(3), Buffer.concat([Buffer.from([0, 0, 0, 0]), Buffer.alloc(4)])]) {
        const f = fixture({sc}); fails(f, sc.length < 4 ? 'ERR_PDB_FRAMING' : 'ERR_ROUTING_UNSUPPORTED');
    }
    const f = fixture(); f.streams[3].writeInt32LE(f.sc.length + 4, 28);
    f.streams[3] = Buffer.concat([f.streams[3].subarray(0, f.optionalAt), Buffer.alloc(4), f.streams[3].subarray(f.optionalAt)]);
    fails(f, 'ERR_ROUTING_FRAMING');
    for (const c of [{offset: -1}, {size: -1}, {offset: 0x7fffffff}, {size: 0x7fffffff}, {module_index: 1}]) {
        fails(fixture({contributions: [{}, c]}), 'ERR_ROUTING_FRAMING');
    }
});

test('routing scans past 4 MiB using bounded chunk reads', () => {
    const f = repeatedContributions(fixture(), 150000), p = pack(f);
    const r = inspect(p.reader, p.exe, p.identity, query());
    a.equal(r.routing.contribution_records, 150000);
    a.equal(r.routing.address_routes[0].matching_contribution_records, 150000);
    a.ok(p.reads.every(x => x.count <= LIMITS.read));
});

test('module union 32 is accepted; 33 refuses without truncation including modules after first 32', () => {
    const f32 = fixture({modules: Array.from({length: 32}, () => ({records: [proc()]}))});
    a.equal(run(f32).selected_modules.length, 32);
    fails(fixture({modules: Array.from({length: 33}, () => ({records: [proc()]}))}), 'ERR_MODULE_BOUND');
    const f = fixture({modules: Array.from({length: 33}, () => ({records: [proc()]})), contributions: [{module_index: 32}]});
    a.deepEqual(run(f).routing.indices, [32]);
});

test('malformed selectors refuse before all reader I/O, including sparse arrays and explicit undefined', () => {
    let reads = 0;
    const reader = {size: 4096, read() { reads++; throw Error('private path must never escape'); }};
    const addresses = Array.from({length: 33}, (_, i) => '0x' + (0x140001000n + BigInt(i)).toString(16));
    const bad = [undefined, null, [], {}, {extra: 1, addresses: [VA]},
        ...[undefined, null, [], [VA, VA], Array(1), [7], ['0x0140001020'], ['0X140001020'],
            ['0x14000102A'], ['140001020'], ['0x800000000000'], ['0xffff7fffffffffff'],
            ['0x10000000000000000'], addresses].map(v => ({addresses: v})),
        ...[undefined, null, [], Array(1), [0, 0], [-1], [1.1], ['0'], [65535], Array.from({length: 33}, (_, i) => i)]
            .map(v => query({module_indices: v})),
        {[Symbol('secret')]: 1, addresses: [VA]}];
    for (const q of bad) a.throws(() => inspect(reader, Buffer.alloc(0), {}, q), {code: 'ERR_SELECTORS'});
    a.equal(reads, 0);
});

test('32 addresses and 32 modules preserve query order and stay under output ceiling', () => {
    const addresses = Array.from({length: 32}, (_, i) => '0x' + (0x140001020n + BigInt(4 * i)).toString(16)).reverse();
    const records = addresses.map(v => proc({offset: Number(BigInt(v) - 0x140001000n), size: 4, name: ''}));
    const r = run(fixture({modules: Array.from({length: 32}, () => ({records}))}), query({addresses}));
    a.deepEqual(r.addresses.map(x => x.address), addresses);
    a.ok(r.addresses.every(x => x.procedure_record_count === 32));
    a.ok(Buffer.byteLength(JSON.stringify(r) + '\n') <= 65536);
});

test('selected extent size zero, overflowing code size and tail/cross-section ranges refuse atomically', () => {
    fails(fixture({modules: [{records: [proc(), proc({size: 0})]}]}), 'ERR_PROCEDURE_SIZE');
    for (const size of [0xffffffff, 0x261, 0x1000]) {
        fails(fixture({modules: [{records: [proc({size})]}]}), 'ERR_SOURCE_MAPPING');
    }
    a.equal(run(fixture({modules: [{records: [proc({size: 0x260})]}]})).addresses[0].extent.end_rva, 0x1280);
    fails(sectionChange(fixture(), (b, at) => b.writeUInt32LE(0x200, at + 16)), 'ERR_SOURCE_MAPPING',
        query({addresses: ['0x140001220']}));
    fails(sectionChange(fixture({modules: [{records: [proc({size: 0x200})]}]}),
        (b, at) => b.writeUInt32LE(0x200, at + 16)), 'ERR_SOURCE_MAPPING');
});

test('segment, section flags, resources and image overlap mapping are enforced', () => {
    for (const segment of [0, 4]) fails(fixture({modules: [{records: [proc({segment})]}]}), 'ERR_SOURCE_MAPPING');
    for (const flags of [0xe0000020, 0x20000020, 0x60000040]) {
        fails(sectionChange(fixture(), (b, at) => b.writeUInt32LE(flags, at + 36)), 'ERR_SOURCE_MAPPING');
    }
    fails(fixture(), 'ERR_SOURCE_MAPPING', query({addresses: ['0x140003020']}));
    const f = fixture(); f.exe.writeUInt32LE(0x1000, f.op + 128); f.exe.writeUInt32LE(16, f.op + 132);
    fails(f, 'ERR_SOURCE_MAPPING');
    fails(sectionChange(fixture(), (b, at) => b.writeUInt32LE(0x1000, at + 40 + 12)), 'ERR_SOURCE_MAPPING');
});

test('canonical high-half VA endpoints pass; exclusive endpoint at the canonical hole refuses', () => {
    const f = fixture({modules: [{records: [proc({offset: 0x27f, size: 1})]}],
        contributions: [{offset: 0x27f, size: 1}]});
    f.exe.writeBigUInt64LE(0xffff800000000000n, f.op + 24);
    const r = run(f, query({addresses: ['0xffff80000000127f']}));
    a.equal(r.addresses[0].extent.end_exclusive, '0xffff800000001280');
    const g = fixture({modules: [{records: [proc({offset: 0xff0, size: 0x10})]}],
        contributions: [{offset: 0xff0, size: 0x10}]});
    const old = g.exe; g.exe = Buffer.alloc(0x1600); old.copy(g.exe);
    // Move code to the last page below the canonical hole; move the other raw
    // sections to preserve disjoint complete file mappings.
    g.exe.writeBigUInt64LE(0x7fffffff0000n, g.op + 24);
    g.exe.writeUInt32LE(0xf000, g.op + 16); g.exe.writeUInt32LE(0x10000, g.op + 56);
    g.exe.writeUInt32LE(0xf000, g.table + 12); g.exe.writeUInt32LE(0x1000, g.table + 8);
    g.exe.writeUInt32LE(0x1000, g.table + 16); g.exe.writeUInt32LE(0x1200, g.table + 40 + 20);
    g.exe.writeUInt32LE(0x1400, g.table + 80 + 20);
    old.subarray(0x800, 0xa00).copy(g.exe, 0x1400); g.exe.writeUInt32LE(0x14a0, 0x1480 + 24);
    g.streams[8] = Buffer.from(g.exe.subarray(g.table, g.table + 120));
    fails(g, 'ERR_SOURCE_MAPPING', query({addresses: ['0x7ffffffffff0']}));
});

test('multi-selector errors are atomic and overlapping declarations at different starts remain scoped', () => {
    const f = fixture({modules: [{records: [proc(), proc({offset: 0x30, size: 0x20})]}]});
    const r = run(f, query({addresses: [VA, '0x140001030']}));
    a.equal(r.addresses.length, 2);
    fails(f, 'ERR_PROCEDURE_ABSENT', query({addresses: [VA, '0x140001031']}));
});

test('DBI descriptor names stay opaque; fixed framing, terminators and zero alignment are strict', () => {
    const f = fixture({modules: [{module_name: Buffer.from([0xff, 0xfe, 0x4d, 0x49, 0x4e, 0x49]),
        object_name: '/never/open/private/secret.obj'}]});
    a.equal(run(f).addresses[0].procedure_record_count, 1);
    const short = fixture({moduleInfo: Buffer.alloc(60)}); fails(short, 'ERR_DBI_MODULE_INFO');
    const names = fixture(); names.streams[3].fill(0x41, 64 + 64, names.scAt); fails(names, 'ERR_DBI_MODULE_INFO');
    const padding = fixture({modules: [{module_name: 'a', object_name: 'bb'}]});
    padding.streams[3][padding.scAt - 1] = 1; fails(padding, 'ERR_DBI_MODULE_INFO');
    const tail = fixture({moduleInfo: Buffer.concat([f.moduleInfo, Buffer.alloc(4)])}); fails(tail, 'ERR_DBI_MODULE_INFO');
});

test('DBI header, size sum, optional header and module metadata caps are checked', () => {
    for (const off of [24, 28, 32, 36, 40, 48, 52]) {
        const f = fixture(); f.streams[3].writeInt32LE(-1, off); fails(f, 'ERR_PDB_FRAMING');
    }
    const sum = fixture(); sum.streams[3].writeInt32LE(4, 52); fails(sum, 'ERR_PDB_FRAMING');
    const huge = fixture(); huge.streams[3].writeInt32LE(LIMITS.module_info + 4, 24);
    // Extend its declared stream lazily; the metadata bound precedes reading it.
    const original = huge.streams[3];
    huge.streams[3] = {size: original.length + LIMITS.module_info + 4 - huge.moduleInfo.length,
        read(o, n) { const b = Buffer.alloc(n); original.subarray(o, o + n).copy(b); return b; }};
    fails(huge, 'ERR_DBI_MODULE_INFO');
    const c = repeatedContributions(fixture(), 1, {declaredBytes: LIMITS.contributions + 4});
    fails(c, 'ERR_ROUTING_BOUND');
});

test('exact module-info and info-stream byte ceilings pass; the next byte/aligned step refuses', () => {
    const name = Buffer.alloc(LIMITS.module_info - 66, 0x41);
    const atCap = fixture({modules: [{module_name: name, object_name: ''}]});
    a.equal(atCap.moduleInfo.length, LIMITS.module_info);
    a.equal(run(atCap).addresses[0].procedure_record_count, 1);
    fails(fixture({modules: [{module_name: Buffer.concat([name, Buffer.alloc(4, 0x41)]), object_name: ''}]}), 'ERR_DBI_MODULE_INFO');
    const infoName = 'A'.repeat(LIMITS.info - 65), full = info({names: [{name: infoName, stream: 8}]});
    a.equal(full.length, LIMITS.info);
    a.equal(run(fixture({info: full})).info_profile.named_streams, 1);
    fails(fixture({info: info({names: [{name: infoName + 'A', stream: 8}]})}), 'ERR_PDB_FRAMING');
});

test('missing module streams, 0xffff, reserved role aliasing and duplicate selected streams refuse', () => {
    const absent = fixture(); absent.streams[9] = null; fails(absent, 'ERR_MODULE_BOUND');
    for (const stream of [0xffff, 0, 1, 2, 3, 4, 6, 8]) {
        const f = fixture(); f.streams[3].writeUInt16LE(stream, dbiModuleAt(f) + 34); fails(f, 'ERR_MODULE_BOUND');
    }
    const duplicate = fixture({modules: [{}, {}]});
    duplicate.streams[3].writeUInt16LE(9, dbiModuleAt(duplicate, 1) + 34); fails(duplicate, 'ERR_MODULE_BOUND');
    fails(fixture(), 'ERR_MODULE_BOUND', query({module_indices: [1]}));
});

test('module C13 signature, SymBytes including signature, C11 and exact global refs trailer are strict', () => {
    fails(fixture({modules: [{signature: 2}]}), 'ERR_MODULE_FRAMING');
    fails(fixture({modules: [{sym: 0}]}), 'ERR_MODULE_FRAMING');
    fails(fixture({modules: [{sym: proc().length}]}), 'ERR_MODULE_FRAMING');
    fails(fixture({modules: [{sym: 5}]}), 'ERR_MODULE_FRAMING');
    fails(fixture({modules: [{c11: Buffer.alloc(4), c13: Buffer.alloc(4)}]}), 'ERR_MODULE_FRAMING');
    fails(fixture({modules: [{c11: Buffer.alloc(4)}]}), 'ERR_MODULE_FRAMING');
    const f = fixture(); f.streams[9].writeUInt32LE(4, f.streams[9].length - 4); fails(f, 'ERR_MODULE_FRAMING');
    const extra = fixture(); extra.streams[9] = Buffer.concat([extra.streams[9], Buffer.alloc(4)]); fails(extra, 'ERR_MODULE_FRAMING');
    const short = fixture(); short.streams[9] = short.streams[9].subarray(0, -1); fails(short, 'ERR_MODULE_FRAMING');
    const opaque = run(fixture({modules: [{c13: Buffer.from([1, 2, 3, 4]), refs: proc({size: 0})}]}));
    a.equal(opaque.records, 1); // Opaque trailer bytes are not module symbols.
});

test('malformed symbol headers, payloads, terminators and padding after a valid hit fail', () => {
    const cases = [];
    const zero = record(); zero.writeUInt16LE(0); cases.push(zero);
    const unaligned = record(); unaligned.writeUInt16LE(3); cases.push(unaligned);
    const beyond = record(); beyond.writeUInt16LE(65535); cases.push(beyond);
    const incomplete = record(0x110f); cases.push(incomplete);
    const unterminated = proc({name: ''}); unterminated.fill(0x41, 39); cases.push(unterminated);
    const pad = proc({name: 'ab'}); pad[pad.length - 1] = 0xee; cases.push(pad);
    const longPad = Buffer.concat([proc({name: ''}), Buffer.alloc(4)]); longPad.writeUInt16LE(longPad.length - 2); cases.push(longPad);
    for (const b of cases) fails(fixture({modules: [{records: [proc(), b]}]}), 'ERR_MODULE_FRAMING');
});

test('known legacy/separate-code/thunk/managed/DPC and external forms refuse selected scope', () => {
    for (const kind of [0x0104, 0x0205, 0x0300, 0x0400, 0x100a, 0x1015, 0x101a,
        0x1102, 0x1114, 0x112a, 0x112b, 0x112c, 0x1132, 0x1148, 0x1155, 0x1156]) {
        fails(fixture({modules: [{records: [proc(), record(kind)]}]}), 'ERR_SYMBOL_UNSUPPORTED');
    }
    for (const kind of [0x115f, 0x1160, 0x1161, 0x1167]) {
        fails(fixture({modules: [{records: [proc(), record(kind)]}]}), 'ERR_FASTLINK_UNSUPPORTED');
    }
    fails(fixture({modules: [{flags: 0x100}]}), 'ERR_REFERENCE_UNSUPPORTED');
});

test('inline-site annotations stay opaque without changing supported declaration extents', () => {
    const site = record(0x114d, 20), site2 = record(0x115d, 24);
    // Synthetic parent/end/inlinee/invocation and annotation words. The reader
    // frames these records; it does not validate their scopes or annotations.
    site.writeUInt32LE(4, 4); site.writeUInt32LE(1, 12);
    site.writeUInt32LE(0x12345678, 16);
    site2.writeUInt32LE(4, 4); site2.writeUInt32LE(2, 12);
    site2.writeUInt32LE(3, 16); site2.writeUInt32LE(0x87654321, 20);
    const end = record(0x114e);
    const base = run(fixture());
    const result = run(fixture({modules: [{records: [proc(), site, end, site2, end]}]}));
    a.deepEqual(result.addresses, base.addresses);
    a.equal(result.opaque_records, 4);
    a.equal(result.supported_procedure_records, 1);
    a.equal(result.unsupported_semantic_coverage, false);
    a.equal(result.function_completeness_confirmed, false);
    a.equal(result.all_pdb_alias_coverage_confirmed, false);
});

test('unknown records and S_PROCREF are framed counters only, with semantic and reference coverage false', () => {
    const r = run(fixture({modules: [{records: [record(0x7777, 65536), record(0x1125), proc()]}]}));
    a.equal(r.records, 3); a.equal(r.opaque_records, 2);
    a.equal(r.unsupported_semantic_coverage, false); a.equal(r.reference_linkage_confirmed, false);
});

test('valid named stream maps and known feature tails, including marker bytes in opaque names', () => {
    for (const features of [[], [20091201], [20140508], [20140508, 20091201]]) {
        const r = run(fixture({info: info({names: [{name: 'MININOTM/private-name', stream: 8}], features})}));
        a.deepEqual(r.info_profile.features, features); a.equal(r.info_profile.named_streams, 1);
    }
});

test('FASTLINK, NoTypeMerge, unknown feature signatures and invalid VC110 tail refuse', () => {
    for (const feature of [0x494e494d, 0x4d544f4e]) {
        fails(fixture({info: info({features: [feature]})}), 'ERR_FASTLINK_UNSUPPORTED');
    }
    fails(fixture({info: info({features: [0x12345678]})}), 'ERR_FEATURE_UNSUPPORTED');
    fails(fixture({info: info({features: [20091201, 0x494e494d]})}), 'ERR_FEATURE_UNSUPPORTED');
    fails(fixture({info: info({features: [20140508, 20140508]})}), 'ERR_FEATURE_UNSUPPORTED');
});

test('info named-map lengths, occupancy, bit vectors, keys, stream IDs and feature framing are validated', () => {
    const variants = [];
    const mutate = fn => { const b = info({names: [{name: 'a', stream: 8}]}); fn(b); variants.push(b); };
    // One two-byte string: map header 34, present count 42, present word 46,
    // deleted count 50, key/value pair 54/58, feature word 62.
    mutate(b => b.writeUInt32LE(65536, 28));
    mutate(b => b[33] = 1);
    mutate(b => b.writeUInt32LE(0, 38));
    mutate(b => b.writeUInt32LE(0, 34));
    mutate(b => b.writeUInt32LE(4, 46)); // Present bit index 2 exceeds capacity 2.
    mutate(b => b.writeUInt32LE(2, 42));
    mutate(b => b.writeUInt32LE(1, 54)); // String interior, not a string start.
    mutate(b => b.writeUInt32LE(0xffffffff, 58));
    mutate(b => b.writeUInt32LE(2, 58)); // Absent TPI stream.
    variants.push(Buffer.concat([info(), Buffer.from([1])]));
    variants.push(info().subarray(0, 28));
    const duplicate = info({names: [{name: 'same', stream: 8}, {name: 'same', stream: 9}]}); variants.push(duplicate);
    // Present and deleted intersect (add a deleted word before the key/value).
    const base = info({names: [{name: 'a', stream: 8}]}); base.writeUInt32LE(1, 50);
    const bit = Buffer.alloc(4); bit.writeUInt32LE(1);
    variants.push(Buffer.concat([base.subarray(0, 54), bit, base.subarray(54)]));
    for (const b of variants) fails(fixture({info: b}), 'ERR_PDB_FRAMING');
});

test('identity, DBI age/machine, RSDS, section equality, absent streams and OMAP fail closed', () => {
    const fs = [];
    let f = fixture(); f.streams[1][12] ^= 1; fs.push([f, 'ERR_IDENTITY']);
    f = fixture(); f.streams[1].writeUInt32LE(2, 8); fs.push([f, 'ERR_IDENTITY']);
    f = fixture(); f.streams[3].writeUInt32LE(2, 8); fs.push([f, 'ERR_IDENTITY']);
    f = fixture(); f.streams[3].writeUInt16LE(0x14c, 58); fs.push([f, 'ERR_IDENTITY']);
    f = fixture(); f.exe[f.rsds + 4] ^= 1; fs.push([f, 'ERR_IDENTITY']);
    f = fixture(); f.exe.writeUInt32LE(2, f.rsds + 20); fs.push([f, 'ERR_IDENTITY']);
    f = fixture(); f.exe.writeUInt32LE(0, f.debug + 12); fs.push([f, 'ERR_IDENTITY']);
    f = fixture(); f.exe.writeUInt32LE(0x1020, f.op + 112 + 6 * 8); fs.push([f, 'ERR_IDENTITY']);
    f = fixture(); f.streams[8][8] ^= 1; fs.push([f, 'ERR_SOURCE_MAPPING']);
    f = fixture(); f.streams[8] = null; fs.push([f, 'ERR_PDB_FRAMING']);
    f = fixture(); f.streams[1] = null; fs.push([f, 'ERR_PDB_FRAMING']);
    f = fixture(); f.streams[3] = null; fs.push([f, 'ERR_PDB_FRAMING']);
    for (const at of [6, 8]) {
        f = fixture(); f.streams[3].writeUInt16LE(8, f.optionalAt + at); fs.push([f, 'ERR_OMAP_UNSUPPORTED']);
    }
    for (const [candidate, code] of fs) fails(candidate, code);
});

test('foreign-realm shared buffers refuse before interpreting EXE or PDB metadata', () => {
    const {runInNewContext} = require('node:vm');
    const foreign = runInNewContext('SharedArrayBuffer');
    const f = pack(fixture());
    const sharedExe = Buffer.from(new foreign(f.exe.length));
    f.exe.copy(sharedExe);
    a.equal(sharedExe.buffer instanceof SharedArrayBuffer, false);
    let reads = 0;
    const reader = {size: f.reader.size, read(o, n) { reads++; return f.reader.read(o, n); }};
    a.throws(() => inspect(reader, sharedExe, f.identity, query()), {code: 'ERR_INPUT_BOUND'});
    a.equal(reads, 0);
    const sharedReader = {size: f.reader.size, read(o, n) {
        const b = Buffer.from(new foreign(n)); f.reader.read(o, n).copy(b); return b;
    }};
    a.throws(() => inspect(sharedReader, f.exe, f.identity, query()), {code: 'ERR_INPUT_BOUND'});
});

test('shared input buffers and short/custom failed reads yield stable redacted errors', () => {
    const f = pack(fixture());
    const sharedExe = Buffer.from(new SharedArrayBuffer(f.exe.length)); f.exe.copy(sharedExe);
    a.throws(() => inspect(f.reader, sharedExe, f.identity, query()), {code: 'ERR_INPUT_BOUND'});
    const sharedReader = {size: f.reader.size, read(o, n) {
        const b = Buffer.from(new SharedArrayBuffer(n)); f.reader.read(o, n).copy(b); return b;
    }};
    a.throws(() => inspect(sharedReader, f.exe, f.identity, query()), {code: 'ERR_INPUT_BOUND'});
    for (const value of [Buffer.alloc(0), Buffer.alloc(55), new Uint8Array(56), null]) {
        a.throws(() => inspect({size: f.reader.size, read() { return value; }}, f.exe, f.identity, query()), {code: 'ERR_SHORT_READ'});
    }
    a.throws(() => inspect({size: f.reader.size, read() { throw Error('/private/save/NEVER_EXPORT'); }},
        f.exe, f.identity, query()), error => error.code === 'ERR_PDB_FRAMING' && !error.message.includes('NEVER_EXPORT'));
    let reads = 0;
    a.throws(() => inspect({size: f.reader.size, read(o, n) {
        const b = f.reader.read(o, n); reads++; return reads === 8 ? b.subarray(1) : b;
    }}, f.exe, f.identity, query()), {code: 'ERR_SHORT_READ'});
});

test('MSF superblock/directory corruption is redacted and never converted to absence', () => {
    for (const change of [p => p.superblock.writeUInt32LE(512, 32),
        p => p.directory.writeUInt32LE(5000),
        p => p.blockmap.writeUInt32LE(1),
        p => p.directory.writeUInt32LE(0xffffffff, 4 + 3 * 4)]) {
        const p = pack(fixture()); change(p);
        a.throws(() => inspect(p.reader, p.exe, p.identity, query()), {code: 'ERR_PDB_FRAMING'});
    }
});

test('aggregate symbol-byte bound is checked before reads, at the exact ceiling and one aligned step beyond', () => {
    for (const [bytes, code] of [[LIMITS.symbols, 'ERR_MODULE_FRAMING'], [LIMITS.symbols + 4, 'ERR_MODULE_BOUND']]) {
        const f = fixture({modules: [{sym: bytes, body: repeatedModule(bytes, record(), 2)}]});
        fails(f, code);
    }
    const half = LIMITS.symbols / 2;
    const f = fixture({modules: [{sym: half, body: repeatedModule(half, record(), 2)},
        {sym: half + 4, body: repeatedModule(half + 4, record(), 2)}]});
    fails(f, 'ERR_MODULE_BOUND');
});

test('exact 256 MiB aggregate symbol ceiling accepts fully framed records with bounded output', () => {
    const first = proc({name: Buffer.alloc(65492, 0x41)}); // 65,532 + signature = 65,536.
    a.equal(first.length, 65532);
    const body = repeatedModule(LIMITS.symbols, record(0x7777, 65536), 4, first);
    const f = fixture({modules: [{sym: LIMITS.symbols, body}]}), p = pack(f);
    const r = inspect(p.reader, p.exe, p.identity, query());
    a.equal(r.symbol_bytes, LIMITS.symbols); a.equal(r.records, 4096);
    a.ok(p.reads.every(x => x.count <= LIMITS.read));
    a.ok(Buffer.byteLength(JSON.stringify(r) + '\n') <= LIMITS.output);
});

test('aggregate record ceiling accepts exactly 2,500,000 and refuses the next record', () => {
    for (const count of [LIMITS.records, LIMITS.records + 1]) {
        const sym = 4 + 40 + (count - 1) * 4;
        const f = fixture({modules: [{sym, body: repeatedModule(sym)}]});
        if (count === LIMITS.records) a.equal(run(f).records, count);
        else fails(f, 'ERR_MODULE_BOUND');
    }
});

test('section-contribution record ceiling accepts exactly 2,500,000 and refuses the next record', () => {
    a.equal(run(repeatedContributions(fixture(), LIMITS.contribution_records)).routing.contribution_records, LIMITS.contribution_records);
    fails(repeatedContributions(fixture(), LIMITS.contribution_records + 1), 'ERR_ROUTING_BOUND');
});

test('LIMITS are frozen; old public API and caps stay unchanged', () => {
    a.ok(Object.isFrozen(LIMITS)); a.throws(() => { LIMITS.output = 1; }, TypeError);
    a.equal(LIMITS.record, 65537); a.equal(LIMITS.output, 65536);
    const legacy = require('../pdb_public_symbols.cjs');
    a.deepEqual(Object.keys(legacy).sort(), ['LIMITS', 'PIN', 'inspect', 'msf', 'selector']);
    a.equal(legacy.LIMITS.hits, 32); a.equal(legacy.LIMITS.output, 65536);
    a.equal(legacy.LIMITS.symbols, 256 * 1024 * 1024);
    a.equal(legacy.LIMITS.records, 2500000);
    const old = require('./pdb_public_symbols_fixtures.cjs');
    const p = old.pack(old.fixture());
    const r = legacy.inspect(p.reader, p.exe, p.identity, {
        addresses: ['0x140001020'], exact_addresses_only: true, exact_summary_only: true});
    a.equal(r.complete_symbol_record_framing, true); a.equal(r.unselected_names_exported, false);
});

test('success and errors export no names, paths, type payloads, raw records or code bytes', () => {
    const secret = 'NEVER_EXPORT_NAME_OR_PATH', f = fixture({modules: [{records: [proc({name: secret}),
        proc({offset: 0x90, name: 'UNSELECTED_ADDRESS_SECRET'})], module_name: secret, object_name: secret}]});
    f.exe.write('CODE_BYTES_MUST_NOT_BE_READ', 0x220);
    const first = run(f), json = JSON.stringify(first);
    for (const forbidden of [secret, 'UNSELECTED_ADDRESS_SECRET', 'CODE_BYTES_MUST_NOT_BE_READ',
        '0x140001090', 'type_index', 'record_bytes_hex', 'private.obj']) a.ok(!json.includes(forbidden));
    f.exe.fill(0x5a, 0x200, 0x800);
    a.deepEqual(run(f), first); // Arbitrary mapped code changes do not affect metadata binding.
    a.equal(first.code_bytes_read, false); a.equal(first.names_exported, false);
    a.equal(first.declaration_extent_only, true); a.equal(first.function_completeness_confirmed, false);
    a.equal(first.instruction_boundaries_confirmed, false); a.equal(first.purity_confirmed, false);
    a.equal(first.unwind_interpretation_confirmed, false); a.equal(first.runtime_behavior_confirmed, false);
    const bad = fixture({modules: [{records: [proc({name: secret}), proc({size: 0, name: secret})]}]});
    fails(bad, 'ERR_PROCEDURE_SIZE');
});
