'use strict';
// Synthetic source-only acceptance cases. Primary source pins/license references
// are preserved in ../pdb_procedure_ranges.cjs and this suite's fixture module.
// Run only by root: node --test tools/tests/pdb_procedure_ranges.test.cjs
const test = require('node:test');
const a = require('node:assert/strict');
const {createHash} = require('node:crypto');
const {inspect, diagnoseInfoFeatures, inspectNamedStreamNiMacZero,
    diagnoseInfoNamedStreamNiMacZero, diagnoseSourceMappingNamedStreamNiMacZero,
    diagnoseSourceMappingIdentityNamedStreamNiMacZero, diagnoseTailFrameNamedStreamNiMacZero,
    LIMITS} = require('../pdb_procedure_ranges.cjs');
const {fixture, pack, proc, record, info, namedStreamNiMacZeroInfo, segmentMap, repeatedModule, repeatedContributions,
    SC60, SC2, VA} = require('./pdb_procedure_ranges_fixtures.cjs');

const query = (extra = {}) => ({addresses: [VA], ...extra});
const run = (f = fixture(), q = query()) => {
    const p = pack(f); return inspect(p.reader, p.exe, p.identity, q);
};
function fails(f, code, q = query()) {
    redactedThrows(() => run(f, q), code);
}
function redactedThrows(call, code) {
    a.throws(call, error => {
        a.equal(error.code, code); a.equal(error.message, code);
        a.deepEqual(Object.keys(error), ['code']);
        a.equal(JSON.stringify(error), JSON.stringify({code}));
        return true;
    });
}
function diagnose(f = fixture()) {
    const p = pack(f); return diagnoseInfoFeatures(p.reader, p.identity);
}
function diagnosticResult(b, featureStart, wordCount, refusal = null) {
    return {profile: 'MSFPDB7_Info_feature_guard_diagnostic_v1', info_bytes: b.length,
        feature_start_offset: featureStart, feature_word_count: wordCount,
        status: refusal === null ? 'accepted_profile' : 'refused_profile', refusal};
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

test('explicit info diagnostics accept only the existing profiles with a closed bounded schema', () => {
    for (const names of [[], [{name: 'MININOTM/private-name', stream: 8}],
        [{name: 'MINI', stream: 8}, {name: 'NOTM', stream: 9}]]) {
        for (const features of [[], [20091201], [20140508], [20140508, 20091201]]) {
            const b = info({names, features}), result = diagnose(fixture({info: b}));
            // Empty map = 48 bytes; each name adds its NUL, pair, and the
            // nonempty present vector adds one word for these small maps.
            const start = 48 + names.reduce((n, x) => n + Buffer.byteLength(x.name) + 1 + 8, 0) +
                (names.length ? 4 : 0);
            a.deepEqual(result, diagnosticResult(b, start, features.length));
            a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024);
            a.ok(!JSON.stringify(result).includes('private-name'));
        }
    }
});

test('diagnostics identify every first feature guard with exact unsigned words and offset arithmetic', () => {
    const vc110 = 20091201, vc140 = 20140508, mini = 0x494e494d, notm = 0x4d544f4e;
    const cases = [
        [[0xffffffff, mini], 0, 'ERR_FEATURE_UNSUPPORTED', 'unknown_signature'],
        [[vc140, 0, notm], 1, 'ERR_FEATURE_UNSUPPORTED', 'unknown_signature'],
        [[vc140, vc140, mini], 1, 'ERR_FEATURE_UNSUPPORTED', 'duplicate_vc140'],
        [[vc140, vc140, vc110], 1, 'ERR_FEATURE_UNSUPPORTED', 'duplicate_vc140'],
        [[mini, vc140, notm], 0, 'ERR_FASTLINK_UNSUPPORTED', 'minimal_debug_info'],
        [[vc140, mini, vc110], 1, 'ERR_FASTLINK_UNSUPPORTED', 'minimal_debug_info'],
        [[notm, mini], 0, 'ERR_FASTLINK_UNSUPPORTED', 'no_type_merge'],
        [[vc140, notm, mini], 1, 'ERR_FASTLINK_UNSUPPORTED', 'no_type_merge'],
    ];
    for (const prefix of [[], [vc140]]) for (const suffix of [[vc110], [vc140], [0], [mini], [notm],
        [mini, notm, 0xffffffff]]) {
        cases.push([[...prefix, vc110, ...suffix], prefix.length, 'ERR_FEATURE_UNSUPPORTED', 'vc110_nonterminal']);
    }
    for (const [features, index, code, reason] of cases) {
        const b = info({features}), f = fixture({info: b}), result = diagnose(f);
        a.deepEqual(result, diagnosticResult(b, 48, features.length, {code, reason,
            word_index: index, word_offset: 48 + index * 4, word_u32: features[index],
            remaining_words: features.length - index - 1}));
        a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024);
        fails(f, code); // Ordinary API still throws only its original code.
        fails(f, code, query({module_indices: [0]}));
    }
});

test('diagnostic feature walking never interprets any word after its first refusal', () => {
    const readWord = Buffer.prototype.readUInt32LE;
    for (const prefix of [[0xffffffff], [0x494e494d], [0x4d544f4e],
        [20140508, 20140508], [20140508, 20091201]]) {
        const features = [...prefix, ...Array(6 - prefix.length).fill(0x12345678)];
        const b = info({features}), p = pack(fixture({info: b})), visited = [];
        // This fixture's 72-byte info buffer is distinct from its 56-byte
        // superblock read, 60-byte directory, and four-byte block map.
        a.equal(b.length, 72); a.equal(p.directory.length, 60);
        Buffer.prototype.readUInt32LE = function(offset = 0) {
            if (this.length === b.length && offset >= 48) {
                a.ok(offset < 48 + prefix.length * 4, 'feature suffix must remain uninterpreted');
                visited.push(offset);
            }
            return readWord.call(this, offset);
        };
        try { a.equal(diagnoseInfoFeatures(p.reader, p.identity).status, 'refused_profile'); }
        finally { Buffer.prototype.readUInt32LE = readWord; }
        a.deepEqual(visited, prefix.map((_, i) => 48 + i * 4));
    }
});

test('accepted and refused diagnostics read only MSF framing and info, even with invalid later streams', () => {
    for (const [features, status] of [[[], 'accepted_profile'], [[20140508, 20091201], 'accepted_profile'],
        [[20140508, 20140508], 'refused_profile'], [[20091201, 0x494e494d], 'refused_profile'],
        [[0x4d544f4e], 'refused_profile']]) {
        const f = fixture({info: info({names: [{name: 'NEVER_EXPORT_NAME', stream: 9}], features})});
        for (const index of [2, 4, 5, 6, 7]) f.streams[index] = Buffer.from('NEVER_EXPORT_OTHER_STREAM');
        const p = pack(f), allowed = new Set([0, 3, 4, ...p.entries[1].pages]);
        for (const index of [3, 8, 9]) f.streams[index].fill(0xff);
        const reader = {size: p.reader.size, read(o, n) {
            a.ok(n <= LIMITS.read);
            for (let page = Math.floor(o / 4096); page <= Math.floor((o + n - 1) / 4096); page++) {
                a.ok(allowed.has(page), 'diagnostic may not read DBI/module/symbol/type/section streams');
            }
            return p.reader.read(o, n);
        }};
        const result = diagnoseInfoFeatures(reader, p.identity), json = JSON.stringify(result);
        a.equal(result.status, status);
        for (const secret of ['NEVER_EXPORT_NAME', 'NEVER_EXPORT_OTHER_STREAM', p.identity.guid_bytes,
            'selected_modules', 'records', 'extent']) {
            a.ok(!json.includes(secret));
        }
        a.ok(p.reads.some(r => p.entries[1].pages.includes(Math.floor(r.offset / 4096))));
    }
    const f = fixture(); f.streams[3] = null; f.streams[8] = null; f.streams[9] = null;
    a.equal(diagnose(f).status, 'accepted_profile');
    fails(f, 'ERR_PDB_FRAMING');
});

test('diagnostic offsets remain info-relative across MSF pages and at the info ceiling', () => {
    for (const bytes of [4094, 4096, 4098, LIMITS.info]) {
        const b = info({names: [{name: 'A'.repeat(bytes - 69), stream: 8}],
            features: [20140508, 0xffffffff]});
        a.equal(b.length, bytes);
        const result = diagnose(fixture({info: b}));
        a.deepEqual(result, diagnosticResult(b, bytes - 8, 2, {code: 'ERR_FEATURE_UNSUPPORTED',
            reason: 'unknown_signature', word_index: 1, word_offset: bytes - 4,
            word_u32: 0xffffffff, remaining_words: 0}));
    }
    // Many framed words are counted without walking the suffix after VC110.
    const features = [20091201, ...Array((LIMITS.info - 48) / 4 - 1).fill(0x494e494d)];
    const b = info({features}); a.equal(b.length, LIMITS.info);
    a.deepEqual(diagnose(fixture({info: b})), diagnosticResult(b, 48, features.length, {
        code: 'ERR_FEATURE_UNSUPPORTED', reason: 'vc110_nonterminal', word_index: 0,
        word_offset: 48, word_u32: 20091201, remaining_words: features.length - 1}));
    const full = info({names: [{name: 'A'.repeat(LIMITS.info - 65), stream: 8}]});
    a.equal(full.length, LIMITS.info); a.equal(diagnose(fixture({info: full})).status, 'accepted_profile');
    redactedThrows(() => diagnose(fixture({info: Buffer.concat([full, Buffer.alloc(1)])})), 'ERR_PDB_FRAMING');
});

test('diagnostics enforce identity and reader bounds before I/O with unchanged code-only errors', () => {
    const p = pack(fixture()); let reads = 0;
    const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
    for (const identity of [undefined, null, {}, {guid_bytes: p.identity.guid_bytes, age: 0},
        {...p.identity, age: 0x100000000}, {...p.identity, age: 1.5},
        {...p.identity, guid_bytes: p.identity.guid_bytes.toUpperCase()}]) {
        redactedThrows(() => diagnoseInfoFeatures(reader, identity), 'ERR_IDENTITY');
    }
    for (const input of [null, {}, {size: 56}, ...[0, 55, LIMITS.pdb + 1, NaN, Infinity, 56.5]
        .map(size => ({...reader, size}))]) {
        redactedThrows(() => diagnoseInfoFeatures(input, p.identity), 'ERR_INPUT_BOUND');
    }
    a.equal(reads, 0);
    for (const off of [0, 8, 12]) {
        const f = fixture(); f.streams[1][off] ^= 1;
        redactedThrows(() => diagnose(f), 'ERR_IDENTITY');
    }
    for (const size of [56, LIMITS.pdb]) {
        redactedThrows(() => diagnoseInfoFeatures({size, read() {
            reads++; throw Error('/private/NEVER_EXPORT');
        }}, p.identity), 'ERR_PDB_FRAMING');
    }
    a.equal(reads, 2); // Both inclusive reader size bounds reach the first read.
});

test('diagnostics retain structural MSF and named-map bounds as framing errors', () => {
    for (const change of [p => p.superblock.writeUInt32LE(512, 32),
        p => p.superblock.writeUInt32LE(LIMITS.directory + 4, 44),
        p => p.directory.writeUInt32LE(LIMITS.streams + 1),
        p => p.blockmap.writeUInt32LE(1),
        p => p.directory.writeUInt32LE(0xffffffff, 4 + 1 * 4)]) {
        const p = pack(fixture()); change(p);
        redactedThrows(() => diagnoseInfoFeatures(p.reader, p.identity), 'ERR_PDB_FRAMING');
    }
    for (const change of [b => b.writeUInt32LE(LIMITS.info * 8 + 1, 38),
        b => b.writeUInt32LE(4, 34)]) {
        const b = info({names: [{name: 'a', stream: 8}], features: [0x494e494d]}); change(b);
        redactedThrows(() => diagnose(fixture({info: b})), 'ERR_PDB_FRAMING');
    }
});

test('diagnostic shared and foreign-realm shared reads, short reads and reader exceptions remain redacted', () => {
    const {runInNewContext} = require('node:vm'), foreign = runInNewContext('SharedArrayBuffer');
    const p = pack(fixture());
    for (const Shared of [SharedArrayBuffer, foreign]) for (const infoOnly of [false, true]) {
        const reader = {size: p.reader.size, read(o, n) {
            const source = p.reader.read(o, n);
            if (infoOnly && !p.entries[1].pages.includes(Math.floor(o / 4096))) return source;
            const b = Buffer.from(new Shared(n)); source.copy(b); return b;
        }};
        redactedThrows(() => diagnoseInfoFeatures(reader, p.identity), 'ERR_INPUT_BOUND');
    }
    for (const infoOnly of [false, true]) for (const value of [Buffer.alloc(0), Buffer.alloc(55),
        new Uint8Array(56), null, 'throw']) {
        const reader = {size: p.reader.size, read(o, n) {
            if (infoOnly && !p.entries[1].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
            if (value === 'throw') throw Error('/private/NEVER_EXPORT');
            return value;
        }};
        redactedThrows(() => diagnoseInfoFeatures(reader, p.identity),
            value === 'throw' ? 'ERR_PDB_FRAMING' : 'ERR_SHORT_READ');
    }
});

test('diagnostics copy exact reader results before another read can reuse their backing storage', () => {
    const f = fixture({info: info({names: [{name: 'A'.repeat(4100), stream: 8}],
        features: [20140508, 20091201]})});
    while (f.streams.length < 1500) f.streams.push(null); // Two directory pages retain the block map.
    const p = pack(f), scratch = Buffer.alloc(4096);
    a.ok(p.directory.length > 4096);
    const reader = {size: p.reader.size, read(o, n) {
        a.ok(n <= scratch.length && n <= LIMITS.read);
        scratch.fill(0); p.reader.read(o, n).copy(scratch); return scratch.subarray(0, n);
    }};
    a.deepEqual(diagnoseInfoFeatures(reader, p.identity), diagnoseInfoFeatures(p.reader, p.identity));
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
    for (const length of [1, 2, 3]) {
        variants.push(Buffer.concat([info({features: [0x494e494d]}), Buffer.alloc(length)]));
    }
    variants.push(info().subarray(0, 28));
    const duplicate = info({names: [{name: 'same', stream: 8}, {name: 'same', stream: 9}]}); variants.push(duplicate);
    // Present and deleted intersect (add a deleted word before the key/value).
    const base = info({names: [{name: 'a', stream: 8}]}); base.writeUInt32LE(1, 50);
    const bit = Buffer.alloc(4); bit.writeUInt32LE(1);
    variants.push(Buffer.concat([base.subarray(0, 54), bit, base.subarray(54)]));
    for (const b of variants) {
        fails(fixture({info: b}), 'ERR_PDB_FRAMING');
        redactedThrows(() => diagnose(fixture({info: b})), 'ERR_PDB_FRAMING');
    }
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

// Opt-in structural NI grammar. All values below are authored synthetic data;
// the 136-byte layout does not identify any original input's successor word.
const NI_PROFILE = 'MSF7_DBI_C13_scoped_supported_procedure_declarations_named_stream_ni_mac_zero_v1';
const VC110 = 20091201, VC140 = 20140508, MINI = 0x494e494d, NOTM = 0x4d544f4e;
const niFixture = (options = {}) => fixture({info: namedStreamNiMacZeroInfo(), ...options});
const runNi = (f = niFixture(), q = query()) => {
    const p = pack(f); return inspectNamedStreamNiMacZero(p.reader, p.exe, p.identity, q);
};
const diagnoseNi = (f = niFixture()) => {
    const p = pack(f); return diagnoseInfoNamedStreamNiMacZero(p.reader, p.identity);
};
function niDiagnosticResult(b, mapEnd, wordCount, refusal = null) {
    return {profile: 'MSFPDB7_Info_named_stream_ni_mac_zero_diagnostic_v1', info_bytes: b.length,
        named_map_end_offset: mapEnd, ni_mac_offset: mapEnd, named_stream_ni_mac: 0,
        feature_start_offset: mapEnd + 4, feature_word_count: wordCount,
        status: refusal === null ? 'accepted_profile' : 'refused_profile', refusal};
}
function niFails(f, code) {
    redactedThrows(() => diagnoseNi(f), code);
    for (const q of [query(), query({module_indices: [0]})]) redactedThrows(() => runNi(f, q), code);
}

test('NI grammar accepts exactly four suffixes with only full-result profile metadata changed', () => {
    for (const names of [[], [{name: 'MININOTM/opaque/path', stream: 8}],
        [{name: 'MINI', stream: 8}, {name: 'NOTM', stream: 9}]]) {
        for (const features of [[], [VC140], [VC110], [VC140, VC110]]) {
            for (const q of [query(), query({module_indices: [1, 0]})]) {
                const modules = [{records: [proc(), proc({name: 'second alias'})]},
                    {records: [proc({kind: 0x1147}), record()]}];
                const old = run(fixture({info: info({names, features}), modules}), q);
                const b = namedStreamNiMacZeroInfo({names, features});
                const result = runNi(niFixture({info: b, modules}), q);
                a.deepEqual(result, {...old, profile: NI_PROFILE, info_profile: {
                    grammar: 'named_stream_ni_mac_zero_v1', named_stream_ni_mac: 0,
                    named_streams: names.length, features,
                }});
                a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= LIMITS.output);
                const mapEnd = b.length - 4 - features.length * 4;
                a.deepEqual(diagnoseNi(niFixture({info: b})), niDiagnosticResult(b, mapEnd, features.length));
                // The legacy APIs retain their original first-zero refusal.
                const legacy = diagnose(niFixture({info: b}));
                a.deepEqual(legacy, diagnosticResult(b, mapEnd, features.length + 1, {
                    code: 'ERR_FEATURE_UNSUPPORTED', reason: 'unknown_signature', word_index: 0,
                    word_offset: mapEnd, word_u32: 0, remaining_words: features.length,
                }));
                fails(niFixture({info: b, modules}), 'ERR_FEATURE_UNSUPPORTED', q);
            }
        }
    }
    // Ordinary success still has exactly the old Info metadata, with no NI guess.
    a.deepEqual(run().info_profile, {named_streams: 0, features: [VC140]});
});

test('NI diagnostic explicitly separates map end 128, NI 128 and synthetic feature start 132', () => {
    const b = namedStreamNiMacZeroInfo({names: [{name: 'A'.repeat(67), stream: 8}], features: [VC140]});
    a.equal(b.length, 136);
    a.equal(b.readUInt32LE(128), 0); a.equal(b.readUInt32LE(132), VC140);
    a.deepEqual(diagnoseNi(niFixture({info: b})), niDiagnosticResult(b, 128, 1));
    a.deepEqual(runNi(niFixture({info: b})).info_profile, {
        grammar: 'named_stream_ni_mac_zero_v1', named_stream_ni_mac: 0, named_streams: 1, features: [VC140],
    });
});

test('NI writer-shaped maps retain sparse and deleted buckets and variable vector extents', () => {
    const names = [{name: 'MINI', stream: 8}, {name: 'NOTM', stream: 9}];
    const variants = [
        {names, capacity: 96, present: [1, 65], deleted: [40], deletedWords: 3},
        {names, capacity: 96, present: [1, 2], presentWords: 3, deleted: [65]},
        {names, capacity: 96, present: [31, 32], deleted: [], deletedWords: 0},
        {names: [], capacity: 96, present: [], presentWords: 3, deleted: [1, 65]},
        {names: [], capacity: LIMITS.info * 8, present: [], deleted: []},
        {names: Array.from({length: 33}, (_, i) => ({name: 'entry' + i, stream: 8}))},
    ];
    for (const options of variants) {
        const b = namedStreamNiMacZeroInfo({...options, features: [VC140, VC110]});
        const f = niFixture({info: b});
        a.deepEqual(diagnoseNi(f), niDiagnosticResult(b, b.length - 12, 2));
        a.equal(runNi(f).info_profile.named_streams, options.names.length);
    }
});

test('NI is mandatory, complete and zero; nonzero known signatures are never features', () => {
    for (const niBytes of [0, 1, 2, 3]) {
        niFails(niFixture({info: namedStreamNiMacZeroInfo({features: [], niBytes})}), 'ERR_PDB_FRAMING');
    }
    for (const niMac of [1, 0x12345678, 0xffffffff, VC110, VC140, MINI, NOTM]) {
        for (const features of [[], [VC140], [MINI], [VC110, NOTM]]) {
            niFails(niFixture({info: namedStreamNiMacZeroInfo({niMac, features})}), 'ERR_NI_MAC_UNSUPPORTED');
        }
    }
    // An ordinary known feature is not retried or silently shifted to NI.
    niFails(fixture(), 'ERR_NI_MAC_UNSUPPORTED');
});

test('NI feature diagnostics keep exact first-refusal ordering and actual suffix offsets', () => {
    const cases = [];
    for (const prefix of [[], [VC140]]) {
        for (const unknown of [0, 1, 0x12345678, 0xffffffff]) {
            cases.push([[...prefix, unknown, MINI, NOTM], prefix.length,
                'ERR_FEATURE_UNSUPPORTED', 'unknown_signature']);
        }
        for (const [word, reason] of [[MINI, 'minimal_debug_info'], [NOTM, 'no_type_merge']]) {
            cases.push([[...prefix, word, VC140, 0], prefix.length, 'ERR_FASTLINK_UNSUPPORTED', reason]);
        }
        for (const suffix of [[VC110], [VC140], [0], [1], [0x12345678], [0xffffffff], [MINI], [NOTM],
            [MINI, NOTM, 0], [VC140, VC110]]) {
            cases.push([[...prefix, VC110, ...suffix], prefix.length,
                'ERR_FEATURE_UNSUPPORTED', 'vc110_nonterminal']);
        }
    }
    for (const suffix of [[], [0], [MINI], [NOTM], [VC110], [0xffffffff]]) {
        cases.push([[VC140, VC140, ...suffix], 1, 'ERR_FEATURE_UNSUPPORTED', 'duplicate_vc140']);
    }
    cases.push([[0, VC140], 0, 'ERR_FEATURE_UNSUPPORTED', 'unknown_signature']);
    cases.push([[VC140, 0], 1, 'ERR_FEATURE_UNSUPPORTED', 'unknown_signature']);
    for (const [features, index, code, reason] of cases) {
        const b = namedStreamNiMacZeroInfo({features}), f = niFixture({info: b});
        a.deepEqual(diagnoseNi(f), niDiagnosticResult(b, 48, features.length, {code, reason,
            word_index: index, word_offset: 52 + index * 4, word_u32: features[index],
            remaining_words: features.length - index - 1}));
        for (const q of [query(), query({module_indices: [0]})]) redactedThrows(() => runNi(f, q), code);
    }
});

test('NI first refusals and nonzero NI never interpret their successors', () => {
    const readWord = Buffer.prototype.readUInt32LE;
    for (const prefix of [[0], [1], [0xffffffff], [MINI], [NOTM], [VC140, VC140],
        [VC110], [VC140, VC110]]) {
        for (const successor of [0, 1, 0x12345678, 0xffffffff, VC110, VC140, MINI, NOTM]) {
            const features = [...prefix, ...Array(6 - prefix.length).fill(successor)];
            const b = namedStreamNiMacZeroInfo({features}), f = niFixture({info: b});
            for (const full of [false, true]) {
                const p = pack(f), visited = [];
                a.equal(b.length, 76); a.equal(p.directory.length, 60);
                Buffer.prototype.readUInt32LE = function(offset = 0) {
                    if (this.length === b.length && offset >= 52) {
                        a.ok(offset < 52 + prefix.length * 4, 'refused successor must remain uninterpreted');
                        visited.push(offset);
                    }
                    return readWord.call(this, offset);
                };
                try {
                    if (full) redactedThrows(() => inspectNamedStreamNiMacZero(p.reader, p.exe, p.identity, query()),
                        prefix[0] === MINI || prefix[0] === NOTM ? 'ERR_FASTLINK_UNSUPPORTED' : 'ERR_FEATURE_UNSUPPORTED');
                    else a.equal(diagnoseInfoNamedStreamNiMacZero(p.reader, p.identity).status, 'refused_profile');
                } finally { Buffer.prototype.readUInt32LE = readWord; }
                a.deepEqual(visited, prefix.map((_, i) => 52 + i * 4));
            }
        }
    }
    const b = namedStreamNiMacZeroInfo({niMac: VC140, features: Array(6).fill(NOTM)});
    const p = pack(niFixture({info: b}));
    Buffer.prototype.readUInt32LE = function(offset = 0) {
        if (this.length === b.length) a.ok(offset < 52, 'nonzero NI must not enter feature walking');
        return readWord.call(this, offset);
    };
    try { redactedThrows(() => diagnoseInfoNamedStreamNiMacZero(p.reader, p.identity), 'ERR_NI_MAC_UNSUPPORTED'); }
    finally { Buffer.prototype.readUInt32LE = readWord; }
});

test('NI malformed whole tails take priority over NI or feature interpretation', () => {
    for (const extra of [1, 2, 3]) for (const niMac of [0, VC140, MINI]) {
        for (const features of [[], [MINI], [VC110, NOTM], [VC140, VC140], [0xffffffff]]) {
            niFails(niFixture({info: namedStreamNiMacZeroInfo({niMac, features, extraTail: Buffer.alloc(extra)})}),
                'ERR_PDB_FRAMING');
        }
    }
});

test('NI named-map validation precedes structural NI and feature errors', () => {
    const variants = [];
    const mutate = change => {
        const b = namedStreamNiMacZeroInfo({names: [{name: 'a', stream: 8}], niMac: VC140, features: [MINI]});
        change(b); variants.push(b);
    };
    // Strings end at 34; present vector at 42/46, deleted at 50, pair
    // at 54/58, NI at 62. No alignment padding is inserted in this grammar.
    for (const change of [b => b.writeUInt32LE(65536, 28), b => b[33] = 1,
        b => b.writeUInt32LE(0, 38), b => b.writeUInt32LE(LIMITS.info * 8 + 1, 38),
        b => b.writeUInt32LE(0, 34), b => b.writeUInt32LE(4, 34),
        b => b.writeUInt32LE(4, 46), b => b.writeUInt32LE(2, 42),
        b => b.writeUInt32LE(1, 54), b => b.writeUInt32LE(0xffffffff, 58),
        b => b.writeUInt32LE(2, 58)]) mutate(change);
    variants.push(namedStreamNiMacZeroInfo({names: [{name: 'same', stream: 8}, {name: 'same', stream: 9}]}));
    const duplicateKey = namedStreamNiMacZeroInfo({names: [{name: 'a', stream: 8}, {name: 'b', stream: 9}]});
    duplicateKey.writeUInt32LE(0, 64); // Second pair key, following a four-byte string buffer.
    variants.push(duplicateKey);
    for (const mask of [1, 4]) {
        const b = namedStreamNiMacZeroInfo({names: [{name: 'a', stream: 8}], features: [MINI]});
        b.writeUInt32LE(1, 50);
        const deleted = Buffer.alloc(4); deleted.writeUInt32LE(mask);
        variants.push(Buffer.concat([b.subarray(0, 54), deleted, b.subarray(54)]));
    }
    variants.push(namedStreamNiMacZeroInfo().subarray(0, 28));
    variants.push(namedStreamNiMacZeroInfo().subarray(0, 47));
    for (const b of variants) niFails(niFixture({info: b}), 'ERR_PDB_FRAMING');
});

test('NI and feature words cross MSF pages at their exact logical offsets', () => {
    for (const mapEnd of [4090, 4092, 4094, 4096, 4098]) {
        const b = namedStreamNiMacZeroInfo({names: [{name: 'A'.repeat(mapEnd - 61), stream: 8}],
            features: [VC140, 0xffffffff]});
        a.equal(b.length, mapEnd + 12);
        const f = niFixture({info: b});
        a.deepEqual(diagnoseNi(f), niDiagnosticResult(b, mapEnd, 2, {
            code: 'ERR_FEATURE_UNSUPPORTED', reason: 'unknown_signature', word_index: 1,
            word_offset: mapEnd + 8, word_u32: 0xffffffff, remaining_words: 0,
        }));
        const accepted = namedStreamNiMacZeroInfo({names: [{name: 'A'.repeat(mapEnd - 61), stream: 8}],
            features: [VC140, VC110]});
        a.equal(runNi(niFixture({info: accepted})).addresses[0].procedure_record_count, 1);
    }
});

test('NI Info ceiling is inclusive and extra zeros or unknowns never escape it', () => {
    for (const features of [[], [VC140], [VC110], [VC140, VC110]]) {
        const b = namedStreamNiMacZeroInfo({names: [{name: 'A'.repeat(LIMITS.info - 65 - features.length * 4), stream: 8}],
            features});
        a.equal(b.length, LIMITS.info);
        a.deepEqual(diagnoseNi(niFixture({info: b})), niDiagnosticResult(b, b.length - 4 - features.length * 4, features.length));
        a.equal(runNi(niFixture({info: b})).info_profile.named_streams, 1);
        for (const extra of [1, 4]) {
            niFails(niFixture({info: Buffer.concat([b, Buffer.alloc(extra)])}), 'ERR_PDB_FRAMING');
        }
    }
    // 52 bytes before actual features for an empty map and structural NI.
    const count = (LIMITS.info - 52) / 4;
    for (const [first, reason] of [[0, 'unknown_signature'], [0xffffffff, 'unknown_signature'],
        [VC110, 'vc110_nonterminal']]) {
        const features = [first, ...Array(count - 1).fill(NOTM)];
        const b = namedStreamNiMacZeroInfo({features}); a.equal(b.length, LIMITS.info);
        const result = diagnoseNi(niFixture({info: b}));
        a.deepEqual(result, niDiagnosticResult(b, 48, count, {code: 'ERR_FEATURE_UNSUPPORTED', reason,
            word_index: 0, word_offset: 52, word_u32: first, remaining_words: count - 1}));
        a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024);
        redactedThrows(() => runNi(niFixture({info: b})), 'ERR_FEATURE_UNSUPPORTED');
    }
    // A four-byte NI addition cannot quietly expand a legacy ceiling fixture.
    const name = 'A'.repeat(LIMITS.info - 65);
    a.equal(info({names: [{name, stream: 8}]}).length, LIMITS.info);
    niFails(niFixture({info: namedStreamNiMacZeroInfo({names: [{name, stream: 8}]})}), 'ERR_PDB_FRAMING');
});

test('NI diagnostic is Info-only and NI failures precede every DBI or module read for both selectors', () => {
    const cases = [
        [{features: []}, 'accepted_profile', null],
        [{features: [VC140, VC110]}, 'accepted_profile', null],
        [{features: [MINI]}, 'refused_profile', 'ERR_FASTLINK_UNSUPPORTED'],
        [{features: [VC110, NOTM]}, 'refused_profile', 'ERR_FEATURE_UNSUPPORTED'],
        [{features: [0]}, 'refused_profile', 'ERR_FEATURE_UNSUPPORTED'],
        [{niMac: VC140}, null, 'ERR_NI_MAC_UNSUPPORTED'],
        [{features: [], niBytes: 0}, null, 'ERR_PDB_FRAMING'],
        [{features: [MINI], extraTail: Buffer.alloc(1)}, null, 'ERR_PDB_FRAMING'],
    ];
    for (const [options, status, code] of cases) {
        const f = niFixture({info: namedStreamNiMacZeroInfo({
            names: [{name: 'NEVER_EXPORT_MININOTM/private/path', stream: 9}], ...options})});
        for (const index of [2, 4, 5, 6, 7]) f.streams[index] = Buffer.from('NEVER_EXPORT_OTHER_STREAM');
        const p = pack(f), allowed = new Set([0, 3, 4, ...p.entries[1].pages]);
        for (const index of [3, 8, 9]) f.streams[index].fill(0xff);
        const reader = {size: p.reader.size, read(o, n) {
            a.ok(n <= LIMITS.read);
            for (let page = Math.floor(o / 4096); page <= Math.floor((o + n - 1) / 4096); page++) {
                a.ok(allowed.has(page), 'only MSF and Info reads are permitted here');
            }
            return p.reader.read(o, n);
        }};
        if (status === null) redactedThrows(() => diagnoseInfoNamedStreamNiMacZero(reader, p.identity), code);
        else {
            const result = diagnoseInfoNamedStreamNiMacZero(reader, p.identity), json = JSON.stringify(result);
            a.equal(result.status, status);
            a.deepEqual(Object.keys(result), ['profile', 'info_bytes', 'named_map_end_offset', 'ni_mac_offset',
                'named_stream_ni_mac', 'feature_start_offset', 'feature_word_count', 'status', 'refusal']);
            for (const secret of ['NEVER_EXPORT', p.identity.guid_bytes, 'age', 'features', 'selected_modules',
                'records', 'extent', 'raw', 'symbol', 'export', 'body']) a.ok(!json.includes(secret));
            a.ok(Buffer.byteLength(json + '\n') <= 1024);
        }
        if (code !== null) for (const q of [query(), query({module_indices: [0]})]) {
            redactedThrows(() => inspectNamedStreamNiMacZero(reader, p.exe, p.identity, q), code);
        }
    }
    const f = niFixture(); f.streams[3] = null; f.streams[8] = null; f.streams[9] = null;
    a.equal(diagnoseNi(f).status, 'accepted_profile');
    redactedThrows(() => runNi(f), 'ERR_PDB_FRAMING');
});

test('NI selectors remain closed and malformed selectors precede reader I/O', () => {
    let reads = 0;
    const reader = {size: 4096, read() { reads++; throw Error('/private/NEVER_EXPORT'); }};
    const bad = [undefined, null, [], {}, query({grammar: 'named_stream_ni_mac_zero_v1'}),
        query({named_stream_ni_mac: 0}), query({exact_addresses_only: true}),
        {[Symbol('private')]: 0, addresses: [VA]}];
    for (const addresses of [undefined, null, [], Array(1), [VA, VA], [7], ['0X140001020'],
        ['0x0140001020'], ['0x800000000000'], Array(33).fill(VA)]) bad.push({addresses});
    for (const module_indices of [undefined, null, [], Array(1), [0, 0], [-1], [1.5], ['0'], [65535],
        Array.from({length: 33}, (_, i) => i)]) bad.push(query({module_indices}));
    for (const q of bad) {
        redactedThrows(() => inspectNamedStreamNiMacZero(reader, Buffer.alloc(0), {}, q), 'ERR_SELECTORS');
        redactedThrows(() => inspect(reader, Buffer.alloc(0), {}, q), 'ERR_SELECTORS');
    }
    a.equal(reads, 0);
});

test('NI diagnostic identity, MSF, directory, stream and input caps keep redacted legacy errors', () => {
    const p = pack(niFixture()); let reads = 0;
    const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
    for (const identity of [undefined, null, {}, {...p.identity, age: 0}, {...p.identity, age: 1.5},
        {...p.identity, age: 0x100000000}, {...p.identity, guid_bytes: p.identity.guid_bytes.toUpperCase()}]) {
        redactedThrows(() => diagnoseInfoNamedStreamNiMacZero(reader, identity), 'ERR_IDENTITY');
    }
    for (const input of [null, {}, {size: 56}, ...[0, 55, LIMITS.pdb + 1, NaN, Infinity, 56.5]
        .map(size => ({...reader, size}))]) {
        redactedThrows(() => diagnoseInfoNamedStreamNiMacZero(input, p.identity), 'ERR_INPUT_BOUND');
    }
    a.equal(reads, 0);
    for (const size of [56, LIMITS.pdb]) redactedThrows(() => diagnoseInfoNamedStreamNiMacZero({size,
        read() { reads++; throw Error('/private/NEVER_EXPORT'); }}, p.identity), 'ERR_PDB_FRAMING');
    a.equal(reads, 2);
    for (const offset of [0, 8, 12]) {
        const f = niFixture(); f.streams[1][offset] ^= 1; niFails(f, 'ERR_IDENTITY');
    }
    for (const change of [p => p.superblock.writeUInt32LE(512, 32),
        p => p.superblock.writeUInt32LE(LIMITS.directory + 4, 44),
        p => p.directory.writeUInt32LE(LIMITS.streams + 1), p => p.blockmap.writeUInt32LE(1),
        p => p.directory.writeUInt32LE(0xffffffff, 8)]) {
        const p = pack(niFixture()); change(p);
        redactedThrows(() => diagnoseInfoNamedStreamNiMacZero(p.reader, p.identity), 'ERR_PDB_FRAMING');
        redactedThrows(() => inspectNamedStreamNiMacZero(p.reader, p.exe, p.identity, query()), 'ERR_PDB_FRAMING');
    }
});

test('NI APIs reject local and foreign shared storage and preserve short-read and exception redaction', () => {
    const {runInNewContext} = require('node:vm'), foreign = runInNewContext('SharedArrayBuffer');
    const p = pack(niFixture());
    for (const Shared of [SharedArrayBuffer, foreign]) {
        const exe = Buffer.from(new Shared(p.exe.length)); p.exe.copy(exe);
        let reads = 0;
        const untouched = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
        redactedThrows(() => inspectNamedStreamNiMacZero(untouched, exe, p.identity, query()), 'ERR_INPUT_BOUND');
        a.equal(reads, 0);
        for (const infoOnly of [false, true]) {
            const reader = {size: p.reader.size, read(o, n) {
                const source = p.reader.read(o, n);
                if (infoOnly && !p.entries[1].pages.includes(Math.floor(o / 4096))) return source;
                const b = Buffer.from(new Shared(n)); source.copy(b); return b;
            }};
            redactedThrows(() => diagnoseInfoNamedStreamNiMacZero(reader, p.identity), 'ERR_INPUT_BOUND');
            redactedThrows(() => inspectNamedStreamNiMacZero(reader, p.exe, p.identity, query()), 'ERR_INPUT_BOUND');
        }
    }
    for (const infoOnly of [false, true]) for (const value of [Buffer.alloc(0), Buffer.alloc(55),
        new Uint8Array(56), null, 'throw']) {
        const reader = {size: p.reader.size, read(o, n) {
            if (infoOnly && !p.entries[1].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
            if (value === 'throw') throw Error('/private/NEVER_EXPORT');
            return value;
        }};
        const code = value === 'throw' ? 'ERR_PDB_FRAMING' : 'ERR_SHORT_READ';
        redactedThrows(() => diagnoseInfoNamedStreamNiMacZero(reader, p.identity), code);
        redactedThrows(() => inspectNamedStreamNiMacZero(reader, p.exe, p.identity, query()), code);
    }
});

test('NI APIs copy reused reader storage across directory, Info, module page and record cache boundaries', () => {
    const f = niFixture({info: namedStreamNiMacZeroInfo({names: [{name: 'A'.repeat(4100), stream: 8}],
        features: [VC140, VC110]}), modules: [{records: [record(), proc({name: Buffer.alloc(65496, 0x41)})]}]});
    while (f.streams.length < 1500) f.streams.push(null);
    const p = pack(f), scratch = Buffer.alloc(4096);
    a.ok(p.directory.length > 4096);
    const reader = {size: p.reader.size, read(o, n) {
        a.ok(n <= scratch.length && n <= LIMITS.read);
        scratch.fill(0); p.reader.read(o, n).copy(scratch); return scratch.subarray(0, n);
    }};
    a.deepEqual(diagnoseInfoNamedStreamNiMacZero(reader, p.identity), diagnoseInfoNamedStreamNiMacZero(p.reader, p.identity));
    a.deepEqual(inspectNamedStreamNiMacZero(reader, p.exe, p.identity, query()), runNi(f));
});

test('NI accepted Info retains atomic downstream errors and every false coverage flag', () => {
    const baseline = runNi();
    for (const key of ['unsupported_semantic_coverage', 'all_pdb_alias_coverage_confirmed', 'whole_pdb_coverage_confirmed',
        'reference_linkage_confirmed', 'instruction_boundaries_confirmed', 'function_completeness_confirmed',
        'unique_owner_confirmed', 'purity_confirmed', 'unwind_interpretation_confirmed', 'runtime_behavior_confirmed',
        'names_exported', 'code_bytes_read']) a.equal(baseline[key], false);
    a.equal(baseline.declaration_extent_only, true);
    a.equal(baseline.complete_selected_symbol_record_framing, true);
    const badRecord = record(); badRecord.writeUInt16LE(0);
    const cases = [
        [() => niFixture({moduleInfo: Buffer.alloc(60)}), 'ERR_DBI_MODULE_INFO'],
        [() => { const f = niFixture(); f.streams[3].writeInt32LE(-1, 24); return f; }, 'ERR_PDB_FRAMING'],
        [() => { const f = niFixture(); f.streams[3].writeUInt32LE(2, 8); return f; }, 'ERR_IDENTITY'],
        [() => niFixture({modules: [{signature: 2}]}), 'ERR_MODULE_FRAMING'],
        [() => niFixture({modules: [{sym: 5}]}), 'ERR_MODULE_FRAMING'],
        [() => niFixture({modules: [{records: [proc(), badRecord]}]}), 'ERR_MODULE_FRAMING'],
        [() => niFixture({modules: [{records: [proc(), proc({size: 0})]}]}), 'ERR_PROCEDURE_SIZE'],
        [() => niFixture({modules: [{records: [proc(), proc({size: 0x31})]}]}), 'ERR_PROCEDURE_CONFLICT'],
        [() => niFixture({modules: [{records: [record()]}]}), 'ERR_PROCEDURE_ABSENT'],
        [() => { const f = niFixture(); f.streams[9] = null; return f; }, 'ERR_MODULE_BOUND'],
        [() => niFixture({modules: [{flags: 0x100}]}), 'ERR_REFERENCE_UNSUPPORTED'],
        [() => niFixture({modules: [{records: [proc(), record(0x1102)]}]}), 'ERR_SYMBOL_UNSUPPORTED'],
        ...[0x115f, 0x1160, 0x1161, 0x1167].map(kind =>
            [() => niFixture({modules: [{records: [proc(), record(kind)]}]}), 'ERR_FASTLINK_UNSUPPORTED']),
        [() => niFixture({modules: [{records: [proc(), proc({size: 0xffffffff})]}]}), 'ERR_SOURCE_MAPPING'],
        [() => { const f = niFixture(); f.streams[8][8] ^= 1; return f; }, 'ERR_SOURCE_MAPPING'],
    ];
    for (const [make, code] of cases) for (const q of [query(), query({module_indices: [0]})]) {
        const f = make();
        a.equal(diagnoseNi(f).status, 'accepted_profile');
        redactedThrows(() => runNi(f, q), code);
        f.streams[1] = info(); // Independently qualify the same failure in the legacy path.
        fails(f, code, q);
    }
    // A later bad selector or overlapping module cannot yield a partial result.
    redactedThrows(() => runNi(niFixture(), query({addresses: [VA, '0x140001021']})), 'ERR_PROCEDURE_ABSENT');
    const folded = niFixture({modules: [{records: [proc(), proc()]}, {records: [proc({size: 0x31})]}]});
    redactedThrows(() => runNi(folded), 'ERR_PROCEDURE_CONFLICT');
    const scoped = runNi(folded, query({module_indices: [0]}));
    a.equal(scoped.addresses[0].procedure_record_count, 2);
    a.equal(scoped.routing.scope, 'explicit_modules_only');
    a.equal(scoped.whole_pdb_coverage_confirmed, false);
    a.equal(scoped.unique_owner_confirmed, false);
    // Routing remains scope-sensitive only after accepted Info and DBI framing.
    const unrouted = niFixture({sc: Buffer.alloc(0)});
    redactedThrows(() => runNi(unrouted), 'ERR_ROUTING_ABSENT');
    a.equal(runNi(unrouted, query({module_indices: [0]})).routing.scope, 'explicit_modules_only');
});

test('NI profile retains every existing cap and bounded selection, directory and chunk behavior', () => {
    a.deepEqual(LIMITS, {
        pdb: 1280 * 1024 * 1024, exe: 128 * 1024 * 1024,
        directory: 4 * 1024 * 1024, read: 4 * 1024 * 1024,
        streams: 4096, sections: 96, info: 65536, module_info: 4 * 1024 * 1024,
        descriptors: 65535, modules: 32, selectors: 32,
        symbols: 256 * 1024 * 1024, records: 2500000, record: 65537,
        contributions: 256 * 1024 * 1024, contribution_records: 2500000,
        cache: 65536, output: 65536,
    });
    a.ok(Object.isFrozen(LIMITS));
    const addresses = Array.from({length: 32}, (_, i) => '0x' + (0x140001020n + BigInt(4 * i)).toString(16)).reverse();
    const records = addresses.map(v => proc({offset: Number(BigInt(v) - 0x140001000n), size: 4, name: ''}));
    const r = runNi(niFixture({modules: Array.from({length: 32}, () => ({records}))}), query({addresses}));
    a.deepEqual(r.addresses.map(x => x.address), addresses);
    a.equal(r.selected_modules.length, 32);
    a.ok(r.addresses.every(x => x.procedure_record_count === 32));
    a.ok(Buffer.byteLength(JSON.stringify(r) + '\n') <= LIMITS.output);
    redactedThrows(() => runNi(niFixture({modules: Array.from({length: 33}, () => ({}))})), 'ERR_MODULE_BOUND');
    for (const size of [LIMITS.pdb + 1, LIMITS.exe + 1]) {
        const p = pack(niFixture());
        if (size === LIMITS.pdb + 1) redactedThrows(() => inspectNamedStreamNiMacZero({size, read() {
            a.fail('oversized PDB must refuse before reading');
        }}, p.exe, p.identity, query()), 'ERR_INPUT_BOUND');
        else redactedThrows(() => inspectNamedStreamNiMacZero(p.reader, Buffer.alloc(size), p.identity, query()), 'ERR_INPUT_BOUND');
    }
    const full = niFixture(); while (full.streams.length < LIMITS.streams) full.streams.push(null);
    a.equal(runNi(full).addresses[0].procedure_record_count, 1);
    full.streams.push(null);
    redactedThrows(() => runNi(full), 'ERR_PDB_FRAMING');
    const p = pack(repeatedContributions(niFixture(), 150000));
    a.equal(inspectNamedStreamNiMacZero(p.reader, p.exe, p.identity, query()).routing.contribution_records, 150000);
    a.ok(p.reads.every(x => x.count <= LIMITS.read));
    for (const [bytes, code] of [[LIMITS.symbols, 'ERR_MODULE_FRAMING'], [LIMITS.symbols + 4, 'ERR_MODULE_BOUND']]) {
        const f = niFixture({modules: [{sym: bytes, body: repeatedModule(bytes, record(), 2)}]});
        redactedThrows(() => runNi(f), code);
    }
    redactedThrows(() => runNi(repeatedContributions(niFixture(), LIMITS.contribution_records + 1)), 'ERR_ROUTING_BOUND');
    redactedThrows(() => runNi(repeatedContributions(niFixture(), 1, {declaredBytes: LIMITS.contributions + 4})), 'ERR_ROUTING_BOUND');
    const moduleName = Buffer.alloc(LIMITS.module_info - 66, 0x41);
    const atCap = niFixture({modules: [{module_name: moduleName, object_name: ''}]});
    a.equal(atCap.moduleInfo.length, LIMITS.module_info);
    a.equal(runNi(atCap).addresses[0].procedure_record_count, 1);
    redactedThrows(() => runNi(niFixture({modules: [{module_name: Buffer.concat([moduleName, Buffer.alloc(4, 0x41)]),
        object_name: ''}]})), 'ERR_DBI_MODULE_INFO');
});

test('NI aggregate record ceiling still rejects the next framed record atomically', () => {
    const count = LIMITS.records + 1, sym = 4 + 40 + (count - 1) * 4;
    const f = niFixture({modules: [{sym, body: repeatedModule(sym)}]});
    a.equal(diagnoseNi(f).status, 'accepted_profile');
    redactedThrows(() => runNi(f), 'ERR_MODULE_BOUND');
});

test('NI diagnostic and full result enforce inclusive serialized output ceilings', () => {
    const stringify = JSON.stringify;
    for (const [profile, max, call] of [
        ['MSFPDB7_Info_named_stream_ni_mac_zero_diagnostic_v1', 1024, () => diagnoseNi()],
        [NI_PROFILE, LIMITS.output, () => runNi()],
    ]) {
        // Inject serialization lengths to reach the final guard without
        // changing production caps or manufacturing unbounded metadata fields.
        for (const bytes of [max - 1, max]) {
            JSON.stringify = function(value, ...args) {
                const json = stringify.call(this, value, ...args);
                return value && value.profile === profile ? json.padEnd(bytes, ' ') : json;
            };
            try {
                if (bytes === max - 1) a.equal(call().profile, profile); // Plus newline = exact cap.
                else redactedThrows(call, 'ERR_OUTPUT_BOUND');
            } finally { JSON.stringify = stringify; }
        }
    }
});

test('NI full output keeps names, opaque marker strings and code bytes private', () => {
    const secret = 'NEVER_EXPORT_MININOTM/private/path';
    const b = namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}], features: [VC140]});
    const f = niFixture({info: b, modules: [{module_name: secret, object_name: secret,
        records: [proc({name: secret}), proc({offset: 0x90, name: 'UNSELECTED_SECRET'})]}]});
    f.exe.write('CODE_BYTES_MUST_NOT_BE_READ', 0x220);
    const r = runNi(f), json = JSON.stringify(r);
    for (const word of [secret, 'UNSELECTED_SECRET', 'CODE_BYTES_MUST_NOT_BE_READ', '0x140001090',
        'type_index', 'record_bytes_hex', 'private.obj']) a.ok(!json.includes(word));
    f.exe.fill(0x5a, 0x200, 0x800); a.deepEqual(runNi(f), r);
    a.equal(r.names_exported, false); a.equal(r.code_bytes_read, false);
    // Marker-shaped strings cannot supply the structural NI or rescue it.
    niFails(niFixture({info: namedStreamNiMacZeroInfo({names: [{name: 'MININOTM', stream: 8}],
        features: [], niBytes: 0})}), 'ERR_PDB_FRAMING');
    niFails(niFixture({info: namedStreamNiMacZeroInfo({names: [{name: 'MININOTM', stream: 8}],
        niMac: MINI})}), 'ERR_NI_MAC_UNSUPPORTED');
});

// Authored cases establish mapper policy only; no original input or
// target-specific outcome is implied.
const MAPPING_PROFILE = 'MSF7_procedure_source_mapping_diagnostic_v1';
const sourceMapping = (f = niFixture(), q = query()) => {
    const p = pack(f); return diagnoseSourceMappingNamedStreamNiMacZero(p.reader, p.exe, p.identity, q);
};
function mappingResult(result, stage = 'complete', guard = null) {
    const expected = {profile: MAPPING_PROFILE, status: stage === 'complete' ? 'accepted_profile' : 'refused_mapping',
        stage, guard};
    a.deepEqual(result, expected);
    a.deepEqual(Reflect.ownKeys(result), ['profile', 'status', 'stage', 'guard']);
    a.equal(JSON.stringify(result) + '\n', JSON.stringify(expected) + '\n');
    a.ok(Buffer.byteLength(JSON.stringify(result) + '\n', 'utf8') <= 1024);
}
function changedNi(change, options = {}) {
    const f = niFixture(options); change(f); return f;
}
function mappedNi(change) {
    const sm = segmentMap(); change(sm); return niFixture({sectionMap: sm});
}
// Fourteen reachable pairs, independently specified without importing any
// production guard table. Compound guards intentionally have several fixtures.
const mappingCases = [
    ['DOS framing', 'pe', 'parse', () => changedNi(f => f.exe.writeUInt16LE(0))],
    ['PE section overlap', 'pe', 'parse', () => sectionChange(niFixture(),
        (b, at) => b.writeUInt32LE(0x1000, at + 40 + 12))],
    ['noncanonical base', 'pe', 'base_canonical', () => changedNi(f =>
        f.exe.writeBigUInt64LE(0x800000000000n, f.op + 24))],
    ['section stream sentinel', 'section_binding', 'headers', () => changedNi(f =>
        f.streams[3].writeUInt16LE(0xffff, f.optionalAt + 10))],
    ['short section stream', 'section_binding', 'headers', () => changedNi(f =>
        f.streams[8] = f.streams[8].subarray(0, 116))],
    ['long section stream', 'section_binding', 'headers', () => changedNi(f =>
        f.streams[8] = Buffer.concat([f.streams[8], Buffer.alloc(4)]))],
    ['unequal section name bytes', 'section_binding', 'headers', () => changedNi(f =>
        f.streams[8].write('NEVER', 0))],
    ['map bytes above bound', 'segment_map', 'bytes', () => niFixture({sectionMap: Buffer.alloc(88)})],
    ['map logical count', 'segment_map', 'counts', () => mappedNi(b => b.writeUInt16LE(0, 2))],
    ['map physical count', 'segment_map', 'counts', () => mappedNi(b => {
        b.writeUInt16LE(2); b.writeUInt16LE(2, 2);
    })],
    ['map count byte arithmetic', 'segment_map', 'counts', () => mappedNi(b => {
        b.writeUInt16LE(3); b.writeUInt16LE(3, 2);
    })],
    ...[2, 4, 6, 12].map(offset => ['map identity field ' + offset, 'segment_map', 'identity',
        () => mappedNi(b => offset === 12 ? b.writeUInt32LE(1, 4 + offset) : b.writeUInt16LE(9, 4 + offset))]),
    ['absolute flags', 'segment_map', 'absolute', () => mappedNi(b => b.writeUInt16LE(0, 64))],
    ['absolute length', 'segment_map', 'absolute', () => mappedNi(b => b.writeUInt32LE(0, 80))],
    ['section flags', 'segment_map', 'section', () => mappedNi(b => b.writeUInt16LE(0x50d, 4))],
    ['section length', 'segment_map', 'section', () => mappedNi(b => b.writeUInt32LE(0x281, 20))],
    ['selector below image', 'selector', 'image_range', () => niFixture(), query({addresses: ['0x13fffffff']})],
    ['selector at image end', 'selector', 'image_range', () => niFixture(), query({addresses: ['0x140004000']})],
    ['selector in headers', 'selector', 'mapped_section', () => niFixture(), query({addresses: ['0x140000020']})],
    ['selector in gap', 'selector', 'mapped_section', () => niFixture(), query({addresses: ['0x140001280']})],
    ['selector virtual-only tail', 'selector', 'mapped_section', () => sectionChange(niFixture(),
        (b, at) => b.writeUInt32LE(0x200, at + 16)), query({addresses: ['0x140001220']})],
    ...[0xe0000020, 0x20000020, 0x60000040].map(flags => ['selector permissions ' + flags,
        'selector', 'extent', () => sectionChange(niFixture(), (b, at) => b.writeUInt32LE(flags, at + 36))]),
    ['selector resource overlap', 'selector', 'extent', () => changedNi(f => {
        f.exe.writeUInt32LE(0x1000, f.op + 128); f.exe.writeUInt32LE(16, f.op + 132);
    })],
    ['selector noncode section', 'selector', 'extent', () => niFixture(), query({addresses: ['0x140003020']})],
    ...[0, 4].map(segment => ['procedure segment ' + segment, 'procedure', 'segment', () =>
        niFixture({modules: [{records: [proc(), proc({offset: 0x90, segment})]}]})]),
    ['nonmatching procedure start overflow', 'procedure', 'start_rva', () =>
        niFixture({modules: [{records: [proc(), proc({offset: 0xffffffff})]}]})],
    ...[0x261, 0x1000, 0xffffffff].map(size => ['procedure extent ' + size, 'procedure', 'extent', () =>
        niFixture({modules: [{records: [proc({size})]}]})]),
    ['procedure virtual-only tail', 'procedure', 'extent', () => sectionChange(
        niFixture({modules: [{records: [proc({size: 0x200})]}]}), (b, at) => b.writeUInt32LE(0x200, at + 16))],
];

test('source mapping diagnostic attributes every reachable pair and preserves both ordinary error contracts', () => {
    const seen = new Set();
    for (const [label, stage, guard, make, q = query()] of mappingCases) {
        seen.add(stage + '/' + guard);
        for (let repeat = 0; repeat < 2; repeat++) {
            const f = make(), diagnostic = pack(f), ordinary = pack(make());
            const result = diagnoseSourceMappingNamedStreamNiMacZero(diagnostic.reader, diagnostic.exe, diagnostic.identity, q);
            mappingResult(result, stage, guard);
            redactedThrows(() => inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, q), 'ERR_SOURCE_MAPPING');
            a.deepEqual(diagnostic.reads, ordinary.reads, label + ': same exact bounded reads up to refusal');
            const legacy = make(); legacy.streams[1] = info();
            fails(legacy, 'ERR_SOURCE_MAPPING', q);
        }
    }
    a.deepEqual([...seen].sort(), ['pe/base_canonical', 'pe/parse', 'procedure/extent', 'procedure/segment',
        'procedure/start_rva', 'section_binding/headers', 'segment_map/absolute', 'segment_map/bytes',
        'segment_map/counts', 'segment_map/identity', 'segment_map/section', 'selector/extent',
        'selector/image_range', 'selector/mapped_section'].sort());
    // No fabricated extent_segment fixture: findIndex()+1 passes selector's
    // positive guard and cannot exceed sections.length; procedure's segment
    // guard immediately precedes its extent call. Inputs are immutable. See
    // the source-only assessment's defensive-guard proof; a new caller would
    // need a fresh review, not a mutation during reader I/O to reach this guard.
});

test('source mapping first refusal follows entry, selector, module and record order', () => {
    const cases = [
        [() => changedNi(f => { f.exe.writeUInt16LE(0); f.streams[8].fill(0xff); }), query(), 'pe', 'parse'],
        [() => changedNi(f => { f.streams[8][0] = 1; f.exe[f.rsds + 4] ^= 1; },
            {sectionMap: Buffer.alloc(88)}), query(), 'section_binding', 'headers'],
        [() => mappedNi(b => { b.writeUInt16LE(0x50d, 4); b.writeUInt16LE(0, 64); }), query(), 'segment_map', 'section'],
        [() => mappedNi(b => { b.writeUInt16LE(2, 10); b.writeUInt32LE(0, 20); }), query(), 'segment_map', 'identity'],
        [() => niFixture(), query({addresses: ['0x140003020', '0x140004000']}), 'selector', 'extent'],
        [() => niFixture(), query({addresses: [VA, '0x140004000', '0x140000020']}), 'selector', 'image_range'],
        [() => niFixture(), query({addresses: ['0x140000020', '0x140003020']}), 'selector', 'mapped_section'],
        [() => niFixture({modules: [{records: [proc(), proc({segment: 0}), proc({offset: 0xffffffff})]}]}),
            query(), 'procedure', 'segment'],
        [() => niFixture({modules: [{records: [proc(), proc({offset: 0xffffffff}), proc({segment: 0})]}]}),
            query(), 'procedure', 'start_rva'],
        [() => niFixture({modules: [{records: [proc(), proc({size: 0xffffffff}), record(0x1102)]}]}),
            query(), 'procedure', 'extent'],
        [() => niFixture({modules: [{records: [proc()]}, {records: [proc(), proc({segment: 0})]}]}),
            query(), 'procedure', 'segment'],
        [() => niFixture({modules: [{records: [proc({size: 0xffffffff})]}, {records: [proc({segment: 0})]}]}),
            query({module_indices: [1, 0]}), 'procedure', 'extent'],
    ];
    for (const [make, q, stage, guard] of cases) {
        mappingResult(sourceMapping(make(), q), stage, guard);
        redactedThrows(() => runNi(make(), q), 'ERR_SOURCE_MAPPING');
    }
    // Later module errors matter in both automatic and explicit selection.
    for (const q of [query(), query({module_indices: [1, 0]})]) {
        mappingResult(sourceMapping(cases[10][0](), q), 'procedure', 'segment');
    }
    mappingResult(sourceMapping(cases[10][0](), query({module_indices: [0]})));
});

test('source mapping actually stops I/O before later streams and later symbol-cache blocks', () => {
    for (const [make, stage, guard, forbid] of [
        [() => changedNi(f => f.exe.writeUInt16LE(0)), 'pe', 'parse', () => true],
        [() => changedNi(f => f.streams[8][0] = 1), 'section_binding', 'headers', p => p.index === 9],
        [() => mappedNi(b => b.writeUInt16LE(0, 64)), 'segment_map', 'absolute', p => p.index === 9],
        [() => niFixture(), 'selector', 'extent', p => p.index === 9],
        [() => niFixture({modules: [{records: [proc({segment: 0})]}, {}]}), 'procedure', 'segment', p => p.index === 12],
    ]) {
        const f = make(), p = pack(f); let forbiddenReads = 0;
        const reader = {size: p.reader.size, read(o, n) {
            for (const [index, entry] of p.entries.entries()) for (const page of entry.pages) {
                if (o < (page + 1) * 4096 && o + n > page * 4096 && forbid({index})) {
                    forbiddenReads++; throw Error('NEVER_EXPORT/downstream');
                }
            }
            if (stage === 'pe') { forbiddenReads++; throw Error('NEVER_EXPORT/any-read'); }
            return p.reader.read(o, n);
        }};
        const q = stage === 'selector' ? query({addresses: ['0x140003020']}) : query();
        mappingResult(diagnoseSourceMappingNamedStreamNiMacZero(reader, p.exe, p.identity, q), stage, guard);
        a.equal(forbiddenReads, 0);
    }
    // Signature/trailer checks and one cache prefetch precede the first record.
    // A second cache block and module 1 must never be read after its bad segment.
    const f = niFixture({modules: [{records: [proc({segment: 0}), record(0x7777, 65536),
        record(0x7777, 65536), proc()]}, {}]}), p = pack(f);
    let forbiddenReads = 0;
    const reader = {size: p.reader.size, read(o, n) {
        for (const index of [9, 12]) for (let i = 0; i < p.entries[index].pages.length; i++) {
            const page = p.entries[index].pages[i];
            if (o >= page * 4096 && o < (page + 1) * 4096) {
                const logical = i * 4096 + o % 4096;
                if (index === 12 || (logical >= 65536 && logical < f.modules[0].symbols.length)) {
                    forbiddenReads++; throw Error('NEVER_EXPORT/later-cache-or-module');
                }
            }
        }
        return p.reader.read(o, n);
    }};
    mappingResult(diagnoseSourceMappingNamedStreamNiMacZero(reader, p.exe, p.identity, query()), 'procedure', 'segment');
    a.equal(forbiddenReads, 0);
    a.ok(p.reads.some(r => r.offset === p.entries[9].pages[0] * 4096 && r.count === 4096));
});

test('source mapping accepts only complete existing NI profiles with identical bounded reads', () => {
    for (const features of [[], [VC140], [VC110], [VC140, VC110]]) {
        for (const sectionMap of [Buffer.alloc(0), segmentMap(false), segmentMap(true)]) {
            for (const version of [SC60, SC2]) for (const kind of [0x110f, 0x1110, 0x1146, 0x1147]) {
                for (const q of [query(), query({module_indices: [1, 0]})]) {
                    const options = {features}, names = [{name: 'NEVER_EXPORT_MININOTM/private/path', stream: 8}];
                    const f = niFixture({info: namedStreamNiMacZeroInfo({...options, names}), sectionMap, version,
                        modules: [{module_name: names[0].name, object_name: names[0].name,
                            records: [record(), proc({kind, name: names[0].name}), record(0x114d)]},
                        {records: [proc({kind}), proc({offset: 0x90, size: 0xffffffff})]}]});
                    const ordinary = pack(f), diagnostic = pack(f);
                    const expected = inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, q);
                    mappingResult(diagnoseSourceMappingNamedStreamNiMacZero(diagnostic.reader, diagnostic.exe, diagnostic.identity, q));
                    a.deepEqual(diagnostic.reads, ordinary.reads);
                    a.ok(diagnostic.reads.every(r => r.count <= LIMITS.read));
                    a.equal(expected.addresses[0].procedure_record_count, 2);
                    a.equal(expected.opaque_records, 2);
                    a.equal(expected.names_exported, false); a.equal(expected.code_bytes_read, false);
                    a.deepEqual(inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, q), expected);
                    const legacy = {...f, streams: f.streams.slice()}; legacy.streams[1] = info({names, features});
                    const old = run(legacy, q);
                    a.deepEqual(expected, {...old, profile: NI_PROFILE, info_profile: {
                        grammar: 'named_stream_ni_mac_zero_v1', named_stream_ni_mac: 0, named_streams: 1, features,
                    }});
                }
            }
        }
    }
});

test('source mapping extent boundaries retain matching-only validation and canonical endpoints', () => {
    const last = niFixture({modules: [{records: [proc({offset: 0x27f, size: 1})]}],
        contributions: [{offset: 0x27f, size: 1}]});
    const q = query({addresses: ['0x14000127f']});
    mappingResult(sourceMapping(last, q));
    last.exe.writeBigUInt64LE(0xffff800000000000n, last.op + 24);
    mappingResult(sourceMapping(last, query({addresses: ['0xffff80000000127f']})));
    mappingResult(sourceMapping(niFixture({modules: [{records: [proc({size: 0x260})]}]})));
    mappingResult(sourceMapping(niFixture({modules: [{records: [proc(), proc({offset: 0x90, size: 0xffffffff})]}]})));
    mappingResult(sourceMapping(niFixture({modules: [{records: [proc({offset: 0x27f, size: 2})]}],
        contributions: [{offset: 0x27f, size: 2}]}), q), 'procedure', 'extent');
    // Last canonical start is valid for the selector's one-byte extent; the
    // matching declaration's exclusive end enters the canonical hole.
    const f = niFixture({modules: [{records: [proc({offset: 0xff0, size: 0x10})]}],
        contributions: [{offset: 0xff0, size: 0x10}]});
    const old = f.exe; f.exe = Buffer.alloc(0x1600); old.copy(f.exe);
    f.exe.writeBigUInt64LE(0x7fffffff0000n, f.op + 24);
    f.exe.writeUInt32LE(0xf000, f.op + 16); f.exe.writeUInt32LE(0x10000, f.op + 56);
    f.exe.writeUInt32LE(0xf000, f.table + 12); f.exe.writeUInt32LE(0x1000, f.table + 8);
    f.exe.writeUInt32LE(0x1000, f.table + 16); f.exe.writeUInt32LE(0x1200, f.table + 40 + 20);
    f.exe.writeUInt32LE(0x1400, f.table + 80 + 20);
    old.subarray(0x800, 0xa00).copy(f.exe, 0x1400); f.exe.writeUInt32LE(0x14a0, 0x1480 + 24);
    f.streams[8] = Buffer.from(f.exe.subarray(f.table, f.table + 120));
    mappingResult(sourceMapping(f, query({addresses: ['0x7ffffffffff0']})), 'procedure', 'extent');
    // The selector's own one-byte exclusive endpoint can also enter the hole.
    mappingResult(sourceMapping(f, query({addresses: ['0x7fffffffffff']})), 'selector', 'extent');
});

test('source mapping keeps nonmapping errors, identity and refusal precedence code-only', () => {
    const cases = [
        [() => niFixture({info: info()}), 'ERR_NI_MAC_UNSUPPORTED'],
        [() => niFixture({info: namedStreamNiMacZeroInfo({features: [], niBytes: 0})}), 'ERR_PDB_FRAMING'],
        [() => niFixture({info: namedStreamNiMacZeroInfo({features: [MINI]})}), 'ERR_FASTLINK_UNSUPPORTED'],
        [() => niFixture({info: namedStreamNiMacZeroInfo({features: [0]})}), 'ERR_FEATURE_UNSUPPORTED'],
        [() => changedNi(f => f.streams[1][12] ^= 1), 'ERR_IDENTITY'],
        [() => changedNi(f => f.streams[3].writeUInt32LE(2, 8)), 'ERR_IDENTITY'],
        [() => changedNi(f => f.exe[f.rsds + 4] ^= 1), 'ERR_IDENTITY'],
        [() => changedNi(f => f.streams[3].writeInt32LE(-1, 24)), 'ERR_PDB_FRAMING'],
        [() => changedNi(f => f.streams[3].writeUInt16LE(8, f.optionalAt + 6)), 'ERR_OMAP_UNSUPPORTED'],
        [() => changedNi(f => f.streams[8] = null), 'ERR_PDB_FRAMING'],
        [() => niFixture({moduleInfo: Buffer.alloc(60)}), 'ERR_DBI_MODULE_INFO'],
        [() => niFixture({sc: Buffer.alloc(0)}), 'ERR_ROUTING_ABSENT'],
        [() => niFixture({version: 1}), 'ERR_ROUTING_UNSUPPORTED'],
        [() => niFixture({contributions: [{module_index: 1}]}), 'ERR_ROUTING_FRAMING'],
        [() => niFixture({modules: [{signature: 2}]}), 'ERR_MODULE_FRAMING'],
        [() => changedNi(f => f.streams[9] = null), 'ERR_MODULE_BOUND'],
        [() => niFixture({modules: [{flags: 0x100}]}), 'ERR_REFERENCE_UNSUPPORTED'],
        [() => niFixture({modules: [{records: [proc(), proc({size: 0})]}]}), 'ERR_PROCEDURE_SIZE'],
        [() => niFixture({modules: [{records: [proc(), proc({size: 0x31})]}]}), 'ERR_PROCEDURE_CONFLICT'],
        [() => niFixture({modules: [{records: [record()]}]}), 'ERR_PROCEDURE_ABSENT'],
        [() => niFixture({modules: [{records: [proc(), record(0x1102)]}]}), 'ERR_SYMBOL_UNSUPPORTED'],
        [() => niFixture({modules: [{records: [proc(), record(0x115f)]}]}), 'ERR_FASTLINK_UNSUPPORTED'],
    ];
    for (const [make, code] of cases) for (let repeat = 0; repeat < 2; repeat++) {
        redactedThrows(() => sourceMapping(make()), code);
        redactedThrows(() => runNi(make()), code);
    }
    // Acceptance is after all selectors, records and selected modules validate.
    for (const q of [query(), query({module_indices: [1, 0]})]) {
        redactedThrows(() => sourceMapping(niFixture({modules: [{records: [proc()]},
            {records: [proc(), record(0x1102)]}]}), q), 'ERR_SYMBOL_UNSUPPORTED');
        redactedThrows(() => sourceMapping(niFixture({modules: [{records: [proc()]},
            {records: [proc({size: 0x31})]}]}), q), 'ERR_PROCEDURE_CONFLICT');
    }
    redactedThrows(() => sourceMapping(niFixture(), query({addresses: [VA, '0x140001021']})), 'ERR_PROCEDURE_ABSENT');
    const p = pack(niFixture()); let reads = 0;
    const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
    for (const api of [diagnoseSourceMappingNamedStreamNiMacZero, inspectNamedStreamNiMacZero, inspect]) {
        for (const q of [undefined, {}, query({grammar: 'named_stream_ni_mac_zero_v1'}),
            query({exact_addresses_only: true}), query({addresses: ['0x800000000000']}),
            query({module_indices: [0, 0]}), {[Symbol('NEVER_EXPORT')]: 1, addresses: [VA]}]) {
            redactedThrows(() => api(reader, Buffer.alloc(0), {}, q), 'ERR_SELECTORS');
        }
        // EXE and identity input checks precede PE parse; reader bounds also
        // precede parse. A previous Info-only acceptance grants no reordering.
        redactedThrows(() => api(reader, Buffer.alloc(0), {}, query()), 'ERR_INPUT_BOUND');
        redactedThrows(() => api(reader, Buffer.alloc(64), {}, query()), 'ERR_IDENTITY');
        redactedThrows(() => api({size: 55, read() { a.fail('invalid reader must not read'); }},
            Buffer.alloc(64), p.identity, query()), 'ERR_INPUT_BOUND');
    }
    a.equal(reads, 0);
    for (const identity of [undefined, null, {}, {...p.identity, age: 0}, {...p.identity, age: 1.5},
        {...p.identity, age: 0x100000000}, {...p.identity, guid_bytes: p.identity.guid_bytes.toUpperCase()}]) {
        redactedThrows(() => diagnoseSourceMappingNamedStreamNiMacZero(reader, p.exe, identity, query()), 'ERR_IDENTITY');
    }
    a.equal(reads, 0);
    const invalidInfo = niFixture({info: namedStreamNiMacZeroInfo({features: [MINI]})});
    invalidInfo.exe.writeUInt16LE(0);
    mappingResult(sourceMapping(invalidInfo), 'pe', 'parse');
    const identityBeforeSelectors = changedNi(f => f.exe[f.rsds + 4] ^= 1);
    redactedThrows(() => sourceMapping(identityBeforeSelectors, query({addresses: ['0x140004000']})), 'ERR_IDENTITY');
    const omapBeforeHeaders = changedNi(f => {
        f.streams[3].writeUInt16LE(8, f.optionalAt + 8); f.streams[8][0] = 1;
    });
    redactedThrows(() => sourceMapping(omapBeforeHeaders), 'ERR_OMAP_UNSUPPORTED');
    mappingResult(sourceMapping(niFixture({sc: Buffer.alloc(0)}), query({module_indices: [0]})));
});

test('source mapping reader failures never export exception text or a stale successful guard', () => {
    const p = pack(niFixture({sectionMap: segmentMap()}));
    for (const index of [null, 1, 8, 9]) for (const mode of ['throw', 'short', 'typed', 'null']) {
        for (const api of [diagnoseSourceMappingNamedStreamNiMacZero, inspectNamedStreamNiMacZero]) {
            let failures = 0;
            const reader = {size: p.reader.size, read(o, n) {
                const selected = index === null || p.entries[index].pages.includes(Math.floor(o / 4096));
                if (!selected) return p.reader.read(o, n);
                failures++;
                if (mode === 'throw') throw Error('ERR_SOURCE_MAPPING/NEVER_EXPORT/private-reader-path');
                if (mode === 'typed') return new Uint8Array(n);
                if (mode === 'null') return null;
                return Buffer.alloc(n - 1);
            }};
            redactedThrows(() => api(reader, p.exe, p.identity, query()), mode === 'throw' ? 'ERR_PDB_FRAMING' : 'ERR_SHORT_READ');
            a.equal(failures, 1);
        }
    }
    for (const change of [p => p.superblock.writeUInt32LE(512, 32), p => p.blockmap.writeUInt32LE(1),
        p => p.directory.writeUInt32LE(0xffffffff, 8)]) {
        const p = pack(niFixture()); change(p);
        redactedThrows(() => diagnoseSourceMappingNamedStreamNiMacZero(p.reader, p.exe, p.identity, query()), 'ERR_PDB_FRAMING');
    }
    const q = Object.defineProperty({}, 'addresses', {enumerable: true, get() {
        throw Error('NEVER_EXPORT/private-selector');
    }});
    redactedThrows(() => diagnoseSourceMappingNamedStreamNiMacZero(p.reader, p.exe, p.identity, q), 'ERR_PDB_FRAMING');
});

test('source mapping refuses shared storage and copies reused read storage independently', () => {
    const {runInNewContext} = require('node:vm'), foreign = runInNewContext('SharedArrayBuffer');
    const p = pack(niFixture());
    for (const Shared of [SharedArrayBuffer, foreign]) {
        const exe = Buffer.from(new Shared(p.exe.length)); p.exe.copy(exe);
        let reads = 0;
        const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
        redactedThrows(() => diagnoseSourceMappingNamedStreamNiMacZero(reader, exe, p.identity, query()), 'ERR_INPUT_BOUND');
        a.equal(reads, 0);
        for (const index of [null, 1, 8, 9]) {
            const reader = {size: p.reader.size, read(o, n) {
                const source = p.reader.read(o, n);
                if (index !== null && !p.entries[index].pages.includes(Math.floor(o / 4096))) return source;
                const out = Buffer.from(new Shared(n)); source.copy(out); return out;
            }};
            redactedThrows(() => diagnoseSourceMappingNamedStreamNiMacZero(reader, p.exe, p.identity, query()), 'ERR_INPUT_BOUND');
        }
    }
    const f = niFixture({info: namedStreamNiMacZeroInfo({names: [{name: 'A'.repeat(4100), stream: 8}]}),
        modules: [{records: [record(), proc({name: Buffer.alloc(65496, 0x41)})]}]});
    while (f.streams.length < 1500) f.streams.push(null);
    const packed = pack(f), scratch = Buffer.alloc(4096);
    a.ok(packed.directory.length > 4096);
    const reader = {size: packed.reader.size, read(o, n) {
        a.ok(n <= 4096); scratch.fill(0); packed.reader.read(o, n).copy(scratch); return scratch.subarray(0, n);
    }};
    for (let repeat = 0; repeat < 2; repeat++) {
        mappingResult(diagnoseSourceMappingNamedStreamNiMacZero(reader, packed.exe, packed.identity, query()));
        a.deepEqual(inspectNamedStreamNiMacZero(reader, packed.exe, packed.identity, query()), runNi(f));
    }
});

test('source mapping context stays private across repeated, interleaved and nested calls', () => {
    const accepted = pack(niFixture()), badHeaders = changedNi(f => f.streams[8][0] = 1);
    const badSegment = niFixture({modules: [{records: [proc({segment: 0})]}]});
    const legacy = pack(fixture());
    const ordinaryNi = inspectNamedStreamNiMacZero(accepted.reader, accepted.exe, accepted.identity, query());
    const ordinaryLegacy = inspect(legacy.reader, legacy.exe, legacy.identity, query());
    const niJson = JSON.stringify(ordinaryNi), legacyJson = JSON.stringify(ordinaryLegacy);
    for (let repeat = 0; repeat < 3; repeat++) {
        mappingResult(sourceMapping(badHeaders), 'section_binding', 'headers');
        redactedThrows(() => sourceMapping(niFixture({modules: [{records: [record()]}]})), 'ERR_PROCEDURE_ABSENT');
        mappingResult(sourceMapping(badSegment), 'procedure', 'segment');
        mappingResult(diagnoseSourceMappingNamedStreamNiMacZero(accepted.reader, accepted.exe, accepted.identity, query()));
        redactedThrows(() => runNi(badHeaders), 'ERR_SOURCE_MAPPING');
        const oldBad = fixture({modules: [{records: [proc({segment: 0})]}]});
        fails(oldBad, 'ERR_SOURCE_MAPPING');
        a.equal(JSON.stringify(inspectNamedStreamNiMacZero(accepted.reader, accepted.exe, accepted.identity, query())), niJson);
        a.equal(JSON.stringify(inspect(legacy.reader, legacy.exe, legacy.identity, query())), legacyJson);
        // Returned result mutation cannot seed another call's private context.
        const result = sourceMapping(); result.stage = 'NEVER_EXPORT'; result.guard = 'private/path';
        mappingResult(sourceMapping());
    }
    let nested = false;
    const reader = {size: accepted.reader.size, read(o, n) {
        if (!nested) {
            nested = true;
            mappingResult(sourceMapping(badSegment), 'procedure', 'segment');
            redactedThrows(() => runNi(badHeaders), 'ERR_SOURCE_MAPPING');
            a.equal(JSON.stringify(inspect(legacy.reader, legacy.exe, legacy.identity, query())), legacyJson);
        }
        return accepted.reader.read(o, n);
    }};
    mappingResult(diagnoseSourceMappingNamedStreamNiMacZero(reader, accepted.exe, accepted.identity, query()));
    a.equal(nested, true);
    mappingResult(sourceMapping(badHeaders), 'section_binding', 'headers');
});

test('source mapping retains the exact legacy and NI ordinary output schemas', () => {
    const expected = {
        profile: 'MSF7_DBI_C13_scoped_supported_procedure_declarations',
        guid_bytes: '0123456789abcdef0123456789abcdef', age: 1, image_base: '0x140000000',
        routing: {indices: [0], scope: 'all_matching_section_contribution_modules', complete_table_framing: true,
            contribution_bytes: 32, contribution_records: 1, contribution_version: SC60,
            address_routes: [{address: VA, module_indices: [0], matching_contribution_records: 1}]},
        selected_modules: [{module_index: 0, stream_id: 9, symbol_bytes: 64, records: 1,
            supported_procedure_records: 1, opaque_records: 0, matching_records: 1}],
        symbol_bytes: 64, records: 1, supported_procedure_records: 1, opaque_records: 0,
        info_profile: {named_streams: 0, features: [VC140]}, omap_absent: true, pdb_pe_section_headers_equal: true,
        complete_selected_symbol_record_framing: true, unsupported_semantic_coverage: false,
        declaration_extent_only: true, all_pdb_alias_coverage_confirmed: false,
        whole_pdb_coverage_confirmed: false, reference_linkage_confirmed: false,
        instruction_boundaries_confirmed: false, function_completeness_confirmed: false,
        unique_owner_confirmed: false, purity_confirmed: false, unwind_interpretation_confirmed: false,
        runtime_behavior_confirmed: false, names_exported: false, code_bytes_read: false,
        addresses: [{address: VA, procedure_record_count: 1, procedure_record_bytes: 60,
            procedure_records_sha256: createHash('sha256').update(proc()).digest('hex'),
            extent: {segment: 1, section_id: 'SEC01', section_offset: 0x20, start_rva: 0x1020,
                end_rva: 0x1050, size: 0x30, start: VA, end_inclusive: '0x14000104f', end_exclusive: '0x140001050',
                file_span: {start: 0x220, end_exclusive: 0x250}}}],
    };
    const niExpected = {...expected, profile: NI_PROFILE, info_profile: {grammar: 'named_stream_ni_mac_zero_v1',
        named_stream_ni_mac: 0, named_streams: 0, features: [VC140]}};
    for (let repeat = 0; repeat < 2; repeat++) {
        a.deepEqual(run(), expected); a.equal(JSON.stringify(run()), JSON.stringify(expected));
        a.deepEqual(runNi(), niExpected); a.equal(JSON.stringify(runNi()), JSON.stringify(niExpected));
        mappingResult(sourceMapping());
        mappingResult(sourceMapping(changedNi(f => f.streams[8][0] = 1)), 'section_binding', 'headers');
    }
});

test('source mapping serializes only the four-field closed schema and retains both output ceilings', () => {
    const secret = 'NEVER_EXPORT_MININOTM/private/path', f = niFixture({
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}),
        modules: [{module_name: secret, object_name: secret, records: [proc({name: secret})]}]});
    f.exe.write('NEVER_EXPORT_CODE_BYTES', 0x220);
    const result = sourceMapping(f); mappingResult(result);
    const ordinary = runNi(f);
    f.exe.fill(0x5a, 0x200, 0x800);
    a.deepEqual(sourceMapping(f), result); a.deepEqual(runNi(f), ordinary);
    f.streams[8].write('NEVER', 0);
    const refusal = sourceMapping(f); mappingResult(refusal, 'section_binding', 'headers');
    for (const value of [result, refusal]) {
        const json = JSON.stringify(value);
        for (const word of ['NEVER_EXPORT', 'private', 'guid', '"age"', 'address', 'bytes', 'count',
            'offset', 'records', 'flags', 'hash', '0x', 'refusal', 'ERR_SOURCE_MAPPING']) a.ok(!json.includes(word));
        a.ok(Object.values(value).every(v => typeof v === 'string' || v === null));
    }
    const stringify = JSON.stringify;
    for (const call of [() => sourceMapping(), () => sourceMapping(f)]) {
        for (const bytes of [1023, 1024]) {
            JSON.stringify = function(value, ...args) {
                const json = stringify.call(this, value, ...args);
                return value && value.profile === MAPPING_PROFILE ? json.padEnd(bytes, ' ') : json;
            };
            try {
                if (bytes === 1023) a.equal(call().profile, MAPPING_PROFILE);
                else redactedThrows(call, 'ERR_OUTPUT_BOUND');
            } finally { JSON.stringify = stringify; }
        }
    }
    // The discarded ordinary result must still pass its original bound first.
    JSON.stringify = function(value, ...args) {
        const json = stringify.call(this, value, ...args);
        return value && value.profile === NI_PROFILE ? json.padEnd(LIMITS.output, ' ') : json;
    };
    try {
        redactedThrows(() => sourceMapping(), 'ERR_OUTPUT_BOUND');
        redactedThrows(() => runNi(), 'ERR_OUTPUT_BOUND');
        mappingResult(sourceMapping(f), 'section_binding', 'headers');
    } finally { JSON.stringify = stringify; }
    JSON.stringify = function(value, ...args) {
        if (value && value.profile === MAPPING_PROFILE) throw Error('NEVER_EXPORT/private-serializer');
        return stringify.call(this, value, ...args);
    };
    try { redactedThrows(() => sourceMapping(), 'ERR_PDB_FRAMING'); }
    finally { JSON.stringify = stringify; }
});

// A tail slot identifies a position, never an accepted flag tuple or a
// target observation.
const IDENTITY_MAPPING_PROFILE = 'MSF7_procedure_source_mapping_identity_diagnostic_v1';
const sourceMappingIdentity = (f = niFixture(), q = query()) => {
    const p = pack(f); return diagnoseSourceMappingIdentityNamedStreamNiMacZero(p.reader, p.exe, p.identity, q);
};
function identityMappingResult(result, stage = 'complete', guard = null,
    descriptor_index = null, descriptor_slot = null, identity_member = null) {
    const expected = {profile: IDENTITY_MAPPING_PROFILE,
        status: stage === 'complete' ? 'accepted_profile' : 'refused_mapping', stage, guard,
        descriptor_index, descriptor_slot, identity_member};
    a.deepEqual(result, expected);
    a.deepEqual(Reflect.ownKeys(result), ['profile', 'status', 'stage', 'guard',
        'descriptor_index', 'descriptor_slot', 'identity_member']);
    a.equal(JSON.stringify(result) + '\n', JSON.stringify(expected) + '\n');
    a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024);
    if (identity_member !== null) {
        a.ok(Number.isInteger(descriptor_index) && descriptor_index >= 0 && descriptor_index <= 96);
        a.ok(['section', 'tail_candidate'].includes(descriptor_slot));
        a.ok(['overlay', 'group', 'frame', 'offset'].includes(identity_member));
    }
}
const identityMembers = ['overlay', 'group', 'frame', 'offset'];
function badIdentityMember(b, index, member, value = 0x7654) {
    const at = 4 + index * 20;
    if (member === 'offset') b.writeUInt32LE(value, at + 12);
    else b.writeUInt16LE(value, at + {overlay: 2, group: 4, frame: 6}[member]);
}

test('identity diagnostic reports each first bad member in each ordinary and tail slot without values', () => {
    for (const index of [0, 1, 2, 3]) for (const member of identityMembers) {
        for (const value of [0x7654, 0xabcd]) {
            const f = mappedNi(b => badIdentityMember(b, index, member, value));
            const diagnostic = pack(f), ordinary = pack(f), oldDiagnostic = pack(f);
            identityMappingResult(diagnoseSourceMappingIdentityNamedStreamNiMacZero(
                diagnostic.reader, diagnostic.exe, diagnostic.identity, query()),
            'segment_map', 'identity', index, index === 3 ? 'tail_candidate' : 'section', member);
            redactedThrows(() => inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, query()),
                'ERR_SOURCE_MAPPING');
            mappingResult(diagnoseSourceMappingNamedStreamNiMacZero(
                oldDiagnostic.reader, oldDiagnostic.exe, oldDiagnostic.identity, query()), 'segment_map', 'identity');
            a.deepEqual(diagnostic.reads, ordinary.reads); a.deepEqual(diagnostic.reads, oldDiagnostic.reads);
            // Both ordinary grammars retain the same code-only error shape.
            f.streams[1] = info(); fails(f, 'ERR_SOURCE_MAPPING');
        }
    }
    // Zero remains a refused tail frame, with no alternate sentinel contract.
    identityMappingResult(sourceMappingIdentity(mappedNi(b => badIdentityMember(b, 3, 'frame', 0))),
        'segment_map', 'identity', 3, 'tail_candidate', 'frame');
});

test('identity diagnostic preserves short circuit within descriptors, between descriptors and before flags', () => {
    for (const index of [0, 3]) for (let first = 0; first < identityMembers.length; first++) {
        const f = mappedNi(b => {
            for (const member of identityMembers.slice(first)) badIdentityMember(b, index, member);
            b.writeUInt16LE(0, 4 + index * 20); b.writeUInt32LE(0, 20 + index * 20);
        });
        identityMappingResult(sourceMappingIdentity(f), 'segment_map', 'identity', index,
            index === 3 ? 'tail_candidate' : 'section', identityMembers[first]);
    }
    for (let index = 0; index < 3; index++) {
        const f = mappedNi(b => {
            badIdentityMember(b, index, 'offset'); badIdentityMember(b, index + 1, 'overlay');
        });
        identityMappingResult(sourceMappingIdentity(f), 'segment_map', 'identity', index, 'section', 'offset');
        for (const field of ['flags', 'length']) {
            const earlier = mappedNi(b => {
                badIdentityMember(b, index + 1, 'overlay');
                if (field === 'flags') b.writeUInt16LE(0, 4 + index * 20);
                else b.writeUInt32LE(0, 20 + index * 20);
            });
            identityMappingResult(sourceMappingIdentity(earlier), 'segment_map', 'section');
        }
    }
    const headersFirst = mappedNi(b => badIdentityMember(b, 0, 'overlay'));
    headersFirst.streams[8][0] ^= 1;
    identityMappingResult(sourceMappingIdentity(headersFirst), 'section_binding', 'headers');
});

test('identity diagnostic preserves every existing mapping refusal and has null detail elsewhere', () => {
    for (const [label, stage, guard, make, q = query()] of mappingCases) {
        const diagnostic = pack(make()), ordinary = pack(make());
        const result = diagnoseSourceMappingIdentityNamedStreamNiMacZero(
            diagnostic.reader, diagnostic.exe, diagnostic.identity, q);
        if (stage === 'segment_map' && guard === 'identity') {
            const member = {'map identity field 2': 'overlay', 'map identity field 4': 'group',
                'map identity field 6': 'frame', 'map identity field 12': 'offset'}[label];
            identityMappingResult(result, stage, guard, 0, 'section', member);
        } else identityMappingResult(result, stage, guard);
        redactedThrows(() => inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, q), 'ERR_SOURCE_MAPPING');
        a.deepEqual(diagnostic.reads, ordinary.reads, label);
    }
    // A valid trailing descriptor never becomes a routable physical section.
    for (const f of [niFixture({sectionMap: segmentMap(), modules: [{records: [proc({segment: 4})]}]}),
        niFixture({sectionMap: segmentMap(), contributions: [{segment: 4}]})]) {
        if (f.modules[0].symbols.readUInt16LE(4 + 36) === 4) {
            identityMappingResult(sourceMappingIdentity(f), 'procedure', 'segment');
        } else redactedThrows(() => sourceMappingIdentity(f), 'ERR_ROUTING_FRAMING');
    }
});

test('identity diagnostic accepts absent, N and N+1 maps only after complete ordinary traversal', () => {
    for (const sectionMap of [Buffer.alloc(0), segmentMap(false), segmentMap(true)]) {
        for (const features of [[], [VC140], [VC110], [VC140, VC110]]) {
            for (const version of [SC60, SC2]) for (const q of [query(), query({module_indices: [1, 0]})]) {
                const f = niFixture({sectionMap, version, info: namedStreamNiMacZeroInfo({features}),
                    modules: [{records: [proc()]}, {records: [proc()]}]});
                const diagnostic = pack(f), ordinary = pack(f);
                const before = inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, q);
                identityMappingResult(diagnoseSourceMappingIdentityNamedStreamNiMacZero(
                    diagnostic.reader, diagnostic.exe, diagnostic.identity, q));
                a.deepEqual(diagnostic.reads, ordinary.reads);
                a.ok(diagnostic.reads.every(r => r.count <= LIMITS.read));
                a.deepEqual(runNi(f, q), before); mappingResult(sourceMapping(f, q));
                const old = {...f, streams: f.streams.slice()}; old.streams[1] = info({features});
                const legacy = run(old, q);
                a.deepEqual(before, {...legacy, profile: NI_PROFILE, info_profile: {
                    grammar: 'named_stream_ni_mac_zero_v1', named_stream_ni_mac: 0, named_streams: 0, features,
                }});
            }
        }
        for (const [records, code] of [
            [[proc(), record(0x1102)], 'ERR_SYMBOL_UNSUPPORTED'],
            [[proc(), proc({size: 0x31})], 'ERR_PROCEDURE_CONFLICT'],
            [[proc(), proc({size: 0})], 'ERR_PROCEDURE_SIZE'],
        ]) {
            const f = niFixture({sectionMap, modules: [{records: [proc()]}, {records}]});
            redactedThrows(() => sourceMappingIdentity(f), code);
            redactedThrows(() => runNi(f), code);
        }
        redactedThrows(() => sourceMappingIdentity(niFixture({sectionMap, modules: [{records: [record()]}]})),
            'ERR_PROCEDURE_ABSENT');
    }
});

// Independently extend the authored PE envelope up to the physical-section cap.
// Extra sections have virtual storage only; none is used for routing or code.
function identityLimitFixture(count = 96) {
    const sm = Buffer.alloc(4 + (count + 1) * 20), ordinaryMap = segmentMap(false);
    sm.writeUInt16LE(count + 1); sm.writeUInt16LE(count + 1, 2);
    ordinaryMap.copy(sm, 4, 4);
    for (let i = 3; i <= count; i++) {
        const at = 4 + i * 20;
        sm.writeUInt16LE(i === count ? 0x208 : 0x109, at); sm.writeUInt16LE(i + 1, at + 6);
        sm.writeUInt32LE(i === count ? 0xffffffff : 0x80, at + 16);
    }
    const f = niFixture({sectionMap: sm}), old = f.exe, exe = Buffer.alloc(0x1a00);
    old.copy(exe, 0, 0, f.table + 120); old.copy(exe, 0x1200, 0x200);
    exe.writeUInt16LE(count, exe.readUInt32LE(60) + 6);
    exe.writeUInt32LE(0x2000, f.op + 16); exe.writeUInt32LE((count + 2) * 4096, f.op + 56);
    exe.writeUInt32LE(0x1200, f.op + 60); exe.writeUInt32LE(0x4080, f.op + 112 + 6 * 8);
    exe.writeUInt32LE(0x40a0, f.debug + 0x1000 + 20);
    exe.writeUInt32LE(f.rsds + 0x1000, f.debug + 0x1000 + 24);
    for (let i = 0; i < count; i++) {
        const at = f.table + i * 40;
        if (i < 3) exe.writeUInt32LE(old.readUInt32LE(at + 20) + 0x1000, at + 20);
        else { exe.writeUInt32LE(0x80, at + 8); exe.writeUInt32LE(0x40000040, at + 36); }
        exe.writeUInt32LE((i + 2) * 4096, at + 12);
    }
    f.exe = exe; f.streams[8] = Buffer.from(exe.subarray(f.table, f.table + count * 40));
    return f;
}

test('identity diagnostic bounds the zero-based descriptor index through 96', () => {
    const q = query({addresses: ['0x140002020']}), accepted = identityLimitFixture();
    identityMappingResult(sourceMappingIdentity(accepted, q));
    a.equal(runNi(accepted, q).addresses[0].procedure_record_count, 1);
    for (const index of [0, 95, 96]) for (const member of identityMembers) {
        const f = identityLimitFixture(), sm = Buffer.from(f.sectionMap);
        badIdentityMember(sm, index, member);
        sm.copy(f.streams[3], f.scAt + f.sc.length);
        const diagnostic = pack(f), ordinary = pack(f);
        identityMappingResult(diagnoseSourceMappingIdentityNamedStreamNiMacZero(
            diagnostic.reader, diagnostic.exe, diagnostic.identity, q),
        'segment_map', 'identity', index, index === 96 ? 'tail_candidate' : 'section', member);
        redactedThrows(() => inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, q), 'ERR_SOURCE_MAPPING');
        a.deepEqual(diagnostic.reads, ordinary.reads);
    }
});

test('identity diagnostic keeps nonmapping failures code-only and refuses reader or selector details', () => {
    for (const [make, code] of [
        [() => niFixture({info: info()}), 'ERR_NI_MAC_UNSUPPORTED'],
        [() => niFixture({info: namedStreamNiMacZeroInfo({features: [MINI]})}), 'ERR_FASTLINK_UNSUPPORTED'],
        [() => niFixture({info: namedStreamNiMacZeroInfo({features: [0]})}), 'ERR_FEATURE_UNSUPPORTED'],
        [() => changedNi(f => f.exe[f.rsds + 4] ^= 1), 'ERR_IDENTITY'],
        [() => changedNi(f => f.streams[3].writeUInt16LE(8, f.optionalAt + 6)), 'ERR_OMAP_UNSUPPORTED'],
        [() => niFixture({moduleInfo: Buffer.alloc(60)}), 'ERR_DBI_MODULE_INFO'],
        [() => niFixture({sc: Buffer.alloc(0)}), 'ERR_ROUTING_ABSENT'],
        [() => niFixture({modules: [{signature: 2}]}), 'ERR_MODULE_FRAMING'],
    ]) {
        const diagnostic = pack(make()), ordinary = pack(make());
        redactedThrows(() => diagnoseSourceMappingIdentityNamedStreamNiMacZero(
            diagnostic.reader, diagnostic.exe, diagnostic.identity, query()), code);
        redactedThrows(() => inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, query()), code);
        a.deepEqual(diagnostic.reads, ordinary.reads);
    }
    const p = pack(niFixture()); let reads = 0;
    const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
    for (const q of [undefined, {}, query({grammar: 'NEVER_EXPORT'}), query({addresses: ['0x800000000000']})]) {
        redactedThrows(() => diagnoseSourceMappingIdentityNamedStreamNiMacZero(reader, p.exe, p.identity, q), 'ERR_SELECTORS');
    }
    a.equal(reads, 0);
    for (const index of [1, 3, 8, 9]) for (const mode of ['throw', 'short', 'shared']) {
        const broken = {size: p.reader.size, read(o, n) {
            if (!p.entries[index].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
            if (mode === 'throw') throw Error('ERR_SOURCE_MAPPING/NEVER_EXPORT/private');
            return mode === 'shared' ? Buffer.from(new SharedArrayBuffer(n)) : Buffer.alloc(n - 1);
        }};
        redactedThrows(() => diagnoseSourceMappingIdentityNamedStreamNiMacZero(broken, p.exe, p.identity, query()),
            mode === 'throw' ? 'ERR_PDB_FRAMING' : mode === 'shared' ? 'ERR_INPUT_BOUND' : 'ERR_SHORT_READ');
    }
});

test('identity diagnostic context is private across nested calls and mutated returned details', () => {
    const bad = mappedNi(b => badIdentityMember(b, 3, 'offset'));
    const accepted = pack(niFixture()), outer = pack(bad); let nested = false;
    const reader = {size: outer.reader.size, read(o, n) {
        if (!nested) {
            nested = true;
            identityMappingResult(sourceMappingIdentity(mappedNi(b => badIdentityMember(b, 0, 'group'))),
                'segment_map', 'identity', 0, 'section', 'group');
            identityMappingResult(diagnoseSourceMappingIdentityNamedStreamNiMacZero(
                accepted.reader, accepted.exe, accepted.identity, query()));
            mappingResult(sourceMapping(bad), 'segment_map', 'identity');
            redactedThrows(() => runNi(bad), 'ERR_SOURCE_MAPPING');
        }
        return outer.reader.read(o, n);
    }};
    const result = diagnoseSourceMappingIdentityNamedStreamNiMacZero(reader, outer.exe, outer.identity, query());
    identityMappingResult(result, 'segment_map', 'identity', 3, 'tail_candidate', 'offset'); a.equal(nested, true);
    result.descriptor_index = 999; result.descriptor_slot = 'NEVER_EXPORT'; result.identity_member = 'private';
    for (let repeat = 0; repeat < 3; repeat++) {
        identityMappingResult(sourceMappingIdentity());
        identityMappingResult(sourceMappingIdentity(bad), 'segment_map', 'identity', 3, 'tail_candidate', 'offset');
        identityMappingResult(sourceMappingIdentity(changedNi(f => f.streams[8][0] ^= 1)), 'section_binding', 'headers');
        redactedThrows(() => sourceMappingIdentity(niFixture({modules: [{records: [record()]}]})), 'ERR_PROCEDURE_ABSENT');
        mappingResult(sourceMapping(bad), 'segment_map', 'identity');
    }
});

test('identity diagnostic exports only seven redacted fields and retains both serialization bounds', () => {
    const secret = 'NEVER_EXPORT/private/name', f = niFixture({sectionMap: segmentMap(),
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}),
        modules: [{module_name: secret, object_name: secret, records: [proc({name: secret})]}]});
    const accepted = sourceMappingIdentity(f); identityMappingResult(accepted);
    f.exe.fill(0x5a, 0x200, 0x800); a.deepEqual(sourceMappingIdentity(f), accepted);
    badIdentityMember(f.streams[3].subarray(f.scAt + f.sc.length), 3, 'offset', 0xdeadbeef);
    const refusal = sourceMappingIdentity(f);
    identityMappingResult(refusal, 'segment_map', 'identity', 3, 'tail_candidate', 'offset');
    for (const result of [accepted, refusal]) {
        const json = JSON.stringify(result);
        for (const word of [secret, 'deadbeef', '3735928559', 'guid', 'address', 'bytes', 'flags',
            'records', 'hash', '0x', 'ERR_SOURCE_MAPPING', 'absolute']) a.ok(!json.includes(word));
        a.ok(Object.values(result).every(v => v === null || typeof v === 'string' || Number.isInteger(v)));
        a.ok(Object.values(result).every(v => !Array.isArray(v)));
    }
    const stringify = JSON.stringify;
    for (const call of [() => sourceMappingIdentity(), () => sourceMappingIdentity(f)]) {
        for (const bytes of [1023, 1024]) {
            JSON.stringify = function(value, ...args) {
                const json = stringify.call(this, value, ...args);
                return value && value.profile === IDENTITY_MAPPING_PROFILE ? json.padEnd(bytes, ' ') : json;
            };
            try {
                if (bytes === 1023) a.equal(call().profile, IDENTITY_MAPPING_PROFILE);
                else redactedThrows(call, 'ERR_OUTPUT_BOUND');
            } finally { JSON.stringify = stringify; }
        }
    }
    JSON.stringify = function(value, ...args) {
        const json = stringify.call(this, value, ...args);
        return value && value.profile === NI_PROFILE ? json.padEnd(LIMITS.output, ' ') : json;
    };
    try {
        redactedThrows(() => sourceMappingIdentity(), 'ERR_OUTPUT_BOUND');
        identityMappingResult(sourceMappingIdentity(f), 'segment_map', 'identity', 3, 'tail_candidate', 'offset');
    } finally { JSON.stringify = stringify; }
    JSON.stringify = function(value, ...args) {
        if (value && value.profile === IDENTITY_MAPPING_PROFILE) throw Error('NEVER_EXPORT/private-serializer');
        return stringify.call(this, value, ...args);
    };
    try { redactedThrows(() => sourceMappingIdentity(), 'ERR_PDB_FRAMING'); }
    finally { JSON.stringify = stringify; }
});

// Exact scalar cases are authored source only; no target value is assumed.
const TAIL_FRAME_PROFILE = 'MSF7_procedure_tail_frame_diagnostic_v1';
const tailQuery = () => query({addresses: ['0x140002020']});
const tailFrame = (f = niFixture(), q = query()) => {
    const p = pack(f); return diagnoseTailFrameNamedStreamNiMacZero(p.reader, p.exe, p.identity, q);
};
function tailFrameResult(result, stage = 'complete', guard = null,
    descriptor_index = null, descriptor_slot = null, identity_member = null, actual_frame = null) {
    const expected = {profile: TAIL_FRAME_PROFILE,
        status: stage === 'complete' ? 'accepted_profile' : 'refused_mapping', stage, guard,
        descriptor_index, descriptor_slot, identity_member, actual_frame};
    a.deepEqual(result, expected);
    a.deepEqual(Reflect.ownKeys(result), ['profile', 'status', 'stage', 'guard',
        'descriptor_index', 'descriptor_slot', 'identity_member', 'actual_frame']);
    a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024);
    if (identity_member !== null) {
        a.ok(Number.isInteger(descriptor_index) && descriptor_index >= 0 && descriptor_index <= 96);
        a.ok(['section', 'tail_candidate'].includes(descriptor_slot));
        a.ok(identityMembers.includes(identity_member));
    }
    if (actual_frame !== null) {
        a.equal(stage, 'segment_map'); a.equal(guard, 'identity');
        a.equal(descriptor_slot, 'tail_candidate'); a.equal(identity_member, 'frame');
        a.ok(Number.isInteger(actual_frame) && actual_frame >= 0 && actual_frame <= 65535);
        a.notEqual(actual_frame, descriptor_index + 1);
    } else a.ok(!(stage === 'segment_map' && guard === 'identity' &&
        descriptor_slot === 'tail_candidate' && identity_member === 'frame'));
}
function tailFrameFixture(count = 8, change = () => {}) {
    const f = identityLimitFixture(count), sm = Buffer.from(f.sectionMap);
    change(sm); f.sectionMap = sm; sm.copy(f.streams[3], f.scAt + f.sc.length);
    return f;
}
function tailFrameParity(f, q, stage = 'complete', guard = null,
    index = null, slot = null, member = null, actual = null) {
    const diagnostic = pack(f), four = pack(f), seven = pack(f), ordinary = pack(f);
    const result = diagnoseTailFrameNamedStreamNiMacZero(diagnostic.reader, diagnostic.exe, diagnostic.identity, q);
    tailFrameResult(result, stage, guard, index, slot, member, actual);
    mappingResult(diagnoseSourceMappingNamedStreamNiMacZero(four.reader, four.exe, four.identity, q), stage, guard);
    identityMappingResult(diagnoseSourceMappingIdentityNamedStreamNiMacZero(
        seven.reader, seven.exe, seven.identity, q), stage, guard, index, slot, member);
    if (stage === 'complete') {
        a.deepEqual(inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, q), runNi(f, q));
    } else redactedThrows(() => inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, q),
        'ERR_SOURCE_MAPPING');
    for (const p of [four, seven, ordinary]) a.deepEqual(diagnostic.reads, p.reads);
    a.ok(diagnostic.reads.every(r => r.count <= LIMITS.read));
    return result;
}

test('tail frame diagnostic exports exact unsigned u16 mismatches for eight sections and the maximum tail', () => {
    for (const count of [8, 96]) for (const value of [0, 1, 8, 10, 256, 0x1234, 0x8000, 65535]) {
        const f = tailFrameFixture(count, b => badIdentityMember(b, count, 'frame', value));
        tailFrameParity(f, tailQuery(), 'segment_map', 'identity', count, 'tail_candidate', 'frame', value);
        const legacy = {...f, streams: f.streams.slice()}; legacy.streams[1] = info();
        fails(legacy, 'ERR_SOURCE_MAPPING', tailQuery());
    }
});

test('tail frame diagnostic preserves correct frames, later offset and absolute guards without a scalar', () => {
    for (const count of [8, 96]) {
        tailFrameParity(tailFrameFixture(count), tailQuery());
        for (const field of ['offset', 'flags', 'length']) {
            const f = tailFrameFixture(count, b => {
                const at = 4 + count * 20;
                if (field === 'offset') b.writeUInt32LE(0xdeadbeef, at + 12);
                else if (field === 'flags') b.writeUInt16LE(0, at);
                else b.writeUInt32LE(0, at + 16);
            });
            if (field === 'offset') tailFrameParity(f, tailQuery(), 'segment_map', 'identity',
                count, 'tail_candidate', 'offset');
            else tailFrameParity(f, tailQuery(), 'segment_map', 'absolute');
        }
    }
    for (const sectionMap of [Buffer.alloc(0), segmentMap(false), segmentMap(true)]) {
        for (const version of [SC60, SC2]) for (const features of [[], [VC140], [VC110], [VC140, VC110]]) {
            const f = niFixture({sectionMap, version, info: namedStreamNiMacZeroInfo({features}),
                modules: [{records: [proc()]}, {records: [proc()]}]});
            for (const q of [query(), query({module_indices: [1, 0]})]) tailFrameParity(f, q);
        }
    }
});

test('tail frame diagnostic preserves overlay, group, header and ordinary descriptor precedence', () => {
    for (const member of ['overlay', 'group']) {
        const f = tailFrameFixture(8, b => {
            badIdentityMember(b, 8, 'frame', 0x8000); badIdentityMember(b, 8, member);
            if (member === 'overlay') badIdentityMember(b, 8, 'group');
        });
        tailFrameParity(f, tailQuery(), 'segment_map', 'identity', 8, 'tail_candidate', member);
    }
    for (const index of [0, 7]) {
        for (const member of identityMembers) {
            const f = tailFrameFixture(8, b => {
                badIdentityMember(b, 8, 'frame', 65535); badIdentityMember(b, index, member, 0x8000);
            });
            tailFrameParity(f, tailQuery(), 'segment_map', 'identity', index, 'section', member);
        }
        for (const field of ['flags', 'length']) {
            const f = tailFrameFixture(8, b => {
                badIdentityMember(b, 8, 'frame', 65535);
                if (field === 'flags') b.writeUInt16LE(0, 4 + index * 20);
                else b.writeUInt32LE(0, 20 + index * 20);
            });
            tailFrameParity(f, tailQuery(), 'segment_map', 'section');
        }
    }
    const headersFirst = tailFrameFixture(8, b => badIdentityMember(b, 8, 'frame', 65535));
    headersFirst.streams[8][0] ^= 1;
    tailFrameParity(headersFirst, tailQuery(), 'section_binding', 'headers');
});

// Trace field reads on the copied map as well as reader requests. This catches
// a second frame read, eager offset/length reads, and scans of later descriptors.
function segmentFieldTrace(f, call) {
    const u16 = Buffer.prototype.readUInt16LE, u32 = Buffer.prototype.readUInt32LE, fields = [];
    for (const [name, original, width] of [['readUInt16LE', u16, 2], ['readUInt32LE', u32, 4]]) {
        Buffer.prototype[name] = function(offset = 0, ...args) {
            if (this.length === f.sectionMap.length && this.equals(f.sectionMap)) fields.push({width, offset});
            return original.call(this, offset, ...args);
        };
    }
    try { call(); return fields; }
    finally { Buffer.prototype.readUInt16LE = u16; Buffer.prototype.readUInt32LE = u32; }
}

test('tail frame diagnostic preserves exact short circuit reads and stops before later guards and descriptors', () => {
    for (const index of [0, 7, 8]) for (let first = 0; first < identityMembers.length; first++) {
        const member = identityMembers[first];
        const f = tailFrameFixture(8, b => {
            for (const later of identityMembers.slice(first)) badIdentityMember(b, index, later,
                later === 'offset' ? 0xdeadbeef : 0x1234);
            b.writeUInt16LE(0, 4 + index * 20); b.writeUInt32LE(0, 20 + index * 20);
            if (index < 8) badIdentityMember(b, index + 1, 'overlay');
        });
        const at = 4 + index * 20, traces = [];
        for (const api of [diagnoseTailFrameNamedStreamNiMacZero, diagnoseSourceMappingIdentityNamedStreamNiMacZero,
            diagnoseSourceMappingNamedStreamNiMacZero, inspectNamedStreamNiMacZero]) {
            const p = pack(f);
            traces.push(segmentFieldTrace(f, () => {
                if (api === inspectNamedStreamNiMacZero) redactedThrows(() => api(p.reader, p.exe, p.identity, tailQuery()),
                    'ERR_SOURCE_MAPPING');
                else {
                    const result = api(p.reader, p.exe, p.identity, tailQuery());
                    if (api === diagnoseTailFrameNamedStreamNiMacZero) tailFrameResult(result, 'segment_map', 'identity',
                        index, index === 8 ? 'tail_candidate' : 'section', member,
                        index === 8 && member === 'frame' ? 0x1234 : null);
                }
            }));
            a.ok(!p.reads.some(r => p.entries[9].pages.includes(Math.floor(r.offset / 4096))));
        }
        for (const trace of traces) {
            a.deepEqual(trace, traces[0]);
            // Existing traversal reads flags before identity. The failing
            // comparison is the last read, including a single u16 frame read.
            const expected = [0, 2, 4, 6, 12].slice(0, first + 2)
                .map(offset => ({width: offset === 12 ? 4 : 2, offset: at + offset}));
            a.deepEqual(trace.filter(r => r.offset >= at), expected);
        }
    }
});

test('tail frame diagnostic preserves every mapping guard, ordinary frame privacy and nonroutable tails', () => {
    for (const [label, stage, guard, make, q = query()] of mappingCases) {
        if (stage === 'segment_map' && guard === 'identity') {
            const member = {'map identity field 2': 'overlay', 'map identity field 4': 'group',
                'map identity field 6': 'frame', 'map identity field 12': 'offset'}[label];
            tailFrameParity(make(), q, stage, guard, 0, 'section', member);
        } else tailFrameParity(make(), q, stage, guard);
    }
    tailFrameParity(niFixture({sectionMap: segmentMap(), modules: [{records: [proc({segment: 4})]}]}),
        query(), 'procedure', 'segment');
    redactedThrows(() => tailFrame(niFixture({sectionMap: segmentMap(), contributions: [{segment: 4}]})),
        'ERR_ROUTING_FRAMING');
});

test('tail frame diagnostic keeps nonmapping failures code-only through full traversal', () => {
    for (const [make, code] of [
        [() => niFixture({info: info()}), 'ERR_NI_MAC_UNSUPPORTED'],
        [() => niFixture({info: namedStreamNiMacZeroInfo({features: [MINI]})}), 'ERR_FASTLINK_UNSUPPORTED'],
        [() => niFixture({info: namedStreamNiMacZeroInfo({features: [0]})}), 'ERR_FEATURE_UNSUPPORTED'],
        [() => changedNi(f => f.exe[f.rsds + 4] ^= 1), 'ERR_IDENTITY'],
        [() => changedNi(f => f.streams[3].writeUInt16LE(8, f.optionalAt + 6)), 'ERR_OMAP_UNSUPPORTED'],
        [() => niFixture({moduleInfo: Buffer.alloc(60)}), 'ERR_DBI_MODULE_INFO'],
        [() => niFixture({sc: Buffer.alloc(0)}), 'ERR_ROUTING_ABSENT'],
        [() => niFixture({modules: [{signature: 2}]}), 'ERR_MODULE_FRAMING'],
        [() => niFixture({modules: [{records: [record()]}]}), 'ERR_PROCEDURE_ABSENT'],
        [() => niFixture({modules: [{records: [proc(), record(0x1102)]}]}), 'ERR_SYMBOL_UNSUPPORTED'],
        [() => niFixture({modules: [{records: [proc(), proc({size: 0x31})]}]}), 'ERR_PROCEDURE_CONFLICT'],
        [() => niFixture({modules: [{records: [proc(), proc({size: 0})]}]}), 'ERR_PROCEDURE_SIZE'],
    ]) {
        const diagnostic = pack(make()), ordinary = pack(make());
        redactedThrows(() => diagnoseTailFrameNamedStreamNiMacZero(
            diagnostic.reader, diagnostic.exe, diagnostic.identity, query()), code);
        redactedThrows(() => inspectNamedStreamNiMacZero(ordinary.reader, ordinary.exe, ordinary.identity, query()), code);
        a.deepEqual(diagnostic.reads, ordinary.reads);
    }
    const p = pack(niFixture()); let reads = 0;
    const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
    for (const q of [undefined, {}, query({grammar: 'NEVER_EXPORT'}), query({addresses: ['0x800000000000']})]) {
        redactedThrows(() => diagnoseTailFrameNamedStreamNiMacZero(reader, p.exe, p.identity, q), 'ERR_SELECTORS');
    }
    a.equal(reads, 0);
    for (const index of [1, 3, 8, 9]) for (const mode of ['throw', 'short', 'shared']) {
        const broken = {size: p.reader.size, read(o, n) {
            if (!p.entries[index].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
            if (mode === 'throw') throw Error('ERR_SOURCE_MAPPING/NEVER_EXPORT/private');
            return mode === 'shared' ? Buffer.from(new SharedArrayBuffer(n)) : Buffer.alloc(n - 1);
        }};
        redactedThrows(() => diagnoseTailFrameNamedStreamNiMacZero(broken, p.exe, p.identity, query()),
            mode === 'throw' ? 'ERR_PDB_FRAMING' : mode === 'shared' ? 'ERR_INPUT_BOUND' : 'ERR_SHORT_READ');
    }
});

test('tail frame context is private across repeated, nested, older API and mutated-result calls', () => {
    const f = tailFrameFixture(8, b => badIdentityMember(b, 8, 'frame', 0x1234)), outer = pack(f);
    let nested = false;
    const reader = {size: outer.reader.size, read(o, n) {
        if (!nested) {
            nested = true;
            tailFrameResult(tailFrame(tailFrameFixture(8, b => badIdentityMember(b, 8, 'frame', 65535)), tailQuery()),
                'segment_map', 'identity', 8, 'tail_candidate', 'frame', 65535);
            tailFrameResult(tailFrame());
            tailFrameResult(tailFrame(mappedNi(b => badIdentityMember(b, 0, 'frame', 0))),
                'segment_map', 'identity', 0, 'section', 'frame');
            identityMappingResult(sourceMappingIdentity(f, tailQuery()), 'segment_map', 'identity', 8, 'tail_candidate', 'frame');
            mappingResult(sourceMapping(f, tailQuery()), 'segment_map', 'identity');
        }
        return outer.reader.read(o, n);
    }};
    const result = diagnoseTailFrameNamedStreamNiMacZero(reader, outer.exe, outer.identity, tailQuery());
    tailFrameResult(result, 'segment_map', 'identity', 8, 'tail_candidate', 'frame', 0x1234); a.equal(nested, true);
    result.actual_frame = 9; result.descriptor_index = 999; result.identity_member = 'NEVER_EXPORT';
    for (let repeat = 0; repeat < 3; repeat++) {
        tailFrameParity(f, tailQuery(), 'segment_map', 'identity', 8, 'tail_candidate', 'frame', 0x1234);
        tailFrameParity(niFixture(), query());
        tailFrameParity(changedNi(b => b.streams[8][0] ^= 1), query(), 'section_binding', 'headers');
        redactedThrows(() => tailFrame(niFixture({modules: [{records: [record()]}]})), 'ERR_PROCEDURE_ABSENT');
    }
});

test('tail frame diagnostic exports only eight redacted fields and enforces both serialization ceilings', () => {
    const secret = 'NEVER_EXPORT/private/name', f = niFixture({sectionMap: segmentMap(),
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}),
        modules: [{module_name: secret, object_name: secret, records: [proc({name: secret})]}]});
    const accepted = tailFrame(f); tailFrameResult(accepted);
    f.exe.fill(0x5a, 0x200, 0x800); a.deepEqual(tailFrame(f), accepted);
    const sm = f.streams[3].subarray(f.scAt + f.sc.length);
    badIdentityMember(sm, 3, 'frame', 0x1234); badIdentityMember(sm, 3, 'offset', 0xdeadbeef);
    const refusal = tailFrame(f); tailFrameResult(refusal, 'segment_map', 'identity', 3, 'tail_candidate', 'frame', 0x1234);
    for (const result of [accepted, refusal]) {
        const json = JSON.stringify(result);
        for (const word of [secret, 'deadbeef', '3735928559', 'guid', 'address', 'bytes', 'flags', 'length',
            'records', 'hash', '0x', 'ERR_SOURCE_MAPPING', 'absolute']) a.ok(!json.includes(word));
        a.ok(Object.values(result).every(v => v === null || typeof v === 'string' || Number.isInteger(v)));
    }
    const stringify = JSON.stringify;
    for (const call of [() => tailFrame(), () => tailFrame(f)]) {
        for (const bytes of [1023, 1024]) {
            JSON.stringify = function(value, ...args) {
                const json = stringify.call(this, value, ...args);
                return value && value.profile === TAIL_FRAME_PROFILE ? json.padEnd(bytes, ' ') : json;
            };
            try {
                if (bytes === 1023) a.equal(call().profile, TAIL_FRAME_PROFILE);
                else redactedThrows(call, 'ERR_OUTPUT_BOUND');
            } finally { JSON.stringify = stringify; }
        }
    }
    JSON.stringify = function(value, ...args) {
        const json = stringify.call(this, value, ...args);
        return value && value.profile === NI_PROFILE ? json.padEnd(LIMITS.output, ' ') : json;
    };
    try {
        redactedThrows(() => tailFrame(), 'ERR_OUTPUT_BOUND');
        tailFrameResult(tailFrame(f), 'segment_map', 'identity', 3, 'tail_candidate', 'frame', 0x1234);
    } finally { JSON.stringify = stringify; }
    JSON.stringify = function(value, ...args) {
        if (value && value.profile === TAIL_FRAME_PROFILE) throw Error('NEVER_EXPORT/private-serializer');
        return stringify.call(this, value, ...args);
    };
    try { for (const call of [() => tailFrame(), () => tailFrame(f)]) redactedThrows(call, 'ERR_PDB_FRAMING'); }
    finally { JSON.stringify = stringify; }
});
