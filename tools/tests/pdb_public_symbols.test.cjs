'use strict';
const test=require('node:test'),a=require('node:assert/strict'),{fixture,pack,pub}=require('./pdb_public_symbols_fixtures.cjs'),{inspect,msf,selector,LIMITS}=require('../pdb_public_symbols.cjs');
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
});
