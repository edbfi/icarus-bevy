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
    inspectNamedStreamNiMacZeroAbsoluteTailFrameZero,
    inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
    inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExactThunk3FramedOpaqueTail,
    diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero,
    diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
    diagnoseThunk32OrdinalNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
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

// Independently authored candidate-variant envelopes only. No original tuple,
// extent, standard producer, ABI or ownership qualification is asserted.
const ZERO_TAIL_PROFILE = 'MSF7_DBI_C13_scoped_supported_procedure_declarations_' +
    'named_stream_ni_mac_zero_v1_absolute_tail_frame_zero_v1';
const runZeroTail = (f = zeroTailFixture(), q = query()) => {
    const p = pack(f);
    return inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(p.reader, p.exe, p.identity, q);
};
// Replace only the authored DBI map, retaining header mutations and all other
// substreams. The shared fixture source stays untouched.
function replaceTestMap(f, sm) {
    const start = f.scAt + f.sc.length, old = f.streams[3];
    f.streams[3] = Buffer.concat([old.subarray(0, start), sm, old.subarray(f.optionalAt)]);
    f.streams[3].writeInt32LE(sm.length, 32);
    f.sectionMap = sm; f.optionalAt = start + sm.length;
    return f;
}
function authoredZeroTailMap(f) {
    const n = f.exe.readUInt16LE(f.exe.readUInt32LE(60) + 6), sm = Buffer.alloc(4 + (n + 1) * 20);
    sm.writeUInt16LE(n + 1); sm.writeUInt16LE(n + 1, 2);
    for (let i = 0; i < n; i++) {
        const at = 4 + i * 20, header = f.table + i * 40, flags = f.exe.readUInt32LE(header + 36);
        sm.writeUInt16LE(0x108 | ((flags >>> 30) & 1) | (((flags >>> 31) & 1) << 1) |
            (((flags >>> 29) & 1) << 2), at);
        sm.writeUInt16LE(i + 1, at + 6); sm.writeUInt32LE(f.exe.readUInt32LE(header + 8), at + 16);
        sm.writeUInt16LE(0xffff, at + 8); sm.writeUInt16LE(0xffff, at + 10);
    }
    sm.writeUInt16LE(0x208, 4 + n * 20); sm.writeUInt32LE(0xffffffff, 20 + n * 20);
    return sm;
}
function zeroTailFixture(options = {}) {
    const f = niFixture(options); return replaceTestMap(f, authoredZeroTailMap(f));
}
function ordinalTailTwin(f) {
    const twin = {...f, streams: f.streams.slice()}, start = f.scAt + f.sc.length;
    const sm = Buffer.from(f.streams[3].subarray(start, f.optionalAt)), n = sm.readUInt16LE() - 1;
    sm.writeUInt16LE(n + 1, 10 + n * 20);
    return replaceTestMap(twin, sm);
}
function expectedZeroTailFacts(n) {
    return {grammar: 'identity_sections_absolute_tail_frame_zero_v1',
        physical_section_count: n, descriptor_count: n + 1, tail_descriptor_index: n,
        tail_overlay: 0, tail_group: 0, tail_frame: 0, tail_offset: 0, tail_flags: 520,
        tail_length: 4294967295, tail_routable: false};
}
function zeroTailResultParity(f, q = query()) {
    const n = f.exe.readUInt16LE(f.exe.readUInt32LE(60) + 6), modern = pack(f), old = pack(ordinalTailTwin(f));
    const previous = inspectNamedStreamNiMacZero(old.reader, old.exe, old.identity, q);
    const result = inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(modern.reader, modern.exe, modern.identity, q);
    a.deepEqual(result, {...previous, profile: ZERO_TAIL_PROFILE, segment_map_profile: expectedZeroTailFacts(n)});
    a.deepEqual(Reflect.ownKeys(result), [...Reflect.ownKeys(previous), 'segment_map_profile']);
    a.deepEqual(Reflect.ownKeys(result.segment_map_profile), Reflect.ownKeys(expectedZeroTailFacts(n)));
    a.equal(Reflect.ownKeys(result.segment_map_profile).length, 11);
    a.deepEqual(modern.reads, old.reads);
    a.ok(modern.reads.every(r => r.count <= LIMITS.read));
    a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= 65536);
    return result;
}
function zeroTailNonmappingParity(f, code, q = query()) {
    const p = pack(f);
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(p.reader, p.exe, p.identity, q), code);
    for (const api of [inspectNamedStreamNiMacZero, diagnoseSourceMappingNamedStreamNiMacZero,
        diagnoseSourceMappingIdentityNamedStreamNiMacZero, diagnoseTailFrameNamedStreamNiMacZero]) {
        const old = pack(ordinalTailTwin(f));
        redactedThrows(() => api(old.reader, old.exe, old.identity, q), code);
        a.deepEqual(p.reads, old.reads);
    }
}
function zeroTailMapRefusal(f) {
    const p = pack(f);
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(p.reader, p.exe, p.identity, query()),
        'ERR_SOURCE_MAPPING');
    for (const mod of f.modules) a.ok(!p.reads.some(r =>
        p.entries[mod.stream].pages.includes(Math.floor(r.offset / 4096))));
}

test('zero-tail profile accepts eight and 96 sections and the minimum two-section code/metadata envelope', () => {
    const two = niFixture();
    two.exe.copy(two.exe, two.table + 40, two.table + 80, two.table + 120);
    two.exe.writeUInt16LE(2, two.exe.readUInt32LE(60) + 6);
    two.streams[8] = Buffer.from(two.exe.subarray(two.table, two.table + 80));
    replaceTestMap(two, authoredZeroTailMap(two));
    for (const [f, q] of [[two, query()], [zeroTailFixture(), query()],
        ...[8, 96].map(n => {
            const f = identityLimitFixture(n); replaceTestMap(f, authoredZeroTailMap(f)); return [f, tailQuery()];
        })]) {
        const result = zeroTailResultParity(f, q);
        a.equal(result.addresses[0].extent.segment, 1);
        a.equal(result.routing.scope, 'all_matching_section_contribution_modules');
        a.equal(result.routing.complete_table_framing, true);
        a.equal(result.selected_modules.length, 1);
        a.equal(result.addresses[0].extent.size, 0x30);
    }
    // PE admits one section, but separate code and noncode debug storage cannot
    // both fit its permission contract. No exception is added for this variant.
    const one = niFixture(); one.exe.writeUInt16LE(1, one.exe.readUInt32LE(60) + 6);
    one.streams[8] = Buffer.from(one.exe.subarray(one.table, one.table + 40));
    replaceTestMap(one, authoredZeroTailMap(one));
    zeroTailNonmappingParity(one, 'ERR_IDENTITY');
    for (const n of [0, 97]) {
        const f = zeroTailFixture(); f.exe.writeUInt16LE(n, f.exe.readUInt32LE(60) + 6);
        redactedThrows(() => runZeroTail(f), 'ERR_SOURCE_MAPPING');
    }
});

test('zero-tail profile preserves NI grammar, declaration extents/digests and all scoped coverage assertions', () => {
    const secret = 'NEVER_EXPORT_MININOTM/private/variant';
    for (const features of [[], [VC140], [VC110], [VC140, VC110]]) {
        for (const version of [SC60, SC2]) for (const kind of [0x110f, 0x1110, 0x1146, 0x1147]) {
            const f = zeroTailFixture({version, info: namedStreamNiMacZeroInfo({features,
                names: [{name: secret, stream: 8}]}), modules: [
                {module_name: secret, object_name: secret, records: [record(), proc({kind, name: secret}), record(0x114d)]},
                {records: [proc({kind}), proc({offset: 0x90, size: 0xffffffff})]},
            ]});
            for (const q of [query(), query({module_indices: [1, 0]})]) {
                const r = zeroTailResultParity(f, q);
                a.equal(r.addresses[0].procedure_record_count, 2);
                a.equal(r.opaque_records, 2); a.equal(r.names_exported, false); a.equal(r.code_bytes_read, false);
                a.equal(r.info_profile.grammar, 'named_stream_ni_mac_zero_v1');
                a.equal(r.info_profile.named_stream_ni_mac, 0);
                a.equal(r.routing.scope, q.module_indices ? 'explicit_modules_only' : 'all_matching_section_contribution_modules');
                a.equal(r.routing.complete_table_framing, !q.module_indices);
                a.ok(!JSON.stringify(r).includes(secret));
            }
        }
    }
});

test('zero-tail is explicit and every old mapping API retains strict frames and exact diagnostic schemas', () => {
    for (const n of [3, 8, 96]) {
        const f = n === 3 ? zeroTailFixture() : identityLimitFixture(n);
        if (n !== 3) replaceTestMap(f, authoredZeroTailMap(f));
        const q = n === 3 ? query() : tailQuery();
        tailFrameParity(f, q, 'segment_map', 'identity', n, 'tail_candidate', 'frame', 0);
        const legacy = {...f, streams: f.streams.slice()}; legacy.streams[1] = info();
        fails(legacy, 'ERR_SOURCE_MAPPING', q);
        const baseline = zeroTailResultParity(f, q);
        a.equal(diagnoseNi(f).status, 'accepted_profile'); // Info-only APIs do not map sections.
        a.deepEqual(runZeroTail(f, q), baseline);
    }
    for (const sectionMap of [Buffer.alloc(0), segmentMap(false), segmentMap(true)]) {
        const f = niFixture({sectionMap});
        const before = runNi(f); tailFrameParity(f, query());
        zeroTailMapRefusal(f);
        a.deepEqual(runNi(f), before); a.ok(!Object.hasOwn(before, 'segment_map_profile'));
        const legacy = {...f, streams: f.streams.slice()}; legacy.streams[1] = info();
        a.ok(!Object.hasOwn(run(legacy), 'segment_map_profile'));
    }
    // Query keys cannot select a profile on either old or new entry point.
    const p = pack(zeroTailFixture()); let reads = 0;
    const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
    for (const api of [inspect, inspectNamedStreamNiMacZero, inspectNamedStreamNiMacZeroAbsoluteTailFrameZero,
        diagnoseSourceMappingNamedStreamNiMacZero, diagnoseSourceMappingIdentityNamedStreamNiMacZero,
        diagnoseTailFrameNamedStreamNiMacZero]) {
        redactedThrows(() => api(reader, p.exe, p.identity,
            query({segment_map_profile: 'identity_sections_absolute_tail_frame_zero_v1'})), 'ERR_SELECTORS');
    }
    a.equal(reads, 0);
});

test('zero-tail requires exact present N+1 counts and bytes, positional tail and no extra descriptors', () => {
    const good = zeroTailFixture().sectionMap;
    const malformed = [Buffer.alloc(4), good.subarray(0, -4), good.subarray(0, -20),
        Buffer.concat([good, Buffer.alloc(4)]), Buffer.concat([good, good.subarray(-20)]),
        Buffer.concat([good.subarray(0, 4), good.subarray(-20), good.subarray(4, -20)])];
    for (const count of [0, 2, 3, 5, 65535]) for (const offset of [0, 2]) {
        const b = Buffer.from(good); b.writeUInt16LE(count, offset); malformed.push(b);
    }
    const five = Buffer.concat([good, good.subarray(-20)]);
    five.writeUInt16LE(5); five.writeUInt16LE(5, 2); malformed.push(five);
    for (const sm of malformed) zeroTailMapRefusal(niFixture({sectionMap: sm}));
    for (const length of [1, 2, 3]) {
        redactedThrows(() => runZeroTail(niFixture({sectionMap: good.subarray(0, -length)})), 'ERR_PDB_FRAMING');
    }
});

test('zero-tail validates every ordinary identity/flags/length and each full tail tuple member exactly', () => {
    const base = identityLimitFixture(8); replaceTestMap(base, authoredZeroTailMap(base));
    const refuse = change => {
        const f = {...base, streams: base.streams.slice()}, sm = Buffer.from(base.sectionMap);
        change(sm); replaceTestMap(f, sm); zeroTailMapRefusal(f);
    };
    for (let i = 0; i < 8; i++) {
        for (const member of identityMembers) refuse(b => badIdentityMember(b, i, member,
            member === 'frame' ? 0 : member === 'offset' ? 0xffffffff : 1));
        for (const flags of [0, 0x208, 0x8000 | base.sectionMap.readUInt16LE(4 + i * 20),
            0x200 | base.sectionMap.readUInt16LE(4 + i * 20)]) {
            refuse(b => b.writeUInt16LE(flags, 4 + i * 20));
        }
        refuse(b => b.writeUInt16LE(b.readUInt16LE(4 + i * 20) | 0x80, 4 + i * 20));
        for (const length of [0, 1, 0xffffffff]) refuse(b => b.writeUInt32LE(length, 20 + i * 20));
    }
    for (const member of ['overlay', 'group']) for (const value of [1, 65535]) {
        refuse(b => badIdentityMember(b, 8, member, value));
    }
    for (const frame of [1, 8, 9, 65535]) refuse(b => badIdentityMember(b, 8, 'frame', frame));
    for (const offset of [1, 0x80000000, 0xffffffff]) refuse(b => badIdentityMember(b, 8, 'offset', offset));
    for (const flags of [0, 0x108, 0x209, 0x20c, 0x288, 0x308, 0x8208, 65535]) {
        refuse(b => b.writeUInt16LE(flags, 4 + 8 * 20));
    }
    for (const length of [0, 1, 0x80000000, 0xfffffffe]) refuse(b => b.writeUInt32LE(length, 20 + 8 * 20));
    // Name/class indices remain outside the interpreted mapping contract.
    const names = zeroTailFixture(), sm = Buffer.from(names.sectionMap);
    for (let i = 0; i < 4; i++) { sm.writeUInt16LE(123, 12 + i * 20); sm.writeUInt16LE(456, 14 + i * 20); }
    replaceTestMap(names, sm); zeroTailResultParity(names);
});

test('zero-tail malformed full tuples leave all old APIs and their first-refusal reads isolated', () => {
    for (const [change, member] of [
        [b => badIdentityMember(b, 3, 'overlay', 1), 'overlay'],
        [b => badIdentityMember(b, 3, 'group', 1), 'group'],
        [b => badIdentityMember(b, 3, 'offset', 0xffffffff), 'frame'],
        [b => b.writeUInt16LE(0x209, 64), 'frame'],
        [b => b.writeUInt32LE(0xfffffffe, 80), 'frame'],
    ]) {
        const f = zeroTailFixture(), sm = Buffer.from(f.sectionMap);
        change(sm); replaceTestMap(f, sm); zeroTailMapRefusal(f);
        tailFrameParity(f, query(), 'segment_map', 'identity', 3, 'tail_candidate', member,
            member === 'frame' ? 0 : null);
        const legacy = {...f, streams: f.streams.slice()}; legacy.streams[1] = info();
        fails(legacy, 'ERR_SOURCE_MAPPING');
    }
});

test('zero-tail keeps identity short circuit order, exact field reads and no second traversal on refusal', () => {
    for (const index of [0, 2, 3]) for (let first = 0; first < identityMembers.length; first++) {
        const f = zeroTailFixture(), sm = Buffer.from(f.sectionMap);
        for (const member of identityMembers.slice(first)) badIdentityMember(sm, index, member,
            member === 'offset' ? 0xdeadbeef : 0x1234);
        sm.writeUInt16LE(0, 4 + index * 20); sm.writeUInt32LE(0, 20 + index * 20);
        replaceTestMap(f, sm);
        const trace = segmentFieldTrace(f, () => zeroTailMapRefusal(f)), at = 4 + index * 20;
        a.deepEqual(trace.filter(r => r.offset >= at), [0, 2, 4, 6, 12].slice(0, first + 2)
            .map(offset => ({width: offset === 12 ? 4 : 2, offset: at + offset})));
    }
    const f = zeroTailFixture();
    const trace = segmentFieldTrace(f, () => runZeroTail(f));
    a.equal(trace.filter(r => r.offset === 70).length, 1); // Only tail frame read.
    const bad = Buffer.from(f.sectionMap); bad.writeUInt32LE(1, 80); replaceTestMap(f, bad);
    zeroTailMapRefusal(f);
});

test('zero-tail never routes segment zero, N+1 or larger, including nonmatching supported procedures', () => {
    for (const segment of [0, 4, 65535]) {
        zeroTailNonmappingParity(zeroTailFixture({contributions: [{}, {segment, offset: 0x90}]}), 'ERR_ROUTING_FRAMING');
        for (const kind of [0x110f, 0x1110, 0x1146, 0x1147]) for (const offset of [0x20, 0x90]) {
            const f = zeroTailFixture({modules: [{records: [proc(), proc({kind, segment, offset})]}]});
            redactedThrows(() => runZeroTail(f), 'ERR_SOURCE_MAPPING');
            mappingResult(sourceMapping(ordinalTailTwin(f)), 'procedure', 'segment');
        }
    }
    for (const c of [{offset: -1}, {size: -1}, {offset: 0x7fffffff}, {size: 0x7fffffff}, {module_index: 1}]) {
        zeroTailNonmappingParity(zeroTailFixture({contributions: [{}, c]}), 'ERR_ROUTING_FRAMING');
    }
    for (const n of [8, 96]) for (const segment of [0, n + 1]) {
        const f = identityLimitFixture(n); replaceTestMap(f, authoredZeroTailMap(f));
        f.streams[3].writeUInt16LE(segment, f.scAt + 4);
        zeroTailNonmappingParity(f, 'ERR_ROUTING_FRAMING', tailQuery());
        const g = identityLimitFixture(n); replaceTestMap(g, authoredZeroTailMap(g));
        g.streams[9].writeUInt16LE(segment, 4 + 36);
        redactedThrows(() => runZeroTail(g, tailQuery()), 'ERR_SOURCE_MAPPING');
    }
});

test('zero-tail preserves every non-map physical mapping guard after exact tuple validation', () => {
    for (const [, stage, guard, make, q = query()] of mappingCases) {
        if (stage === 'segment_map') continue;
        const f = make(); replaceTestMap(f, authoredZeroTailMap(f));
        redactedThrows(() => runZeroTail(f, q), 'ERR_SOURCE_MAPPING');
        mappingResult(sourceMapping(ordinalTailTwin(f), q), stage, guard);
    }
    const f = zeroTailFixture(); f.streams[3].writeUInt16LE(8, f.optionalAt + 6);
    zeroTailNonmappingParity(f, 'ERR_OMAP_UNSUPPORTED');
    for (const change of [g => g.exe[g.rsds + 4] ^= 1,
        g => g.exe.writeUInt32LE(0x3000, g.debug + 24),
        g => g.exe.writeUInt32LE(0xe0000040, g.table + 80 + 36),
        g => g.exe.writeUInt32LE(0x3000, g.op + 128)]) {
        const g = niFixture(); change(g);
        if (g.exe.readUInt32LE(g.op + 128)) g.exe.writeUInt32LE(16, g.op + 132);
        g.streams[8] = Buffer.from(g.exe.subarray(g.table, g.table + 120));
        replaceTestMap(g, authoredZeroTailMap(g));
        zeroTailNonmappingParity(g, 'ERR_IDENTITY');
    }
});

test('zero-tail preserves NI/name/feature failures without skipping unknown words or resolving FASTLINK', () => {
    for (const [b, code] of [
        [info(), 'ERR_NI_MAC_UNSUPPORTED'],
        [namedStreamNiMacZeroInfo({niBytes: 0, features: []}), 'ERR_PDB_FRAMING'],
        ...[1, 0xffffffff, MINI].map(niMac => [namedStreamNiMacZeroInfo({niMac}), 'ERR_NI_MAC_UNSUPPORTED']),
        ...[[MINI], [NOTM]].map(features => [namedStreamNiMacZeroInfo({features}), 'ERR_FASTLINK_UNSUPPORTED']),
        ...[[0], [0xffffffff], [VC140, VC140], [VC110, VC140]].map(features =>
            [namedStreamNiMacZeroInfo({features}), 'ERR_FEATURE_UNSUPPORTED']),
        [namedStreamNiMacZeroInfo({capacity: 0}), 'ERR_PDB_FRAMING'],
        [namedStreamNiMacZeroInfo({names: [{name: 'NEVER_EXPORT', stream: 65535}]}), 'ERR_PDB_FRAMING'],
        [namedStreamNiMacZeroInfo({names: [{name: 'a', stream: 8}], present: [], presentWords: 0}), 'ERR_PDB_FRAMING'],
        [namedStreamNiMacZeroInfo({extraTail: Buffer.alloc(1)}), 'ERR_PDB_FRAMING'],
    ]) zeroTailNonmappingParity(zeroTailFixture({info: b}), code);
});

test('zero-tail completes later selected modules and rejects post-match framing, absence, conflict and extent failures', () => {
    const malformed = record(); malformed.writeUInt16LE(65535);
    for (const [records, code] of [
        [[proc(), malformed], 'ERR_MODULE_FRAMING'],
        [[proc(), record(0x1102)], 'ERR_SYMBOL_UNSUPPORTED'],
        ...[0x115f, 0x1160, 0x1161, 0x1167].map(kind => [[proc(), record(kind)], 'ERR_FASTLINK_UNSUPPORTED']),
        [[proc(), proc({size: 0x31})], 'ERR_PROCEDURE_CONFLICT'],
        [[proc(), proc({size: 0})], 'ERR_PROCEDURE_SIZE'],
    ]) {
        zeroTailNonmappingParity(zeroTailFixture({modules: [{records}]}), code);
        const f = zeroTailFixture({modules: [{records: [proc()]}, {records}]});
        for (const q of [query(), query({module_indices: [1, 0]})]) zeroTailNonmappingParity(f, code, q);
        a.equal(runZeroTail(f, query({module_indices: [0]})).addresses[0].procedure_record_count, 1);
    }
    for (const options of [{modules: [{records: []}]}, {modules: [{records: [record()]}]}]) {
        zeroTailNonmappingParity(zeroTailFixture(options), 'ERR_PROCEDURE_ABSENT');
    }
    zeroTailNonmappingParity(zeroTailFixture(), 'ERR_PROCEDURE_ABSENT', query({addresses: [VA, '0x140001021']}));
    const agreed = zeroTailResultParity(zeroTailFixture({modules: [
        {records: [proc(), proc()]}, {records: [proc()]},
    ]}));
    a.equal(agreed.addresses[0].procedure_record_count, 3); a.equal(agreed.unique_owner_confirmed, false);
    for (const [options, code] of [
        [{moduleInfo: Buffer.alloc(60)}, 'ERR_DBI_MODULE_INFO'],
        [{modules: [{signature: 2}]}, 'ERR_MODULE_FRAMING'],
        [{modules: [{sym: 5}]}, 'ERR_MODULE_FRAMING'],
        [{modules: [{c11: Buffer.alloc(4)}]}, 'ERR_MODULE_FRAMING'],
        [{modules: [{flags: 0x100}]}, 'ERR_REFERENCE_UNSUPPORTED'],
        [{sc: Buffer.alloc(0)}, 'ERR_ROUTING_ABSENT'],
        [{version: 1}, 'ERR_ROUTING_UNSUPPORTED'],
    ]) zeroTailNonmappingParity(zeroTailFixture(options), code);
    for (const change of [f => f.streams[9] = null,
        f => f.streams[3].writeUInt16LE(8, dbiModuleAt(f) + 34)]) {
        const f = zeroTailFixture(); change(f); zeroTailNonmappingParity(f, 'ERR_MODULE_BOUND');
    }
    for (const options of [{sc: Buffer.alloc(0)}, {version: 1}]) {
        const r = zeroTailResultParity(zeroTailFixture(options), query({module_indices: [0]}));
        a.equal(r.routing.scope, 'explicit_modules_only'); a.equal(r.routing.complete_table_framing, false);
    }
});

test('zero-tail keeps canonical endpoints and mapped file limits independent of the absolute sentinel', () => {
    const f = zeroTailFixture({modules: [{records: [proc({offset: 0x27f, size: 1})]}],
        contributions: [{offset: 0x27f, size: 1}]});
    f.exe.writeBigUInt64LE(0xffff800000000000n, f.op + 24);
    const q = query({addresses: ['0xffff80000000127f']});
    a.equal(zeroTailResultParity(f, q).addresses[0].extent.end_exclusive, '0xffff800000001280');
    const g = niFixture({modules: [{records: [proc({offset: 0xff0, size: 0x10})]}],
        contributions: [{offset: 0xff0, size: 0x10}]});
    const old = g.exe; g.exe = Buffer.alloc(0x1600); old.copy(g.exe);
    g.exe.writeBigUInt64LE(0x7fffffff0000n, g.op + 24);
    g.exe.writeUInt32LE(0xf000, g.op + 16); g.exe.writeUInt32LE(0x10000, g.op + 56);
    g.exe.writeUInt32LE(0xf000, g.table + 12); g.exe.writeUInt32LE(0x1000, g.table + 8);
    g.exe.writeUInt32LE(0x1000, g.table + 16); g.exe.writeUInt32LE(0x1200, g.table + 40 + 20);
    g.exe.writeUInt32LE(0x1400, g.table + 80 + 20);
    old.subarray(0x800, 0xa00).copy(g.exe, 0x1400); g.exe.writeUInt32LE(0x14a0, 0x1480 + 24);
    g.streams[8] = Buffer.from(g.exe.subarray(g.table, g.table + 120)); replaceTestMap(g, authoredZeroTailMap(g));
    for (const address of ['0x7ffffffffff0', '0x7fffffffffff']) {
        redactedThrows(() => runZeroTail(g, query({addresses: [address]})), 'ERR_SOURCE_MAPPING');
    }
});

test('zero-tail validates queries before I/O and keeps reader failures code-only and copied/nonshared', () => {
    const p = pack(zeroTailFixture()); let reads = 0;
    const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
    for (const q of [undefined, {}, query({grammar: 'NEVER_EXPORT'}), query({addresses: ['0x800000000000']}),
        query({addresses: Array(1)}), query({module_indices: [0, 0]})]) {
        redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(reader, p.exe, p.identity, q), 'ERR_SELECTORS');
    }
    a.equal(reads, 0);
    for (const index of [1, 3, 8, 9]) for (const mode of ['throw', 'short', 'shared']) {
        const broken = {size: p.reader.size, read(o, n) {
            if (!p.entries[index].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
            if (mode === 'throw') throw Error('ERR_SOURCE_MAPPING/NEVER_EXPORT/private');
            return mode === 'shared' ? Buffer.from(new SharedArrayBuffer(n)) : Buffer.alloc(n - 1);
        }};
        redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(broken, p.exe, p.identity, query()),
            mode === 'throw' ? 'ERR_PDB_FRAMING' : mode === 'shared' ? 'ERR_INPUT_BOUND' : 'ERR_SHORT_READ');
    }
    let borrowed = null;
    const recycled = {size: p.reader.size, read(o, n) {
        // Mutating a previously returned buffer must not mutate retained parser
        // state. Each current read still supplies the correct immutable bytes.
        if (borrowed !== null) borrowed.fill(0xee);
        borrowed = p.reader.read(o, n); return borrowed;
    }};
    a.deepEqual(inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(recycled, p.exe, p.identity, query()), runZeroTail());
    for (const size of [55, LIMITS.pdb + 1]) {
        redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero({size, read() {
            a.fail('reader bounds precede I/O');
        }}, p.exe, p.identity, query()), 'ERR_INPUT_BOUND');
    }
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(p.reader,
        Buffer.from(new SharedArrayBuffer(p.exe.length)), p.identity, query()), 'ERR_INPUT_BOUND');
});

test('zero-tail retains selection/module/contribution resource caps and bounded reads', () => {
    const addresses = Array.from({length: 32}, (_, i) => '0x' + (0x140001020n + BigInt(4 * i)).toString(16)).reverse();
    const records = addresses.map(v => proc({offset: Number(BigInt(v) - 0x140001000n), size: 4, name: ''}));
    const r = zeroTailResultParity(zeroTailFixture({modules: Array.from({length: 32}, () => ({records}))}), query({addresses}));
    a.deepEqual(r.addresses.map(v => v.address), addresses); a.equal(r.selected_modules.length, 32);
    zeroTailNonmappingParity(zeroTailFixture({modules: Array.from({length: 33}, () => ({}))}), 'ERR_MODULE_BOUND');
    for (const [count, options, code] of [[LIMITS.contribution_records + 1, {}, 'ERR_ROUTING_BOUND'],
        [1, {declaredBytes: LIMITS.contributions + 4}, 'ERR_ROUTING_BOUND']]) {
        const f = repeatedContributions(zeroTailFixture(), count, options);
        redactedThrows(() => runZeroTail(f), code);
    }
    const f = repeatedContributions(zeroTailFixture(), 150000), p = pack(f);
    const result = inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(p.reader, p.exe, p.identity, query());
    a.equal(result.routing.contribution_records, 150000); a.ok(p.reads.every(v => v.count <= LIMITS.read));
    for (const [bytes, code] of [[LIMITS.symbols, 'ERR_MODULE_FRAMING'], [LIMITS.symbols + 4, 'ERR_MODULE_BOUND']]) {
        zeroTailNonmappingParity(zeroTailFixture({modules: [{sym: bytes, body: repeatedModule(bytes, record(), 2)}]}), code);
    }
});

test('zero-tail selection stays private across nested calls, older APIs, refusal and mutated result facts', () => {
    const f = zeroTailFixture(), p = pack(f); let nested = false;
    const reader = {size: p.reader.size, read(o, n) {
        if (!nested) {
            nested = true;
            redactedThrows(() => runNi(f), 'ERR_SOURCE_MAPPING');
            tailFrameResult(tailFrame(f), 'segment_map', 'identity', 3, 'tail_candidate', 'frame', 0);
            mappingResult(sourceMapping()); identityMappingResult(sourceMappingIdentity());
            redactedThrows(() => runZeroTail(niFixture()), 'ERR_SOURCE_MAPPING');
            a.equal(runZeroTail().profile, ZERO_TAIL_PROFILE);
        }
        return p.reader.read(o, n);
    }};
    const r = inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(reader, p.exe, p.identity, query());
    a.equal(nested, true); a.deepEqual(r, runZeroTail());
    r.segment_map_profile.tail_frame = 9; r.segment_map_profile.tail_routable = true;
    for (let repeat = 0; repeat < 3; repeat++) {
        zeroTailResultParity(f); tailFrameParity(f, query(), 'segment_map', 'identity', 3, 'tail_candidate', 'frame', 0);
        tailFrameParity(niFixture(), query());
        const failed = zeroTailFixture(); failed.streams[3].writeUInt32LE(0, failed.optionalAt - 4);
        redactedThrows(() => runZeroTail(failed), 'ERR_SOURCE_MAPPING');
        zeroTailResultParity(f);
    }
});

test('zero-tail output adds only validated facts, redacts raw data and applies the full unchanged JSON/newline ceiling', () => {
    const secret = 'NEVER_EXPORT/private/zero-tail', f = zeroTailFixture({
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}),
        modules: [{module_name: secret, object_name: secret, records: [proc({name: secret}),
            proc({offset: 0x90, name: 'UNSELECTED_SECRET'})]}],
    });
    const accepted = zeroTailResultParity(f), json = JSON.stringify(accepted);
    for (const word of [secret, 'UNSELECTED_SECRET', 'record_bytes_hex', 'private.obj', '0x140001090']) {
        a.ok(!json.includes(word));
    }
    f.exe.fill(0x5a, 0x200, 0x800); a.deepEqual(runZeroTail(f), accepted);
    const stringify = JSON.stringify;
    for (const bytes of [65535, 65536]) {
        let complete = false;
        JSON.stringify = function(value, ...args) {
            const out = stringify.call(this, value, ...args);
            if (!value || value.profile !== ZERO_TAIL_PROFILE) return out;
            a.deepEqual(value.segment_map_profile, expectedZeroTailFacts(3));
            a.equal(value.complete_selected_symbol_record_framing, true);
            a.equal(value.addresses[0].procedure_record_count, 1);
            complete = true; return out.padEnd(bytes, ' ');
        };
        try {
            if (bytes === 65535) a.equal(runZeroTail().profile, ZERO_TAIL_PROFILE);
            else redactedThrows(() => runZeroTail(), 'ERR_OUTPUT_BOUND');
            a.equal(complete, true);
            // All old result schemas and ceilings are independent of the new one.
            a.equal(runNi().profile, NI_PROFILE); mappingResult(sourceMapping());
            identityMappingResult(sourceMappingIdentity()); tailFrameResult(tailFrame());
        } finally { JSON.stringify = stringify; }
    }
    JSON.stringify = function(value, ...args) {
        if (value && value.profile === ZERO_TAIL_PROFILE) throw Error('NEVER_EXPORT/private/serializer');
        return stringify.call(this, value, ...args);
    };
    try { redactedThrows(() => runZeroTail(), 'ERR_PDB_FRAMING'); }
    finally { JSON.stringify = stringify; }
    a.equal(LIMITS.output, 65536);
});

// Closed discriminator diagnostic: all values below enumerate source policy,
// never an observed target kind. Shared fixture ownership remains unchanged.
const UNSUPPORTED_KIND_PROFILE = 'MSF7_procedure_unsupported_kind_zero_tail_diagnostic_v1';
const guardedSymbolKinds = [
    0x0104, 0x0105, 0x0106, 0x0204, 0x0205, 0x0206, 0x0300, 0x0301,
    0x0400, 0x0403, 0x100a, 0x100b, 0x1010, 0x1011, 0x1015, 0x1016,
    0x101a, 0x101b, 0x1020, 0x1021, 0x1022, 0x1023, 0x1024, 0x1025, 0x1026, 0x1027, 0x1028,
    0x1102, 0x1114, 0x1115, 0x1118, 0x1119, 0x111c, 0x111d, 0x111e, 0x111f,
    0x1120, 0x1121, 0x1122, 0x1123, 0x112a, 0x112b, 0x112c, 0x112d, 0x1132,
    0x1148, 0x1149, 0x114a, 0x114b, 0x1154, 0x1155, 0x1156, 0x1157, 0x1158, 0x1168,
];
function unsupportedSymbol(f = zeroTailFixture(), q = query()) {
    const p = pack(f);
    return diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(p.reader, p.exe, p.identity, q);
}
function unsupportedSymbolResult(result, kind = null) {
    a.deepEqual(result, {profile: UNSUPPORTED_KIND_PROFILE,
        status: kind === null ? 'accepted_profile' : 'refused_symbol',
        stage: kind === null ? 'complete' : 'procedure_metadata',
        guard: kind === null ? null : 'unsupported_kind', kind_u16: kind});
    a.deepEqual(Reflect.ownKeys(result), ['profile', 'status', 'stage', 'guard', 'kind_u16']);
    a.ok(Buffer.byteLength(JSON.stringify(result) + '\n', 'utf8') <= 1024);
    if (kind !== null) {
        a.ok(Number.isInteger(result.kind_u16) && !Object.is(result.kind_u16, -0));
        a.ok(guardedSymbolKinds.includes(result.kind_u16));
    }
}
function unsupportedSymbolParity(f, {kind = null, code = null, q = query()} = {}) {
    const diagnostic = pack(f), ordinary = pack(f);
    const call = () => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(
        diagnostic.reader, diagnostic.exe, diagnostic.identity, q);
    if (code !== null) redactedThrows(call, code);
    else unsupportedSymbolResult(call(), kind);
    if (code !== null || kind !== null) {
        redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(
            ordinary.reader, ordinary.exe, ordinary.identity, q), code || 'ERR_SYMBOL_UNSUPPORTED');
    } else {
        a.equal(inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(
            ordinary.reader, ordinary.exe, ordinary.identity, q).profile, ZERO_TAIL_PROFILE);
    }
    a.deepEqual(diagnostic.reads, ordinary.reads);
    a.ok(diagnostic.reads.every(r => r.count <= LIMITS.read));
    return diagnostic;
}

test('unsupported-symbol diagnostic covers every unchanged guarded kind and preserves legacy code-only errors', () => {
    a.equal(guardedSymbolKinds.length, 55); a.equal(new Set(guardedSymbolKinds).size, 55);
    for (const kind of guardedSymbolKinds) {
        const f = zeroTailFixture({modules: [{records: [proc(), record(kind)]}]});
        for (const q of [query(), query({module_indices: [0]})]) {
            unsupportedSymbolParity(f, {kind, q});
            zeroTailNonmappingParity(f, 'ERR_SYMBOL_UNSUPPORTED', q);
            const strict = ordinalTailTwin(f); strict.streams[1] = info();
            fails(strict, 'ERR_SYMBOL_UNSUPPORTED', q);
        }
    }
});

test('unsupported-symbol first kind follows record order and ascending selected modules, never matches or query order', () => {
    for (const [first, second] of [[0x1168, 0x0104], [0x0104, 0x1168]]) {
        unsupportedSymbolParity(zeroTailFixture({modules: [
            {records: [proc(), record(first), record(second)]},
        ]}), {kind: first});
        const f = zeroTailFixture({modules: [{records: [proc(), record(first)]},
            {records: [record(second), proc()]}], contributions: [{module_index: 1}, {module_index: 0}]});
        for (const q of [query(), query({module_indices: [1, 0]})]) unsupportedSymbolParity(f, {kind: first, q});
        unsupportedSymbolParity(f, {kind: second, q: query({module_indices: [1]})});
    }
    // Earlier valid metadata and an address miss do not prevent a prefix refusal.
    unsupportedSymbolParity(zeroTailFixture({modules: [{records: [proc({offset: 0x90}), record(0x112c)]}]}),
        {kind: 0x112c});
});

test('unsupported-symbol diagnostic retains EXTERNAL, record framing and selected-stream preflight precedence', () => {
    for (const kind of [0x115f, 0x1160, 0x1161, 0x1167]) {
        for (const modules of [[{records: [proc(), record(kind), record(0x0104)]}],
            [{records: [proc(), record(kind)]}, {records: [record(0x0104)]}]]) {
            unsupportedSymbolParity(zeroTailFixture({modules}), {code: 'ERR_FASTLINK_UNSUPPORTED'});
        }
        unsupportedSymbolParity(zeroTailFixture({modules: [{records: [record(0x0104), record(kind)]}]}),
            {kind: 0x0104});
    }
    for (const length of [0, 1, 3, 65534, 65535]) {
        const bad = record(0x0104); bad.writeUInt16LE(length);
        unsupportedSymbolParity(zeroTailFixture({modules: [{records: [proc(), bad, record(0x1168)]}]}),
            {code: 'ERR_MODULE_FRAMING'});
    }
    for (const options of [{signature: 2}, {sym: 5}, {c11: Buffer.alloc(4)}]) {
        unsupportedSymbolParity(zeroTailFixture({modules: [{...options, records: [record(0x0104)]}]}),
            {code: 'ERR_MODULE_FRAMING'});
    }
    const badRefs = zeroTailFixture({modules: [{records: [record(0x0104)]}]});
    badRefs.streams[9].writeUInt32LE(4, badRefs.streams[9].length - 4);
    unsupportedSymbolParity(badRefs, {code: 'ERR_MODULE_FRAMING'});
    for (const change of [f => f.streams[12] = null,
        f => f.streams[3].writeUInt16LE(9, dbiModuleAt(f, 1) + 34),
        f => f.streams[3].writeUInt16LE(8, dbiModuleAt(f, 1) + 34)]) {
        const f = zeroTailFixture({modules: [{records: [record(0x0104)]}, {}]}); change(f);
        const p = unsupportedSymbolParity(f, {code: 'ERR_MODULE_BOUND'});
        a.ok(!p.reads.some(r => p.entries[9].pages.includes(Math.floor(r.offset / 4096))));
    }
    unsupportedSymbolParity(zeroTailFixture({modules: [
        {records: [record(0x0104)]}, {flags: 0x100},
    ]}), {code: 'ERR_REFERENCE_UNSUPPORTED'});
    unsupportedSymbolParity(zeroTailFixture({modules: Array.from({length: 33}, () =>
        ({records: [record(0x0104)]}))}), {code: 'ERR_MODULE_BOUND'});
    const sym = LIMITS.symbols + 4;
    unsupportedSymbolParity(zeroTailFixture({modules: [{sym, body: repeatedModule(sym, record(0x0104))}]}),
        {code: 'ERR_MODULE_BOUND'});
});

test('record-count guard precedes an unsupported kind at the first record beyond its unchanged ceiling', () => {
    const sym = 4 + 40 + LIMITS.records * 4, original = repeatedModule(sym);
    const body = {...original, read(o, n) {
        const b = original.read(o, n);
        // Replace only the final repeated record's kind; keep framing valid.
        for (const [at, byte] of [[sym - 2, 4], [sym - 1, 1]]) if (at >= o && at < o + n) b[at - o] = byte;
        return b;
    }};
    unsupportedSymbolParity(zeroTailFixture({modules: [{sym, body}]}), {code: 'ERR_MODULE_BOUND'});
});

test('unsupported-symbol refusal adds no reads and stops interpretation before later records, caches and modules', () => {
    const f = zeroTailFixture({modules: [{records: [proc(), record(0x1168), record(0x7777, 65536),
        record(0x7777, 65536), record(0x0104)]}, {records: [record(0x112c)]}]});
    unsupportedSymbolParity(f, {kind: 0x1168});
    const p = pack(f); let forbiddenReads = 0, kindReads = 0;
    const reader = {size: p.reader.size, read(o, n) {
        for (const index of [9, 12]) for (let i = 0; i < p.entries[index].pages.length; i++) {
            const page = p.entries[index].pages[i];
            if (o >= page * 4096 && o < (page + 1) * 4096) {
                const logical = i * 4096 + o % 4096;
                // Signature/trailer and the existing first 64 KiB prefetch
                // already read some suffix bytes. Only subsequent cache I/O
                // and the later module are forbidden, not all suffix byte I/O.
                if (index === 12 || (logical >= LIMITS.cache && logical < f.modules[0].symbols.length)) {
                    forbiddenReads++; throw Error('NEVER_EXPORT/later-cache-or-module');
                }
            }
        }
        return p.reader.read(o, n);
    }};
    const u16 = Buffer.prototype.readUInt16LE;
    Buffer.prototype.readUInt16LE = function(offset = 0) {
        if (this.length === 4 && offset === 2) {
            kindReads++;
            a.ok(kindReads <= 2, 'only the supported header and first rejected header are interpreted');
        }
        return u16.call(this, offset);
    };
    try {
        unsupportedSymbolResult(diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(
            reader, p.exe, p.identity, query()), 0x1168);
    } finally { Buffer.prototype.readUInt16LE = u16; }
    a.equal(kindReads, 2); a.equal(forbiddenReads, 0);
    a.ok(p.reads.some(r => r.offset === p.entries[9].pages[0] * 4096 && r.count === 4096));
});

test('unsupported-symbol acceptance requires every selected module and final absence, conflict and ordinary output checks', () => {
    const clean = zeroTailFixture({modules: [{records: [proc()]},
        {records: [record(0x114d), proc(), proc({offset: 0x90, size: 0xffffffff})]}]});
    for (const q of [query(), query({module_indices: [1, 0]})]) {
        const p = unsupportedSymbolParity(clean, {q});
        a.ok(p.reads.some(r => p.entries[12].pages.includes(Math.floor(r.offset / 4096))));
    }
    const malformed = record(); malformed.writeUInt16LE(65535);
    for (const [records, code, kind] of [
        [[record(0x0104)], null, 0x0104], [[malformed], 'ERR_MODULE_FRAMING'],
        [[proc({size: 0x31})], 'ERR_PROCEDURE_CONFLICT'], [[proc({size: 0})], 'ERR_PROCEDURE_SIZE'],
        [[proc({segment: 4})], 'ERR_SOURCE_MAPPING'], [[record(0x115f)], 'ERR_FASTLINK_UNSUPPORTED'],
    ]) {
        for (const modules of [[{records: [proc(), ...records]}], [{records: [proc()]}, {records}]]) {
            unsupportedSymbolParity(zeroTailFixture({modules}), {code, kind});
        }
    }
    unsupportedSymbolParity(zeroTailFixture({modules: [{records: [record()]}]}), {code: 'ERR_PROCEDURE_ABSENT'});
    unsupportedSymbolParity(clean, {q: query({addresses: [VA, '0x140001021']}), code: 'ERR_PROCEDURE_ABSENT'});
    const stringify = JSON.stringify;
    JSON.stringify = function(value, ...args) {
        const out = stringify.call(this, value, ...args);
        return value && value.profile === ZERO_TAIL_PROFILE ? out.padEnd(LIMITS.output, ' ') : out;
    };
    try {
        redactedThrows(() => unsupportedSymbol(clean), 'ERR_OUTPUT_BOUND');
        unsupportedSymbolResult(unsupportedSymbol(zeroTailFixture({modules: [{records: [record(0x0104)]}]})), 0x0104);
    } finally { JSON.stringify = stringify; }
});

test('unsupported-symbol diagnostic keeps exact zero-tail grammar and earlier identity, info, routing and selector guards', () => {
    const good = zeroTailFixture().sectionMap;
    const maps = [Buffer.alloc(0), segmentMap(false), segmentMap(true), good.subarray(0, -20),
        Buffer.concat([good, Buffer.alloc(4)])];
    for (const [offset, value, width] of [[0, 3, 2], [2, 3, 2], [10, 0, 2],
        [64, 0x209, 2], [66, 1, 2], [68, 1, 2], [70, 4, 2], [76, 1, 4], [80, 0xfffffffe, 4]]) {
        const sm = Buffer.from(good); sm[width === 2 ? 'writeUInt16LE' : 'writeUInt32LE'](value, offset); maps.push(sm);
    }
    for (const sectionMap of maps) unsupportedSymbolParity(niFixture({sectionMap,
        modules: [{records: [record(0x0104)]}]}), {code: 'ERR_SOURCE_MAPPING'});
    unsupportedSymbolParity(niFixture({sectionMap: good.subarray(0, -1)}), {code: 'ERR_PDB_FRAMING'});
    for (const [make, code] of [
        [() => zeroTailFixture({info: info()}), 'ERR_NI_MAC_UNSUPPORTED'],
        [() => zeroTailFixture({info: namedStreamNiMacZeroInfo({features: [MINI]})}), 'ERR_FASTLINK_UNSUPPORTED'],
        [() => zeroTailFixture({info: namedStreamNiMacZeroInfo({features: [0]})}), 'ERR_FEATURE_UNSUPPORTED'],
        [() => zeroTailFixture({version: 1}), 'ERR_ROUTING_UNSUPPORTED'],
        [() => zeroTailFixture({sc: Buffer.alloc(0)}), 'ERR_ROUTING_ABSENT'],
        [() => zeroTailFixture({moduleInfo: Buffer.alloc(60)}), 'ERR_DBI_MODULE_INFO'],
    ]) unsupportedSymbolParity(make(), {code});
    for (const [change, code] of [[f => f.exe[f.rsds + 4] ^= 1, 'ERR_IDENTITY'],
        [f => f.streams[3].writeUInt16LE(8, f.optionalAt + 6), 'ERR_OMAP_UNSUPPORTED'],
        [f => f.streams[8][0] ^= 1, 'ERR_SOURCE_MAPPING']]) {
        const f = zeroTailFixture(); change(f); unsupportedSymbolParity(f, {code});
    }
    for (const [, stage, , make, q = query()] of mappingCases) {
        if (stage === 'segment_map') continue;
        const f = make(); replaceTestMap(f, authoredZeroTailMap(f));
        unsupportedSymbolParity(f, {code: 'ERR_SOURCE_MAPPING', q});
    }
    const p = pack(zeroTailFixture()); let reads = 0;
    const reader = {size: p.reader.size, read(o, n) { reads++; return p.reader.read(o, n); }};
    for (const q of [undefined, {}, query({diagnose: true}), query({module_indices: [0, 0]}),
        query({addresses: Array(1)})]) redactedThrows(() =>
        diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(reader, p.exe, p.identity, q), 'ERR_SELECTORS');
    a.equal(reads, 0);
});

test('unsupported-symbol diagnostic isolates repeated and reentrant calls and preserves unrelated or forged errors', () => {
    const refused = zeroTailFixture({modules: [{records: [record(0x1168)]}]}), clean = zeroTailFixture();
    let captured;
    try { runZeroTail(refused); } catch (error) { captured = error; }
    a.equal(captured.code, 'ERR_SYMBOL_UNSUPPORTED');
    for (let repeat = 0; repeat < 3; repeat++) {
        unsupportedSymbolResult(unsupportedSymbol(refused), 0x1168);
        unsupportedSymbolResult(unsupportedSymbol(clean));
        redactedThrows(() => unsupportedSymbol(zeroTailFixture({modules: [{records: []}]})), 'ERR_PROCEDURE_ABSENT');
        unsupportedSymbolResult(unsupportedSymbol(refused), 0x1168);
        const p = pack(clean); let reads = 0;
        const reader = {size: p.reader.size, read() { reads++; throw Error('NEVER_EXPORT'); }};
        a.throws(() => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(reader, p.exe, p.identity,
            {get addresses() { throw captured; }}), error => error === captured);
        a.equal(reads, 0); // Typed code alone has no site in this invocation.
        redactedThrows(() => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(
            reader, p.exe, p.identity, {get addresses() { throw {code: 'ERR_SYMBOL_UNSUPPORTED'}; }}), 'ERR_PDB_FRAMING');
    }
    for (const [outerFixture, expectedKind] of [[clean, null], [refused, 0x1168]]) {
        const p = pack(outerFixture); let nested = false;
        const reader = {size: p.reader.size, read(o, n) {
            if (!nested) {
                nested = true;
                unsupportedSymbolResult(unsupportedSymbol(zeroTailFixture({modules: [{records: [record(0x0104)]}]})), 0x0104);
                unsupportedSymbolResult(unsupportedSymbol(clean));
                redactedThrows(() => runZeroTail(refused), 'ERR_SYMBOL_UNSUPPORTED');
            }
            return p.reader.read(o, n);
        }};
        unsupportedSymbolResult(diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(
            reader, p.exe, p.identity, query()), expectedKind);
        a.equal(nested, true);
    }
});

test('unsupported-symbol reader failures retain legacy redaction, bounds and immutable copied nonshared storage', () => {
    const f = zeroTailFixture({modules: [{records: [proc(), record(0x0104)]}]}), p = pack(f);
    for (const index of [1, 3, 8, 9]) for (const mode of ['throw', 'forged', 'short', 'shared', 'foreign']) {
        const ForeignShared = require('node:vm').runInNewContext('SharedArrayBuffer');
        const reader = {size: p.reader.size, read(o, n) {
            if (!p.entries[index].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
            if (mode === 'throw') throw Error('ERR_SYMBOL_UNSUPPORTED/NEVER_EXPORT/private');
            if (mode === 'forged') throw {code: 'ERR_SYMBOL_UNSUPPORTED', message: 'NEVER_EXPORT'};
            if (mode === 'short') return Buffer.alloc(n - 1);
            return Buffer.from(mode === 'shared' ? new SharedArrayBuffer(n) : new ForeignShared(n));
        }};
        redactedThrows(() => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(
            reader, p.exe, p.identity, query()), mode === 'short' ? 'ERR_SHORT_READ' :
            mode === 'throw' || mode === 'forged' ? 'ERR_PDB_FRAMING' : 'ERR_INPUT_BOUND');
    }
    for (const size of [55, LIMITS.pdb + 1]) redactedThrows(() =>
        diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero({size, read() { a.fail('no I/O'); }},
            p.exe, p.identity, query()), 'ERR_INPUT_BOUND');
    redactedThrows(() => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(p.reader,
        Buffer.from(new SharedArrayBuffer(p.exe.length)), p.identity, query()), 'ERR_INPUT_BOUND');
    let borrowed = null;
    const reader = {size: p.reader.size, read(o, n) {
        if (borrowed !== null) borrowed.fill(0xee);
        borrowed = p.reader.read(o, n); return borrowed;
    }};
    unsupportedSymbolResult(diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero(
        reader, p.exe, p.identity, query()), 0x0104);
});

// Test-only lexical access in a separate VM. Real u16 reads cannot produce
// negative zero, nonintegers or out-of-range values, and the private site is
// inaccessible to callers. These fault-injection cases test defensive branches;
// they do not claim those branches are reachable with immutable real inputs.
// This changes neither the production exports nor the fixture builder module.
function unsupportedSymbolHarness() {
    const filename = require.resolve('../pdb_procedure_ranges.cjs');
    const source = require('node:fs').readFileSync(filename, 'utf8');
    const sandbox = {module: {exports: {}}, require: require('node:module').createRequire(filename), Buffer};
    require('node:vm').runInNewContext(source + `
module.exports = {legalUnsupportedKind, MetadataError,
    diagnose: diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero,
    setInspector(fn) { inspectProfile = fn; }};
`, sandbox, {filename});
    return sandbox.module.exports;
}

test('unsupported-symbol defensive kind validation rejects negative zero and nonmembers without a new public API', () => {
    const h = unsupportedSymbolHarness();
    for (const kind of guardedSymbolKinds) a.equal(h.legalUnsupportedKind(kind), true);
    for (const kind of [null, undefined, -0, 0, -1, 1.5, NaN, Infinity, 0x10000, '260', 260n,
        {}, [], 0x110f, 0x1147, 0x115f, 0x1167, 0x7777]) {
        a.equal(h.legalUnsupportedKind(kind), false);
        const error = new h.MetadataError('ERR_SYMBOL_UNSUPPORTED');
        h.setInspector((...args) => { args[7].first = kind; throw error; });
        a.throws(() => h.diagnose(), actual => actual === error);
    }
    for (const [site, error] of [[null, new h.MetadataError('ERR_SYMBOL_UNSUPPORTED')],
        [0x0104, new h.MetadataError('ERR_PROCEDURE_ABSENT')],
        [0x0104, {code: 'ERR_SYMBOL_UNSUPPORTED'}], [0x0104, Error('NEVER_EXPORT')]]) {
        h.setInspector((...args) => { args[7].first = site; throw error; });
        if (error instanceof h.MetadataError) a.throws(() => h.diagnose(), actual => actual === error);
        else redactedThrows(() => h.diagnose(), 'ERR_PDB_FRAMING');
    }
    h.setInspector((...args) => { args[7].first = 0x0104; });
    redactedThrows(() => h.diagnose(), 'ERR_SYMBOL_UNSUPPORTED'); // No successful branch with a retained site.
    h.setInspector((...args) => {
        a.equal(args[4], true); a.equal(args[5], null);
        a.equal(args[6], 'identity_sections_absolute_tail_frame_zero_v1');
        a.equal(args[7].first, null);
        args[7].first = 0x0104; throw new h.MetadataError('ERR_SYMBOL_UNSUPPORTED');
    });
    a.equal(h.diagnose().kind_u16, 0x0104);
    a.ok(!Object.hasOwn(require('../pdb_procedure_ranges.cjs'), 'legalUnsupportedKind'));
});

test('unsupported-symbol diagnostic exports five scalars without locations, partial metadata or private extras', () => {
    const secret = 'NEVER_EXPORT/private/unsupported', options = {
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}),
        modules: [{module_name: secret, object_name: secret, records: [proc({name: secret})]}],
    };
    const clean = zeroTailFixture(options), refused = zeroTailFixture({...options, modules: [
        {...options.modules[0], records: [...options.modules[0].records, record(0x1168, 32)]},
    ]});
    refused.streams[9].fill(0x5a, 4 + options.modules[0].records[0].length + 4, refused.modules[0].symbols.length);
    for (const [f, kind] of [[clean, null], [refused, 0x1168]]) {
        const result = unsupportedSymbol(f), json = JSON.stringify(result);
        unsupportedSymbolResult(result, kind);
        for (const word of [secret, 'module', 'stream', 'offset', 'ordinal', 'length', 'record', 'name',
            'address', 'guid', 'sha', 'digest', 'extent', 'coverage', 'buffer', 'stack', '0x']) a.ok(!json.includes(word));
        a.ok(Object.values(result).every(v => v === null || typeof v === 'string' || Number.isInteger(v)));
        result.kind_u16 = 0x7777; result.extra = secret;
        unsupportedSymbolResult(unsupportedSymbol(f), kind);
    }
});

test('unsupported-symbol diagnostic enforces UTF-8 plus newline 1024-byte bound and serializer redaction in both branches', () => {
    const refused = zeroTailFixture({modules: [{records: [record(0x0104)]}]}), stringify = JSON.stringify;
    for (const call of [() => unsupportedSymbol(), () => unsupportedSymbol(refused)]) {
        for (const bytes of [1023, 1024]) {
            JSON.stringify = function(value, ...args) {
                const out = stringify.call(this, value, ...args);
                if (!value || value.profile !== UNSUPPORTED_KIND_PROFILE) return out;
                const padding = bytes - Buffer.byteLength(out);
                return out + 'é'.repeat(Math.floor(padding / 2)) + (padding % 2 ? ' ' : '');
            };
            try {
                if (bytes === 1023) a.equal(call().profile, UNSUPPORTED_KIND_PROFILE);
                else redactedThrows(call, 'ERR_OUTPUT_BOUND');
            } finally { JSON.stringify = stringify; }
        }
    }
    for (const profile of [ZERO_TAIL_PROFILE, UNSUPPORTED_KIND_PROFILE]) {
        JSON.stringify = function(value, ...args) {
            if (value && value.profile === profile) throw Error('NEVER_EXPORT/private-serializer');
            return stringify.call(this, value, ...args);
        };
        try {
            redactedThrows(() => unsupportedSymbol(), 'ERR_PDB_FRAMING');
            if (profile === UNSUPPORTED_KIND_PROFILE) redactedThrows(() => unsupportedSymbol(refused), 'ERR_PDB_FRAMING');
            else unsupportedSymbolResult(unsupportedSymbol(refused), 0x0104);
        } finally { JSON.stringify = stringify; }
    }
});

test('unsupported-symbol calls leave serialized old inspector and diagnostic results and errors unchanged', () => {
    const strict = fixture(), ni = niFixture(), zero = zeroTailFixture();
    const calls = [() => run(strict), () => runNi(ni), () => runZeroTail(zero),
        () => diagnose(strict), () => diagnoseNi(ni), () => sourceMapping(ni),
        () => sourceMappingIdentity(ni), () => tailFrame(ni),
        () => sourceMapping(zero), () => sourceMappingIdentity(zero), () => tailFrame(zero)];
    const before = calls.map(call => JSON.stringify(call()));
    for (let repeat = 0; repeat < 2; repeat++) {
        unsupportedSymbolResult(unsupportedSymbol(zero));
        unsupportedSymbolResult(unsupportedSymbol(zeroTailFixture({modules: [{records: [record(0x1168)]}]})), 0x1168);
        a.deepEqual(calls.map(call => JSON.stringify(call())), before);
        zeroTailNonmappingParity(zeroTailFixture({modules: [{records: [proc(), record(0x0104)]}]}), 'ERR_SYMBOL_UNSUPPORTED');
        a.deepEqual(runZeroTail(zero).segment_map_profile, expectedZeroTailFacts(3));
    }
});

// Authored auxiliary framing only: these values are arbitrary u32 slots, never
// target IDs, references or evidence of valid semantic CodeView relationships.
const INLINEES_GRAMMAR = 'inlinees_counted_u32_exact_v1';
const INLINEES_PROFILE = ZERO_TAIL_PROFILE + '_' + INLINEES_GRAMMAR;
const runInlinees = (f = zeroTailFixture(), q = query()) => {
    const p = pack(f);
    return inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(p.reader, p.exe, p.identity, q);
};
function inlineesRecord(values = [], {count = values.length, bytes = 8 + values.length * 4} = {}) {
    const b = record(0x1168, bytes);
    if (bytes >= 8) b.writeUInt32LE(count, 4);
    values.forEach((v, i) => { if (12 + i * 4 <= bytes) b.writeUInt32LE(v, 8 + i * 4); });
    return b;
}
function inlineesFacts(result, records, entries) {
    a.equal(result.profile, INLINEES_PROFILE);
    a.deepEqual(result.inlinees_profile, {grammar: INLINEES_GRAMMAR, records, entries});
    a.deepEqual(Reflect.ownKeys(result.inlinees_profile), ['grammar', 'records', 'entries']);
    a.ok(Number.isSafeInteger(records) && records >= 0 && Number.isSafeInteger(entries) && entries >= 0);
    a.ok(records <= result.opaque_records);
    a.equal(result.records, result.supported_procedure_records + result.opaque_records);
    for (const m of result.selected_modules) a.equal(m.records, m.supported_procedure_records + m.opaque_records);
    a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= LIMITS.output);
    return result;
}
function inlineesNoListParity(f = zeroTailFixture(), q = query()) {
    const modern = pack(f), previous = pack(f);
    const before = inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(previous.reader, previous.exe, previous.identity, q);
    const result = inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(modern.reader, modern.exe, modern.identity, q);
    a.deepEqual(result, {...before, profile: INLINEES_PROFILE,
        inlinees_profile: {grammar: INLINEES_GRAMMAR, records: 0, entries: 0}});
    a.deepEqual(Reflect.ownKeys(result), [...Reflect.ownKeys(before), 'inlinees_profile']);
    a.deepEqual(modern.reads, previous.reads);
    return inlineesFacts(result, 0, 0);
}
function inlineesRefusalParity(f, code, q = query()) {
    const modern = pack(f), previous = pack(f);
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        modern.reader, modern.exe, modern.identity, q), code);
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(
        previous.reader, previous.exe, previous.identity, q), code);
    a.deepEqual(modern.reads, previous.reads);
}

test('inlinees absent still emits its fixed profile and zero counters with identical old metadata, digests and reads', () => {
    for (const q of [query(), query({module_indices: [0]})]) {
        const r = inlineesNoListParity(zeroTailFixture(), q);
        a.equal(r.routing.scope, Object.hasOwn(q, 'module_indices') ?
            'explicit_modules_only' : 'all_matching_section_contribution_modules');
    }
    inlineesNoListParity(zeroTailFixture({modules: [{records: [record(0x114d), proc(), record(0x115d), record()]}]}));
    for (const n of [8, 96]) {
        const f = identityLimitFixture(n); replaceTestMap(f, authoredZeroTailMap(f));
        inlineesNoListParity(f, tailQuery());
    }
});

test('inlinees accepts exact empty lists before and after a match as opaque physical records only', () => {
    const empty = inlineesRecord(); a.equal(empty.toString('hex'), '0600681100000000');
    const p = proc(), f = zeroTailFixture({modules: [{records: [empty, p, empty]}]});
    const r = inlineesFacts(runInlinees(f), 2, 0), old = runZeroTail();
    a.deepEqual(r.addresses, old.addresses); a.equal(r.records, 3); a.equal(r.opaque_records, 2);
    a.equal(r.selected_modules[0].opaque_records, 2); a.equal(r.supported_procedure_records, 1);
    a.equal(r.symbol_bytes, 4 + p.length + 16);
    a.deepEqual(r.segment_map_profile, old.segment_map_profile);
});

test('inlinees accepts every u32 pattern, repeats and unsorted slots and sums lists across all selected modules', () => {
    const values = [0, 0xfff, 0x1000, 0x80000000, 0xffffffff, 0xf1f2f3f4, 0x20, 0x1020];
    for (const value of values) inlineesFacts(runInlinees(zeroTailFixture({modules: [
        {records: [inlineesRecord([value]), proc()]},
    ]})), 1, 1);
    const list = inlineesRecord([0xffffffff, 0, 0x80000000, 0, 0xfff]);
    const f = zeroTailFixture({modules: [{records: [list, proc(), list]},
        {records: [proc(), inlineesRecord(), list, record(0x114d), record(0x115d), record()]}]});
    for (const q of [query(), query({module_indices: [1, 0]})]) {
        const r = inlineesFacts(runInlinees(f, q), 4, 15);
        a.deepEqual(r.routing.indices, [0, 1]); a.equal(r.addresses[0].procedure_record_count, 2);
        a.deepEqual(r.selected_modules.map(m => m.opaque_records), [2, 5]);
        a.equal(r.opaque_records, 7); a.equal(r.records, 9);
        a.equal(r.addresses[0].procedure_records_sha256, createHash('sha256').update(proc()).update(proc()).digest('hex'));
    }
    inlineesFacts(runInlinees(f, query({module_indices: [1]})), 2, 5);
});

test('inlinees maximum exact record crosses the 64 KiB cache and reads only its count without interpreting slots', () => {
    const list = inlineesRecord(Array(16318).fill(0xffffffff)); a.equal(list.length, 65280);
    const f = zeroTailFixture({modules: [{records: [record(0x7777, 65516), list, proc()]}]}), p = pack(f);
    const readWord = Buffer.prototype.readUInt32LE, fields = [];
    Buffer.prototype.readUInt32LE = function(offset = 0) {
        if (this.length === 65280) fields.push(offset);
        return readWord.call(this, offset);
    };
    let r;
    try { r = inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(p.reader, p.exe, p.identity, query()); }
    finally { Buffer.prototype.readUInt32LE = readWord; }
    inlineesFacts(r, 1, 16318); a.deepEqual(fields, [4]); a.equal(r.opaque_records, 2);
    a.ok(p.reads.every(({offset, count}) => count <= LIMITS.read && offset + count <= p.reader.size));
    a.ok(p.reads.some(v => p.entries[9].pages[16] === Math.floor(v.offset / 4096)));
    a.deepEqual(r.addresses, runZeroTail().addresses);
    // All general-kind behavior and its larger record ceiling stay independent.
    inlineesNoListParity(zeroTailFixture({modules: [{records: [record(0x7777, 65536), proc()]}]}));
});

test('inlinees refuses missing or truncated count/payload, invalid outer framing and overflow-oriented counts', () => {
    const bad = [
        ...[4, 5, 6, 7, 9, 10, 11].map(bytes => inlineesRecord([], {bytes})),
        ...[1, 2, 0x3ffffffe, 0x40000000, 0x80000000, 0xffffffff].map(count => inlineesRecord([], {count})),
        ...[0, 2, 0x40000001, 0xffffffff].map(count => inlineesRecord([0], {count})),
    ];
    const shortPayload = inlineesRecord([0]); shortPayload.writeUInt16LE(14); bad.push(shortPayload);
    for (const reclen of [0, 1, 2, 3, 65535]) {
        const b = inlineesRecord(); b.writeUInt16LE(reclen); bad.push(b);
    }
    for (const b of bad) {
        redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records: [proc(), inlineesRecord(), b]}]})),
            'ERR_MODULE_FRAMING');
    }
    // A bounded aligned symbol extent with a final header claiming bytes beyond it.
    const f = zeroTailFixture({modules: [{records: [proc(), inlineesRecord([0])]}]});
    f.streams[3].writeUInt32LE(f.modules[0].symbols.length - 4, dbiModuleAt(f) + 36);
    redactedThrows(() => runInlinees(f), 'ERR_MODULE_FRAMING');
});

test('inlinees rejects every unexplained tail including zeros, invocation-like words and LF_PAD-like bytes', () => {
    for (const word of [0, 1, 17, 0x80000000, 0xf1f2f3f4, 0xffffffff]) {
        for (const b of [inlineesRecord([word], {count: 0}), inlineesRecord([0x1000, word], {count: 1})]) {
            redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records: [proc(), b]}]})),
                'ERR_MODULE_FRAMING');
        }
    }
    for (const extra of [1, 2, 3]) {
        const b = inlineesRecord([0x1000], {bytes: 12 + extra}); b.fill(0xf0 + extra, 12);
        redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records: [proc(), b]}]})), 'ERR_MODULE_FRAMING');
    }
});

test('inlinees exact equation does not bypass its stricter 0xff00 inclusive serialization ceiling', () => {
    for (const entries of [16319, 16382]) {
        const b = inlineesRecord(Array(entries).fill(0));
        a.equal(b.length, 8 + 4 * entries); a.ok(b.length > 65280 && b.length <= 65536);
        redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records: [proc(), b]}]})), 'ERR_MODULE_FRAMING');
    }
    a.equal(LIMITS.record, 65537); a.equal(LIMITS.cache, 65536);
});

test('inlinees retains all remaining 54 unsupported kinds and all four external kinds at every record position', () => {
    const remaining = guardedSymbolKinds.filter(k => k !== 0x1168); a.equal(remaining.length, 54);
    for (const kind of [...remaining, 0x115f, 0x1160, 0x1161, 0x1167]) {
        const code = remaining.includes(kind) ? 'ERR_SYMBOL_UNSUPPORTED' : 'ERR_FASTLINK_UNSUPPORTED';
        for (const records of [[record(kind), inlineesRecord(), proc()],
            [inlineesRecord([0xffffffff]), record(kind), proc()],
            [proc(), inlineesRecord(), record(kind)], [inlineesRecord(), proc(), record(kind)]]) {
            redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records}]})), code);
        }
        // Automatic union must include the second matching contribution module.
        const f = zeroTailFixture({modules: [{records: [inlineesRecord(), proc()]},
            {records: [proc(), inlineesRecord(), record(kind)]}]});
        redactedThrows(() => runInlinees(f), code);
        inlineesFacts(runInlinees(f, query({module_indices: [0]})), 1, 0);
    }
});

test('inlinees cannot satisfy absence or bypass late malformed records, conflicting extents or other selected modules', () => {
    const list = inlineesRecord([0x20, 0x1020, 0x40001020]);
    redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records: [list]}]})), 'ERR_PROCEDURE_ABSENT');
    redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records: [list, proc()]}]}),
        query({addresses: [VA, '0x140001021']})), 'ERR_PROCEDURE_ABSENT');
    const malformed = record(); malformed.writeUInt16LE(65535);
    for (const [records, code] of [
        [[list, proc(), inlineesRecord([0], {count: 2})], 'ERR_MODULE_FRAMING'],
        [[proc(), list, malformed], 'ERR_MODULE_FRAMING'],
        [[proc(), list, proc({size: 0x31})], 'ERR_PROCEDURE_CONFLICT'],
        [[proc(), list, proc({size: 0})], 'ERR_PROCEDURE_SIZE'],
        [[proc(), list, proc({segment: 4})], 'ERR_SOURCE_MAPPING'],
    ]) {
        redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records}]})), code);
        const f = zeroTailFixture({modules: [{records: [proc(), list]}, {records}]});
        for (const q of [query(), query({module_indices: [1, 0]})]) redactedThrows(() => runInlinees(f, q), code);
        inlineesFacts(runInlinees(f, query({module_indices: [0]})), 1, 3);
    }
    const conflict = zeroTailFixture({modules: [{records: [proc(), list]},
        {records: [inlineesRecord(), proc({size: 0x31})]}]});
    redactedThrows(() => runInlinees(conflict), 'ERR_PROCEDURE_CONFLICT');
});

test('inlinees opt-in preserves exact NI-zero, tail, mapping, identity, routing and physical guards', () => {
    for (const [, stage, , make, q = query()] of mappingCases) {
        if (stage === 'segment_map') continue;
        const f = make(); replaceTestMap(f, authoredZeroTailMap(f));
        inlineesRefusalParity(f, 'ERR_SOURCE_MAPPING', q);
    }
    for (const makeMap of [() => Buffer.alloc(0), () => segmentMap(false), () => segmentMap(true),
        f => { const b = authoredZeroTailMap(f); b.writeUInt32LE(1, b.length - 8); return b; },
        f => { const b = authoredZeroTailMap(f); b.writeUInt32LE(0, b.length - 4); return b; }]) {
        const f = zeroTailFixture(); replaceTestMap(f, makeMap(f)); inlineesRefusalParity(f, 'ERR_SOURCE_MAPPING');
    }
    for (const index of [0, 2, 3]) for (const offset of [0, 2, 4, 6, 12, 16]) {
        const f = zeroTailFixture(), sm = authoredZeroTailMap(f), at = 4 + index * 20 + offset;
        if (offset >= 12) sm.writeUInt32LE(offset === 12 ? 1 : 0, at);
        else sm.writeUInt16LE(offset === 0 ? 0 : 9, at);
        replaceTestMap(f, sm); inlineesRefusalParity(f, 'ERR_SOURCE_MAPPING');
    }
    for (const [b, code] of [
        [info(), 'ERR_NI_MAC_UNSUPPORTED'], [namedStreamNiMacZeroInfo({niBytes: 0, features: []}), 'ERR_PDB_FRAMING'],
        [namedStreamNiMacZeroInfo({niMac: 1}), 'ERR_NI_MAC_UNSUPPORTED'],
        ...[MINI, NOTM].map(feature => [namedStreamNiMacZeroInfo({features: [feature]}), 'ERR_FASTLINK_UNSUPPORTED']),
        ...[[0], [0xffffffff], [VC140, VC140], [VC110, VC140]].map(features =>
            [namedStreamNiMacZeroInfo({features}), 'ERR_FEATURE_UNSUPPORTED']),
        [namedStreamNiMacZeroInfo({capacity: 0}), 'ERR_PDB_FRAMING'],
    ]) inlineesRefusalParity(zeroTailFixture({info: b}), code);
    for (const [options, code] of [
        [{modules: [{signature: 2}]}, 'ERR_MODULE_FRAMING'], [{modules: [{c11: Buffer.alloc(4)}]}, 'ERR_MODULE_FRAMING'],
        [{modules: [{flags: 0x100}]}, 'ERR_REFERENCE_UNSUPPORTED'], [{sc: Buffer.alloc(0)}, 'ERR_ROUTING_ABSENT'],
        [{version: 1}, 'ERR_ROUTING_UNSUPPORTED'], [{contributions: [{module_index: 1}]}, 'ERR_ROUTING_FRAMING'],
    ]) inlineesRefusalParity(zeroTailFixture(options), code);
    for (const [change, code] of [
        [f => f.streams[3].writeUInt16LE(8, f.optionalAt + 6), 'ERR_OMAP_UNSUPPORTED'],
        [f => f.streams[3].writeUInt16LE(4, 56), 'ERR_REFERENCE_UNSUPPORTED'],
        [f => f.exe[f.rsds + 4] ^= 1, 'ERR_IDENTITY'],
        [f => f.streams[1].writeUInt32LE(2, 8), 'ERR_IDENTITY'],
        [f => f.streams[8][0] ^= 1, 'ERR_SOURCE_MAPPING'],
        [f => f.streams[3].writeUInt16LE(8, dbiModuleAt(f) + 34), 'ERR_MODULE_BOUND'],
        [f => f.streams[9] = null, 'ERR_MODULE_BOUND'],
    ]) { const f = zeroTailFixture(); change(f); inlineesRefusalParity(f, code); }
    for (const options of [{sc: Buffer.alloc(0)}, {version: 1}]) {
        const r = inlineesNoListParity(zeroTailFixture(options), query({module_indices: [0]}));
        a.equal(r.routing.scope, 'explicit_modules_only'); a.equal(r.routing.complete_table_framing, false);
    }
    for (const segment of [0, 4, 65535]) {
        const f = zeroTailFixture({modules: [{records: [inlineesRecord(), proc({segment})]}]});
        redactedThrows(() => runInlinees(f), 'ERR_SOURCE_MAPPING');
    }
});

test('inlinees rejects closed-query violations before any I/O including grammar knobs, sparse arrays and inherited keys', () => {
    const p = pack(zeroTailFixture()); let reads = 0;
    const reader = {size: p.reader.size, read() { reads++; throw Error('NEVER_EXPORT'); }};
    const qs = [undefined, null, [], {}, Object.create({addresses: [VA]}),
        query({grammar: INLINEES_GRAMMAR}), query({inlinees: true}), query({inlinees_profile: {}}),
        query({addresses: Array(1)}), query({addresses: [VA, VA]}), query({addresses: ['0x800000000000']}),
        query({module_indices: undefined}), query({module_indices: [0, 0]}), query({module_indices: Array(1)}),
        query({[Symbol('NEVER_EXPORT')]: true}), query({addresses: ['0x0140001020']}),
        query({addresses: Array.from({length: 33}, (_, i) => '0x' + (0x140001020n + BigInt(i)).toString(16))})];
    for (const q of qs) redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        reader, p.exe, p.identity, q), 'ERR_SELECTORS');
    a.equal(reads, 0);
    const q = Object.assign(Object.create(null), query()); inlineesNoListParity(zeroTailFixture(), q);
});

test('inlinees immutable-reader failures stay copied, nonshared, exact and code-only through a cross-cache list', () => {
    const list = inlineesRecord(Array(16318).fill(0xf1f2f3f4)), f = zeroTailFixture({modules: [
        {records: [record(0x7777, 65516), list, proc()]},
    ]}), p = pack(f), ForeignShared = require('node:vm').runInNewContext('SharedArrayBuffer');
    const call = reader => inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(reader, p.exe, p.identity, query());
    for (const index of [1, 3, 8, 9]) for (const mode of ['throw', 'short', 'long', 'view', 'shared', 'foreign']) {
        const page = index === 9 ? p.entries[9].pages[16] : p.entries[index].pages[0];
        const reader = {size: p.reader.size, read(o, n) {
            if (Math.floor(o / 4096) !== page) return p.reader.read(o, n);
            if (mode === 'throw') throw Error('NEVER_EXPORT/private/inlinees');
            if (mode === 'short') return Buffer.alloc(n - 1);
            if (mode === 'long') return Buffer.alloc(n + 1);
            if (mode === 'view') return new Uint8Array(n);
            return Buffer.from(new (mode === 'foreign' ? ForeignShared : SharedArrayBuffer)(n));
        }};
        redactedThrows(() => call(reader), mode === 'throw' ? 'ERR_PDB_FRAMING' :
            ['shared', 'foreign'].includes(mode) ? 'ERR_INPUT_BOUND' : 'ERR_SHORT_READ');
    }
    let borrowed = null;
    const reader = {size: p.reader.size, read(o, n) {
        if (borrowed !== null) borrowed.fill(0xee);
        borrowed = p.reader.read(o, n); return borrowed;
    }};
    a.deepEqual(call(reader), runInlinees(f));
    for (const size of [55, LIMITS.pdb + 1, NaN]) redactedThrows(() => call({size, read() {
        a.fail('reader size must refuse before I/O');
    }}), 'ERR_INPUT_BOUND');
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(p.reader,
        Buffer.from(new SharedArrayBuffer(p.exe.length)), p.identity, query()), 'ERR_INPUT_BOUND');
});

test('inlinees keeps real unchanged selector/module/symbol/record/contribution caps and preflight precedence', () => {
    a.equal(LIMITS.selectors, 32); a.equal(LIMITS.modules, 32); a.equal(LIMITS.symbols, 256 * 1024 * 1024);
    a.equal(LIMITS.records, 2500000); a.equal(LIMITS.read, 4 * 1024 * 1024);
    const addresses = Array.from({length: 32}, (_, i) => '0x' + (0x140001020n + BigInt(i * 4)).toString(16));
    const records = [inlineesRecord([0xffffffff]), ...addresses.map(v => proc({
        offset: Number(BigInt(v) - 0x140001000n), size: 4, name: '',
    }))];
    const r = inlineesFacts(runInlinees(zeroTailFixture({modules: Array.from({length: 32}, () => ({records}))}),
        query({addresses})), 32, 32);
    a.deepEqual(r.addresses.map(x => x.address), addresses); a.equal(r.selected_modules.length, 32);
    inlineesRefusalParity(zeroTailFixture({modules: Array.from({length: 33}, () => ({}))}), 'ERR_MODULE_BOUND');
    for (const sym of [LIMITS.symbols, LIMITS.symbols + 4]) inlineesRefusalParity(zeroTailFixture({modules: [
        {sym, body: repeatedModule(sym, record(), 2)},
    ]}), sym === LIMITS.symbols ? 'ERR_MODULE_FRAMING' : 'ERR_MODULE_BOUND');
    for (const [count, options] of [[LIMITS.contribution_records + 1, {}],
        [1, {declaredBytes: LIMITS.contributions + 4}]]) {
        inlineesRefusalParity(repeatedContributions(zeroTailFixture(), count, options), 'ERR_ROUTING_BOUND');
    }
    // Sparse authored input exercises the real production record ceiling. The
    // final valid empty list must hit the record guard before its interpretation.
    const sym = 4 + 40 + (LIMITS.records - 1) * 4 + 8, original = repeatedModule(sym);
    const last = inlineesRecord(), body = {...original, read(o, n) {
        const b = original.read(o, n);
        for (let i = 0; i < last.length; i++) if (sym - 8 + i >= o && sym - 8 + i < o + n) {
            b[sym - 8 + i - o] = last[i];
        }
        return b;
    }};
    redactedThrows(() => runInlinees(zeroTailFixture({modules: [{sym, body}]})), 'ERR_MODULE_BOUND');
    // Stream uniqueness and module flags are preflighted even when the first
    // module already contains a successful declaration and auxiliary list.
    const duplicate = zeroTailFixture({modules: [{records: [proc(), inlineesRecord()]}, {stream: 9}]});
    redactedThrows(() => runInlinees(duplicate), 'ERR_MODULE_BOUND');
    const flags = zeroTailFixture({modules: [{records: [proc(), inlineesRecord()]}, {flags: 0x100}]});
    redactedThrows(() => runInlinees(flags), 'ERR_REFERENCE_UNSUPPORTED');
});

test('inlinees selection and aggregate counters stay isolated across nested, repeated, old and diagnostic calls', () => {
    const f = zeroTailFixture({modules: [{records: [inlineesRecord([0, 0xffffffff]), proc()]}]}), p = pack(f);
    const expected = runInlinees(f); let nested = false;
    const reader = {size: p.reader.size, read(o, n) {
        if (!nested) {
            nested = true;
            a.deepEqual(runInlinees(f), expected); inlineesFacts(runInlinees(), 0, 0);
            redactedThrows(() => runZeroTail(f), 'ERR_SYMBOL_UNSUPPORTED');
            unsupportedSymbolResult(unsupportedSymbol(f), 0x1168);
            redactedThrows(() => runInlinees(zeroTailFixture({modules: [{records: [inlineesRecord([], {count: 1})]}]})),
                'ERR_MODULE_FRAMING');
        }
        return p.reader.read(o, n);
    }};
    a.deepEqual(inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(reader, p.exe, p.identity, query()), expected);
    a.equal(nested, true); expected.inlinees_profile.entries = 999; expected.inlinees_profile.extra = 'NEVER_EXPORT';
    for (let repeat = 0; repeat < 3; repeat++) {
        inlineesFacts(runInlinees(f), 1, 2); inlineesNoListParity();
        redactedThrows(() => runZeroTail(f), 'ERR_SYMBOL_UNSUPPORTED'); unsupportedSymbolResult(unsupportedSymbol(f), 0x1168);
    }
});

test('inlinees leaves every previous success, diagnostic contract and all 55 refusal kinds unchanged', () => {
    const calls = [() => run(), () => runNi(), () => runZeroTail(), () => diagnose(), () => diagnoseNi(),
        () => sourceMapping(), () => sourceMappingIdentity(), () => tailFrame(), () => unsupportedSymbol()];
    const before = calls.map(call => JSON.stringify(call()));
    for (const b of [inlineesRecord(), inlineesRecord([0xffffffff]), inlineesRecord([], {count: 0xffffffff})]) {
        const zero = zeroTailFixture({modules: [{records: [proc(), b]}]}), ni = ordinalTailTwin(zero);
        zeroTailNonmappingParity(zero, 'ERR_SYMBOL_UNSUPPORTED'); unsupportedSymbolParity(zero, {kind: 0x1168});
        const strict = {...ni, streams: ni.streams.slice()}; strict.streams[1] = info(); fails(strict, 'ERR_SYMBOL_UNSUPPORTED');
    }
    for (const kind of guardedSymbolKinds) {
        const f = zeroTailFixture({modules: [{records: [proc(), record(kind)]}]});
        unsupportedSymbolParity(f, {kind}); zeroTailNonmappingParity(f, 'ERR_SYMBOL_UNSUPPORTED');
    }
    inlineesFacts(runInlinees(zeroTailFixture({modules: [{records: [inlineesRecord([0]), proc()]}]})), 1, 1);
    a.deepEqual(calls.map(call => JSON.stringify(call())), before);
    // The existing discriminator still stops at 0x1168 without a payload read,
    // including malformed lists and later records that the new profile reaches.
    const f = zeroTailFixture({modules: [{records: [proc(), inlineesRecord(), record(0x0104)]}]});
    unsupportedSymbolParity(f, {kind: 0x1168}); redactedThrows(() => runInlinees(f), 'ERR_SYMBOL_UNSUPPORTED');
});

test('inlinees schema adds only aggregate framing facts and preserves redaction and the exact UTF-8/newline output bound', () => {
    const secret = 'NEVER_EXPORT/private/inlinees', f = zeroTailFixture({
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}),
        modules: [{module_name: secret, object_name: secret, records: [proc({name: secret}),
            inlineesRecord([0xdeadbeef, 0xdeadbeef, 0xf1f2f3f4])]}],
    });
    const r = inlineesFacts(runInlinees(f), 1, 3), old = runZeroTail();
    a.deepEqual(Reflect.ownKeys(r), [...Reflect.ownKeys(old), 'inlinees_profile']);
    a.deepEqual(Reflect.ownKeys(r.selected_modules[0]), Reflect.ownKeys(old.selected_modules[0]));
    a.deepEqual(Reflect.ownKeys(r.addresses[0]), Reflect.ownKeys(old.addresses[0]));
    for (const key of Object.keys(old).filter(k => typeof old[k] === 'boolean')) a.equal(r[key], old[key]);
    const json = JSON.stringify(r);
    for (const word of [secret, 'deadbeef', '3735928559', 'f1f2f3f4', 'record_bytes_hex', 'entries_values',
        'payloads_validated', 'inlinee_ids']) a.ok(!json.includes(word));
    f.exe.fill(0x55, 0x200, 0x800); a.deepEqual(runInlinees(f), r);
    const stringify = JSON.stringify;
    for (const bytes of [65535, 65536]) {
        let complete = false;
        JSON.stringify = function(value, ...args) {
            const out = stringify.call(this, value, ...args);
            if (!value || value.profile !== INLINEES_PROFILE) return out;
            a.deepEqual(value.inlinees_profile, {grammar: INLINEES_GRAMMAR, records: 1, entries: 3});
            complete = true;
            const padding = bytes - Buffer.byteLength(out);
            return out + 'é'.repeat(Math.floor(padding / 2)) + (padding % 2 ? ' ' : '');
        };
        try {
            if (bytes === 65535) a.equal(runInlinees(f).profile, INLINEES_PROFILE);
            else redactedThrows(() => runInlinees(f), 'ERR_OUTPUT_BOUND');
            a.equal(complete, true); a.equal(runZeroTail().profile, ZERO_TAIL_PROFILE);
            unsupportedSymbolResult(unsupportedSymbol());
        } finally { JSON.stringify = stringify; }
    }
    JSON.stringify = function(value, ...args) {
        if (value && value.profile === INLINEES_PROFILE) throw Error('NEVER_EXPORT/private/serializer');
        return stringify.call(this, value, ...args);
    };
    try { redactedThrows(() => runInlinees(f), 'ERR_PDB_FRAMING'); }
    finally { JSON.stringify = stringify; }
    a.equal(LIMITS.output, 65536);
});

// Defensive VM fault injection only: real callers cannot supply the lexical
// grammar selector, scan totals or retained counter state. These tests do not
// claim such states are reachable under the immutable-reader contract and do
// not alter any production limit, exports or the shared fixture module.
function inlineesDefensiveHarness() {
    const filename = require.resolve('../pdb_procedure_ranges.cjs');
    const source = require('node:fs').readFileSync(filename, 'utf8');
    const sandbox = {module: {exports: {}}, require: require('node:module').createRequire(filename), Buffer};
    require('node:vm').runInNewContext(source + `
module.exports = {scanModule,
    grammar: INLINEES_COUNTED_U32_EXACT_GRAMMAR,
    api: inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
    old: inspectNamedStreamNiMacZeroAbsoluteTailFrameZero,
    setInspector(fn) { inspectProfile = fn; }};
`, sandbox, {filename});
    return sandbox.module.exports;
}
function inlineesDefensiveScan(h, b, totals, grammar = h.grammar) {
    const body = Buffer.concat([Buffer.from([4, 0, 0, 0]), b, Buffer.alloc(4)]), sym = 4 + b.length;
    const m = {streams: [{size: body.length}], stream(i, o, n) {
        a.equal(i, 0); a.ok(o >= 0 && o + n <= body.length); return Buffer.from(body.subarray(o, o + n));
    }};
    return h.scanModule({m, image: {sections: []}},
        {stream_id: 0, module_index: 0, sym, c11: 0, c13: 0}, new Map(), totals, null, grammar);
}

test('inlinees defensive VM injection verifies exact lexical selector and post-validation counter updates', () => {
    const h = inlineesDefensiveHarness(); a.equal(h.grammar, INLINEES_GRAMMAR);
    const makeTotals = () => ({records: 0, procedures: 0, opaque: 0,
        inlinees: {grammar: INLINEES_GRAMMAR, records: 0, entries: 0}});
    for (const b of [inlineesRecord([], {count: 1}), inlineesRecord([0], {count: 0}),
        inlineesRecord([], {count: 0xffffffff}), record(0x1168), inlineesRecord(Array(16319).fill(0))]) {
        const totals = makeTotals();
        redactedThrows(() => inlineesDefensiveScan(h, b, totals), 'ERR_MODULE_FRAMING');
        a.deepEqual(totals.inlinees, makeTotals().inlinees);
        a.equal(totals.records, 1); a.equal(totals.opaque, 0);
    }
    for (const selector of [null, true, 0x1168, 'inlinees_counted_u32', 'INLINEES_COUNTED_U32_EXACT_V1', {}]) {
        const totals = makeTotals();
        redactedThrows(() => inlineesDefensiveScan(h, inlineesRecord(), totals, selector), 'ERR_SYMBOL_UNSUPPORTED');
        a.deepEqual(totals.inlinees, makeTotals().inlinees);
    }
    const totals = makeTotals(), r = inlineesDefensiveScan(h, inlineesRecord([0, 0xffffffff]), totals);
    a.equal(r.records, 1); a.equal(r.opaque_records, 1); a.equal(r.supported_procedure_records, 0);
    a.equal(totals.records, 1); a.equal(totals.opaque, 1); a.equal(totals.inlinees.records, 1);
    a.equal(totals.inlinees.entries, 2);
    let called = false;
    h.setInspector((...args) => {
        called = true;
        a.equal(args[4], true); a.equal(args[5], null);
        a.equal(args[6], 'identity_sections_absolute_tail_frame_zero_v1');
        a.equal(args[7], null); a.equal(args[8], INLINEES_GRAMMAR);
    });
    h.api(); a.equal(called, true);
    h.setInspector((...args) => { a.equal(args.length, 7); a.equal(args[8], undefined); }); h.old();
    a.ok(!Object.hasOwn(require('../pdb_procedure_ranges.cjs'), 'scanModule'));
});

test('inlinees defensive VM injection checks nonnegative exact aggregate bounds without weakening real caps', () => {
    const h = inlineesDefensiveHarness();
    for (const [records, entries] of [[-1, 0], [NaN, 0], [Infinity, 0], [1, 0],
        [0, -1], [0, NaN], [0, Infinity], [0, 0.5], [0, Math.floor(LIMITS.symbols / 4) + 1]]) {
        const totals = {records: 0, procedures: 0, opaque: 0,
            inlinees: {grammar: INLINEES_GRAMMAR, records, entries}};
        redactedThrows(() => inlineesDefensiveScan(h, inlineesRecord(), totals), 'ERR_MODULE_FRAMING');
        a.ok(Object.is(totals.inlinees.records, records)); a.ok(Object.is(totals.inlinees.entries, entries));
    }
});

const INLINEES_UNSUPPORTED_KIND_PROFILE = 'MSF7_procedure_unsupported_kind_inlinees_exact_diagnostic_v1';
const inlineesGuardedKinds = guardedSymbolKinds.filter(kind => kind !== 0x1168);
function unsupportedInlinees(f = zeroTailFixture(), q = query()) {
    const p = pack(f);
    return diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        p.reader, p.exe, p.identity, q);
}
function unsupportedInlineesResult(result, kind = null) {
    a.deepEqual(result, {profile: INLINEES_UNSUPPORTED_KIND_PROFILE,
        status: kind === null ? 'accepted_profile' : 'refused_symbol',
        stage: kind === null ? 'complete' : 'procedure_metadata',
        guard: kind === null ? null : 'unsupported_kind', kind_u16: kind});
    a.deepEqual(Reflect.ownKeys(result), ['profile', 'status', 'stage', 'guard', 'kind_u16']);
    a.ok(Object.values(result).every(v => v === null || typeof v === 'string' || typeof v === 'number'));
    if (kind !== null) {
        a.ok(Number.isSafeInteger(result.kind_u16) && result.kind_u16 >= 0 && result.kind_u16 <= 0xffff);
        a.equal(Object.is(result.kind_u16, -0), false);
        a.ok(inlineesGuardedKinds.includes(result.kind_u16));
    }
    a.ok(Buffer.byteLength(JSON.stringify(result) + '\n', 'utf8') <= 1024);
}
function unsupportedInlineesParity(f, {kind = null, code = null, q = query()} = {}) {
    const diagnostic = pack(f), full = pack(f);
    const call = () => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        diagnostic.reader, diagnostic.exe, diagnostic.identity, q);
    if (code !== null) redactedThrows(call, code);
    else unsupportedInlineesResult(call(), kind);
    if (code !== null || kind !== null) redactedThrows(() =>
        inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(full.reader, full.exe, full.identity, q),
        code || 'ERR_SYMBOL_UNSUPPORTED');
    else a.equal(inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        full.reader, full.exe, full.identity, q).profile, INLINEES_PROFILE);
    a.deepEqual(diagnostic.reads, full.reads);
    a.ok(diagnostic.reads.every(r => r.count <= LIMITS.read));
    return diagnostic;
}

test('inlinees unsupported diagnostic reports exactly the remaining 54 kinds at multiple positions and later modules', () => {
    a.equal(guardedSymbolKinds.length, 55); a.equal(inlineesGuardedKinds.length, 54);
    a.equal(new Set(inlineesGuardedKinds).size, 54);
    for (const kind of inlineesGuardedKinds) {
        for (const records of [[record(kind), inlineesRecord(), proc()],
            [inlineesRecord([0xffffffff]), record(kind), proc()],
            [proc(), inlineesRecord(), record(kind)], [inlineesRecord(), proc(), record(kind)]]) {
            const f = zeroTailFixture({modules: [{records}]});
            for (const q of [query(), query({module_indices: [0]})]) unsupportedInlineesParity(f, {kind, q});
        }
        const f = zeroTailFixture({modules: [{records: [inlineesRecord(), proc()]},
            {records: [proc(), inlineesRecord([0, 0xffffffff]), record(kind)]}]});
        for (const q of [query(), query({module_indices: [1, 0]})]) unsupportedInlineesParity(f, {kind, q});
        unsupportedInlineesParity(f, {q: query({module_indices: [0]})});
    }
});

test('inlinees unsupported diagnostic chooses the first remaining guard in physical and sorted selected-module order', () => {
    for (const [first, second] of [[0x0104, 0x1158], [0x1158, 0x0104]]) {
        unsupportedInlineesParity(zeroTailFixture({modules: [{records: [inlineesRecord(),
            proc(), record(first), inlineesRecord(), record(second)]}]}), {kind: first});
        const f = zeroTailFixture({modules: [{records: [inlineesRecord(), proc(), record(first)]},
            {records: [inlineesRecord([0xffffffff]), record(second), proc()]}],
        contributions: [{module_index: 1}, {module_index: 0}]});
        for (const q of [query(), query({module_indices: [1, 0]})]) unsupportedInlineesParity(f, {kind: first, q});
        unsupportedInlineesParity(f, {kind: second, q: query({module_indices: [1]})});
    }
    unsupportedInlineesParity(zeroTailFixture({modules: [{records: [inlineesRecord([0]),
        proc({offset: 0x90}), record(0x112c)]}]}), {kind: 0x112c});
});

test('inlinees unsupported diagnostic skips only validated exact lists and keeps malformed-list refusal precedence', () => {
    for (const values of [[], [0], [0xffffffff, 0, 0x80000000, 0], Array(16318).fill(0xffffffff)]) {
        const f = zeroTailFixture({modules: [{records: [inlineesRecord(values), proc(), inlineesRecord()]}]});
        unsupportedInlineesParity(f);
        unsupportedSymbolResult(unsupportedSymbol(f), 0x1168);
        redactedThrows(() => runZeroTail(f), 'ERR_SYMBOL_UNSUPPORTED');
    }
    const badLists = [record(0x1168), inlineesRecord([], {count: 1}), inlineesRecord([0], {count: 0}),
        inlineesRecord([], {count: 0xffffffff}), inlineesRecord(Array(16319).fill(0)),
        ...[5, 6, 7, 9, 10, 11].map(bytes => inlineesRecord([], {bytes}))];
    for (const bad of badLists) {
        for (const modules of [[{records: [proc(), inlineesRecord(), bad, record(0x0104)]}],
            [{records: [inlineesRecord(), proc()]}, {records: [bad, record(0x0104)]}]]) {
            unsupportedInlineesParity(zeroTailFixture({modules}), {code: 'ERR_MODULE_FRAMING'});
        }
        // A later malformed list is never interpreted past an earlier guard.
        // Preserve whole-symbol-stream alignment so its earlier framing guard
        // does not independently reject the deliberately malformed suffix.
        const alignedBad = Buffer.concat([bad, Buffer.alloc((4 - bad.length % 4) % 4)]);
        unsupportedInlineesParity(zeroTailFixture({modules: [{records: [inlineesRecord(),
            record(0x0104), alignedBad]}]}), {kind: 0x0104});
    }
});

test('inlinees unsupported diagnostic preserves external, outer-record and selected-stream preflight precedence', () => {
    for (const kind of [0x115f, 0x1160, 0x1161, 0x1167]) {
        for (const modules of [[{records: [inlineesRecord(), proc(), record(kind), record(0x0104)]}],
            [{records: [inlineesRecord(), proc()]}, {records: [record(kind), record(0x0104)]}]]) {
            unsupportedInlineesParity(zeroTailFixture({modules}), {code: 'ERR_FASTLINK_UNSUPPORTED'});
        }
        unsupportedInlineesParity(zeroTailFixture({modules: [{records: [inlineesRecord(),
            record(0x0104), record(kind)]}]}), {kind: 0x0104});
    }
    for (const length of [0, 1, 3, 65534, 65535]) {
        const bad = record(0x0104); bad.writeUInt16LE(length);
        unsupportedInlineesParity(zeroTailFixture({modules: [{records: [inlineesRecord(), proc(), bad]}]}),
            {code: 'ERR_MODULE_FRAMING'});
    }
    for (const [change, code] of [
        [f => f.streams[12] = null, 'ERR_MODULE_BOUND'],
        [f => f.streams[3].writeUInt16LE(9, dbiModuleAt(f, 1) + 34), 'ERR_MODULE_BOUND'],
        [f => f.streams[3].writeUInt16LE(8, dbiModuleAt(f, 1) + 34), 'ERR_MODULE_BOUND'],
        [f => f.streams[3].writeUInt16LE(0x100, dbiModuleAt(f, 1) + 32), 'ERR_REFERENCE_UNSUPPORTED'],
    ]) {
        const f = zeroTailFixture({modules: [{records: [inlineesRecord(), record(0x0104)]}, {}]});
        change(f); unsupportedInlineesParity(f, {code});
    }
    for (const options of [{signature: 2}, {sym: 5}, {c11: Buffer.alloc(4)}]) {
        unsupportedInlineesParity(zeroTailFixture({modules: [{...options,
            records: [inlineesRecord(), record(0x0104)]}]}), {code: 'ERR_MODULE_FRAMING'});
    }
});

test('inlinees unsupported diagnostic retains real resource limits and the record ceiling before kind capture', () => {
    unsupportedInlineesParity(zeroTailFixture({modules: Array.from({length: 33}, () =>
        ({records: [inlineesRecord(), record(0x0104)]}))}), {code: 'ERR_MODULE_BOUND'});
    const tooManySymbols = LIMITS.symbols + 4;
    unsupportedInlineesParity(zeroTailFixture({modules: [{sym: tooManySymbols,
        body: repeatedModule(tooManySymbols, record(0x0104))}]}), {code: 'ERR_MODULE_BOUND'});
    for (const [count, options] of [[LIMITS.contribution_records + 1, {}],
        [1, {declaredBytes: LIMITS.contributions + 4}]]) unsupportedInlineesParity(
        repeatedContributions(zeroTailFixture(), count, options), {code: 'ERR_ROUTING_BOUND'});
    // Real sparse authored input: final record is the first beyond the ceiling.
    const sym = 4 + 40 + LIMITS.records * 4, original = repeatedModule(sym);
    const body = {...original, read(o, n) {
        const b = original.read(o, n);
        for (const [at, byte] of [[sym - 2, 4], [sym - 1, 1]]) if (at >= o && at < o + n) b[at - o] = byte;
        return b;
    }};
    unsupportedInlineesParity(zeroTailFixture({modules: [{sym, body}]}), {code: 'ERR_MODULE_BOUND'});
});

test('inlinees unsupported diagnostic acceptance requires every selected module and all final metadata checks', () => {
    const clean = zeroTailFixture({modules: [{records: [inlineesRecord(), proc()]},
        {records: [inlineesRecord([0, 0xffffffff]), proc(), proc({offset: 0x90, size: 0xffffffff})]}]});
    for (const q of [query(), query({module_indices: [1, 0]})]) {
        const p = unsupportedInlineesParity(clean, {q});
        a.ok(p.reads.some(r => p.entries[12].pages.includes(Math.floor(r.offset / 4096))));
    }
    const malformed = record(); malformed.writeUInt16LE(65535);
    for (const [bad, code] of [[malformed, 'ERR_MODULE_FRAMING'],
        [inlineesRecord([], {count: 1}), 'ERR_MODULE_FRAMING'], [proc({size: 0x31}), 'ERR_PROCEDURE_CONFLICT'],
        [proc({size: 0}), 'ERR_PROCEDURE_SIZE'], [proc({segment: 4}), 'ERR_SOURCE_MAPPING'],
        [record(0x115f), 'ERR_FASTLINK_UNSUPPORTED']]) {
        for (const modules of [[{records: [inlineesRecord(), proc(), bad]}],
            [{records: [inlineesRecord(), proc()]}, {records: [inlineesRecord(), bad]}]]) {
            unsupportedInlineesParity(zeroTailFixture({modules}), {code});
        }
    }
    unsupportedInlineesParity(zeroTailFixture({modules: [{records: [inlineesRecord([0x20, 0x1020])]}]}),
        {code: 'ERR_PROCEDURE_ABSENT'});
    unsupportedInlineesParity(clean, {q: query({addresses: [VA, '0x140001021']}), code: 'ERR_PROCEDURE_ABSENT'});
    const stringify = JSON.stringify; let complete = false;
    JSON.stringify = function(value, ...args) {
        const out = stringify.call(this, value, ...args);
        if (!value || value.profile !== INLINEES_PROFILE) return out;
        complete = true; a.equal(value.complete_selected_symbol_record_framing, true);
        a.equal(value.selected_modules.length, 2); a.equal(value.inlinees_profile.entries, 2);
        return out.padEnd(LIMITS.output, ' ');
    };
    try {
        redactedThrows(() => unsupportedInlinees(clean), 'ERR_OUTPUT_BOUND'); a.equal(complete, true);
        unsupportedInlineesResult(unsupportedInlinees(zeroTailFixture({modules: [
            {records: [inlineesRecord(), record(0x0104)]}]})), 0x0104);
    } finally { JSON.stringify = stringify; }
});

test('inlinees unsupported diagnostic adds no reads or later interpretation after its first guard', () => {
    const f = zeroTailFixture({modules: [{records: [inlineesRecord(), proc(), record(0x0104),
        record(0x7777, 65536), record(0x7777, 65536), record(0x1158)]},
    {records: [inlineesRecord([], {count: 1})]}]});
    unsupportedInlineesParity(f, {kind: 0x0104});
    const p = pack(f); let forbidden = 0, headers = 0;
    const reader = {size: p.reader.size, read(o, n) {
        for (const index of [9, 12]) for (let i = 0; i < p.entries[index].pages.length; i++) {
            const page = p.entries[index].pages[i];
            if (o >= page * 4096 && o < (page + 1) * 4096) {
                const logical = i * 4096 + o % 4096;
                // Existing signature/trailer and first-cache prefetch are allowed.
                if (index === 12 || (logical >= LIMITS.cache && logical < f.modules[0].symbols.length)) {
                    forbidden++; throw Error('NEVER_EXPORT/later-cache-or-module');
                }
            }
        }
        return p.reader.read(o, n);
    }};
    const u16 = Buffer.prototype.readUInt16LE;
    Buffer.prototype.readUInt16LE = function(offset = 0) {
        if (this.length === 4 && offset === 2) { headers++; a.ok(headers <= 3); }
        return u16.call(this, offset);
    };
    try { unsupportedInlineesResult(diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        reader, p.exe, p.identity, query()), 0x0104); }
    finally { Buffer.prototype.readUInt16LE = u16; }
    a.equal(headers, 3); a.equal(forbidden, 0);
});

test('inlinees unsupported diagnostic keeps NI-zero, exact-tail, identity, feature and routing gates before symbols', () => {
    for (const [options, code] of [
        [{info: info()}, 'ERR_NI_MAC_UNSUPPORTED'],
        [{info: namedStreamNiMacZeroInfo({niMac: 1})}, 'ERR_NI_MAC_UNSUPPORTED'],
        [{info: namedStreamNiMacZeroInfo({niBytes: 0, features: []})}, 'ERR_PDB_FRAMING'],
        ...[MINI, NOTM].map(feature => [{info: namedStreamNiMacZeroInfo({features: [feature]})}, 'ERR_FASTLINK_UNSUPPORTED']),
        [{info: namedStreamNiMacZeroInfo({features: [0]})}, 'ERR_FEATURE_UNSUPPORTED'],
        [{info: namedStreamNiMacZeroInfo({features: [20140508, 20140508]})}, 'ERR_FEATURE_UNSUPPORTED'],
        [{version: 1}, 'ERR_ROUTING_UNSUPPORTED'], [{sc: Buffer.alloc(0)}, 'ERR_ROUTING_ABSENT'],
        [{moduleInfo: Buffer.alloc(60)}, 'ERR_DBI_MODULE_INFO'],
    ]) unsupportedInlineesParity(zeroTailFixture({...options, modules: [
        {records: [inlineesRecord(), record(0x0104)]}]}), {code});
    for (const sectionMap of [Buffer.alloc(0), segmentMap(false), segmentMap(true)]) {
        unsupportedInlineesParity(niFixture({sectionMap, modules: [
            {records: [inlineesRecord(), record(0x0104)]}]}), {code: 'ERR_SOURCE_MAPPING'});
    }
    for (const [change, code] of [[f => f.exe[f.rsds + 4] ^= 1, 'ERR_IDENTITY'],
        [f => f.streams[3].writeUInt16LE(8, f.optionalAt + 6), 'ERR_OMAP_UNSUPPORTED'],
        [f => f.streams[8][0] ^= 1, 'ERR_SOURCE_MAPPING'],
        [f => f.sectionMap.writeUInt16LE(4, 70), 'ERR_SOURCE_MAPPING']]) {
        const f = zeroTailFixture({modules: [{records: [inlineesRecord(), record(0x0104)]}]});
        change(f);
        if (code === 'ERR_SOURCE_MAPPING') replaceTestMap(f, f.sectionMap);
        unsupportedInlineesParity(f, {code});
    }
    for (const [, stage, , make, q = query()] of mappingCases) {
        if (stage === 'segment_map') continue;
        const f = make(); replaceTestMap(f, authoredZeroTailMap(f));
        unsupportedInlineesParity(f, {code: 'ERR_SOURCE_MAPPING', q});
    }
});

test('inlinees unsupported diagnostic rejects closed-query violations before reader or PE access', () => {
    const p = pack(zeroTailFixture()); let reads = 0, peReads = 0;
    const reader = {get size() { reads++; throw Error('NEVER_EXPORT'); }, read() { reads++; }};
    const exe = new Proxy({}, {get() { peReads++; throw Error('NEVER_EXPORT'); }});
    const inherited = Object.assign(Object.create({diagnose: true}), query());
    const symbol = query(); symbol[Symbol('NEVER_EXPORT')] = true;
    for (const q of [undefined, {}, inherited, symbol, query({diagnose: true}), query({inlinees: true}),
        query({grammar: INLINEES_GRAMMAR}), query({addresses: Array(1)}), query({module_indices: Array(1)}),
        query({module_indices: [0, 0]}), query({addresses: Array(33).fill(VA)}),
        query({addresses: ['0x0140001020']}), query({module_indices: Array.from({length: 33}, (_, i) => i)})]) {
        redactedThrows(() => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
            reader, exe, p.identity, q), 'ERR_SELECTORS');
    }
    a.equal(reads, 0); a.equal(peReads, 0);
    unsupportedInlineesParity(zeroTailFixture(), {q: Object.assign(Object.create(null), query())});
});

test('inlinees unsupported diagnostic isolates nested, repeated, shared-reader and older API calls', () => {
    const clean = zeroTailFixture({modules: [{records: [inlineesRecord([0, 0xffffffff]), proc()]}]}),
        refused = zeroTailFixture({modules: [{records: [inlineesRecord(), record(0x1158)]}]});
    for (const [f, kind] of [[clean, null], [refused, 0x1158]]) {
        const p = pack(f); let nested = false;
        const reader = {size: p.reader.size, read(o, n) {
            if (!nested) {
                nested = true;
                unsupportedInlineesResult(unsupportedInlinees(refused), 0x1158);
                unsupportedInlineesResult(unsupportedInlinees(clean));
                unsupportedSymbolResult(unsupportedSymbol(clean), 0x1168);
                inlineesFacts(runInlinees(clean), 1, 2);
            }
            return p.reader.read(o, n);
        }};
        for (let repeat = 0; repeat < 3; repeat++) {
            unsupportedInlineesResult(diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
                reader, p.exe, p.identity, query()), kind);
            redactedThrows(() => unsupportedInlinees(zeroTailFixture({modules: [
                {records: [inlineesRecord([], {count: 1})]}]})), 'ERR_MODULE_FRAMING');
            unsupportedInlineesResult(unsupportedInlinees());
        }
        a.equal(nested, true);
    }
    const p = pack(clean), full = inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        p.reader, p.exe, p.identity, query());
    unsupportedInlineesResult(diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        p.reader, p.exe, p.identity, query()));
    a.deepEqual(inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        p.reader, p.exe, p.identity, query()), full);
});

test('inlinees unsupported diagnostic redacts forged or typed reader errors, refuses shared storage and copies reused buffers', () => {
    const f = zeroTailFixture({modules: [{records: [inlineesRecord(), proc(), record(0x0104)]}]}), p = pack(f);
    let typed;
    try { runInlinees(f); } catch (error) { typed = error; }
    a.equal(typed.code, 'ERR_SYMBOL_UNSUPPORTED');
    const ForeignShared = require('node:vm').runInNewContext('SharedArrayBuffer');
    for (const index of [1, 3, 8, 9]) for (const mode of ['error', 'string', 'forged', 'typed', 'short', 'shared', 'foreign']) {
        const reader = {size: p.reader.size, read(o, n) {
            if (!p.entries[index].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
            if (mode === 'error') throw Error('ERR_SYMBOL_UNSUPPORTED/NEVER_EXPORT');
            if (mode === 'string') throw 'ERR_SYMBOL_UNSUPPORTED';
            if (mode === 'forged') throw {code: 'ERR_SYMBOL_UNSUPPORTED', message: 'NEVER_EXPORT'};
            if (mode === 'typed') throw typed;
            if (mode === 'short') return Buffer.alloc(n - 1);
            return Buffer.from(mode === 'shared' ? new SharedArrayBuffer(n) : new ForeignShared(n));
        }};
        redactedThrows(() => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
            reader, p.exe, p.identity, query()), mode === 'short' ? 'ERR_SHORT_READ' :
            mode === 'shared' || mode === 'foreign' ? 'ERR_INPUT_BOUND' : 'ERR_PDB_FRAMING');
    }
    let borrowed = null;
    const reader = {size: p.reader.size, read(o, n) {
        if (borrowed !== null) borrowed.fill(0xee);
        borrowed = p.reader.read(o, n); return borrowed;
    }};
    unsupportedInlineesResult(diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        reader, p.exe, p.identity, query()), 0x0104);
    // A previously genuine typed error is insufficient without this call's site.
    let io = 0;
    a.throws(() => diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        {get size() { io++; return p.reader.size; }}, p.exe, p.identity,
        {get addresses() { throw typed; }}), error => error === typed);
    a.equal(io, 0);
    for (const forged of ['ERR_SYMBOL_UNSUPPORTED', {code: 'ERR_SYMBOL_UNSUPPORTED'}]) redactedThrows(() =>
        diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(p.reader, p.exe, p.identity,
            {get addresses() { throw forged; }}), 'ERR_PDB_FRAMING');
});

// Test-only lexical VM fault injection. Real callers cannot mutate the private
// capture or inspector, or obtain non-u16 discriminator values from a header.
// These cases exercise defensive branches, not reachable immutable-input states.
function unsupportedInlineesHarness() {
    const filename = require.resolve('../pdb_procedure_ranges.cjs');
    const source = require('node:fs').readFileSync(filename, 'utf8');
    const sandbox = {module: {exports: {}}, require: require('node:module').createRequire(filename), Buffer};
    require('node:vm').runInNewContext(source + `
module.exports = {legalInlineesUnsupportedKind, MetadataError,
    diagnose: diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
    setInspector(fn) { inspectProfile = fn; }};
`, sandbox, {filename});
    return sandbox.module.exports;
}

test('inlinees unsupported defensive VM injection accepts only exact 54 membership and a typed captured guard', () => {
    const h = unsupportedInlineesHarness();
    for (const kind of inlineesGuardedKinds) a.equal(h.legalInlineesUnsupportedKind(kind), true);
    for (const kind of [null, undefined, -0, 0, -1, 1.5, NaN, Infinity, 0x10000, '260', 260n,
        new Number(260), {}, [], 0x110f, 0x1147, 0x115f, 0x1167, 0x1168, 0x7777]) {
        a.equal(h.legalInlineesUnsupportedKind(kind), false);
        const error = new h.MetadataError('ERR_SYMBOL_UNSUPPORTED');
        h.setInspector((...args) => { args[7].first = kind; throw error; });
        a.throws(() => h.diagnose(), actual => actual === error);
    }
    for (const [site, error] of [[null, new h.MetadataError('ERR_SYMBOL_UNSUPPORTED')],
        [0x0104, new h.MetadataError('ERR_PROCEDURE_ABSENT')],
        [0x0104, {code: 'ERR_SYMBOL_UNSUPPORTED'}], [0x0104, 'ERR_SYMBOL_UNSUPPORTED'],
        [0x0104, Error('ERR_SYMBOL_UNSUPPORTED/NEVER_EXPORT')]]) {
        h.setInspector((...args) => { args[7].first = site; throw error; });
        if (error instanceof h.MetadataError) a.throws(() => h.diagnose(), actual => actual === error);
        else redactedThrows(() => h.diagnose(), 'ERR_PDB_FRAMING');
    }
    h.setInspector((...args) => { args[7].first = 0x0104; });
    redactedThrows(() => h.diagnose(), 'ERR_SYMBOL_UNSUPPORTED');
    let calls = 0;
    h.setInspector((...args) => {
        calls++; a.equal(args.length, 9); a.equal(args[4], true); a.equal(args[5], null);
        a.equal(args[6], 'identity_sections_absolute_tail_frame_zero_v1');
        a.deepEqual(Reflect.ownKeys(args[7]), ['first']); a.equal(args[7].first, null);
        a.equal(args[8], INLINEES_GRAMMAR);
        args[7].first = 0x0104; throw new h.MetadataError('ERR_SYMBOL_UNSUPPORTED');
    });
    // VM object prototypes differ; compare the primitive fields individually.
    const r = h.diagnose(); a.equal(r.kind_u16, 0x0104); a.equal(r.profile, INLINEES_UNSUPPORTED_KIND_PROFILE);
    a.equal(calls, 1);
    a.ok(!Object.hasOwn(require('../pdb_procedure_ranges.cjs'), 'legalInlineesUnsupportedKind'));
});

test('inlinees unsupported diagnostic exports exactly five primitive fields and discards full metadata in both branches', () => {
    const secret = 'NEVER_EXPORT/private/inlinees-unsupported', base = {
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}),
        modules: [{module_name: secret, object_name: secret,
            records: [inlineesRecord([0xdeadbeef, 0xf1f2f3f4]), proc({name: secret})]}],
    };
    for (const kind of [null, 0x0104]) {
        const records = [...base.modules[0].records];
        if (kind !== null) { const bad = record(kind, 32); bad.fill(0x5a, 4); records.push(bad); }
        const f = zeroTailFixture({...base, modules: [{...base.modules[0], records}]});
        const result = unsupportedInlinees(f), json = JSON.stringify(result);
        unsupportedInlineesResult(result, kind);
        for (const word of [secret, 'module', 'stream', 'offset', 'ordinal', 'length', 'record', 'name',
            'address', 'guid', 'sha', 'digest', 'extent', 'coverage', 'buffer', 'stack', 'deadbeef', '0x']) {
            a.ok(!json.includes(word));
        }
        result.kind_u16 = 0x7777; result.extra = secret;
        unsupportedInlineesResult(unsupportedInlinees(f), kind);
    }
});

test('inlinees unsupported diagnostic enforces UTF-8 newline output boundaries and serializer-error redaction', () => {
    const clean = zeroTailFixture({modules: [{records: [inlineesRecord([0]), proc()]}]}),
        refused = zeroTailFixture({modules: [{records: [inlineesRecord(), record(0x0104)]}]}),
        stringify = JSON.stringify;
    for (const f of [clean, refused]) for (const bytes of [1023, 1024]) {
        JSON.stringify = function(value, ...args) {
            const out = stringify.call(this, value, ...args);
            if (!value || value.profile !== INLINEES_UNSUPPORTED_KIND_PROFILE) return out;
            const padding = bytes - Buffer.byteLength(out);
            return out + 'é'.repeat(Math.floor(padding / 2)) + (padding % 2 ? ' ' : '');
        };
        try {
            if (bytes === 1023) a.equal(unsupportedInlinees(f).profile, INLINEES_UNSUPPORTED_KIND_PROFILE);
            else redactedThrows(() => unsupportedInlinees(f), 'ERR_OUTPUT_BOUND');
        } finally { JSON.stringify = stringify; }
    }
    for (const profile of [INLINEES_PROFILE, INLINEES_UNSUPPORTED_KIND_PROFILE]) {
        JSON.stringify = function(value, ...args) {
            if (value && value.profile === profile) throw Error('NEVER_EXPORT/private-serializer');
            return stringify.call(this, value, ...args);
        };
        try {
            redactedThrows(() => unsupportedInlinees(clean), 'ERR_PDB_FRAMING');
            if (profile === INLINEES_UNSUPPORTED_KIND_PROFILE) redactedThrows(() => unsupportedInlinees(refused), 'ERR_PDB_FRAMING');
            else unsupportedInlineesResult(unsupportedInlinees(refused), 0x0104);
        } finally { JSON.stringify = stringify; }
    }
});

test('inlinees unsupported diagnostic leaves previous API results, profiles, errors and the full-inlinees API unchanged', () => {
    const strict = fixture(), ni = niFixture(), zero = zeroTailFixture(),
        lists = zeroTailFixture({modules: [{records: [inlineesRecord([0, 0xffffffff]), proc()]}]});
    const calls = [() => run(strict), () => runNi(ni), () => runZeroTail(zero), () => runInlinees(zero),
        () => runInlinees(lists), () => diagnose(strict), () => diagnoseNi(ni), () => sourceMapping(ni),
        () => sourceMappingIdentity(ni), () => tailFrame(ni), () => unsupportedSymbol(zero),
        () => unsupportedSymbol(lists)];
    const before = calls.map(call => JSON.stringify(call()));
    for (let repeat = 0; repeat < 2; repeat++) {
        unsupportedInlineesResult(unsupportedInlinees(lists));
        unsupportedInlineesResult(unsupportedInlinees(zeroTailFixture({modules: [
            {records: [inlineesRecord(), record(0x0104)]}]})), 0x0104);
        a.deepEqual(calls.map(call => JSON.stringify(call())), before);
        for (const kind of guardedSymbolKinds) {
            const f = zeroTailFixture({modules: [{records: [proc(), record(kind)]}]});
            unsupportedSymbolParity(f, {kind});
            if (kind !== 0x1168) unsupportedInlineesParity(f, {kind});
            else unsupportedInlineesParity(f, {code: 'ERR_MODULE_FRAMING'});
        }
    }
});

const THUNK_ORDINAL_PROFILE = 'MSF7_procedure_thunk32_ordinal_exact_inlinees_diagnostic_v1';
function thunkOrdinal(f = zeroTailFixture(), q = query()) {
    const p = pack(f);
    return diagnoseThunk32OrdinalNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(p.reader, p.exe, p.identity, q);
}
function thunkOrdinalResult(result, ordinal = null) {
    a.deepEqual(Reflect.ownKeys(result), ['profile', 'status', 'stage', 'guard', 'ordinal_u8']);
    a.deepEqual(result, {profile: THUNK_ORDINAL_PROFILE,
        status: ordinal === null ? 'accepted_profile' : 'refused_symbol',
        stage: ordinal === null ? 'complete' : 'procedure_metadata',
        guard: ordinal === null ? null : 'unsupported_kind', ordinal_u8: ordinal});
    a.ok(Buffer.byteLength(JSON.stringify(result) + '\n') <= 1024);
}
// Synthetic raw layout only; arbitrary bytes deliberately make no full thunk
// grammar assertion (in particular, the name need not contain a terminator).
function ordinalRecord(ordinal, bytes = 28, fill = 0xa7) {
    const b = record(0x1102, bytes); b.fill(fill, 4); b[24] = ordinal; return b;
}
function invokeThunk(p, reader = p.reader, q = query()) {
    return diagnoseThunk32OrdinalNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(reader, p.exe, p.identity, q);
}
function modulePhysicalIntervals(p, stream, start, end) {
    const intervals = [];
    for (let pos = start; pos < end;) {
        const take = Math.min(end - pos, 4096 - pos % 4096);
        intervals.push({offset: p.entries[stream].pages[Math.floor(pos / 4096)] * 4096 + pos % 4096, count: take});
        pos += take;
    }
    return intervals;
}
function observedModuleIntervals(p, stream) {
    const intervals = [];
    for (const read of p.reads) for (let i = 0; i < p.entries[stream].pages.length; i++) {
        const page = p.entries[stream].pages[i] * 4096,
            start = Math.max(read.offset, page), end = Math.min(read.offset + read.count, page + 4096);
        if (start < end) intervals.push({offset: i * 4096 + start - page, count: end - start});
    }
    return intervals;
}
function excludesPhysical(p, forbidden, fault = null) {
    const attempts = [];
    return {attempts, reader: {size: p.reader.size, read(o, n) {
        attempts.push({offset: o, count: n});
        for (const span of forbidden) if (o < span.offset + span.count && span.offset < o + n) {
            if (fault !== null) return fault(o, n);
            throw Error('NEVER_EXPORT/excluded-physical-sentinel');
        }
        return p.reader.read(o, n);
    }}};
}

test('thunk ordinal retains all 256 raw u8 values without interpreting fixed fields, invalid names or tails', () => {
    for (let ordinal = 0; ordinal <= 255; ordinal++) for (const fill of [0, 0xff]) {
        const b = ordinalRecord(ordinal, ordinal % 2 ? 28 : 64, fill);
        thunkOrdinalResult(thunkOrdinal(zeroTailFixture({modules: [{records: [b]}]})), ordinal);
    }
    for (const bytes of [28, 32, 65532, 65536]) {
        const b = ordinalRecord(231, bytes); b.fill(0xf3, 25);
        thunkOrdinalResult(thunkOrdinal(zeroTailFixture({modules: [{records: [b]}]})), 231);
    }
});

test('thunk exact reads exclude physical fixed fields, suffix and later records across MSF and old cache boundaries', () => {
    for (const thunkAt of [4, 4068, 4072, 4092, 65508, 65512, 65532, 65536]) {
        const before = thunkAt === 4 ? [] : [inlineesRecord([0xffffffff, 0]), proc({name: 'earlier name'})];
        const used = 4 + before.reduce((n, b) => n + b.length, 0);
        if (used < thunkAt) before.push(record(0x7777, thunkAt - used));
        const b = ordinalRecord(197, 65536), later = record(0x7777, 65536);
        const p = pack(zeroTailFixture({modules: [{records: [...before, b, later]}, {records: [proc()]}]}));
        const forbidden = [...modulePhysicalIntervals(p, 9, thunkAt + 4, thunkAt + 24),
            ...modulePhysicalIntervals(p, 9, thunkAt + 25, p.modules[0].symbols.length),
            ...modulePhysicalIntervals(p, 12, 0, p.modules[1].body.length)];
        const guarded = excludesPhysical(p, forbidden);
        thunkOrdinalResult(invokeThunk(p, guarded.reader), 197);
        const observed = observedModuleIntervals(p, 9), expected = [{offset: 0, count: 4},
            {offset: p.modules[0].symbols.length, count: 4}];
        let at = 4;
        for (const previous of before) {
            expected.push({offset: at, count: 4});
            if ([0x110f, 0x1168].includes(previous.readUInt16LE(2))) {
                for (const span of modulePhysicalIntervals(p, 9, at, at + previous.length)) {
                    const pageIndex = p.entries[9].pages.indexOf(Math.floor(span.offset / 4096));
                    expected.push({offset: pageIndex * 4096 + span.offset % 4096, count: span.count});
                }
            }
            at += previous.length;
        }
        expected.push({offset: thunkAt, count: 4}, {offset: thunkAt + 24, count: 1});
        a.deepEqual(observed, expected);
        a.deepEqual(observedModuleIntervals(p, 12), []);
        a.ok(guarded.attempts.every(r => r.count <= LIMITS.read));
    }
});

test('ordinal path scans every preceding module without cache prefetch and stops at the first selected thunk', () => {
    const first = [inlineesRecord([0xffffffff]), proc(), record(0x7777, 65536)], second =
        [inlineesRecord(), proc(), ordinalRecord(82), ordinalRecord(91)];
    const p = pack(zeroTailFixture({modules: [{records: first}, {records: second}, {records: [ordinalRecord(17)]}]}));
    const at = 4 + second[0].length + second[1].length;
    const forbidden = [...modulePhysicalIntervals(p, 12, at + 4, at + 24),
        ...modulePhysicalIntervals(p, 12, at + 25, p.modules[1].symbols.length),
        ...modulePhysicalIntervals(p, 15, 0, p.modules[2].body.length)];
    thunkOrdinalResult(invokeThunk(p, excludesPhysical(p, forbidden).reader, query({module_indices: [2, 1, 0]})), 82);
    const oldCacheRead = p.entries[9].pages[0] * 4096;
    a.ok(!p.reads.some(r => r.offset === oldCacheRead && r.count === 4096));
    const scoped = pack(zeroTailFixture({modules: [{records: first}, {records: second}, {records: [ordinalRecord(17)]}]}));
    thunkOrdinalResult(invokeThunk(scoped, scoped.reader, query({module_indices: [2]})), 17);
});

test('required earlier procedure and inlinees bodies cross MSF and cache boundaries without touching a later thunk payload', () => {
    for (const before of [[proc({name: Buffer.alloc(65496, 0x41)}), inlineesRecord(Array(16318).fill(0xffffffff))],
        [record(0x7777, 4088), proc({name: Buffer.alloc(65496, 0x42)}), inlineesRecord([0xffffffff, 0])],
        [record(0x7777, 65524), inlineesRecord([0, 0xffffffff]), proc()]]) {
        const at = 4 + before.reduce((n, b) => n + b.length, 0),
            p = pack(zeroTailFixture({modules: [{records: [...before, ordinalRecord(139, 65536)]}]}));
        const forbidden = [...modulePhysicalIntervals(p, 9, at + 4, at + 24),
            ...modulePhysicalIntervals(p, 9, at + 25, p.modules[0].symbols.length)];
        thunkOrdinalResult(invokeThunk(p, excludesPhysical(p, forbidden).reader), 139);
        const expected = [{offset: 0, count: 4}, {offset: p.modules[0].symbols.length, count: 4}];
        let pos = 4;
        for (const b of before) {
            expected.push({offset: pos, count: 4});
            if ([0x110f, 0x1168].includes(b.readUInt16LE(2))) {
                for (let done = 0; done < b.length;) {
                    const take = Math.min(b.length - done, 4096 - (pos + done) % 4096);
                    expected.push({offset: pos + done, count: take}); done += take;
                }
            }
            pos += b.length;
        }
        expected.push({offset: at, count: 4}, {offset: at + 24, count: 1});
        a.deepEqual(observedModuleIntervals(p, 9), expected);
    }
});

test('new exact diagnostic can avoid an excluded-byte reader fault that legacy speculative prefetch encounters', () => {
    const f = zeroTailFixture({modules: [{records: [proc(), inlineesRecord(), ordinalRecord(157, 64)]}]}),
        at = 4 + proc().length + inlineesRecord().length;
    const exact = pack(f), cached = pack(f), excluded = p => [
        ...modulePhysicalIntervals(p, 9, at + 4, at + 24),
        ...modulePhysicalIntervals(p, 9, at + 25, p.modules[0].symbols.length)];
    thunkOrdinalResult(invokeThunk(exact, excludesPhysical(exact, excluded(exact)).reader), 157);
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        excludesPhysical(cached, excluded(cached)).reader, cached.exe, cached.identity, query()), 'ERR_PDB_FRAMING');
    // Distinct read/error timing is intentional for the new wrapper only.
    unsupportedInlineesParity(f, {kind: 0x1102});
});

test('thunk n=4..24, misalignment, overrun and truncated prefixes refuse before any ordinal read', () => {
    for (let n = 4; n <= 27; n++) {
        const b = record(0x1102, 28); b.writeUInt16LE(n - 2);
        const p = pack(zeroTailFixture({modules: [{records: [b]}]})), forbidden = modulePhysicalIntervals(p, 9, 8, 32);
        redactedThrows(() => invokeThunk(p, excludesPhysical(p, forbidden).reader), 'ERR_MODULE_FRAMING');
        a.deepEqual(observedModuleIntervals(p, 9), [{offset: 0, count: 4}, {offset: 32, count: 4}, {offset: 4, count: 4}]);
    }
    for (const length of [0, 1, 29, 65535]) {
        const b = ordinalRecord(3); b.writeUInt16LE(length);
        const p = pack(zeroTailFixture({modules: [{records: [b]}]}));
        redactedThrows(() => invokeThunk(p, excludesPhysical(p, modulePhysicalIntervals(p, 9, 8, 32)).reader),
            'ERR_MODULE_FRAMING');
    }
    for (const size of [5, 6, 7]) {
        const body = Buffer.alloc(size + 4); body.writeUInt32LE(4);
        const p = pack(zeroTailFixture({modules: [{sym: size, body}]}));
        redactedThrows(() => invokeThunk(p), 'ERR_MODULE_FRAMING');
    }
});

test('all other 53 unsupported kinds and external guards retain priority, with no forward search or ordinal I/O', () => {
    const external = [0x115f, 0x1160, 0x1161, 0x1167];
    a.equal(inlineesGuardedKinds.filter(k => k !== 0x1102).length, 53);
    for (const kind of [...inlineesGuardedKinds.filter(k => k !== 0x1102), ...external]) {
        const f = zeroTailFixture({modules: [{records: [record(kind), ordinalRecord(44)]}]}), p = pack(f);
        const forbidden = modulePhysicalIntervals(p, 9, 8, p.modules[0].symbols.length);
        redactedThrows(() => invokeThunk(p, excludesPhysical(p, forbidden).reader),
            external.includes(kind) ? 'ERR_FASTLINK_UNSUPPORTED' : 'ERR_SYMBOL_UNSUPPORTED');
        const across = pack(zeroTailFixture({modules: [{records: [record(kind)]}, {records: [ordinalRecord(44)]}]}));
        redactedThrows(() => invokeThunk(across, excludesPhysical(across,
            modulePhysicalIntervals(across, 12, 0, across.modules[1].body.length)).reader,
        query({module_indices: [1, 0]})), external.includes(kind) ? 'ERR_FASTLINK_UNSUPPORTED' : 'ERR_SYMBOL_UNSUPPORTED');
        thunkOrdinalResult(thunkOrdinal(zeroTailFixture({modules: [{records: [ordinalRecord(44), record(kind)]}]})), 44);
    }
    for (const [bad, code] of [[inlineesRecord([], {count: 1}), 'ERR_MODULE_FRAMING'],
        [proc({segment: 0}), 'ERR_SOURCE_MAPPING'], [proc({size: 0}), 'ERR_PROCEDURE_SIZE']]) {
        const p = pack(zeroTailFixture({modules: [{records: [bad, ordinalRecord(44)]}]}));
        const at = 4 + bad.length;
        redactedThrows(() => invokeThunk(p, excludesPhysical(p,
            modulePhysicalIntervals(p, 9, at, p.modules[0].symbols.length)).reader), code);
    }
});

test('thunk ordinal preserves context, global module, module header and reference-count prerequisites', () => {
    const cases = [
        [f => { f.streams[1].writeUInt32LE(0, 8); }, 'ERR_IDENTITY'],
        [f => { f.streams[3].writeUInt16LE(8, f.optionalAt + 6); }, 'ERR_OMAP_UNSUPPORTED'],
        [f => { f.streams[3].writeUInt16LE(0x100, dbiModuleAt(f, 1) + 32); }, 'ERR_REFERENCE_UNSUPPORTED'],
        [f => { f.streams[3].writeUInt16LE(9, dbiModuleAt(f, 1) + 34); }, 'ERR_MODULE_BOUND'],
        [f => { f.streams[9].writeUInt32LE(2); }, 'ERR_MODULE_FRAMING'],
        [f => { f.streams[9].writeUInt32LE(1, f.modules[0].symbols.length); }, 'ERR_MODULE_FRAMING'],
    ];
    for (const [change, code] of cases) {
        const f = zeroTailFixture({modules: [{records: [ordinalRecord(12)]}, {}]}); change(f);
        const p = pack(f), forbidden = modulePhysicalIntervals(p, 9, 4, p.modules[0].symbols.length);
        redactedThrows(() => invokeThunk(p, excludesPhysical(p, forbidden).reader), code);
    }
    for (const change of [f => ordinalTailTwin(f), f => replaceTestMap(f, segmentMap(false)),
        f => replaceTestMap(f, Buffer.alloc(0))]) {
        const f = zeroTailFixture({modules: [{records: [ordinalRecord(12)]}]}), p = pack(change(f));
        redactedThrows(() => invokeThunk(p), 'ERR_SOURCE_MAPPING');
    }
    for (const [options, code] of [
        [{info: info()}, 'ERR_NI_MAC_UNSUPPORTED'],
        [{info: namedStreamNiMacZeroInfo({features: [MINI]})}, 'ERR_FASTLINK_UNSUPPORTED'],
        [{info: namedStreamNiMacZeroInfo({features: [0]})}, 'ERR_FEATURE_UNSUPPORTED'],
        [{version: 1}, 'ERR_ROUTING_UNSUPPORTED'], [{sc: Buffer.alloc(0)}, 'ERR_ROUTING_ABSENT'],
        [{moduleInfo: Buffer.alloc(60)}, 'ERR_DBI_MODULE_INFO'],
    ]) {
        const p = pack(zeroTailFixture({...options, modules: [{records: [ordinalRecord(12)]}]}));
        redactedThrows(() => invokeThunk(p, excludesPhysical(p,
            modulePhysicalIntervals(p, 9, 0, p.modules[0].body.length)).reader), code);
    }
});

test('ordinal path keeps module-count, aggregate-symbol and routing caps before selected symbol reads', () => {
    for (const f of [zeroTailFixture({modules: Array.from({length: 33}, () => ({records: [ordinalRecord(12)]}))}),
        zeroTailFixture({modules: [{sym: LIMITS.symbols + 4,
            body: repeatedModule(LIMITS.symbols + 4, ordinalRecord(12))}]})]) {
        const p = pack(f), reader = excludesPhysical(p, modulePhysicalIntervals(p, 9, 0, p.entries[9].size)).reader;
        redactedThrows(() => invokeThunk(p, reader), 'ERR_MODULE_BOUND');
        a.deepEqual(observedModuleIntervals(p, 9), []);
    }
    const p = pack(repeatedContributions(zeroTailFixture({modules: [{records: [ordinalRecord(12)]}]}),
        1, {declaredBytes: LIMITS.contributions + 4}));
    redactedThrows(() => invokeThunk(p, excludesPhysical(p,
        modulePhysicalIntervals(p, 9, 0, p.modules[0].body.length)).reader), 'ERR_ROUTING_BOUND');
});

test('MSF globally rejects reused or aliased physical pages before any ordinal or module reads', () => {
    const pageSlot = (p, stream, index = 0) => 4 + p.entries.length * 4 +
        p.entries.slice(0, stream).reduce((n, e) => n + e.pages.length * 4, 0) + index * 4;
    for (const mutation of ['same-stream', 'other-stream', 'info-alias', 'directory-alias', 'blockmap-alias']) {
        const p = pack(zeroTailFixture({modules: [{records: [ordinalRecord(11, 65536)]}, {}]}));
        if (mutation === 'same-stream') p.directory.writeUInt32LE(p.entries[9].pages[0], pageSlot(p, 9, 1));
        if (mutation === 'other-stream') p.directory.writeUInt32LE(p.entries[9].pages[0], pageSlot(p, 12));
        if (mutation === 'info-alias') p.directory.writeUInt32LE(p.entries[9].pages[0], pageSlot(p, 1));
        if (mutation === 'directory-alias') p.directory.writeUInt32LE(4, pageSlot(p, 9));
        if (mutation === 'blockmap-alias') p.directory.writeUInt32LE(3, pageSlot(p, 9));
        redactedThrows(() => invokeThunk(p), 'ERR_PDB_FRAMING');
        a.deepEqual(observedModuleIntervals(p, 9), []);
        a.deepEqual(observedModuleIntervals(p, 12), []);
    }
});

test('ordinal byte physical reader faults fail closed after the prefix without fabricating a zero ordinal', () => {
    let typed;
    try { runInlinees(zeroTailFixture({modules: [{records: [record(0x1102)]}]})); }
    catch (error) { typed = error; }
    a.equal(typed.code, 'ERR_SYMBOL_UNSUPPORTED');
    const ForeignShared = require('node:vm').runInNewContext('SharedArrayBuffer');
    for (const thunkAt of [4, 4072, 65512]) for (const mode of
        ['throw', 'string', 'forged', 'typed', 'short', 'shared', 'foreign']) {
        const records = thunkAt === 4 ? [] : [record(0x7777, thunkAt - 4)];
        records.push(ordinalRecord(0));
        const p = pack(zeroTailFixture({modules: [{records}]}));
        const target = modulePhysicalIntervals(p, 9, thunkAt + 24, thunkAt + 25)[0];
        const fault = excludesPhysical(p, [target], (o, n) => {
            a.equal(o, target.offset); a.equal(n, 1);
            if (mode === 'throw') throw Error('NEVER_EXPORT/private-reader');
            if (mode === 'string') throw 'ERR_SYMBOL_UNSUPPORTED';
            if (mode === 'forged') throw {code: 'ERR_SYMBOL_UNSUPPORTED', ordinal_u8: 0};
            if (mode === 'typed') throw typed;
            if (mode === 'short') return Buffer.alloc(0);
            return Buffer.from(mode === 'shared' ? new SharedArrayBuffer(n) : new ForeignShared(n));
        });
        redactedThrows(() => invokeThunk(p, fault.reader), mode === 'short' ? 'ERR_SHORT_READ' :
            mode === 'shared' || mode === 'foreign' ? 'ERR_INPUT_BOUND' : 'ERR_PDB_FRAMING');
        a.equal(fault.attempts.filter(r => r.offset === target.offset && r.count === 1).length, 1);
        const observed = observedModuleIntervals(p, 9);
        a.deepEqual(observed.at(-1), {offset: thunkAt, count: 4});
    }
});

test('ordinal diagnostic copies reused input buffers and rejects prerequisite stream read faults', () => {
    const f = zeroTailFixture({modules: [{records: [inlineesRecord([0xffffffff]), proc(), ordinalRecord(173)]}]});
    const ForeignShared = require('node:vm').runInNewContext('SharedArrayBuffer');
    for (const index of [1, 3, 8, 9]) for (const mode of ['throw', 'short', 'shared', 'foreign']) {
        const p = pack(f), reader = {size: p.reader.size, read(o, n) {
            if (!p.entries[index].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
            if (mode === 'throw') throw Error('NEVER_EXPORT/private-stream');
            if (mode === 'short') return Buffer.alloc(n - 1);
            return Buffer.from(mode === 'shared' ? new SharedArrayBuffer(n) : new ForeignShared(n));
        }};
        redactedThrows(() => invokeThunk(p, reader), mode === 'short' ? 'ERR_SHORT_READ' :
            mode === 'throw' ? 'ERR_PDB_FRAMING' : 'ERR_INPUT_BOUND');
    }
    const p = pack(f); let previous = null;
    const reader = {size: p.reader.size, read(o, n) {
        if (previous !== null) previous.fill(0xee);
        previous = p.reader.read(o, n); return previous;
    }};
    thunkOrdinalResult(invokeThunk(p, reader), 173);
    thunkOrdinalResult(thunkOrdinal(f), 173);
});

test('ordinal selectors are validated before PE or reader access and retain the exact whitelist', () => {
    let io = 0, pe = 0;
    const reader = {get size() { io++; throw Error('NEVER_EXPORT'); }, read() { io++; }},
        exe = {get length() { pe++; throw Error('NEVER_EXPORT'); }};
    for (const q of [null, {}, [], {addresses: []}, {addresses: Array(1)},
        query({addresses: [VA, VA]}), query({module_indices: undefined}),
        query({module_indices: [1, 1]}), query({module_indices: Array(1)}),
        query({exact: true}), query({profile: THUNK_ORDINAL_PROFILE}), query({retry: true}),
        query({ordinal_u8: 0}), query({[Symbol('private')]: 1})]) {
        redactedThrows(() => diagnoseThunk32OrdinalNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
            reader, exe, {}, q), 'ERR_SELECTORS');
    }
    a.equal(io, 0); a.equal(pe, 0);
    thunkOrdinalResult(thunkOrdinal(zeroTailFixture(), Object.assign(Object.create(null), query())));
});

test('no-thunk acceptance requires full later exact-inlinees scanning, selected procedures and conflict checks', () => {
    const clean = zeroTailFixture({modules: [{records: [inlineesRecord([0xffffffff]), proc()]},
        {records: [proc(), inlineesRecord([0, 0xffffffff]), proc({offset: 0x90, size: 0xffffffff})]}]});
    const p = pack(clean); thunkOrdinalResult(invokeThunk(p));
    a.ok(observedModuleIntervals(p, 12).some(r => r.offset === 4 && r.count === 4));
    const malformed = record(); malformed.writeUInt16LE(65535);
    for (const [bad, code] of [[malformed, 'ERR_MODULE_FRAMING'],
        [inlineesRecord([], {count: 1}), 'ERR_MODULE_FRAMING'],
        [proc({size: 0x31}), 'ERR_PROCEDURE_CONFLICT'], [proc({segment: 0}), 'ERR_SOURCE_MAPPING'],
        [record(0x115f), 'ERR_FASTLINK_UNSUPPORTED'], [record(0x0104), 'ERR_SYMBOL_UNSUPPORTED']]) {
        for (const modules of [[{records: [inlineesRecord(), proc(), bad]}],
            [{records: [inlineesRecord(), proc()]}, {records: [inlineesRecord(), bad]}]]) {
            redactedThrows(() => thunkOrdinal(zeroTailFixture({modules})), code);
        }
    }
    redactedThrows(() => thunkOrdinal(zeroTailFixture({modules: [{records: [inlineesRecord([0, 1])]}]})),
        'ERR_PROCEDURE_ABSENT');
    redactedThrows(() => thunkOrdinal(clean, query({addresses: [VA, '0x140001021']})), 'ERR_PROCEDURE_ABSENT');
    const failed = pack(clean), last = modulePhysicalIntervals(failed, 12, 4,
        failed.modules[1].symbols.length);
    redactedThrows(() => invokeThunk(failed, excludesPhysical(failed, last).reader), 'ERR_PDB_FRAMING');
});

test('ordinal refusal bypasses later checks while no-thunk acceptance finishes ordinary digests and output checks', () => {
    const clean = zeroTailFixture({modules: [{records: [inlineesRecord([0, 0xffffffff]), proc()]}]}),
        refused = zeroTailFixture({modules: [{records: [ordinalRecord(63)]},
            {records: [inlineesRecord([], {count: 1})]}]}), stringify = JSON.stringify;
    let finished = false;
    JSON.stringify = function(value, ...args) {
        if (!value || value.profile !== INLINEES_PROFILE) return stringify.call(this, value, ...args);
        a.equal(value.selected_modules.length, 1); a.equal(value.complete_selected_symbol_record_framing, true);
        a.equal(value.inlinees_profile.entries, 2);
        a.equal(value.addresses[0].procedure_records_sha256,
            createHash('sha256').update(clean.modules[0].symbols.subarray(4 + inlineesRecord([0, 0xffffffff]).length)).digest('hex'));
        finished = true; return 'x'.repeat(LIMITS.output);
    };
    try {
        redactedThrows(() => thunkOrdinal(clean), 'ERR_OUTPUT_BOUND'); a.equal(finished, true);
        finished = false; thunkOrdinalResult(thunkOrdinal(refused), 63); a.equal(finished, false);
    } finally { JSON.stringify = stringify; }
});

test('ordinal calls isolate capture across nested, failed and accepted calls and preserve every old API read trace', () => {
    const clean = zeroTailFixture({modules: [{records: [inlineesRecord([0]), proc()]}]}),
        refused = zeroTailFixture({modules: [{records: [inlineesRecord(), ordinalRecord(208)]}]}), p = pack(refused);
    let nested = false;
    const reader = {size: p.reader.size, read(o, n) {
        if (!nested) {
            nested = true;
            thunkOrdinalResult(thunkOrdinal(clean));
            thunkOrdinalResult(thunkOrdinal(zeroTailFixture({modules: [{records: [ordinalRecord(19)]}]})), 19);
            redactedThrows(() => thunkOrdinal(zeroTailFixture({modules: [{records: [record(0x1102)]}]})), 'ERR_MODULE_FRAMING');
        }
        return p.reader.read(o, n);
    }};
    thunkOrdinalResult(invokeThunk(p, reader), 208); a.equal(nested, true);
    const calls = [inspect, inspectNamedStreamNiMacZero, inspectNamedStreamNiMacZeroAbsoluteTailFrameZero,
        inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
        diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero,
        diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact];
    // The legacy zero-tail full API must retain its inlinees refusal.
    const fixtures = [fixture(), niFixture(), zeroTailFixture(), clean, clean, clean];
    for (let i = 0; i < calls.length; i++) {
        const before = pack(fixtures[i]), after = pack(fixtures[i]);
        const result = calls[i](before.reader, before.exe, before.identity, query());
        thunkOrdinalResult(thunkOrdinal(refused), 208); thunkOrdinalResult(thunkOrdinal(clean));
        a.deepEqual(calls[i](after.reader, after.exe, after.identity, query()), result);
        a.deepEqual(after.reads, before.reads);
    }
    const legacyBefore = pack(clean), legacyAfter = pack(clean);
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(
        legacyBefore.reader, legacyBefore.exe, legacyBefore.identity, query()), 'ERR_SYMBOL_UNSUPPORTED');
    thunkOrdinalResult(thunkOrdinal(refused), 208); thunkOrdinalResult(thunkOrdinal(clean));
    redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZero(
        legacyAfter.reader, legacyAfter.exe, legacyAfter.identity, query()), 'ERR_SYMBOL_UNSUPPORTED');
    a.deepEqual(legacyAfter.reads, legacyBefore.reads);
    for (const kind of guardedSymbolKinds) {
        const f = zeroTailFixture({modules: [{records: [proc(), record(kind)]}]});
        unsupportedSymbolParity(f, {kind});
        if (kind === 0x1168) unsupportedInlineesParity(f, {code: 'ERR_MODULE_FRAMING'});
        else unsupportedInlineesParity(f, {kind});
    }
});

// Test-only lexical VM fault injection. These private sites, inspector
// replacements and record totals are unreachable through the public API.
// They exercise defensive branches, not immutable-input reachability claims.
function thunkOrdinalHarness() {
    const filename = require.resolve('../pdb_procedure_ranges.cjs'),
        source = require('node:fs').readFileSync(filename, 'utf8'),
        sandbox = {module: {exports: {}}, require: require('node:module').createRequire(filename), Buffer};
    require('node:vm').runInNewContext(source + `
module.exports = {legalThunk32Ordinal, MetadataError, scanModule,
    diagnose: diagnoseThunk32OrdinalNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
    setInspector(fn) { inspectProfile = fn; }};
`, sandbox, {filename});
    return sandbox.module.exports;
}

test('ordinal defensive VM injection requires a primitive u8 and the identical privately captured typed refusal', () => {
    const h = thunkOrdinalHarness();
    for (let value = 0; value <= 255; value++) a.equal(h.legalThunk32Ordinal(value), true);
    for (const value of [null, undefined, -0, -1, 256, 1.5, NaN, Infinity, '0', 0n, new Number(0), {}, []]) {
        a.equal(h.legalThunk32Ordinal(value), false);
        const error = new h.MetadataError('ERR_SYMBOL_UNSUPPORTED');
        h.setInspector((...args) => { args[9].ordinal = value; args[9].error = error; throw error; });
        a.throws(() => h.diagnose(), actual => actual === error);
    }
    const captured = new h.MetadataError('ERR_SYMBOL_UNSUPPORTED');
    for (const error of [new h.MetadataError('ERR_SYMBOL_UNSUPPORTED'), new h.MetadataError('ERR_MODULE_FRAMING'),
        {code: 'ERR_SYMBOL_UNSUPPORTED'}, 'ERR_SYMBOL_UNSUPPORTED', Error('NEVER_EXPORT')]) {
        h.setInspector((...args) => { args[9].ordinal = 7; args[9].error = captured; throw error; });
        if (error instanceof h.MetadataError) a.throws(() => h.diagnose(), actual => actual === error);
        else redactedThrows(() => h.diagnose(), 'ERR_PDB_FRAMING');
    }
    const wrongCode = new h.MetadataError('ERR_MODULE_FRAMING');
    h.setInspector((...args) => { args[9].ordinal = 7; args[9].error = wrongCode; throw wrongCode; });
    a.throws(() => h.diagnose(), actual => actual === wrongCode);
    for (const [ordinal, error] of [[0, null], [null, captured], [7, captured]]) {
        h.setInspector((...args) => { args[9].ordinal = ordinal; args[9].error = error; });
        redactedThrows(() => h.diagnose(), 'ERR_SYMBOL_UNSUPPORTED');
    }
    let calls = 0;
    h.setInspector((...args) => {
        calls++; a.equal(args.length, 10); a.equal(args[4], true); a.equal(args[5], null);
        a.equal(args[6], 'identity_sections_absolute_tail_frame_zero_v1');
        a.equal(args[7], null); a.equal(args[8], INLINEES_GRAMMAR);
        a.deepEqual(Reflect.ownKeys(args[9]), ['ordinal', 'error']);
        a.equal(args[9].ordinal, null); a.equal(args[9].error, null);
        args[9].ordinal = 255; args[9].error = captured; throw captured;
    });
    const r = h.diagnose(); a.equal(r.ordinal_u8, 255); a.equal(r.profile, THUNK_ORDINAL_PROFILE); a.equal(calls, 1);
    a.equal(Object.hasOwn(require('../pdb_procedure_ranges.cjs'), 'legalThunk32Ordinal'), false);
});

test('ordinal defensive VM injection keeps the record cap and earlier inlinees accounting ahead of capture', () => {
    const h = thunkOrdinalHarness(), b = ordinalRecord(12), body = Buffer.concat([
        Buffer.from([4, 0, 0, 0]), b, Buffer.alloc(4)]), reads = [], site = {ordinal: null, error: null};
    const m = {streams: [{size: body.length}], stream(i, o, n) {
        reads.push({offset: o, count: n}); return Buffer.from(body.subarray(o, o + n));
    }};
    redactedThrows(() => h.scanModule({m, image: {sections: []}}, {stream_id: 0,
        module_index: 0, sym: 4 + b.length, c11: 0, c13: 0}, new Map(),
    {records: LIMITS.records, procedures: 0, opaque: 0}, null, INLINEES_GRAMMAR, site), 'ERR_MODULE_BOUND');
    a.deepEqual(reads, [{offset: 0, count: 4}, {offset: 32, count: 4}, {offset: 4, count: 4}]);
    a.deepEqual(site, {ordinal: null, error: null});
    const list = inlineesRecord(), withList = Buffer.concat([Buffer.from([4, 0, 0, 0]), list, b, Buffer.alloc(4)]);
    for (const inlinees of [{records: Infinity, entries: 0}, {records: 0, entries: -1},
        {records: 0, entries: Math.floor(LIMITS.symbols / 4) + 1}]) {
        const listReads = [], listSite = {ordinal: null, error: null};
        const listMsf = {streams: [{size: withList.length}], stream(i, o, n) {
            listReads.push({offset: o, count: n}); return Buffer.from(withList.subarray(o, o + n));
        }};
        redactedThrows(() => h.scanModule({m: listMsf, image: {sections: []}}, {stream_id: 0,
            module_index: 0, sym: withList.length - 4, c11: 0, c13: 0}, new Map(),
        {records: 0, procedures: 0, opaque: 0, inlinees}, null, INLINEES_GRAMMAR, listSite), 'ERR_MODULE_FRAMING');
        a.deepEqual(listReads, [{offset: 0, count: 4}, {offset: withList.length - 4, count: 4},
            {offset: 4, count: 4}, {offset: 4, count: list.length}]);
        a.deepEqual(listSite, {ordinal: null, error: null});
    }
});

test('ordinal redacts forged typed reader and query errors and cannot reuse a previous captured exception', () => {
    const h = thunkOrdinalHarness(), p = pack(zeroTailFixture());
    let previous;
    h.setInspector((...args) => {
        previous = new h.MetadataError('ERR_SYMBOL_UNSUPPORTED');
        args[9].ordinal = 23; args[9].error = previous; throw previous;
    });
    a.equal(h.diagnose().ordinal_u8, 23);
    h.setInspector(() => { throw previous; });
    a.throws(() => h.diagnose(), error => error === previous);
    for (const error of [previous, {code: 'ERR_SYMBOL_UNSUPPORTED', ordinal_u8: 23}, 'ERR_SYMBOL_UNSUPPORTED']) {
        const reader = {size: p.reader.size, read() { throw error; }};
        redactedThrows(() => invokeThunk(p, reader), 'ERR_PDB_FRAMING');
    }
    let genuine;
    try { runInlinees(zeroTailFixture({modules: [{records: [record(0x1102)]}]})); }
    catch (error) { genuine = error; }
    let io = 0;
    a.throws(() => diagnoseThunk32OrdinalNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact(
        {get size() { io++; return p.reader.size; }}, p.exe, p.identity,
        {get addresses() { throw genuine; }}), error => error === genuine);
    a.equal(io, 0);
});

test('ordinal exports exactly five primitive fields, discards ordinary results and enforces UTF-8 bounds including newline', () => {
    const secret = 'NEVER_EXPORT/private/ordinal', clean = zeroTailFixture({
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}), modules: [
            {module_name: secret, object_name: secret, records: [proc({name: secret}), inlineesRecord([0xdeadbeef])]}]}),
        refused = zeroTailFixture({modules: [{module_name: secret, object_name: secret,
            records: [proc({name: secret}), ordinalRecord(254, 65536)]}]}), stringify = JSON.stringify;
    for (const [f, ordinal] of [[clean, null], [refused, 254]]) {
        const result = thunkOrdinal(f); thunkOrdinalResult(result, ordinal);
        for (const forbidden of [secret, 'kind_u16', 'address', 'index', 'length', 'count', 'name', 'hash',
            'payload', 'bytes', 'extent', 'selected_modules', 'guid', 'deadbeef', '0x']) {
            a.ok(!JSON.stringify(result).includes(forbidden));
        }
        result.ordinal_u8 = 9; result.extra = secret; thunkOrdinalResult(thunkOrdinal(f), ordinal);
        for (const bytes of [1023, 1024]) {
            JSON.stringify = function(value, ...args) {
                const out = stringify.call(this, value, ...args);
                if (!value || value.profile !== THUNK_ORDINAL_PROFILE) return out;
                const padding = bytes - Buffer.byteLength(out);
                return out + 'é'.repeat(Math.floor(padding / 2)) + (padding % 2 ? ' ' : '');
            };
            try {
                if (bytes === 1023) a.equal(thunkOrdinal(f).profile, THUNK_ORDINAL_PROFILE);
                else redactedThrows(() => thunkOrdinal(f), 'ERR_OUTPUT_BOUND');
            } finally { JSON.stringify = stringify; }
        }
    }
    for (const profile of [INLINEES_PROFILE, THUNK_ORDINAL_PROFILE]) {
        JSON.stringify = function(value, ...args) {
            if (value && value.profile === profile) throw Error('NEVER_EXPORT/private-serializer');
            return stringify.call(this, value, ...args);
        };
        try {
            redactedThrows(() => thunkOrdinal(clean), 'ERR_PDB_FRAMING');
            if (profile === THUNK_ORDINAL_PROFILE) redactedThrows(() => thunkOrdinal(refused), 'ERR_PDB_FRAMING');
            else thunkOrdinalResult(thunkOrdinal(refused), 254);
        } finally { JSON.stringify = stringify; }
    }
});

const THUNK3_GRAMMAR = 'thunk32_ordinal3_common_prefix_namez_opaque_tail_disjoint_v1';
const THUNK3_PROFILE = INLINEES_PROFILE + '_framing_' + THUNK3_GRAMMAR;
function thunk3Record({bytes, ordinal = 3, offset = 0x80, segment = 1, length = 0x10,
    name = Buffer.alloc(0), terminate = true, tail = Buffer.from([0xf2, 0xf1])} = {}) {
    const text = Buffer.isBuffer(name) ? name : Buffer.from(name),
        n = bytes ?? Math.ceil((25 + text.length + Number(terminate) + tail.length) / 4) * 4,
        b = record(0x1102, n);
    b.fill(0xa7, 4); b.writeUInt32LE(offset, 16); b.writeUInt16LE(segment, 20);
    b.writeUInt16LE(length, 22);
    if (n > 24) b[24] = ordinal;
    if (n > 25) text.copy(b, 25);
    const end = 25 + text.length;
    if (terminate && end < n) b[end] = 0;
    if (terminate && end + 1 < n) tail.copy(b, end + 1);
    return b;
}
function invokeFramedThunk3(p, reader = p.reader, q = query()) {
    return inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExactThunk3FramedOpaqueTail(
        reader, p.exe, p.identity, q);
}
function runFramedThunk3(f = zeroTailFixture(), q = query()) {
    return invokeFramedThunk3(pack(f), undefined, q);
}
function framedThunk3Result(result, records) {
    a.equal(result.profile, THUNK3_PROFILE);
    a.deepEqual(result.thunk32_profile, {grammar: THUNK3_GRAMMAR, records,
        variant_data_read: false, variant_data_validated: false});
    a.deepEqual(Reflect.ownKeys(result.thunk32_profile),
        ['grammar', 'records', 'variant_data_read', 'variant_data_validated']);
    a.equal(result.records, result.supported_procedure_records + result.opaque_records);
    a.ok(records + result.inlinees_profile.records <= result.opaque_records);
    a.equal(result.selected_modules.reduce((n, m) => n + m.thunk_records, 0), records);
    for (const m of result.selected_modules) {
        a.equal(m.records, m.supported_procedure_records + m.opaque_records);
        a.ok(m.thunk_records <= m.opaque_records);
    }
    for (const key of ['unsupported_semantic_coverage', 'all_pdb_alias_coverage_confirmed',
        'whole_pdb_coverage_confirmed', 'reference_linkage_confirmed', 'instruction_boundaries_confirmed',
        'function_completeness_confirmed', 'unique_owner_confirmed', 'purity_confirmed',
        'unwind_interpretation_confirmed', 'runtime_behavior_confirmed', 'names_exported', 'code_bytes_read']) {
        a.equal(result[key], false);
    }
    a.equal(result.declaration_extent_only, true);
    a.equal(result.complete_selected_symbol_record_framing, true);
}

test('ordinal-3 framing admits bounded names with arbitrary opaque tails and only adds closed profile accounting', () => {
    for (const b of [thunk3Record(), thunk3Record({tail: Buffer.from([0, 0])}),
        thunk3Record({tail: Buffer.from([0x19, 0xef])}),
        thunk3Record({name: Buffer.from([0xff, 0xfe]), tail: Buffer.alloc(0)}),
        thunk3Record({bytes: 0xff00})]) {
        const selected = proc(), list = inlineesRecord([0xffffffff, 0]), f = zeroTailFixture({modules: [
            {records: [b, list, record(), selected]}, {records: [proc({name: 'second alias'})]}]}),
            r = runFramedThunk3(f);
        framedThunk3Result(r, 1);
        const before = runInlinees(zeroTailFixture({modules: [
            {records: [record(0x7777, b.length), list, record(), selected]},
            {records: [proc({name: 'second alias'})]}]}));
        a.deepEqual(r, {...before, profile: THUNK3_PROFILE,
            selected_modules: before.selected_modules.map((m, i) => ({...m, thunk_records: i === 0 ? 1 : 0})),
            thunk32_profile: {grammar: THUNK3_GRAMMAR, records: 1,
                variant_data_read: false, variant_data_validated: false}});
    }
    const f = zeroTailFixture({modules: [{records: [inlineesRecord(), proc()]}]}), r = runFramedThunk3(f);
    framedThunk3Result(r, 0);
    a.equal(r.addresses[0].procedure_records_sha256, runInlinees(f).addresses[0].procedure_records_sha256);
});

test('ordinal-3 record bounds precede all ordinal and body reads and non-three ordinals precede address/name reads', () => {
    for (const bytes of [4, 24, 25, 0xff04, 65536]) {
        const b = bytes >= 24 ? thunk3Record({bytes}) : record(0x1102, bytes),
            p = pack(zeroTailFixture({modules: [{records: [b, Buffer.alloc((4 - bytes % 4) % 4), proc()]}]}));
        redactedThrows(() => invokeFramedThunk3(p), 'ERR_MODULE_FRAMING');
        a.deepEqual(observedModuleIntervals(p, 9), [{offset: 0, count: 4},
            {offset: p.modules[0].symbols.length, count: 4}, {offset: 4, count: 4}]);
    }
    for (let ordinal = 0; ordinal <= 255; ordinal++) {
        if (ordinal === 3) continue;
        const b = thunk3Record({ordinal, length: 0, name: 'NEVER_EXPORT'}),
            p = pack(zeroTailFixture({modules: [{records: [b, proc()]}]})),
            guard = excludesPhysical(p, [...modulePhysicalIntervals(p, 9, 8, 28),
                ...modulePhysicalIntervals(p, 9, 29, 4 + b.length)]);
        redactedThrows(() => invokeFramedThunk3(p, guard.reader), 'ERR_SYMBOL_UNSUPPORTED');
        a.deepEqual(observedModuleIntervals(p, 9).slice(2), [{offset: 4, count: 4}, {offset: 28, count: 1}]);
    }
    for (const change of [b => b.writeUInt16LE(0xffff), b => b.writeUInt16LE(30),
        b => b.writeUInt16LE(65534)]) {
        const b = thunk3Record(); change(b);
        const p = pack(zeroTailFixture({modules: [{records: [b]}]}));
        redactedThrows(() => invokeFramedThunk3(p), 'ERR_MODULE_FRAMING');
        a.equal(observedModuleIntervals(p, 9).length, 3);
    }
});

test('ordinal-3 names stop at the first NUL and enforce record and 1024-byte candidate limits without lookahead', () => {
    for (const [name, terminate, bytes, accepted, expected] of [
        [Buffer.alloc(0), true, 28, true, 1], [Buffer.from([0xff, 0xfe]), true, 28, true, 3],
        [Buffer.alloc(1023, 0xf1), true, 1052, true, 1024],
        [Buffer.alloc(1024, 0x41), true, 1052, false, 1024],
        [Buffer.alloc(3, 0x41), false, 28, false, 3],
    ]) {
        const b = thunk3Record({name, terminate, bytes}), p = pack(zeroTailFixture({modules: [{records: [b, proc()]}]})),
            guard = excludesPhysical(p, modulePhysicalIntervals(p, 9, 4 + 25 + expected, 4 + b.length));
        if (accepted) framedThunk3Result(invokeFramedThunk3(p, guard.reader), 1);
        else redactedThrows(() => invokeFramedThunk3(p, guard.reader), 'ERR_MODULE_FRAMING');
        a.deepEqual(observedModuleIntervals(p, 9).filter(r => r.offset >= 29 && r.offset < 4 + b.length),
            Array.from({length: expected}, (_, i) => ({offset: 29 + i, count: 1})));
    }
    const b = thunk3Record({name: Buffer.from([0, 0xff, 0xfe])}), p = pack(zeroTailFixture({modules: [{records: [b, proc()]}]}));
    framedThunk3Result(invokeFramedThunk3(p, excludesPhysical(p, modulePhysicalIntervals(p, 9, 30, 4 + b.length)).reader), 1);
});

test('ordinal-3 thunk count cap is global, accepts 256 and refuses 257 before any extra body or name read', () => {
    const first = Array.from({length: 128}, () => thunk3Record()), second = first.map(b => Buffer.from(b));
    const accepted = zeroTailFixture({modules: [{records: [proc(), ...first]}, {records: second}]}), r = runFramedThunk3(accepted);
    framedThunk3Result(r, 256); a.deepEqual(r.selected_modules.map(m => m.thunk_records), [128, 128]);
    for (const extra of [thunk3Record(), thunk3Record({ordinal: 0})]) {
        const p = pack(zeroTailFixture({modules: [{records: [proc(), ...first]}, {records: [...second, extra]}]})),
            at = 4 + second.reduce((sum, b) => sum + b.length, 0),
            guard = excludesPhysical(p, modulePhysicalIntervals(p, 12, at + 4, at + extra.length));
        redactedThrows(() => invokeFramedThunk3(p, guard.reader), 'ERR_MODULE_BOUND');
        a.deepEqual(observedModuleIntervals(p, 12).at(-1), {offset: at, count: 4});
    }
});

test('ordinal-3 aggregate name budget counts terminators and refuses before candidate 65537 across modules', () => {
    const long = () => thunk3Record({name: Buffer.alloc(1023, 0x41)}),
        first = Array.from({length: 32}, long), second = Array.from({length: 32}, long),
        f = zeroTailFixture({modules: [{records: [proc(), ...first]}, {records: second}]});
    framedThunk3Result(runFramedThunk3(f), 64);
    const extra = thunk3Record(), p = pack(zeroTailFixture({modules: [
        {records: [proc(), ...first]}, {records: [...second, extra]}]})),
        at = 4 + second.reduce((n, b) => n + b.length, 0),
        guard = excludesPhysical(p, modulePhysicalIntervals(p, 12, at + 25, at + extra.length));
    redactedThrows(() => invokeFramedThunk3(p, guard.reader), 'ERR_MODULE_BOUND');
    a.deepEqual(observedModuleIntervals(p, 12).slice(-3),
        [{offset: at, count: 4}, {offset: at + 24, count: 1}, {offset: at + 16, count: 8}]);
    // The last allowed byte can be a non-NUL; its following terminator must
    // still be refused without being fetched, even inside the same name.
    const short = thunk3Record({name: Buffer.alloc(1022, 0x42)}), partial = pack(zeroTailFixture({modules: [
        {records: [proc(), ...first]}, {records: [...second.slice(0, 31), short, thunk3Record({name: 'A'})]}]})),
        finalAt = 4 + second.slice(0, 31).reduce((n, b) => n + b.length, 0) + short.length,
        blocked = excludesPhysical(partial, modulePhysicalIntervals(partial, 12, finalAt + 26, finalAt + 28));
    redactedThrows(() => invokeFramedThunk3(partial, blocked.reader), 'ERR_MODULE_BOUND');
    a.deepEqual(observedModuleIntervals(partial, 12).at(-1), {offset: finalAt + 25, count: 1});
});

test('ordinal-3 declared intervals require positive length and mapped physical code before any name read', () => {
    for (const [fields, code] of [[{length: 0}, 'ERR_PROCEDURE_SIZE'],
        ...[{segment: 0}, {segment: 4}, {segment: 3}, {offset: 0xffffffff},
            {offset: 0x27f, length: 2}, {offset: 0x280, length: 1}, {length: 65535},
            {segment: 2, offset: 0x17f, length: 2}].map(fields => [fields, 'ERR_SOURCE_MAPPING'])]) {
        const p = pack(zeroTailFixture({modules: [{records: [thunk3Record(fields), proc()]}]})),
            guard = excludesPhysical(p, modulePhysicalIntervals(p, 9, 29, 32));
        redactedThrows(() => invokeFramedThunk3(p, guard.reader), code);
        a.deepEqual(observedModuleIntervals(p, 9).slice(2),
            [{offset: 4, count: 4}, {offset: 28, count: 1}, {offset: 20, count: 8}]);
    }
    for (const fields of [{offset: 0x270, length: 0x10}, {segment: 2, offset: 0x170, length: 0x10}]) {
        framedThunk3Result(runFramedThunk3(zeroTailFixture({modules: [{records: [thunk3Record(fields), proc()]}]})), 1);
    }
    for (const flags of [0xe0000020, 0x20000020, 0x40000020, 0x60000040]) {
        const f = sectionChange(zeroTailFixture({modules: [{records: [thunk3Record({segment: 2}), proc()]}]}),
            (b, at) => b.writeUInt32LE(flags, at + 40 + 36));
        replaceTestMap(f, authoredZeroTailMap(f));
        redactedThrows(() => runFramedThunk3(f), 'ERR_SOURCE_MAPPING');
    }
    const resource = zeroTailFixture({modules: [{records: [thunk3Record({segment: 2}), proc()]}]});
    resource.exe.writeUInt32LE(0x2000, resource.op + 128); resource.exe.writeUInt32LE(0x10, resource.op + 132);
    redactedThrows(() => runFramedThunk3(resource), 'ERR_SOURCE_MAPPING');
    const physical = sectionChange(zeroTailFixture({modules: [{records: [
        thunk3Record({segment: 2, offset: 0x1ff, length: 2}), proc()]}]}),
    (b, at) => b.writeUInt32LE(0x280, at + 40 + 8));
    replaceTestMap(physical, authoredZeroTailMap(physical));
    redactedThrows(() => runFramedThunk3(physical), 'ERR_SOURCE_MAPPING');
});

test('ordinal-3 mapped interval preserves high-half canonical and exclusive canonical endpoint checks', () => {
    const high = zeroTailFixture({modules: [{records: [thunk3Record(), proc()]}]});
    high.exe.writeBigUInt64LE(0xffff800000000000n, high.op + 24);
    framedThunk3Result(runFramedThunk3(high, query({addresses: ['0xffff800000001020']})), 1);
    const f = zeroTailFixture({modules: [{records: [proc(), thunk3Record({offset: 0xff0, length: 0x10})]}]}),
        old = f.exe;
    f.exe = Buffer.alloc(0x1600); old.copy(f.exe);
    f.exe.writeBigUInt64LE(0x7fffffff0000n, f.op + 24);
    f.exe.writeUInt32LE(0xf000, f.op + 16); f.exe.writeUInt32LE(0x10000, f.op + 56);
    f.exe.writeUInt32LE(0xf000, f.table + 12); f.exe.writeUInt32LE(0x1000, f.table + 8);
    f.exe.writeUInt32LE(0x1000, f.table + 16); f.exe.writeUInt32LE(0x1200, f.table + 40 + 20);
    f.exe.writeUInt32LE(0x1400, f.table + 80 + 20);
    old.subarray(0x800, 0xa00).copy(f.exe, 0x1400); f.exe.writeUInt32LE(0x14a0, 0x1480 + 24);
    f.streams[8] = Buffer.from(f.exe.subarray(f.table, f.table + 120)); replaceTestMap(f, authoredZeroTailMap(f));
    redactedThrows(() => runFramedThunk3(f, query({addresses: ['0x7ffffffff020'], module_indices: [0]})), 'ERR_SOURCE_MAPPING');
});

test('ordinal-3 intervals refuse all selectors inside their half-open declaration before reading the name', () => {
    for (const address of ['0x140001080', '0x140001081', '0x14000108f']) {
        const p = pack(zeroTailFixture({modules: [{records: [thunk3Record(), proc()]}]})),
            guard = excludesPhysical(p, modulePhysicalIntervals(p, 9, 29, 32));
        redactedThrows(() => invokeFramedThunk3(p, guard.reader, query({addresses: [VA, address]})), 'ERR_PROCEDURE_CONFLICT');
        a.deepEqual(observedModuleIntervals(p, 9).at(-1), {offset: 20, count: 8});
    }
    const end = zeroTailFixture({modules: [{records: [thunk3Record(), proc({offset: 0x90})]}]});
    framedThunk3Result(runFramedThunk3(end, query({addresses: ['0x140001090']})), 1);
    for (const address of ['0x140001090', '0x14000107f']) {
        redactedThrows(() => runFramedThunk3(zeroTailFixture({modules: [{records: [thunk3Record()]}]}),
            query({addresses: [address]})), 'ERR_PROCEDURE_ABSENT');
    }
    redactedThrows(() => runFramedThunk3(zeroTailFixture({modules: [{records: [thunk3Record()]}]}),
        query({addresses: ['0x140001080']})), 'ERR_PROCEDURE_CONFLICT');
});

test('ordinal-3 final overlap refusal is half-open, order independent and spans all selected modules and addresses', () => {
    for (const [offset, length, conflict] of [[0x30, 0x10, true], [0x10, 0x50, true],
        [0x10, 0x20, true], [0x40, 0x20, true], [0x20, 0x30, true], [0x4f, 1, true],
        [0x10, 0x10, false], [0x1f, 1, false], [0x50, 1, false], [0x50, 0x10, false]]) {
        const t = thunk3Record({offset, length}), p = proc();
        for (const modules of [[{records: [t, p]}], [{records: [p, t]}],
            [{records: [t]}, {records: [p]}], [{records: [p]}, {records: [t]}]]) {
            const f = zeroTailFixture({modules});
            for (const q of [query(), query({module_indices: modules.map((_, i) => i).reverse()})]) {
                if (conflict) redactedThrows(() => runFramedThunk3(f, q), 'ERR_PROCEDURE_CONFLICT');
                else framedThunk3Result(runFramedThunk3(f, q), 1);
            }
        }
    }
    const later = zeroTailFixture({modules: [{records: [proc(), thunk3Record({offset: 0x90})]},
        {records: [proc({offset: 0x80, size: 0x30})]}]});
    redactedThrows(() => runFramedThunk3(later, query({addresses: [VA, '0x140001080']})), 'ERR_PROCEDURE_CONFLICT');
    // A supported declaration whose exact start was not requested retains the
    // old matching-only extent scope; no extra whole-module extent census.
    framedThunk3Result(runFramedThunk3(later), 1);
    framedThunk3Result(runFramedThunk3(zeroTailFixture({modules: [{records: [proc()]},
        {records: [thunk3Record({offset: 0x30})]}]}), query({module_indices: [0]})), 0);
});

test('ordinal-3 complete traversal preserves later unsupported, inlinees, conflict, absence and reader failures', () => {
    const malformed = record(); malformed.writeUInt16LE(65535);
    const cases = [[malformed, 'ERR_MODULE_FRAMING'], [inlineesRecord([], {count: 1}), 'ERR_MODULE_FRAMING'],
        [proc({size: 0x31}), 'ERR_PROCEDURE_CONFLICT'], [proc({segment: 0}), 'ERR_SOURCE_MAPPING'],
        [thunk3Record({length: 0}), 'ERR_PROCEDURE_SIZE'], [thunk3Record({ordinal: 2}), 'ERR_SYMBOL_UNSUPPORTED'],
        ...guardedSymbolKinds.filter(k => ![0x1102, 0x1168].includes(k)).map(k => [record(k), 'ERR_SYMBOL_UNSUPPORTED']),
        ...[0x115f, 0x1160, 0x1161, 0x1167].map(k => [record(k), 'ERR_FASTLINK_UNSUPPORTED'])];
    a.equal(guardedSymbolKinds.filter(k => ![0x1102, 0x1168].includes(k)).length, 53);
    for (const [bad, code] of cases) for (const modules of [
        [{records: [proc(), inlineesRecord([0xffffffff]), thunk3Record(), bad]}],
        [{records: [proc(), thunk3Record()]}, {records: [inlineesRecord(), bad]}],
    ]) redactedThrows(() => runFramedThunk3(zeroTailFixture({modules})), code);
    const absent = zeroTailFixture({modules: [{records: [thunk3Record(), inlineesRecord([0])]}]});
    redactedThrows(() => runFramedThunk3(absent), 'ERR_PROCEDURE_ABSENT');
    const f = zeroTailFixture({modules: [{records: [proc(), thunk3Record()]}, {records: [inlineesRecord(), proc()]}]}), p = pack(f);
    redactedThrows(() => runFramedThunk3(f, query({addresses: [VA, '0x140001021']})), 'ERR_PROCEDURE_ABSENT');
    const guard = excludesPhysical(p, modulePhysicalIntervals(p, 12, 4, 8));
    redactedThrows(() => invokeFramedThunk3(p, guard.reader), 'ERR_PDB_FRAMING');
    framedThunk3Result(runFramedThunk3(f), 1);
});

function pageSplitLogicalReads(spans) {
    return spans.flatMap(({offset, count}) => {
        const out = [];
        while (count > 0) {
            const take = Math.min(count, 4096 - offset % 4096);
            out.push({offset, count: take}); offset += take; count -= take;
        }
        return out;
    });
}

test('ordinal-3 physical read stack is exact from module start across page/cache boundaries and preceding records', () => {
    for (const thunkAt of [4, 4068, 4072, 4076, 4092, 65508, 65512, 65516, 65532, 65536]) {
        const before = thunkAt === 4 ? [] : [inlineesRecord([0xffffffff]), proc({name: 'earlier'})],
            used = 4 + before.reduce((n, b) => n + b.length, 0);
        if (used < thunkAt) before.push(record(0x7777, thunkAt - used));
        const name = Buffer.from([0xff, 0xfe, 0x4e, 0x41, 0x4d, 0x45, 0x41, 0x42, 0x43]),
            b = thunk3Record({name, bytes: 0xff00}), after = record(0x7777, 65536), selected = proc(),
            c13 = Buffer.alloc(71, 0xef), refs = Buffer.alloc(19, 0xfe),
            p = pack(zeroTailFixture({modules: [{records: [...before, b, after, selected], c13, refs},
                {records: [inlineesRecord(), proc()]}]})), sym = p.modules[0].symbols.length,
            forbidden = [...modulePhysicalIntervals(p, 9, thunkAt + 4, thunkAt + 16),
                ...modulePhysicalIntervals(p, 9, thunkAt + 26 + name.length, thunkAt + b.length),
                ...modulePhysicalIntervals(p, 9, thunkAt + b.length + 4, thunkAt + b.length + after.length),
                ...modulePhysicalIntervals(p, 9, sym, sym + c13.length),
                ...modulePhysicalIntervals(p, 9, sym + c13.length + 4, p.modules[0].body.length)],
            expected = [{offset: 0, count: 4}, {offset: sym + c13.length, count: 4}];
        let at = 4;
        for (const prior of before) {
            expected.push({offset: at, count: 4});
            if (prior.readUInt16LE(2) === 0x7777) forbidden.push(...modulePhysicalIntervals(p, 9, at + 4, at + prior.length));
            else expected.push({offset: at, count: prior.length});
            at += prior.length;
        }
        expected.push({offset: thunkAt, count: 4}, {offset: thunkAt + 24, count: 1}, {offset: thunkAt + 16, count: 8},
            ...Array.from({length: name.length + 1}, (_, i) => ({offset: thunkAt + 25 + i, count: 1})),
            {offset: thunkAt + b.length, count: 4}, {offset: thunkAt + b.length + after.length, count: 4},
            {offset: thunkAt + b.length + after.length, count: selected.length});
        const guard = excludesPhysical(p, forbidden), r = invokeFramedThunk3(p, guard.reader);
        framedThunk3Result(r, 1);
        a.deepEqual(observedModuleIntervals(p, 9), pageSplitLogicalReads(expected));
        const list = inlineesRecord(), proc2 = proc(), sym2 = p.modules[1].symbols.length;
        a.deepEqual(observedModuleIntervals(p, 12), [{offset: 0, count: 4}, {offset: sym2, count: 4},
            {offset: 4, count: 4}, {offset: 4, count: list.length}, {offset: 4 + list.length, count: 4},
            {offset: 4 + list.length, count: proc2.length}]);
        a.ok(guard.attempts.every(r => r.count <= LIMITS.read));
    }
});

test('ordinal-3 exact physical reader faults stop at each fixed/name read without zero fill or metadata leakage', () => {
    let typed;
    try { runInlinees(zeroTailFixture({modules: [{records: [record(0x1102)]}]})); } catch (error) { typed = error; }
    const ForeignShared = require('node:vm').runInNewContext('SharedArrayBuffer');
    for (const thunkAt of [4, 4076, 65516]) for (const field of ['ordinal', 'address-first', 'address-last', 'name', 'nul']) {
        const name = Buffer.from([0xff, 0x41]), before = thunkAt === 4 ? [] : [record(0x7777, thunkAt - 4)],
            b = thunk3Record({name}), intervals = {ordinal: [24, 25], 'address-first': [16, 24],
                'address-last': [16, 24], name: [25, 26], nul: [27, 28]}, [start, end] = intervals[field];
        for (const mode of ['throw', 'string', 'forged', 'typed', 'short', 'null', 'shared', 'foreign', 'proxy']) {
            const p = pack(zeroTailFixture({modules: [{records: [...before, b, proc()]}]})),
                targets = modulePhysicalIntervals(p, 9, thunkAt + start, thunkAt + end),
                target = field === 'address-last' ? targets.at(-1) : targets[0],
                guard = excludesPhysical(p, [target], (o, n) => {
                    a.equal(o, target.offset); a.equal(n, target.count);
                    if (mode === 'throw') throw Error('NEVER_EXPORT/private-framing-reader');
                    if (mode === 'string') throw 'ERR_SYMBOL_UNSUPPORTED/private';
                    if (mode === 'forged') throw {code: 'ERR_SOURCE_MAPPING', name: 'NEVER_EXPORT'};
                    if (mode === 'typed') throw typed;
                    if (mode === 'short') return Buffer.alloc(n - 1);
                    if (mode === 'null') return null;
                    if (mode === 'proxy') return new Proxy(Buffer.alloc(n), {get() { throw Error('NEVER_EXPORT/proxy'); }});
                    return Buffer.from(mode === 'shared' ? new SharedArrayBuffer(n) : new ForeignShared(n));
                });
            redactedThrows(() => invokeFramedThunk3(p, guard.reader), ['short', 'null'].includes(mode) ? 'ERR_SHORT_READ' :
                ['shared', 'foreign'].includes(mode) ? 'ERR_INPUT_BOUND' : 'ERR_PDB_FRAMING');
            a.equal(guard.attempts.filter(r => r.offset === target.offset && r.count === target.count).length, 1);
            a.ok(!observedModuleIntervals(p, 9).some(r => r.offset === thunkAt + b.length));
        }
    }
});

test('ordinal-3 prerequisite reader faults and globally aliased pages refuse before thunk reads', () => {
    const ForeignShared = require('node:vm').runInNewContext('SharedArrayBuffer');
    for (const index of [1, 3, 8, 9]) for (const mode of ['throw', 'short', 'shared', 'foreign']) {
        const p = pack(zeroTailFixture({modules: [{records: [thunk3Record(), proc()]}]})),
            reader = {size: p.reader.size, read(o, n) {
                if (!p.entries[index].pages.includes(Math.floor(o / 4096))) return p.reader.read(o, n);
                if (mode === 'throw') throw Error('NEVER_EXPORT/private-prerequisite');
                if (mode === 'short') return Buffer.alloc(n - 1);
                return Buffer.from(mode === 'shared' ? new SharedArrayBuffer(n) : new ForeignShared(n));
            }};
        redactedThrows(() => invokeFramedThunk3(p, reader), mode === 'short' ? 'ERR_SHORT_READ' :
            mode === 'throw' ? 'ERR_PDB_FRAMING' : 'ERR_INPUT_BOUND');
    }
    const pageSlot = (p, stream, index = 0) => 4 + p.entries.length * 4 +
        p.entries.slice(0, stream).reduce((n, e) => n + e.pages.length * 4, 0) + index * 4;
    for (const alias of ['same-stream', 'other-stream', 'info', 'directory', 'blockmap']) {
        const p = pack(zeroTailFixture({modules: [{records: [thunk3Record({bytes: 0xff00}), proc()]}, {}]}));
        if (alias === 'same-stream') p.directory.writeUInt32LE(p.entries[9].pages[0], pageSlot(p, 9, 1));
        if (alias === 'other-stream') p.directory.writeUInt32LE(p.entries[9].pages[0], pageSlot(p, 12));
        if (alias === 'info') p.directory.writeUInt32LE(p.entries[9].pages[0], pageSlot(p, 1));
        if (alias === 'directory') p.directory.writeUInt32LE(4, pageSlot(p, 9));
        if (alias === 'blockmap') p.directory.writeUInt32LE(3, pageSlot(p, 9));
        redactedThrows(() => invokeFramedThunk3(p), 'ERR_PDB_FRAMING');
        a.deepEqual(observedModuleIntervals(p, 9), []); a.deepEqual(observedModuleIntervals(p, 12), []);
    }
});

test('ordinal-3 selector whitelist is unchanged and runs before PE, reader or proxy access', () => {
    let io = 0, pe = 0;
    const reader = new Proxy({}, {get() { io++; throw Error('NEVER_EXPORT/reader'); }}),
        exe = new Proxy({}, {get() { pe++; throw Error('NEVER_EXPORT/exe'); }});
    for (const q of [undefined, null, {}, [], {addresses: []}, {addresses: Array(1)},
        query({addresses: [VA, VA]}), query({addresses: ['0x800000000000']}),
        query({module_indices: undefined}), query({module_indices: [0, 0]}), query({module_indices: Array(1)}),
        query({grammar: THUNK3_GRAMMAR}), query({profile: THUNK3_PROFILE}), query({retry: true}),
        query({ordinal_u8: 3}), query({[Symbol('private')]: true})]) {
        redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExactThunk3FramedOpaqueTail(
            reader, exe, {}, q), 'ERR_SELECTORS');
    }
    a.equal(io, 0); a.equal(pe, 0);
    for (const q of [new Proxy({}, {ownKeys() { throw Error('NEVER_EXPORT/query'); }}),
        {get addresses() { throw Error('NEVER_EXPORT/query'); }}]) {
        redactedThrows(() => inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExactThunk3FramedOpaqueTail(
            reader, exe, {}, q), 'ERR_PDB_FRAMING');
    }
    a.equal(io, 0); a.equal(pe, 0);
    framedThunk3Result(runFramedThunk3(zeroTailFixture(), Object.assign(Object.create(null), query())), 0);
});

test('ordinal-3 framing state is per call, copies borrowed buffers and preserves old results and read modes', () => {
    const clean = zeroTailFixture({modules: [{records: [inlineesRecord([0]), proc()]}]}),
        framed = zeroTailFixture({modules: [{records: [thunk3Record({name: 'NEVER_EXPORT'}), proc()]}]}),
        p = pack(framed); let previous = null, nested = false;
    const reader = new Proxy({size: p.reader.size, read(o, n) {
        if (!nested) {
            nested = true; framedThunk3Result(runFramedThunk3(clean), 0);
            redactedThrows(() => runFramedThunk3(zeroTailFixture({modules: [{records: [thunk3Record({length: 0})]}]})),
                'ERR_PROCEDURE_SIZE');
            thunkOrdinalResult(thunkOrdinal(framed), 3);
        }
        if (previous !== null) previous.fill(0xee);
        previous = p.reader.read(o, n); return previous;
    }}, {get(target, key) { return Reflect.get(target, key); }});
    framedThunk3Result(invokeFramedThunk3(p, reader), 1); a.equal(nested, true);
    const calls = [inspect, inspectNamedStreamNiMacZero, inspectNamedStreamNiMacZeroAbsoluteTailFrameZero,
        inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
        diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZero,
        diagnoseUnsupportedSymbolNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact,
        diagnoseThunk32OrdinalNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact],
        fixtures = [fixture(), niFixture(), zeroTailFixture(), clean, zeroTailFixture(), clean, framed];
    for (let i = 0; i < calls.length; i++) {
        const before = pack(fixtures[i]), after = pack(fixtures[i]),
            expected = calls[i](before.reader, before.exe, before.identity, query());
        framedThunk3Result(runFramedThunk3(framed), 1); framedThunk3Result(runFramedThunk3(clean), 0);
        a.deepEqual(calls[i](after.reader, after.exe, after.identity, query()), expected);
        a.deepEqual(after.reads, before.reads);
    }
    for (const api of [inspectNamedStreamNiMacZeroAbsoluteTailFrameZero,
        inspectNamedStreamNiMacZeroAbsoluteTailFrameZeroInlineesExact]) {
        const before = pack(framed), after = pack(framed);
        redactedThrows(() => api(before.reader, before.exe, before.identity, query()), 'ERR_SYMBOL_UNSUPPORTED');
        framedThunk3Result(runFramedThunk3(framed), 1);
        redactedThrows(() => api(after.reader, after.exe, after.identity, query()), 'ERR_SYMBOL_UNSUPPORTED');
        a.deepEqual(after.reads, before.reads);
    }
});

test('ordinal-3 export excludes thunk fields and bounds UTF-8 output after full traversal and ordinary digests', () => {
    const secret = 'NEVER_EXPORT/private-thunk-name', f = zeroTailFixture({
        info: namedStreamNiMacZeroInfo({names: [{name: secret, stream: 8}]}), modules: [
            {module_name: secret, object_name: secret, records: [proc({name: secret}), thunk3Record({name: secret})]},
            {records: [inlineesRecord([0xdeadbeef]), thunk3Record({tail: Buffer.from(secret)})]}]}),
        r = runFramedThunk3(f), stringify = JSON.stringify;
    framedThunk3Result(r, 2); a.ok(!stringify(r).includes(secret)); a.ok(!stringify(r).includes('deadbeef'));
    const baseline = runInlinees(zeroTailFixture({modules: [{records: [proc({name: secret})]}, {records: [inlineesRecord([0xdeadbeef])]}]}));
    a.deepEqual(r.addresses, baseline.addresses);
    a.deepEqual(Reflect.ownKeys(r).sort(), [...Reflect.ownKeys(baseline), 'thunk32_profile'].sort());
    for (const m of r.selected_modules) a.deepEqual(Reflect.ownKeys(m).sort(),
        [...Reflect.ownKeys(baseline.selected_modules[0]), 'thunk_records'].sort());
    r.thunk32_profile.records = 99; r.thunk32_profile.name = secret;
    framedThunk3Result(runFramedThunk3(f), 2);
    for (const bytes of [65535, 65536]) {
        let finished = false;
        JSON.stringify = function(value, ...args) {
            const out = stringify.call(this, value, ...args);
            if (!value || value.profile !== THUNK3_PROFILE) return out;
            a.equal(value.selected_modules.length, 2); a.equal(value.thunk32_profile.records, 2);
            a.deepEqual(value.addresses, baseline.addresses); finished = true;
            const padding = bytes - Buffer.byteLength(out);
            return out + 'é'.repeat(Math.floor(padding / 2)) + (padding % 2 ? ' ' : '');
        };
        try {
            if (bytes === 65535) framedThunk3Result(runFramedThunk3(f), 2);
            else redactedThrows(() => runFramedThunk3(f), 'ERR_OUTPUT_BOUND');
            a.equal(finished, true);
        } finally { JSON.stringify = stringify; }
    }
    JSON.stringify = function(value, ...args) {
        if (value && value.profile === THUNK3_PROFILE) throw Error('NEVER_EXPORT/private-serializer');
        return stringify.call(this, value, ...args);
    };
    try { redactedThrows(() => runFramedThunk3(f), 'ERR_PDB_FRAMING'); } finally { JSON.stringify = stringify; }
});

// Private lexical injection exercises extent guards outside immutable public
// PE reachability. No alternate grammar or scan primitive is exported.
function thunk3FramingHarness() {
    const filename = require.resolve('../pdb_procedure_ranges.cjs'),
        source = require('node:fs').readFileSync(filename, 'utf8'),
        sandbox = {module: {exports: {}}, require: require('node:module').createRequire(filename), Buffer};
    require('node:vm').runInNewContext(source + `
module.exports = {scanModule, grammar: THUNK32_FRAMED_OPAQUE_TAIL_GRAMMAR,
    inlinees: INLINEES_COUNTED_U32_EXACT_GRAMMAR};
`, sandbox, {filename});
    return sandbox.module.exports;
}

test('ordinal-3 defensive extent fixtures enforce every mapped guard without reading names or mapped code', () => {
    const h = thunk3FramingHarness();
    for (const mutate of [
        image => image.sections[0].mappedBytes = 0x8f,
        image => image.imageBytes = 0x108f,
        image => { image.sections[0].rva = 0xffffffff; image.imageBytes = 0x200000000; },
        image => image.base = 0x7fffffffef80n,
        image => image.base = 0x7fffffffef71n,
        image => image.base = 0x7fffffffef70n,
        image => image.base = 0xffffffffffffffffn,
        ...['code', 'readable', 'executable'].map(key => image => image.sections[0][key] = false),
        ...['writable', 'resourceOverlap'].map(key => image => image.sections[0][key] = true),
    ]) {
        const b = thunk3Record(), body = Buffer.concat([Buffer.from([4, 0, 0, 0]), b, Buffer.alloc(4)]), reads = [],
            image = {base: 0x140000000n, imageBytes: 0x4000, sections: [{id: 'SEC01', raw: 0x200,
                rva: 0x1000, mappedBytes: 0x280, code: true, readable: true, executable: true,
                writable: false, resourceOverlap: false}]},
            m = {streams: [{size: body.length}], stream(i, o, n) {
                reads.push({offset: o, count: n}); return Buffer.from(body.subarray(o, o + n));
            }}, totals = {records: 0, procedures: 0, opaque: 0, inlinees: {records: 0, entries: 0},
                thunks: {intervals: [], nameBytes: 0}};
        mutate(image);
        redactedThrows(() => h.scanModule({m, image}, {stream_id: 0, module_index: 0, sym: 4 + b.length,
            c11: 0, c13: 0}, new Map(), totals, null, h.inlinees, null, h.grammar), 'ERR_SOURCE_MAPPING');
        a.deepEqual(reads, [{offset: 0, count: 4}, {offset: 4 + b.length, count: 4},
            {offset: 4, count: 4}, {offset: 28, count: 1}, {offset: 20, count: 8}]);
        a.equal(totals.thunks.intervals.length, 0); a.equal(totals.thunks.nameBytes, 0);
    }
    a.equal(Object.hasOwn(require('../pdb_procedure_ranges.cjs'), 'THUNK32_FRAMING_CAPS'), false);
    a.equal(Object.hasOwn(require('../pdb_procedure_ranges.cjs'), 'scanModule'), false);
});
