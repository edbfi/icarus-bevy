'use strict';
const test=require('node:test'),a=require('node:assert/strict'),{fixture,pack,pub}=require('./pdb_public_symbols_fixtures.cjs'),{inspect,msf,selector,LIMITS}=require('../pdb_public_symbols.cjs');
const {createHash}=require('node:crypto');
const get=f=>inspect(f.reader,f.exe,f.identity);
test('selectednames-neutralgeometry-identity-noRawBytes',()=>{const f=pack(fixture()),r=get(f);a.equal(r.hit_count,3);a.equal(r.hits[0].address,'0x140001020');a.equal(r.hits[1].file_offset,0x440);a.equal(r.hits[1].section_id,'SEC02');a(!JSON.stringify(r).includes('first16'));a(!r.older_server_equivalence);a(!r.class_owner_confirmed);});
test('allselectedprofiles+unselected-private-name-exclusion',()=>{for(const name of ['??0UWorld@@QEAA@XZ','??1UPendingNetGame@@UEAA@XZ','??_GUWorld@@UEAAPEAXI@Z','??_EUPendingNetGame@@UEAAPEAXI@Z','??_7FNetworkNotify@@6B@','??_7UPendingNetGame@@6B@','??_7UWorld@@6BFNetworkNotify@@@','?NotifyControlMessage@UWorld@@UEAAXXZ','?GetPrivateStaticClass@UPendingNetGame@@CAPEAVUClass@@XZ','?StaticClass@UWorld@@SAPEAVUClass@@XZ'])a(selector(name));for(const name of ['??0UWorldHelper@@QEAA@XZ','?NotifyControlMessage@Other@@UEAAXXZ','??_7UWorld@@6BOther@@@','private-player-marker'])a.equal(selector(name),null);const r=get(pack(fixture([pub('private-player-marker'),pub('?NotifyControlMessage@UWorld@@UEAAXXZ')])));a.equal(r.hit_count,1);a(!JSON.stringify(r).includes('private-player-marker'));});
test('symbolcrossesMSFpage+lastrecordcomplete',()=>{const skip=Buffer.alloc(4092);skip.writeUInt16LE(4090);skip.writeUInt16LE(0x9999,2);const f=pack(fixture([skip,pub('??0UWorld@@QEAA@XZ')]));a.equal(get(f).hits[0].record_offset,4092);});
test('exact32combinedhits+33failsclosed',()=>{a.equal(get(pack(fixture(Array.from({length:32},()=>pub('??0UWorld@@QEAA@XZ'))))).hit_count,32);a.throws(()=>get(pack(fixture(Array.from({length:33},()=>pub('??0UWorld@@QEAA@XZ'))))),/hit cap/);});
test('malformedunselectedPUB32andtrailingrecordreject',()=>{const p=pub('OtherSymbol');p.fill(65,14);a.throws(()=>get(pack(fixture([p]))),/terminator/);const p2=pub('OtherSymbol');p2.writeUInt16LE(65535);a.throws(()=>get(pack(fixture([p2]))),/extent/);a.throws(()=>get(pack(fixture([Buffer.from([2,0,0])]))),/record/);});
test('MSFheaderdirectorypages-stream-overlap-andabsence',()=>{for(const mutate of [f=>f.pdb[0]=0,f=>f.pdb.writeUInt32LE(512,32),f=>f.pdb.writeUInt32LE(LIMITS.directory+1,44),f=>f.pdb.writeUInt32LE(999999,52),f=>f.pdb.writeUInt32LE(4097,4*4096),f=>f.pdb.writeUInt32LE(3,4*4096+4+6*4),f=>f.pdb.writeUInt32LE(999999,4*4096+4+6*4)]){const f=pack(fixture());mutate(f);a.throws(()=>get(f));}const f=pack(fixture());f.streams[1]=null;a.throws(()=>get(pack(f)),/info stream/);});
test('DBInegativeoverflowoddoptionalOMAPoriginal-sectionsmachine-age',()=>{for(const [o,n,type]of [[24,-1,'i32'],[52,2147483647,'i32'],[48,13,'i32'],[70,5,'u16'],[72,5,'u16'],[58,0x14c,'u16'],[8,2,'u32']]){const f=fixture();f.streams[3][type==='i32'?'writeInt32LE':type==='u16'?'writeUInt16LE':'writeUInt32LE'](n,o);a.throws(()=>get(pack(f)));}const f=fixture();f.streams[3]=Buffer.concat([f.streams[3],Buffer.alloc(10,255)]);f.streams[3].writeInt32LE(22,48);f.streams[3].writeUInt16LE(5,84);a.equal(get(pack(f)).hit_count,3);});
test('PDBinfo+EXERSDS+sectionidentity',()=>{for(const mutate of [f=>f.streams[1][12]^=1,f=>f.streams[1].writeUInt32LE(2,8),f=>f.exe[0x4a4]^=1,f=>f.exe.writeUInt32LE(2,0x4b4),f=>f.exe.writeUInt32LE(0x4a1,0x498),f=>f.streams[5][8]^=1]){const f=fixture();mutate(f);a.throws(()=>get(pack(f)));}});
test('selectedsegmentnull-outofrange-rawpadding-kindmapping',()=>{for(const p of [pub('??0UWorld@@QEAA@XZ',0),pub('??0UWorld@@QEAA@XZ',4),pub('??0UWorld@@QEAA@XZ',1,0x180),pub('??0UWorld@@QEAA@XZ',2),pub('??_7UWorld@@6B@',1),pub('??0UWorld@@QEAA@XZ',1,0x20,16)])a.throws(()=>get(pack(fixture([p]))));});
test('oversizedPDB-rejectsbeforefirstread+symbolstreamcap-before-record',()=>{let reads=0;a.throws(()=>msf({size:LIMITS.pdb+1,read(){reads++;throw Error('mustnotread');}}),/size/);a.equal(reads,0);const f=pack(fixture());f.pdb.writeUInt32LE(LIMITS.symbols+1,4*4096+4+4*4);a.throws(()=>get(f));});
test('UObject-qualifiedprimary-vftables-exactinspect',()=>{const r=get(pack(fixture([pub('??_7UWorld@@6BUObject@@@',2,0x40,0),pub('??_7UPendingNetGame@@6BUObject@@@',2,0x60,0)])));a.equal(r.hit_count,2);a.deepEqual(r.hits.map(h=>h.segment_offset),[0x40,0x60]);a(r.hits.every(h=>!h.executable));a.equal(selector('??_7UWorld@@6BUObjectHelper@@@'),null);});
test('zero-selected-results-are-bounded-negativecoverage',()=>{const r=get(pack(fixture([pub('unselected')]))) ;a.equal(r.hit_count,0);a.equal(r.records,1);a(r.complete_symbol_record_framing);});

test('actual total-record cap accepts exactly2500000 and rejects2500001', () => {
    // An independently packed 10 MB non-PUB stream requires a multi-page MSF
    // directory. This exercises the real record loop and cap without targets.
    function largeEnvelope(symbols) {
        const f = fixture();
        f.streams[4] = symbols;
        const bs = 4096;
        const pageCount = f.streams.reduce((n, b) => n + (b ? Math.ceil(b.length / bs) : 0), 0);
        const directoryBytes = 4 + 4 * f.streams.length + 4 * pageCount;
        const directoryPages = Math.ceil(directoryBytes / bs);
        let nextPage = 4 + directoryPages;
        const streams = f.streams.map(b => {
            const pages = Array.from({ length: b ? Math.ceil(b.length / bs) : 0 }, () => nextPage++);
            return { size: b ? b.length : 0xffffffff, pages };
        });
        a(nextPage < bs, 'authored envelope avoids another FPM interval');
        const pdb = Buffer.alloc(nextPage * bs);
        Buffer.from('Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0', 'binary').copy(pdb);
        for (const [at, value] of [[32, bs], [36, 1], [40, nextPage], [44, directoryBytes], [52, 3]]) {
            pdb.writeUInt32LE(value, at);
        }
        const directory = Buffer.alloc(directoryBytes);
        directory.writeUInt32LE(streams.length);
        let at = 4 + streams.length * 4;
        streams.forEach((s, i) => {
            directory.writeUInt32LE(s.size, 4 + i * 4);
            for (const page of s.pages) {
                directory.writeUInt32LE(page, at);
                at += 4;
            }
        });
        a.equal(at, directory.length);
        for (let i = 0; i < directoryPages; i++) {
            pdb.writeUInt32LE(4 + i, 3 * bs + 4 * i);
            directory.subarray(i * bs, (i + 1) * bs).copy(pdb, (4 + i) * bs);
        }
        streams.forEach((s, i) => s.pages.forEach((page, j) => {
            f.streams[i].subarray(j * bs, (j + 1) * bs).copy(pdb, page * bs);
        }));
        return { ...f, reader: { size: pdb.length, read: (offset, length) => pdb.subarray(offset, offset + length) } };
    }
    const symbols = Buffer.alloc((LIMITS.records + 1) * 4);
    symbols.fill(Buffer.from([2, 0, 0x99, 0x99]));
    const exact = largeEnvelope(symbols.subarray(0, LIMITS.records * 4));
    const result = inspect(exact.reader, exact.exe, exact.identity);
    a.equal(result.records, 2_500_000);
    a.equal(result.hit_count, 0);
    a.equal(result.symbol_stream_bytes, 10_000_000);
    a.equal(result.complete_symbol_record_framing, true);
    const extra = largeEnvelope(symbols);
    a.throws(() => inspect(extra.reader, extra.exe, extra.identity), /symbol record\/count cap/);
    const summary = { exact_addresses_only: true, exact_summary_only: true, addresses: ['0x140001030'] };
    const summarized = inspect(exact.reader, exact.exe, exact.identity, summary);
    a.equal(summarized.records, 2_500_000);
    a.equal(summarized.aggregates[0].selected_pub32_record_count, 0);
    a.equal(summarized.complete_symbol_record_framing, true);
    a.throws(() => inspect(extra.reader, extra.exe, extra.identity, summary), /symbol record\/count cap/);
});

test('address queries export only names at exact queried code addresses', () => {
    const f = pack(fixture([pub('?Helper@@YAXXZ', 1, 0x30), pub('?Other@@YAXXZ', 1, 0x34), pub('??0UWorld@@QEAA@XZ', 1, 0x20)]));
    const r = inspect(f.reader, f.exe, f.identity, { addresses: ['0x140001030', '0x140001020'] });
    a.deepEqual(r.hits.map(h => [h.name, h.family, h.address]), [
        ['?Helper@@YAXXZ', 'address_query', '0x140001030'],
        ['??0UWorld@@QEAA@XZ', 'constructor_or_destructor', '0x140001020'],
    ]);
    a.deepEqual(r.address_queries, ['0x140001030', '0x140001020']);
    a.equal(r.unselected_names_exported, false);
    a.deepEqual(get(f).hits.map(h => h.name), ['??0UWorld@@QEAA@XZ']);
});

test('address queries are bounded, exact and fail closed', () => {
    const f = pack(fixture([pub('?Helper@@YAXXZ', 1, 0x30)]));
    a.equal(inspect(f.reader, f.exe, f.identity, { addresses: ['0x140001031'] }).hit_count, 0);
    for (const query of [{}, { addresses: [] }, { addresses: ['140001030'] }, { addresses: ['0x140001030', '0x140001030'] },
        { addresses: ['0x140001030'], extra: 1 }, { addresses: Array.from({ length: 33 }, (_, i) => '0x' + (0x140001000 + i).toString(16)) }]) {
        a.throws(() => inspect(f.reader, f.exe, f.identity, query), /address query/);
    }
    const data = pack(fixture([pub('?Table@@3PAXA', 2, 0x40, 0)]));
    a.throws(() => inspect(data.reader, data.exe, data.identity, { addresses: ['0x140002040'] }), /strict source mapping/);
    const odd = pub('?Helper@@YAXXZ', 1, 0x30);
    odd[15] = 0x7f;
    a.throws(() => inspect(pack(fixture([odd])).reader, f.exe, f.identity, { addresses: ['0x140001030'] }), /queried public name/);
});

// These records and addresses are independently authored fixture geometry only.
const exactQuery = (extra = {}) => ({ addresses: ['0x140001030'], exact_addresses_only: true, ...extra });
const exactRun = (records, query = exactQuery()) => {
    const f = pack(fixture(records));
    return inspect(f.reader, f.exe, f.identity, query);
};

test('exact-only ignores many earlier default hits and retains duplicate and folded aliases in record order', () => {
    const defaults = Array.from({ length: 40 }, () => [
        pub('??1UPendingNetGame@@UEAA@XZ', 1, 0x20),
        pub('??_7UWorld@@6B@', 2, 0x40, 0),
    ]).flat();
    const aliases = ['?CompareAlpha@@YA_NXZ', '?CompareBeta@@YA_NXZ', '?CompareAlpha@@YA_NXZ',
        'undecorated_alias', '??0UWorld@@QEAA@XZ'];
    const records = [...defaults, ...aliases.map(name => pub(name, 1, 0x30)), pub('?Nearby@@YA_NXZ', 1, 0x31)];
    const r = exactRun(records);
    a.equal(r.records, records.length);
    a.equal(r.hit_count, aliases.length);
    a.deepEqual(r.hits.map(h => h.name), aliases);
    a(r.hits.every(h => h.address === '0x140001030' && h.family === 'address_query'));
    a.deepEqual(r.hits.map(h => h.id), ['SYM01', 'SYM02', 'SYM03', 'SYM04', 'SYM05']);
    a.equal(new Set(r.hits.map(h => h.record_offset)).size, aliases.length);
    a(r.hits[0].record_offset >= defaults.reduce((n, p) => n + p.length, 0));
    a.equal(r.hit_limit, 32);
    a.equal(r.exact_addresses_only, true);
    a.equal(r.selected_name_profile, 'exact_address_only_ascii_max1024');
    for (const field of ['class_name_queries', 'class_method_queries', 'struct_ops_queries', 'name_contains_queries']) {
        a.deepEqual(r[field], []);
    }
    a.deepEqual(r.address_queries, ['0x140001030']);
    a.equal(r.complete_symbol_record_framing, true);
    a.equal(r.unselected_names_exported, false);
    a.equal(r.instruction_boundaries_confirmed, false);
    a.equal(r.class_owner_confirmed, false);
    a.equal(r.older_server_equivalence, false);
    a.equal(r.runtime_behavior_proven, false);
    const json = JSON.stringify(r);
    for (const name of ['??1UPendingNetGame@@UEAA@XZ', '??_7UWorld@@6B@', '?Nearby@@YA_NXZ']) {
        a(!json.includes(name));
    }
});

test('exact-only selects only requested VAs with no implicit names and permits complete zero-hit results', () => {
    const records = [pub('??0UWorld@@QEAA@XZ'), pub('?StaticClass@UPendingNetGame@@SAPEAVUClass@@XZ'),
        pub('??_7UWorld@@6B@', 2, 0x40, 0), pub('?ApplyState@UFoo@@QEAAXH@Z', 1, 0x30),
        pub('?DeltaSerializeItem@@YAXXZ', 1, 0x34)];
    const r = exactRun(records, exactQuery({ addresses: ['0x140001034', '0x0140001030'] }));
    a.deepEqual(r.hits.map(h => [h.name, h.address, h.family]), [
        ['?ApplyState@UFoo@@QEAAXH@Z', '0x140001030', 'address_query'],
        ['?DeltaSerializeItem@@YAXXZ', '0x140001034', 'address_query'],
    ]);
    a.deepEqual(r.address_queries, ['0x140001034', '0x140001030']);
    const miss = exactRun(records, exactQuery({ addresses: ['0x140001031'] }));
    a.equal(miss.hit_count, 0);
    a.equal(miss.records, records.length);
    a.equal(miss.complete_symbol_record_framing, true);
});

test('exact-only default and explicit alias budgets reject overflow instead of returning partial aliases', () => {
    const aliases = n => Array.from({ length: n }, (_, i) => pub('?Alias' + i + '@@YAXXZ', 1, 0x30));
    a.equal(exactRun(aliases(32)).hit_count, 32);
    a.throws(() => exactRun(aliases(33)), /selected public hit cap/);
    const expanded = exactRun(aliases(64), exactQuery({ exact_hit_limit: 64 }));
    a.equal(expanded.hit_count, 64);
    a.equal(expanded.hit_limit, 64);
    a.throws(() => exactRun(aliases(65), exactQuery({ exact_hit_limit: 64 })), /selected public hit cap/);
    a.equal(exactRun(aliases(1), exactQuery({ exact_hit_limit: 1 })).hit_count, 1);
    a.throws(() => exactRun(aliases(2), exactQuery({ exact_hit_limit: 1 })), /selected public hit cap/);
    a.equal(exactRun(aliases(1), exactQuery({ exact_hit_limit: 512 })).hit_limit, 512);
    a.throws(() => exactRun(aliases(513), exactQuery({ exact_hit_limit: 512 })), /selected public hit cap/);
    // A larger alias budget does not change the global serialized-output cap.
    a.throws(() => exactRun(aliases(512), exactQuery({ exact_hit_limit: 512 })), /result output cap/);
    a.equal(LIMITS.hits, 32);
    a.equal(LIMITS.output, 65536);
});

test('exact-only malformed or missing address selection and conflicting name modes fail before any reader IO', () => {
    const f = pack(fixture());
    let reads = 0;
    const reader = { size: f.reader.size, read() { reads++; throw Error('unexpected fixture read'); } };
    const invalid = [
        [{ exact_addresses_only: true }, /requires addresses/],
        [exactQuery({ addresses: undefined }), /requires addresses/],
        ...[null, [], '0x140001030', Array(1), [7], [''], ['140001030'], ['0x'], ['0xABC'],
            ['0x10000000000000000'], ['-0x1'], ['0x140001030', '0x0140001030'],
            Array.from({ length: 33 }, (_, i) => '0x' + (0x140001000 + i).toString(16))]
            .map(addresses => [exactQuery({ addresses }), /address query/]),
        ...[false, null, undefined, 0, 1, 'true'].map(exact_addresses_only => [exactQuery({ exact_addresses_only }), /mode must be true/]),
        ...[undefined, null, 0, -1, 1.5, 513, '64', NaN, Infinity]
            .map(exact_hit_limit => [exactQuery({ exact_hit_limit }), /exact address hit limit/]),
        [{ addresses: ['0x140001030'], exact_hit_limit: 64 }, /exact address hit limit/],
        ...[
            { class_names: ['UFoo'] }, { class_methods: [{ class_name: 'UFoo', method: 'ApplyState' }] },
            { struct_ops_methods: [{ struct_name: 'FMove', method: 'NetSerialize' }] }, { name_contains: ['DeltaSerialize'] },
            { class_names: undefined }, { class_methods: [] }, { struct_ops_methods: null }, { name_contains: undefined },
        ].map(mode => [exactQuery(mode), /conflicts with name\/class modes/]),
        [exactQuery({ extra: 1 }), /query shape/],
    ];
    for (const [query, message] of invalid) {
        a.throws(() => inspect(reader, f.exe, f.identity, query), message);
    }
    a.equal(reads, 0);
});

test('legacy defaults and additive address selection retain their families and shared 32-hit cap', () => {
    const records = [pub('??0UWorld@@QEAA@XZ', 1, 0x20), pub('?Helper@@YAXXZ', 1, 0x30),
        pub('??_7UWorld@@6B@', 2, 0x40, 0)];
    const f = pack(fixture(records));
    const ordinary = inspect(f.reader, f.exe, f.identity, { addresses: ['0x140001030'] });
    a.deepEqual(ordinary.hits.map(h => h.family), ['constructor_or_destructor', 'address_query',
        'primary_or_FNetworkNotify_secondary_vftable']);
    a(!Object.hasOwn(ordinary, 'exact_addresses_only'));
    a.equal(ordinary.hit_limit, 32);
    a.equal(ordinary.selected_name_profile, 'explicit_decorated_ascii_allowlist_max1024_not_all_class_members');
    a.deepEqual(get(f).hits.map(h => h.name), ['??0UWorld@@QEAA@XZ', '??_7UWorld@@6B@']);
    const many = pack(fixture([...Array.from({ length: 32 }, () => records[0]), records[1]]));
    a.throws(() => inspect(many.reader, many.exe, many.identity, { addresses: ['0x140001030'] }), /selected public hit cap/);
});

test('exact-only validates complete unselected record framing even after finding requested aliases', () => {
    const unterminated = pub('unselected_private_marker');
    unterminated.fill(65, 14);
    const badPadding = pub('AB');
    badPadding[badPadding.length - 1] = 0x40;
    const extraPadding = Buffer.concat([pub('unselected_private_marker'), Buffer.alloc(4)]);
    extraPadding.writeUInt16LE(extraPadding.length - 2);
    const badExtent = pub('unselected_private_marker');
    badExtent.writeUInt16LE(65535);
    for (const [record, error] of [
        [unterminated, /PUB32 terminator/], [badPadding, /PUB32 padding policy/],
        [extraPadding, /PUB32 padding policy/], [badExtent, /symbol record extent/],
        [Buffer.from([2, 0, 0x0e, 0x11]), /PUB32 minimum size/], [Buffer.from([2, 0, 0]), /symbol record\/count cap/],
    ]) {
        a.throws(() => exactRun([pub('?Selected@@YAXXZ', 1, 0x30), record]), error);
        a.throws(() => exactRun([record], exactQuery({ addresses: ['0x140001031'] })), error);
    }
});

test('exact-only preserves identity, section, DBI, MSF and short-read rejection', () => {
    const mutations = [
        [f => { f.streams[1][12] ^= 1; }, /PDB info identity mismatch/],
        [f => { f.exe[0x4a4] ^= 1; }, /EXE CodeView identity mismatch/],
        [f => { f.streams[5][8] ^= 1; }, /PDB\/PE section headers mismatch/],
        [f => { f.streams[3].writeUInt16LE(5, 70); }, /OMAP unsupported/],
        [f => { f.streams[3].writeUInt32LE(1, 24); }, /DBI substream exact extent/],
    ];
    for (const [mutate, message] of mutations) {
        const f = fixture([pub('?Selected@@YAXXZ', 1, 0x30)]);
        mutate(f);
        const packed = pack(f);
        a.throws(() => inspect(packed.reader, packed.exe, packed.identity, exactQuery()), message);
    }
    const overlap = pack(fixture());
    overlap.pdb.writeUInt32LE(4, 4 * 4096 + 4 + 6 * 4);
    a.throws(() => inspect(overlap.reader, overlap.exe, overlap.identity, exactQuery()), /stream block overlap/);
    const short = pack(fixture());
    short.reader.read = (at, n) => short.pdb.subarray(at, at + n - 1);
    a.throws(() => inspect(short.reader, short.exe, short.identity, exactQuery()), /PDB short read/);
});

test('exact-only retains strict code mapping and name profile with redacted failures', () => {
    for (const record of [pub('?Alias@@YAXXZ', 2, 0x40, 0), pub('?Alias@@YAXXZ', 3, 0x40),
        pub('?Alias@@YAXXZ', 1, 0x180)]) {
        const segment = record.readUInt16LE(12), offset = record.readUInt32LE(8);
        const address = '0x' + (0x140000000 + segment * 0x1000 + offset).toString(16);
        a.throws(() => exactRun([record], exactQuery({ addresses: [address] })), /strict source mapping/);
    }
    a.throws(() => exactRun([pub('?Alias@@YAXXZ', 1, 0x30, 16)]), /selected PUB32 segment\/flags/);
    const marker = 'authored_private_marker';
    const overlong = pub(marker + 'A'.repeat(1024), 1, 0x30);
    const nonAscii = pub(marker, 1, 0x30);
    nonAscii[14] = 0x80;
    for (const record of [overlong, nonAscii]) {
        a.throws(() => exactRun([record]), error => {
            a.equal(error.message, 'queried public name profile unsupported');
            a(!error.message.includes(marker));
            a(!error.message.includes('0x140001030'));
            return true;
        });
    }
    a.throws(() => exactRun([], exactQuery({ addresses: [marker] })), error => {
        a.equal(error.message, 'address query format');
        a(!error.message.includes(marker));
        return true;
    });
});

const summaryQuery = (extra = {}) => exactQuery({ exact_summary_only: true, ...extra });
const summaryRun = (records, query = summaryQuery()) => exactRun(records, query);
// The test oracle hashes authored fixture bytes directly, independently of the
// parser's streaming accumulator. Headers, flags, NULs and padding all matter.
const rawRecordHash = records => createHash('sha256').update(Buffer.concat(records)).digest('hex');

test('exact summary validates over512 folded records, counts duplicates and hashes every complete raw record', () => {
    const duplicate = pub('authored_duplicate_alias', 1, 0x30, 0);
    const records = Array.from({ length: 2049 }, (_, i) => i % 3 === 0 ? duplicate :
        pub('?AuthoredFoldedAlias' + i + '@@YAXXZ', 1, 0x30, i % 16));
    // Include the other permitted padding spelling; its raw bytes enter the hash.
    const padded = pub('AB', 1, 0x30);
    a.equal(padded.length, 20);
    padded.set([0xf3, 0xf2, 0xf1], 17);
    records.splice(900, 0, padded);
    const r = summaryRun(records);
    a.equal(r.profile, 'selected_client_MSFPDB7_DBI_S_PUB32_exact_address_summary_only');
    a.equal(r.exact_addresses_only, true);
    a.equal(r.exact_summary_only, true);
    a.equal(r.records, records.length);
    a(r.symbol_stream_bytes > 65536, 'authored selection crosses the symbol cache boundary');
    a.equal(r.aggregate_count, 1);
    a.equal(r.address_limit, 32);
    a.deepEqual(r.address_queries, ['0x140001030']);
    a.deepEqual(r.aggregates, [{
        address: '0x140001030', match_status: 'validated_matching_pub32_records',
        selected_pub32_record_count: records.length,
        selected_pub32_record_bytes: records.reduce((n, record) => n + record.length, 0),
        selected_pub32_records_sha256: rawRecordHash(records),
        source_mapping: { segment: 1, segment_offset: 0x30, section_id: 'SEC01', rva: 0x1030,
            file_offset: 0x230, executable: true, source_extent_bytes: 1 },
    }]);
    a.equal(r.record_hash_profile,
        'sha256_concatenated_complete_raw_selected_PUB32_records_in_stream_order_duplicates_included');
    a.equal(r.selected_name_profile, 'exact_address_only_ascii_max1024_validated_not_retained');
    a.equal(r.names_retained, false);
    a.equal(r.per_hit_records_retained, false);
    a.equal(r.complete_symbol_record_framing, true);
    for (const field of ['instruction_boundaries_confirmed', 'class_owner_confirmed',
        'unique_ownership_confirmed', 'semantics_confirmed', 'older_server_equivalence', 'runtime_behavior_proven']) {
        a.equal(r[field], false);
    }
    const json = JSON.stringify(r);
    for (const field of ['hits', 'hit_count', 'hit_limit', 'name', 'flags', 'record_offset']) {
        a(!json.includes('"' + field + '":'));
    }
    a(!json.includes('authored_duplicate_alias'));
    a(!json.includes('AuthoredFoldedAlias'));
    a(Buffer.byteLength(json + '\n') <= LIMITS.output);
});

test('exact summary aggregates interleaved addresses in query order and ignores unrelated implicit names', () => {
    const left = [pub('?LeftOne@@YAXXZ', 1, 0x30), pub('?LeftTwo@@YAXXZ', 1, 0x30)];
    const right = [pub('?Right@@YAXXZ', 1, 0x34), pub('?Right@@YAXXZ', 1, 0x34)];
    const unrelated = Array.from({ length: 700 }, (_, i) => i % 2 === 0 ?
        pub('??0UWorld@@QEAA@XZ', 1, 0x20) : pub('??_7UWorld@@6B@', 2, 0x40, 0));
    const records = [right[0], ...unrelated, left[0], right[1], pub('?Nearby@@YAXXZ', 1, 0x31), left[1]];
    const r = summaryRun(records, summaryQuery({ addresses: ['0x0140001030', '0x140001032', '0x140001034'] }));
    a.equal(r.records, records.length);
    a.equal(r.symbol_stream_bytes, records.reduce((n, record) => n + record.length, 0));
    a.deepEqual(r.address_queries, ['0x140001030', '0x140001032', '0x140001034']);
    a.deepEqual(r.aggregates.map(v => [v.address, v.match_status, v.selected_pub32_record_count,
        v.selected_pub32_record_bytes, v.selected_pub32_records_sha256]), [
        ['0x140001030', 'validated_matching_pub32_records', 2, left[0].length + left[1].length, rawRecordHash(left)],
        ['0x140001032', 'no_matching_pub32_records', 0, 0, rawRecordHash([])],
        ['0x140001034', 'validated_matching_pub32_records', 2, right[0].length + right[1].length, rawRecordHash(right)],
    ]);
    a.equal(r.aggregates[0].source_mapping.segment_offset, 0x30);
    a.equal(r.aggregates[1].source_mapping, null);
    a.equal(r.aggregates[2].source_mapping.segment_offset, 0x34);
    a(!JSON.stringify(r).includes('UWorld'));
    a(!JSON.stringify(r).includes('Nearby'));
    // Per-address stream order matters, even if the count and byte count agree.
    const reversed = summaryRun([left[1], right[0], left[0]], summaryQuery({ addresses: ['0x140001030'] }));
    a.equal(reversed.aggregates[0].selected_pub32_records_sha256, rawRecordHash([...left].reverse()));
    a.notEqual(reversed.aggregates[0].selected_pub32_records_sha256, r.aggregates[0].selected_pub32_records_sha256);
});

test('exact summary zero-match entries are explicit for empty and unselected streams', () => {
    for (const records of [[], [pub('??0UWorld@@QEAA@XZ', 1, 0x20), pub('unselected_private_marker')]]) {
        const r = summaryRun(records);
        a.equal(r.records, records.length);
        a.equal(r.complete_symbol_record_framing, true);
        a.deepEqual(r.aggregates, [{ address: '0x140001030', match_status: 'no_matching_pub32_records',
            selected_pub32_record_count: 0, selected_pub32_record_bytes: 0,
            selected_pub32_records_sha256: 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
            source_mapping: null }]);
    }
});

test('exact summary validates malformed tails after over512 selected aliases and returns no partial result', () => {
    const aliases = Array.from({ length: 600 }, () => pub('authored_selected_marker', 1, 0x30));
    const unterminated = pub('unselected_private_marker');
    unterminated.fill(65, 14);
    const badPadding = pub('AB');
    badPadding[badPadding.length - 1] = 0x40;
    const extraPadding = Buffer.concat([pub('unselected_private_marker'), Buffer.alloc(4)]);
    extraPadding.writeUInt16LE(extraPadding.length - 2);
    const badExtent = pub('unselected_private_marker');
    badExtent.writeUInt16LE(65535);
    for (const [tail, message] of [[unterminated, 'PUB32 terminator'], [badPadding, 'PUB32 padding policy'],
        [extraPadding, 'PUB32 padding policy'], [badExtent, 'symbol record extent'],
        [Buffer.from([2, 0, 0x0e, 0x11]), 'PUB32 minimum size'],
        [Buffer.from([2, 0, 0]), 'symbol record/count cap']]) {
        for (const prefix of [aliases, []]) {
            a.throws(() => summaryRun([...prefix, tail]), error => {
                a.equal(error.message, message);
                a(!error.message.includes('marker'));
                a(!error.message.includes('0x140001030'));
                return true;
            });
        }
    }
});

test('exact summary validates every selected name and flags even after many valid aliases with redacted errors', () => {
    const valid = Array.from({ length: 600 }, () => pub('?Valid@@YAXXZ', 1, 0x30));
    const marker = 'authored_private_marker';
    const nonAscii = pub(marker, 1, 0x30);
    nonAscii[14] = 0x80;
    const control = pub(marker, 1, 0x30);
    control[14] = 32;
    for (const [bad, message] of [[nonAscii, 'queried public name profile unsupported'],
        [control, 'queried public name profile unsupported'],
        [pub(marker + 'A'.repeat(1024), 1, 0x30), 'queried public name profile unsupported'],
        [pub(marker, 1, 0x30, 16), 'selected PUB32 segment/flags']]) {
        a.throws(() => summaryRun([...valid, bad]), error => {
            a.equal(error.message, message);
            a(!error.message.includes(marker));
            a(!error.message.includes('0x140001030'));
            return true;
        });
    }
    // Existing exact-list validation accepts an empty name, as does summary.
    const boundary = [pub('', 1, 0x30), pub('A'.repeat(1024), 1, 0x30)];
    a.equal(summaryRun(boundary).aggregates[0].selected_pub32_record_count, 2);
    a.equal(summaryRun(boundary).aggregates[0].selected_pub32_records_sha256, rawRecordHash(boundary));
});

test('exact summary rejects unsafe code mapping after a valid selected address', () => {
    const first = pub('?First@@YAXXZ', 1, 0x30);
    for (const [record, address] of [[pub('?Data@@YAXXZ', 2, 0x40, 0), '0x140002040'],
        [pub('?Writable@@YAXXZ', 3, 0x40), '0x140003040'],
        [pub('?OutsideRaw@@YAXXZ', 1, 0x180), '0x140001180'],
        // Another segment/offset can spell the same VA but fails strict mapping.
        [pub('?CrossSection@@YAXXZ', 1, 0x1040), '0x140002040']]) {
        a.throws(() => summaryRun([first, record], summaryQuery({ addresses: ['0x140001030', address] })),
            error => error.message === 'selected PUB32 strict source mapping');
    }
    for (const flags of [0xe0000020, 0x20000020, 0x60000020]) {
        const f = fixture([first]);
        // Writable, unreadable, or resource-overlapping executable section.
        f.exe.writeUInt32LE(flags, f.table + 36);
        if (flags === 0x60000020) {
            f.exe.writeUInt32LE(0x1030, f.op + 112 + 2 * 8);
            f.exe.writeUInt32LE(1, f.op + 116 + 2 * 8);
        }
        f.streams[5] = Buffer.from(f.exe.subarray(f.table, f.table + 120));
        const packed = pack(f);
        a.throws(() => inspect(packed.reader, packed.exe, packed.identity, summaryQuery()), /strict source mapping/);
    }
});

test('exact summary rejects missing, false, conflicting and excessive options before any reader IO', () => {
    const f = pack(fixture());
    let reads = 0;
    const reader = { size: f.reader.size, read() { reads++; throw Error('unexpected fixture read'); } };
    const invalid = [
        [{ exact_summary_only: true, addresses: ['0x140001030'] }, /requires exact address mode/],
        [{ exact_addresses_only: true, exact_summary_only: true }, /requires addresses/],
        ...[false, undefined, null, 0, 1, 'true'].map(exact_summary_only =>
            [summaryQuery({ exact_summary_only }), /requires exact address mode and true/]),
        ...[false, undefined, null].map(exact_addresses_only =>
            [summaryQuery({ exact_addresses_only }), /mode must be true/]),
        ...[undefined, null, 0, 32, 512, 513].map(exact_hit_limit =>
            [summaryQuery({ exact_hit_limit }), /conflicts with exact hit limit/]),
        ...[{ class_names: ['UFoo'] }, { class_names: undefined },
            { class_methods: [{ class_name: 'UFoo', method: 'ApplyState' }] },
            { struct_ops_methods: [] }, { name_contains: ['DeltaSerialize'] }, { name_contains: undefined }]
            .map(mode => [summaryQuery(mode), /conflicts with name\/class modes/]),
        ...[[], ['0xABC'], ['0x140001030', '0x0140001030'],
            Array.from({ length: 33 }, (_, i) => '0x' + (0x140001000 + i).toString(16))]
            .map(addresses => [summaryQuery({ addresses }), /address query/]),
    ];
    for (const [query, message] of invalid) {
        a.throws(() => inspect(reader, f.exe, f.identity, query), message);
    }
    a.equal(reads, 0);
});

test('exact summary output size depends on query count and counter digits, with unchanged global caps', () => {
    const alias = pub('A'.repeat(1024), 1, 0x30);
    // Stay within this fixture packer's single directory-page envelope.
    const many = Array.from({ length: 2048 }, () => alias);
    const addresses = Array.from({ length: 32 }, (_, i) => '0x' + (0x140001030 + i).toString(16));
    const one = summaryRun([alias], summaryQuery({ addresses }));
    const all = summaryRun(many, summaryQuery({ addresses }));
    const outputBytes = r => Buffer.byteLength(JSON.stringify(r) + '\n');
    a.equal(all.aggregate_count, 32);
    a.equal(all.aggregates[0].selected_pub32_record_count, 2048);
    a.equal(all.aggregates[0].selected_pub32_record_bytes, 2048 * alias.length);
    a.equal(all.aggregates[0].selected_pub32_records_sha256, rawRecordHash(many));
    a(all.aggregates.slice(1).every(v => v.match_status === 'no_matching_pub32_records'));
    a(outputBytes(all) < outputBytes(one) + 64);
    a(outputBytes(all) <= LIMITS.output);
    a.equal(LIMITS.symbols, 256 * 1024 * 1024);
    a.equal(LIMITS.records, 2500000);
    a.equal(LIMITS.record, 65537);
    a.equal(LIMITS.hits, 32);
    a.equal(LIMITS.output, 65536);
    a.throws(() => exactRun(many.slice(0, 513), exactQuery({ exact_hit_limit: 512 })), /selected public hit cap/);
    a.throws(() => exactRun(many.slice(0, 512), exactQuery({ exact_hit_limit: 512 })), /result output cap/);
    a.throws(() => exactRun(many.slice(0, 33)), /selected public hit cap/);
});

test('default and list-exact serialized results retain their complete legacy field schema and byte order', () => {
    const name = '??0UWorld@@QEAA@XZ', record = pub(name, 1, 0x30);
    for (const exact of [false, true]) {
        const f = pack(fixture([record]));
        const r = inspect(f.reader, f.exe, f.identity, exact ? exactQuery() : undefined);
        const expected = {
            profile: 'selected_client_MSFPDB7_DBI_S_PUB32', guid_bytes: f.identity.guid_bytes, age: 1,
            image_base: '0x140000000', stream_count: 6, symbol_stream: 4, symbol_stream_bytes: record.length,
            records: 1, hit_count: 1, hit_limit: 32,
            hits: [{ id: 'SYM01', name, family: exact ? 'address_query' : 'constructor_or_destructor',
                segment: 1, segment_offset: 0x30, record_offset: 0, flags: 2, section_id: 'SEC01',
                rva: 0x1030, address: '0x140001030', file_offset: 0x230, executable: true, source_extent_bytes: 1 }],
            complete_symbol_record_framing: true, ...(exact ? { exact_addresses_only: true } : {}),
            selected_name_profile: exact ? 'exact_address_only_ascii_max1024' :
                'explicit_decorated_ascii_allowlist_max1024_not_all_class_members',
            name_contains_queries: [], address_queries: exact ? ['0x140001030'] : [],
            class_name_queries: [], class_method_queries: [], struct_ops_queries: [], unselected_names_exported: false,
            omap_absent: true, pdb_pe_section_headers_equal: true, instruction_boundaries_confirmed: false,
            class_owner_confirmed: false, older_server_equivalence: false, runtime_behavior_proven: false,
        };
        a.equal(JSON.stringify(r), JSON.stringify(expected));
    }
});
