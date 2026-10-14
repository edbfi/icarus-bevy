'use strict';
// Caller-supplied class_names mode over synthetic fixtures with invented names.
const test=require('node:test'),a=require('node:assert/strict'),{fixture,pack,pub}=require('./pdb_public_symbols_fixtures.cjs'),{inspect}=require('../pdb_public_symbols.cjs');
const run=(records,query)=>{const f=pack(fixture(records));return inspect(f.reader,f.exe,f.identity,query);};
const C='UExampleSave';

test('class mode selects every fixed decorated family for an invented class',()=>{
 const code=[['??0'+C+'@@QEAA@XZ','constructor_or_destructor'],['??1'+C+'@@UEAA@XZ','constructor_or_destructor'],['??_G'+C+'@@UEAAPEAXI@Z','constructor_or_destructor'],['??_E'+C+'@@UEAAPEAXI@Z','constructor_or_destructor'],
  ['??$InternalConstructor@V'+C+'@@@@YAXAEBVFObjectInitializer@@@Z','internal_constructor'],['??$InternalVTableHelperCtorCaller@V'+C+'@@@@YAPEAVUObject@@AEAVFVTableHelper@@@Z','vtable_helper_constructor_caller'],
  ...['?GetPrivateStaticClass@'+C+'@@CAPEAVUClass@@XZ','?StaticClass@'+C+'@@SAPEAVUClass@@XZ','?StaticRegisterNatives'+C+'@'+C+'@@CAXXZ','?Serialize@'+C+'@@UEAAXAEAVFArchive@@@Z','?AddReferencedObjects@'+C+'@@SAXPEAVUObject@@AEAVFReferenceCollector@@@Z','?__DefaultConstructor@'+C+'@@SAXAEBVFObjectInitializer@@@Z'].map(n=>[n,'selected_class_method'])];
 const data=[['??_7'+C+'@@6B@','primary_or_secondary_vftable'],['??_7'+C+'@@6BFExampleIface@@@','primary_or_secondary_vftable']];
 const r=run([...code.map(([n],i)=>pub(n,1,0x20+i*4)),...data.map(([n],i)=>pub(n,2,0x40+i*8,0))],{class_names:[C]});
 a.deepEqual(r.hits.map(h=>[h.name,h.family,h.class_name]),[...code,...data].map(([n,f])=>[n,f,C]));
 a.deepEqual(r.class_name_queries,[C]);a.deepEqual(r.address_queries,[]);a.equal(r.unselected_names_exported,false);a.equal(r.class_owner_confirmed,false);
 a.equal(r.selected_name_profile,'caller_class_fixed_decorated_ascii_forms_max1024_not_all_class_members');
 a(r.hits.filter(h=>h.family.includes('vftable')).every(h=>!h.executable&&h.source_extent_bytes===8));a(r.hits.filter(h=>!h.family.includes('vftable')).every(h=>h.executable&&h.source_extent_bytes===1));
});

test('class mode replaces the default allowlist; default mode carries no class fields',()=>{
 a.equal(run(undefined,{class_names:[C]}).hit_count,0);
 const d=run();a.equal(d.hit_count,3);a.deepEqual(d.class_name_queries,[]);a(d.hits.every(h=>!('class_name' in h)));a.equal(d.selected_name_profile,'explicit_decorated_ascii_allowlist_max1024_not_all_class_members');
});

test('class token matching is exact and unrelated names are not exported',()=>{
 const misses=['??0UFooBar@@QEAA@XZ','??0XUFoo@@QEAA@XZ','??0ufoo@@QEAA@XZ','??0UFoo@Outer@@QEAA@XZ','?StaticClass@UFooBar@@SAPEAVUClass@@XZ','?StaticClass@AOther@@SAPEAVUClass@@XZ','?StaticClass@UFoo@Outer@@SAPEAVUClass@@XZ',
  '?Tick@UFoo@@UEAAXM@Z','?StaticRegisterNativesUFooBar@UFoo@@CAXXZ','?StaticRegisterNativesUFoo@UFooBar@@CAXXZ','??$InternalConstructor@UUFoo@@@@YAXXZ','??$InternalConstructor@VUFooBar@@@@YAXXZ','??$InternalVTableHelperCtorCaller@VUFo@@@@YAXXZ',
  'UFoo_private_marker','?private_UFoo@@3HA','?UFoo@@private_lookalike','??0UFoo'];
 const vmiss=['??_7UFoo@@6BOther@@','??_7UFoo@@6BOther@@@X','??_7UFoo@@6BOther@Ns@@@','??_7UFoo@@6B1Other@@@','??_7UFooBar@@6B@','??_7UFoo@@6B@@'];
 const r=run([...misses.map(n=>pub(n)),...vmiss.map(n=>pub(n,2,0x40,0)),pub('??0UFoo@@QEAA@XZ')],{class_names:['UFoo']});
 a.deepEqual(r.hits.map(h=>[h.name,h.class_name]),[['??0UFoo@@QEAA@XZ','UFoo']]);const json=JSON.stringify(r);for(const n of [...misses,...vmiss])a(!json.includes(JSON.stringify(n)),n);
 const two=run([pub('??0UFooBar@@QEAA@XZ'),pub('??0UFoo@@QEAA@XZ',1,0x24)],{class_names:['UFoo','UFooBar']});
 a.deepEqual(two.hits.map(h=>[h.name,h.class_name]),[['??0UFooBar@@QEAA@XZ','UFooBar'],['??0UFoo@@QEAA@XZ','UFoo']]);a.deepEqual(two.class_name_queries,['UFoo','UFooBar']);
});

test('class mode combines with exact address queries',()=>{
 const r=run([pub('?Helper@@YAXXZ',1,0x30),pub('??0AFooActor@@QEAA@XZ',1,0x20),pub('?Other@@YAXXZ',1,0x34)],{addresses:['0x140001030','0x140001020'],class_names:['AFooActor']});
 a.deepEqual(r.hits.map(h=>[h.name,h.family,h.class_name,h.address]),[['?Helper@@YAXXZ','address_query',undefined,'0x140001030'],['??0AFooActor@@QEAA@XZ','constructor_or_destructor','AFooActor','0x140001020']]);
 a(!('class_name' in r.hits[0]));a.deepEqual(r.address_queries,['0x140001030','0x140001020']);a.deepEqual(r.class_name_queries,['AFooActor']);
});

test('class query validation fails closed',()=>{
 for(const [query,msg]of [[{class_names:['U-Foo']},/class query format/],[{class_names:['1Foo']},/class query format/],[{class_names:['']},/class query format/],[{class_names:['A'.repeat(97)]},/class query format/],[{class_names:[7]},/class query format/],
  [{class_names:['A','B','C','D','E']},/class query shape/],[{class_names:[]},/class query shape/],[{class_names:'UFoo'},/class query shape/],[{class_names:['UFoo','UFoo']},/duplicate class query/],
  [{class_names:['UFoo'],extra:1},/query shape/],[{},/query shape/],[null,/query shape/],[[],/query shape/],[{class_names:['UFoo'],addresses:[]},/address query shape/],[{class_names:['UFoo'],addresses:['0x1','0x1']},/duplicate address query/]])
  a.throws(()=>run(undefined,query),msg,JSON.stringify(query));
 a.equal(run([pub('??0'+'A'.repeat(96)+'@@QEAA@XZ')],{class_names:['A'.repeat(96),'_b','C1','D_2']}).hit_count,1);
});

test('class mode shares the exact 32-hit cap and 33 fails closed',()=>{
 const ctor=i=>pub('??0UFoo@@QEAA@XZ',1,0x20+i);
 a.equal(run(Array.from({length:32},(_,i)=>ctor(i)),{class_names:['UFoo']}).hit_count,32);
 a.throws(()=>run(Array.from({length:33},(_,i)=>ctor(i)),{class_names:['UFoo']}),/selected public hit cap/);
 const mixed=[...Array.from({length:31},(_,i)=>ctor(i)),pub('?Helper@@YAXXZ',1,0x100),pub('?Other@@YAXXZ',1,0x104)];
 a.equal(run(mixed.slice(0,32),{class_names:['UFoo'],addresses:['0x140001100']}).hit_count,32);
 a.throws(()=>run(mixed,{class_names:['UFoo'],addresses:['0x140001100','0x140001104']}),/selected public hit cap/);
});

test('class mode keeps strict source mapping per family',()=>{
 for(const p of [pub('??_7UFoo@@6B@',1,0x40,0),pub('??_7UFoo@@6BFIface@@@',1,0x40,0),pub('??_7UFoo@@6B@',2,0x17c,0),pub('?Serialize@UFoo@@UEAAXAEAVFArchive@@@Z',2,0x40),pub('??0UFoo@@QEAA@XZ',3,0x40),pub('??$InternalConstructor@VUFoo@@@@YAXXZ',2,0x40)])
  a.throws(()=>run([p],{class_names:['UFoo']}),/strict source mapping/);
});

test('class-selected names keep the ASCII/1024-byte profile and long prefixes',()=>{
 a.throws(()=>run([pub('??0UFoo@@'+'A'.repeat(1100))],{class_names:['UFoo']}),/selected public name profile/);
 const odd=pub('??0UFoo@@QEAA@XZ');odd[24]=0x80;a.throws(()=>run([odd],{class_names:['UFoo']}),/selected public name profile/);
 const max=pub('??0UFoo@@'+'A'.repeat(1015)),long='U'+'x'.repeat(95),helper='??$InternalVTableHelperCtorCaller@V'+long+'@@@@YAXXZ';
 const r=run([...Array.from({length:31},()=>max),pub(helper)],{class_names:['UFoo',long]});
 a.equal(r.hit_count,32);a.equal(r.hits[31].family,'vtable_helper_constructor_caller');a.equal(r.hits[31].class_name,long);a.equal(r.hits[0].name.length,1024);
});
