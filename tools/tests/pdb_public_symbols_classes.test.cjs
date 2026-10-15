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

const M=(class_name,method)=>({class_name,method});
test('class_methods select exact ?method@class@@ pairs with class and method fields',()=>{
 const hit='?ApplyState@UFoo@@QEAAXH@Z',misses=['?ApplyState@UFooX@@QEAAXH@Z','?ApplyState@XUFoo@@QEAAXH@Z','?ApplyStateX@UFoo@@QEAAXH@Z','?XApplyState@UFoo@@QEAAXH@Z','?ApplyState@UFoo@Outer@@QEAAXH@Z','?applystate@UFoo@@QEAAXH@Z','?ApplyState@@YAXXZ','?Serialize@UFoo@@UEAAXAEAVFArchive@@@Z'];
 const r=run([...misses.map(n=>pub(n)),pub(hit,1,0x30)],{class_methods:[M('UFoo','ApplyState')]});
 a.deepEqual(r.hits.map(h=>[h.name,h.family,h.class_name,h.method,h.executable]),[[hit,'selected_class_method','UFoo','ApplyState',true]]);
 a.deepEqual(r.class_method_queries,[M('UFoo','ApplyState')]);a.deepEqual(r.class_name_queries,[]);a.equal(r.selected_name_profile,'caller_class_fixed_decorated_ascii_forms_max1024_not_all_class_members');
 const json=JSON.stringify(r);for(const n of misses)a(!json.includes(JSON.stringify(n)),n);
 a.equal(run(undefined,{class_methods:[M('UFoo','ApplyState')]}).hit_count,0);a.deepEqual(run().class_method_queries,[]);
});

test('class_methods do not select operator or special decorated names',()=>{
 const special=['??0UFoo@@QEAA@XZ','??1UFoo@@UEAA@XZ','??_GUFoo@@UEAAPEAXI@Z','??_EUFoo@@UEAAPEAXI@Z','??$InternalConstructor@VUFoo@@@@YAXXZ','??$InternalVTableHelperCtorCaller@VUFoo@@@@YAXXZ','??4UFoo@@QEAAAEAV0@AEBV0@@Z','??_GUFoo@UFoo@@QEAA@XZ'];
 const r=run([...special.map(n=>pub(n)),pub('??_7UFoo@@6B@',2,0x40,0)],{class_methods:[M('UFoo','_G'),M('UFoo','UFoo'),M('UFoo','_7'),M('UFoo','InternalConstructor'),M('UFoo','_E')]});
 a.equal(r.hit_count,0);
});

test('class_methods OR with class_names and addresses; method pairs win overlaps',()=>{
 const r=run([pub('?Helper@@YAXXZ',1,0x30),pub('??0AFooActor@@QEAA@XZ',1,0x20),pub('?Serialize@AFooActor@@UEAAXAEAVFArchive@@@Z',1,0x24),pub('?Tick@UBarThing@@UEAAXM@Z',1,0x28),pub('?Tick@UBarOther@@UEAAXM@Z',1,0x2c)],
  {addresses:['0x140001030'],class_names:['AFooActor'],class_methods:[M('UBarThing','Tick'),M('AFooActor','Serialize')]});
 a.deepEqual(r.hits.map(h=>[h.family,h.class_name,h.method]),[['address_query',undefined,undefined],['constructor_or_destructor','AFooActor',undefined],['selected_class_method','AFooActor','Serialize'],['selected_class_method','UBarThing','Tick']]);
 a(!('method' in r.hits[1])&&!('class_name' in r.hits[0]));a.deepEqual(r.class_name_queries,['AFooActor']);a.deepEqual(r.class_method_queries,[M('UBarThing','Tick'),M('AFooActor','Serialize')]);a.deepEqual(r.address_queries,['0x140001030']);
});

test('class method query validation fails closed',()=>{
 const sixteen=Array.from({length:16},(_,i)=>M('UFoo','Method'+i));
 for(const [query,msg]of [[{class_methods:[]},/class method query shape/],[{class_methods:[...sixteen,M('UFoo','Extra')]},/class method query shape/],[{class_methods:M('UFoo','Tick')},/class method query shape/],
  [{class_methods:[null]},/class method query shape/],[{class_methods:[['UFoo','Tick']]},/class method query shape/],[{class_methods:[{class_name:'UFoo'}]},/class method query shape/],[{class_methods:[{...M('UFoo','Tick'),extra:1}]},/class method query shape/],
  [{class_methods:[Object.assign(Object.create({method:'Tick'}),{class_name:'UFoo',other:1})]},/class method query shape/],
  [{class_methods:[M('U-Foo','Tick')]},/class method query format/],[{class_methods:[M('UFoo','1Tick')]},/class method query format/],[{class_methods:[M('UFoo','')]},/class method query format/],[{class_methods:[M('UFoo','T'.repeat(97))]},/class method query format/],[{class_methods:[M('UFoo',7)]},/class method query format/],
  [{class_methods:[M('UFoo','Tick'),M('UFoo','Tick')]},/duplicate class method query/],[{class_methods:[M('UFoo','Tick')],extra:1},/query shape/],[{class_methods:[M('UFoo','Tick')],class_names:[]},/class query shape/],[{class_methods:[M('UFoo','Tick')],addresses:[]},/address query shape/]])
  a.throws(()=>run(undefined,query),msg,JSON.stringify(query));
 a.equal(run([pub('?Method15@UFoo@@QEAAXXZ'),pub('?'+'T'.repeat(96)+'@'+'C'.repeat(96)+'@@QEAAXXZ',1,0x24)],{class_methods:[...sixteen.slice(1),M('C'.repeat(96),'T'.repeat(96))]}).hit_count,2);
});

test('class_methods share the 32-hit cap and keep code-only strict mapping',()=>{
 const q={class_methods:[M('UFoo','Tick')]},tick=i=>pub('?Tick@UFoo@@UEAAXM@Z',1,0x20+i);
 a.equal(run(Array.from({length:32},(_,i)=>tick(i)),q).hit_count,32);
 a.throws(()=>run(Array.from({length:33},(_,i)=>tick(i)),q),/selected public hit cap/);
 a.throws(()=>run([...Array.from({length:31},(_,i)=>tick(i)),pub('??0UFoo@@QEAA@XZ',1,0x60),pub('?Helper@@YAXXZ',1,0x100)],{...q,class_names:['UFoo'],addresses:['0x140001100']}),/selected public hit cap/);
 for(const p of [pub('?Tick@UFoo@@UEAAXM@Z',2,0x40),pub('?Tick@UFoo@@UEAAXM@Z',3,0x40,0),pub('?Tick@UFoo@@UEAAXM@Z',1,0x180)])a.throws(()=>run([p],q),/strict source mapping/);
 a.throws(()=>run([pub('?Tick@UFoo@@'+'A'.repeat(1100))],q),/selected public name profile/);
});

const S=(struct_name,method)=>({struct_name,method});
test('struct_ops_methods select exact TCppStructOps instance methods',()=>{
 const ops=s=>'?NetSerialize@?$TCppStructOps@U'+s+'@@@UScriptStruct@@UEAA_NAEAVFArchive@@PEAVUPackageMap@@AEA_N@Z',hit=ops('FExampleMove');
 const misses=[ops('FExampleMoveX'),ops('XFExampleMove'),'?NetSerializeX@?$TCppStructOps@UFExampleMove@@@UScriptStruct@@UEAA_NXZ','?NetSerialize@?$TCppStructOps@VFExampleMove@@@UScriptStruct@@UEAA_NXZ',
  '?NetSerialize@?$TCppStructOps@UFExampleMove@@@UOther@@UEAA_NXZ','?NetSerialize@?$TOtherOps@UFExampleMove@@@UScriptStruct@@UEAA_NXZ','?NetSerialize@FExampleMove@@QEAA_NXZ','?Copy@?$TCppStructOps@UFExampleMove@@@UScriptStruct@@UEAA_NXZ'];
 const r=run([...misses.map(n=>pub(n)),pub(hit,1,0x30)],{struct_ops_methods:[S('FExampleMove','NetSerialize')]});
 a.deepEqual(r.hits.map(h=>[h.name,h.family,h.struct_name,h.method,h.executable]),[[hit,'selected_struct_ops_method','FExampleMove','NetSerialize',true]]);
 a.deepEqual(r.struct_ops_queries,[S('FExampleMove','NetSerialize')]);a.deepEqual(r.class_method_queries,[]);a.deepEqual(r.class_name_queries,[]);
 const json=JSON.stringify(r);for(const n of misses)a(!json.includes(JSON.stringify(n)),n);
 a.deepEqual(run().struct_ops_queries,[]);
 const both=run([pub(hit,1,0x30),pub('?Tick@UFoo@@UEAAXM@Z',1,0x34)],{struct_ops_methods:[S('FExampleMove','NetSerialize')],class_methods:[M('UFoo','Tick')]});
 a.deepEqual(both.hits.map(h=>h.family),['selected_struct_ops_method','selected_class_method']);
});

test('struct ops query validation fails closed and keeps code-only mapping',()=>{
 const eight=Array.from({length:8},(_,i)=>S('FMove'+i,'NetSerialize'));
 for(const [query,msg]of [[{struct_ops_methods:[]},/struct ops query shape/],[{struct_ops_methods:[...eight,S('FMore','NetSerialize')]},/struct ops query shape/],[{struct_ops_methods:S('FMove','NetSerialize')},/struct ops query shape/],
  [{struct_ops_methods:[null]},/struct ops query shape/],[{struct_ops_methods:[{struct_name:'FMove'}]},/struct ops query shape/],[{struct_ops_methods:[{...S('FMove','NetSerialize'),extra:1}]},/struct ops query shape/],
  [{struct_ops_methods:[S('F-Move','NetSerialize')]},/struct ops query format/],[{struct_ops_methods:[S('FMove','')]},/struct ops query format/],[{struct_ops_methods:[S('FMove@','NetSerialize')]},/struct ops query format/],
  [{struct_ops_methods:[S('FMove','NetSerialize'),S('FMove','NetSerialize')]},/duplicate struct ops query/],[{struct_ops_methods:[S('FMove','NetSerialize')],extra:1},/query shape/]])
  a.throws(()=>run(undefined,query),msg,JSON.stringify(query));
 a.equal(run(undefined,{struct_ops_methods:eight}).hit_count,0);
 const name='?NetSerialize@?$TCppStructOps@UFMove@@@UScriptStruct@@UEAA_NXZ',q={struct_ops_methods:[S('FMove','NetSerialize')]};
 for(const p of [pub(name,2,0x40),pub(name,3,0x40,0)])a.throws(()=>run([p],q),/strict source mapping/);
 a.equal(run(Array.from({length:32},(_,i)=>pub(name,1,0x20+i)),q).hit_count,32);
 a.throws(()=>run(Array.from({length:33},(_,i)=>pub(name,1,0x20+i)),q),/selected public hit cap/);
});

test('name_contains selects decorated names holding a literal substring in code or read-only data',()=>{
 const hitCode='??$DeltaSerializeExample@UFExampleItem@@UFExampleArray@@@FExampleSerializer@@SA_NXZ',hitData='??_7?$TExampleOps@UFExampleArray@@@@6B@';
 const misses=['?Other@@YAXXZ','DeltaSerializeExample_undecorated','?DeltaSerializeExampl@@YAXXZ','?deltaserializeexample@@YAXXZ'];
 const r=run([...misses.map(n=>pub(n)),pub(hitCode,1,0x30),pub(hitData,2,0x40,0)],{name_contains:['DeltaSerializeExample','TExampleOps@UFExampleArray']});
 a.deepEqual(r.hits.map(h=>[h.name,h.family,h.substring,h.executable]),[[hitCode,'name_substring','DeltaSerializeExample',true],[hitData,'name_substring','TExampleOps@UFExampleArray',false]]);
 a.deepEqual(r.name_contains_queries,['DeltaSerializeExample','TExampleOps@UFExampleArray']);a.equal(r.selected_name_profile,'caller_name_substrings_decorated_ascii_max1024');
 const json=JSON.stringify(r);for(const n of misses)a(!json.includes(JSON.stringify(n)),n);
 // Substring mode replaces the default allowlist; default mode reports no substrings.
 a.equal(run(undefined,{name_contains:['NothingMatchesThis']}).hit_count,0);a.deepEqual(run().name_contains_queries,[]);
 // Exact class methods take precedence and combine with substrings.
 const both=run([pub('?Tick@UFoo@@UEAAXM@Z',1,0x34),pub(hitCode,1,0x30)],{name_contains:['UEAAXM@Z_none','DeltaSerializeExample'],class_methods:[{class_name:'UFoo',method:'Tick'}]});
 a.deepEqual(both.hits.map(h=>h.family),['selected_class_method','name_substring']);
});

test('name_contains validation fails closed and keeps writable data and the hit cap strict',()=>{
 for(const [query,msg]of [[{name_contains:[]},/name substring query shape/],[{name_contains:['AAAAAAAA','BBBBBBBB','CCCCCCCC','DDDDDDDD','EEEEEEEE']},/name substring query shape/],[{name_contains:'DeltaSerialize'},/name substring query shape/],
  [{name_contains:['short']},/name substring query format/],[{name_contains:['A'.repeat(97)]},/name substring query format/],[{name_contains:['has space here']},/name substring query format/],[{name_contains:[7]},/name substring query format/],
  [{name_contains:['DeltaSerialize','DeltaSerialize']},/duplicate name substring query/]])
  a.throws(()=>run(undefined,query),msg,JSON.stringify(query));
 const name='?DeltaSerializeItem@@YAXXZ',q={name_contains:['DeltaSerializeItem']};
 a.throws(()=>run([pub(name,3,0x40,0)],q),/strict source mapping/);
 a.throws(()=>run([pub(name+'A'.repeat(1100))],q),/selected public name profile/);
 a.equal(run(Array.from({length:32},(_,i)=>pub(name,1,0x20+i)),q).hit_count,32);
 a.throws(()=>run(Array.from({length:33},(_,i)=>pub(name,1,0x20+i)),q),/selected public hit cap/);
});
