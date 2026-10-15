'use strict';
// Pure bounded MSF7/DBI/PUB32 reader. No filesystem, engine or target launch.
// Independently authored from the LLVM MSF/DBI/CodeView layout documentation:
// https://github.com/llvm/llvm-project/tree/3623fe661ae35c6c80ac221f14d85be76aa870f1/llvm/docs/PDB
// RawTypes.h, SymbolRecord.h, CodeViewSymbols.def, CodeView.h and their mapping
// sources at the same revision describe the selected layout. Upstream source
// uses Apache-2.0 WITH LLVM-exception; no upstream implementation is copied.
// Preserved source/docs/license provenance: ignored selected-client-symbol case.
// This helper validates a strict bounded profile, not all Microsoft PDBs.
// Callers must authorize and bind inputs before supplying a reader/EXE; PIN
// records the qualified reference pair but inspect itself does not hash files.
// Public symbol associations do not prove instruction boundaries, ownership,
// older-server equivalence or runtime behavior.
// Optional caller-supplied class_names replace the default allowlist with fixed
// decorated MSVC forms for up to 4 exact class tokens, and class_methods select
// ?<method>@<class>@@ names for up to 16 exact pairs; such names may be private
// and are returned only to the caller.
const {parse}=require('./pe_candidates.cjs');
const LIMITS=Object.freeze({pdb:1280*1024*1024,exe:128*1024*1024,directory:4*1024*1024,streams:4096,symbols:256*1024*1024,records:2500000,record:65537,hits:32,output:65536,sections:96});
const PIN=Object.freeze({exe_sha256:'ab11ec1212127e9047128843e584ab636632353edd6808a0db9caedb35048179',pdb_sha256:'944e38c941cdbdf4ec512f4307134fdd307e5e1c61602f06bd2f9600f7f98bef',exe_bytes:109356544,pdb_bytes:1177653248,guid_bytes:'f18774cb0f78a849bf3055a023b53562',age:1});
const check=(ok,msg)=>{if(!ok)throw Error(msg);};
function selector(name){
 if(/^\?\?(?:0|1|_G|_E)(?:UWorld|UPendingNetGame)@@[A-Za-z0-9_?$@]+$/.test(name))return 'constructor_or_destructor';
 if(/^\?\?_7(?:UWorld|UPendingNetGame)@@6B(?:@|UObject@@@|FNetworkNotify@@@)$/.test(name))return 'primary_or_FNetworkNotify_secondary_vftable';
 if(name==='??_7FNetworkNotify@@6B@')return 'FNetworkNotify_primary_vftable';
 if(/^\?(?:NotifyControlMessage|GetPrivateStaticClass|StaticClass)@(?:UWorld|UPendingNetGame|FNetworkNotify)@@[A-Za-z0-9_?$@]+$/.test(name))return 'selected_method';
 return null;
}
function msf(reader){
 check(reader&&Number.isSafeInteger(reader.size)&&reader.size>=56&&reader.size<=LIMITS.pdb&&typeof reader.read==='function','PDB size/reader cap');
 const read=(o,n)=>{check(Number.isSafeInteger(o)&&Number.isSafeInteger(n)&&o>=0&&n>=0&&n<=LIMITS.directory&&o+n<=reader.size,'PDB read range');const b=reader.read(o,n);check(Buffer.isBuffer(b)&&b.length===n,'PDB short read');return b;};
 const h=read(0,56);check(h.subarray(0,32).equals(Buffer.from('Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0','binary')),'MSF7 magic');
 const bs=h.readUInt32LE(32),fpm=h.readUInt32LE(36),blocks=h.readUInt32LE(40),ds=h.readUInt32LE(44),blockmap=h.readUInt32LE(52);
 check(bs===4096&&blocks*bs===reader.size&&(fpm===1||fpm===2)&&h.readUInt32LE(48)===0,'MSF7 strict superblock');
 check(ds>=4&&ds<=LIMITS.directory,'directory cap');const pages=Math.ceil(ds/bs);check(pages*4<=bs,'single directory block map policy');
 const reserved=new Set([0,blockmap]);const dataPage=p=>{check(Number.isSafeInteger(p)&&p>2&&p<blocks&&p%bs!==1&&p%bs!==2,'MSF data block bounds');};dataPage(blockmap);
 const map=read(blockmap*bs,pages*4),dir=Buffer.alloc(ds);
 for(let j=0;j<pages;j++){const p=map.readUInt32LE(j*4);dataPage(p);check(!reserved.has(p),'directory block duplicate');reserved.add(p);read(p*bs,Math.min(bs,ds-j*bs)).copy(dir,j*bs);}
 const count=dir.readUInt32LE();check(count>=4&&count<=LIMITS.streams&&4+count*4<=dir.length,'stream count/table bound');const streams=[];let at=4+count*4;
 for(let i=0;i<count;i++){const n=dir.readUInt32LE(4+i*4);check(n===0xffffffff||n<=reader.size,'stream size bound');const ps=[],np=n===0xffffffff?0:Math.ceil(n/bs);check(at+np*4<=dir.length,'stream page table bound');for(let j=0;j<np;j++){const p=dir.readUInt32LE(at);at+=4;dataPage(p);check(!reserved.has(p),'stream block overlap');reserved.add(p);ps.push(p);}streams.push({size:n,pages:ps});}
 check(at===dir.length,'directory exact extent');
 const stream=(i,o,n)=>{check(Number.isInteger(i)&&i>=0&&i<count,'stream index');const s=streams[i];check(s.size!==0xffffffff&&Number.isSafeInteger(o)&&Number.isSafeInteger(n)&&o>=0&&n>=0&&n<=LIMITS.directory&&o+n<=s.size,'stream extent');const out=Buffer.alloc(n);let done=0;while(done<n){const logical=o+done,page=Math.floor(logical/bs),off=logical%bs,take=Math.min(n-done,bs-off);read(s.pages[page]*bs+off,take).copy(out,done);done+=take;}return out;};
 return {streams,count,stream};
}
// Optional address queries export only public names at up to 32 exact VAs.
function addressQueries(query){
 if(query===undefined||query.addresses===undefined)return null;
 check(Array.isArray(query.addresses)&&query.addresses.length>=1&&query.addresses.length<=LIMITS.hits,'address query shape');
 const set=new Set();for(const a of query.addresses){check(typeof a==='string'&&/^0x[0-9a-f]{1,16}$/.test(a),'address query format');const v=BigInt(a);check(!set.has(v),'duplicate address query');set.add(v);}return set;
}
const CLASS_ID=/^[A-Za-z_][A-Za-z0-9_]{0,95}$/,QUALIFIER='[A-Za-z_][A-Za-z0-9_]{0,95}',esc=t=>t.replace(/[?$]/g,'\\$&');
const CLASS_FORMS=Object.freeze([['constructor_or_destructor',c=>['??0'+c+'@@','??1'+c+'@@','??_G'+c+'@@','??_E'+c+'@@']],['primary_or_secondary_vftable',c=>['??_7'+c+'@@6B']],['internal_constructor',c=>['??$InternalConstructor@V'+c+'@@@@']],['vtable_helper_constructor_caller',c=>['??$InternalVTableHelperCtorCaller@V'+c+'@@@@']],['selected_class_method',c=>['GetPrivateStaticClass','StaticClass','StaticRegisterNatives'+c,'Serialize','AddReferencedObjects','__DefaultConstructor'].map(m=>'?'+m+'@'+c+'@@')]]);
// Optional class queries select fixed decorated forms for 1..4 exact class tokens
// and ?<method>@<class>@@ for 1..16 exact pairs; method pairs are tried first.
// struct_ops_methods select ?<method>@?$TCppStructOps@U<struct>@@@UScriptStruct@@
// for 1..8 exact pairs, the template instance an inline struct method lives in.
function classQueries(query){
 if(query===undefined||(query.class_names===undefined&&query.class_methods===undefined&&query.struct_ops_methods===undefined))return null;
 const forms=[],names=[],methods=[],structOps=[],add=(prefix,f)=>forms.push({prefix,...f,vftable:f.family.includes('vftable')?new RegExp('^'+esc(prefix)+'(?:@|'+QUALIFIER+'@@@)$'):null});
 if(query.class_methods!==undefined){
  check(Array.isArray(query.class_methods)&&query.class_methods.length>=1&&query.class_methods.length<=16,'class method query shape');const seen=new Set();
  for(const q of query.class_methods){check(q&&typeof q==='object'&&!Array.isArray(q)&&Object.keys(q).length===2&&Object.hasOwn(q,'class_name')&&Object.hasOwn(q,'method'),'class method query shape');check(typeof q.class_name==='string'&&CLASS_ID.test(q.class_name)&&typeof q.method==='string'&&CLASS_ID.test(q.method),'class method query format');const prefix='?'+q.method+'@'+q.class_name+'@@';check(!seen.has(prefix),'duplicate class method query');seen.add(prefix);methods.push({class_name:q.class_name,method:q.method});add(prefix,{family:'selected_class_method',extra:{class_name:q.class_name,method:q.method}});}
 }
 if(query.struct_ops_methods!==undefined){
  check(Array.isArray(query.struct_ops_methods)&&query.struct_ops_methods.length>=1&&query.struct_ops_methods.length<=8,'struct ops query shape');const seen=new Set();
  for(const q of query.struct_ops_methods){check(q&&typeof q==='object'&&!Array.isArray(q)&&Object.keys(q).length===2&&Object.hasOwn(q,'struct_name')&&Object.hasOwn(q,'method'),'struct ops query shape');check(typeof q.struct_name==='string'&&CLASS_ID.test(q.struct_name)&&typeof q.method==='string'&&CLASS_ID.test(q.method),'struct ops query format');const prefix='?'+q.method+'@?$TCppStructOps@U'+q.struct_name+'@@@UScriptStruct@@';check(!seen.has(prefix),'duplicate struct ops query');seen.add(prefix);structOps.push({struct_name:q.struct_name,method:q.method});add(prefix,{family:'selected_struct_ops_method',extra:{struct_name:q.struct_name,method:q.method}});}
 }
 if(query.class_names!==undefined){
  check(Array.isArray(query.class_names)&&query.class_names.length>=1&&query.class_names.length<=4,'class query shape');
  const seen=new Set();for(const c of query.class_names){check(typeof c==='string'&&CLASS_ID.test(c),'class query format');check(!seen.has(c),'duplicate class query');seen.add(c);}
  for(const c of seen){names.push(c);for(const [family,make]of CLASS_FORMS)for(const prefix of make(c))add(prefix,{family,extra:{class_name:c}});}
 }
 const stem=new RegExp('^(?:'+forms.map(f=>esc(f.prefix)).join('|')+')'),stemBytes=Math.max(...forms.map(f=>f.prefix.length));
 const select=name=>{for(const f of forms)if(f.vftable?f.vftable.test(name):name.startsWith(f.prefix))return {family:f.family,extra:f.extra};return null;};
 return {names,methods,structOps,stem,stemBytes,select};
}
const DEFAULT_STEM=/^\?\?(?:0|1|_G|_E)(?:UWorld|UPendingNetGame)@@|^\?\?_7(?:UWorld|UPendingNetGame|FNetworkNotify)@@|^\?(?:NotifyControlMessage|GetPrivateStaticClass|StaticClass)@(?:UWorld|UPendingNetGame|FNetworkNotify)@@/;
function selection(query){
 check(query===undefined||(query&&typeof query==='object'&&!Array.isArray(query)&&Object.keys(query).length>=1&&Object.keys(query).every(k=>k==='addresses'||k==='class_names'||k==='class_methods'||k==='struct_ops_methods')),'address query or class query shape');
 const queries=addressQueries(query),classes=classQueries(query);
 return {queries,classes,stem:classes?classes.stem:DEFAULT_STEM,stemBytes:classes?classes.stemBytes:96,select:classes?classes.select:name=>{const family=selector(name);return family?{family}:null;}};
}
function inspect(reader,exe,identity,query){
 const sel=selection(query),queries=sel.queries;
 check(Buffer.isBuffer(exe)&&exe.length>=64&&exe.length<=LIMITS.exe,'EXE input cap');check(identity&&/^[0-9a-f]{32}$/.test(identity.guid_bytes)&&Number.isInteger(identity.age)&&identity.age>0,'explicit debug identity');
 const image=parse(exe),m=msf(reader);check(m.streams[1].size>=28&&m.streams[1].size<=65536,'PDB info stream bound');const info=m.stream(1,0,28);check(info.readUInt32LE()===20000404&&info.readUInt32LE(8)===identity.age&&info.subarray(12,28).toString('hex')===identity.guid_bytes,'PDB info identity mismatch');
 check(m.streams[3].size>=64,'DBI header size');const dbi=m.stream(3,0,64);check(dbi.readInt32LE()===-1&&dbi.readUInt32LE(4)===19990903&&dbi.readUInt32LE(8)===identity.age&&dbi.readUInt16LE(58)===0x8664&&dbi.readUInt32LE(60)===0,'DBI profile/age/machine');
 const index=dbi.readUInt16LE(20),optional=dbi.readInt32LE(48),sizes=[24,28,32,36,40,52].map(p=>dbi.readInt32LE(p));check(sizes.every(n=>n>=0)&&optional>=12&&optional<=22&&optional%2===0,'DBI substream size policy');const optAt=64+sizes.reduce((n,v)=>n+v,0);check(optAt+optional===m.streams[3].size,'DBI substream exact extent');const dbg=m.stream(3,optAt,optional);check(dbg.readUInt16LE(6)===65535&&dbg.readUInt16LE(8)===65535,'OMAP unsupported');
 const pe=exe.readUInt32LE(60),table=pe+24+exe.readUInt16LE(pe+20),count=image.sections.length,sectionIndex=dbg.readUInt16LE(10);check(sectionIndex<m.count&&m.streams[sectionIndex].size===count*40,'PDB section header stream size');const sections=m.stream(sectionIndex,0,count*40);check(sections.equals(exe.subarray(table,table+count*40)),'PDB/PE section headers mismatch');
 // CodeView path bytes remain opaque; only RSDS identity fields are inspected.
 const op=pe+24,nd=exe.readUInt32LE(op+108);check(nd>6,'PE debug directory absent');const drva=exe.readUInt32LE(op+112+6*8),dbytes=exe.readUInt32LE(op+116+6*8);check(dbytes>0&&dbytes<=4096&&dbytes%28===0,'PE debug directory cap');const mapped=(rva,n)=>{const matches=image.sections.filter(s=>rva>=s.rva&&rva+n<=s.rva+s.mappedBytes);check(matches.length===1,'PE file-backed mapping');return {s:matches[0],offset:matches[0].raw+rva-matches[0].rva};};const dd=mapped(drva,dbytes);let cv=0;
 for(let p=dd.offset;p<dd.offset+dbytes;p+=28){if(exe.readUInt32LE(p+12)!==2)continue;cv++;const n=exe.readUInt32LE(p+16),rva=exe.readUInt32LE(p+20),raw=exe.readUInt32LE(p+24);check(n>=25&&n<=4096,'CodeView size cap');const loc=mapped(rva,n);check(loc.offset===raw&&exe.subarray(raw,raw+4).equals(Buffer.from('RSDS'))&&exe.subarray(raw+4,raw+20).toString('hex')===identity.guid_bytes&&exe.readUInt32LE(raw+20)===identity.age&&exe[raw+n-1]===0,'EXE CodeView identity mismatch');}check(cv===1,'exact one CodeView identity');
 check(index<m.count&&m.streams[index].size!==0xffffffff&&m.streams[index].size<=LIMITS.symbols,'symbol stream cap');const total=m.streams[index].size,hits=[];let at=0,records=0,cacheAt=-1,cache=null;const symbolRead=(o,n)=>{check(Number.isSafeInteger(o)&&Number.isSafeInteger(n)&&o>=0&&n>=0&&n<=LIMITS.record&&o+n<=total,'symbol read range');const out=Buffer.alloc(n);let done=0;while(done<n){const pos=o+done,base=Math.floor(pos/65536)*65536;if(base!==cacheAt){cache=m.stream(index,base,Math.min(65536,total-base));cacheAt=base;}const local=pos-base,take=Math.min(n-done,cache.length-local);cache.subarray(local,local+take).copy(out,done);done+=take;}return out;};
 while(at<total){check(at+4<=total&&records<LIMITS.records,'symbol record/count cap');const header=symbolRead(at,4),len=header.readUInt16LE(),kind=header.readUInt16LE(2),bytes=len+2;check(len>=2&&bytes<=LIMITS.record&&bytes%4===0&&at+bytes<=total,'symbol record extent');records++;
  if(kind===0x110e){check(bytes>=15,'PUB32 minimum size');const record=symbolRead(at,bytes),nul=record.indexOf(0,14);check(nul>=14&&nul<bytes,'PUB32 terminator');const padding=record.subarray(nul+1);check(padding.length<=3&&(padding.every(v=>v===0)||padding.every((v,i)=>v===0xf0+padding.length-i)),'PUB32 padding policy');const nameBytes=record.subarray(14,nul);const selectedStem=nameBytes[0]===63&&sel.stem.test(nameBytes.subarray(0,sel.stemBytes).toString('latin1'));let family=null,name=null,extra=null;if(selectedStem){check(nameBytes.length<=1024&&nameBytes.every(byte=>byte>=33&&byte<=126),'selected public name profile unsupported');name=nameBytes.toString('ascii');const picked=sel.select(name);if(picked){family=picked.family;extra=picked.extra;}}if(!family&&queries){const qs=record.readUInt16LE(12),qo=record.readUInt32LE(8);if(qs>0&&qs<=count&&queries.has(image.base+BigInt(image.sections[qs-1].rva)+BigInt(qo))){check(nameBytes.length<=1024&&nameBytes.every(byte=>byte>=33&&byte<=126),'queried public name profile unsupported');name=nameBytes.toString('ascii');family='address_query';}}if(family){check(hits.length<LIMITS.hits,'selected public hit cap');const segment=record.readUInt16LE(12),offset=record.readUInt32LE(8),flags=record.readUInt32LE(4);check(segment>0&&segment<=count&&(flags&~15)===0,'selected PUB32 segment/flags');const s=image.sections[segment-1],vtable=family.includes('vftable'),extent=vtable?8:1;check(offset+extent<=s.mappedBytes&&!s.resourceOverlap&&s.readable&&!s.writable&&(vtable?!s.executable&&s.initialized:s.code&&s.executable),'selected PUB32 strict source mapping');const rva=s.rva+offset;hits.push({id:'SYM'+String(hits.length+1).padStart(2,'0'),name,family,...extra,segment,segment_offset:offset,record_offset:at,flags,section_id:s.id,rva,address:'0x'+(image.base+BigInt(rva)).toString(16),file_offset:s.raw+offset,executable:s.executable,source_extent_bytes:extent});}}
  at+=bytes;
 }
 const result={profile:'selected_client_MSFPDB7_DBI_S_PUB32',guid_bytes:identity.guid_bytes,age:identity.age,image_base:'0x'+image.base.toString(16),stream_count:m.count,symbol_stream:index,symbol_stream_bytes:total,records,hit_count:hits.length,hit_limit:LIMITS.hits,hits,complete_symbol_record_framing:true,selected_name_profile:sel.classes?'caller_class_fixed_decorated_ascii_forms_max1024_not_all_class_members':'explicit_decorated_ascii_allowlist_max1024_not_all_class_members',address_queries:queries?[...queries].map(v=>'0x'+v.toString(16)):[],class_name_queries:sel.classes?sel.classes.names:[],class_method_queries:sel.classes?sel.classes.methods:[],struct_ops_queries:sel.classes?sel.classes.structOps:[],unselected_names_exported:false,omap_absent:true,pdb_pe_section_headers_equal:true,instruction_boundaries_confirmed:false,class_owner_confirmed:false,older_server_equivalence:false,runtime_behavior_proven:false};check(Buffer.byteLength(JSON.stringify(result)+'\n')<=LIMITS.output,'result output cap');return result;
}
module.exports={inspect,msf,selector,LIMITS,PIN};
