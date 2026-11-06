'use strict';
// Independently authored PE envelopes only; no files, originals or tool execution.
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const {test} = require('node:test');
const vm = require('node:vm');
const {image} = require('./pe_candidates_fixtures.cjs');
const {inspectNativeClassParams: inspect, PROFILE} = require('../native_class_params.cjs');
const ERROR = 'invalid native class parameters';
const REQUIREMENTS = [
  'Caller must establish input and selected-address provenance.',
  'Function-link stride and extent remain unresolved; entries are not read.',
  'Opaque slots, property descriptors, interface meanings and layouts outside this profile remain unresolved.',
  'Runtime bindings and release compatibility remain unqualified.',
];
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const va = (f, rva) => '0x' + (f.base + BigInt(rva)).toString(16);
function fixture() {
  const f = image();
  f.bytes.fill(0, 0x400, 0x580);
  f.bytes.writeBigUInt64LE(f.target, 0x400);
  return f;
}
const options = (f, rvas = [0x2000]) => ({sha256: hash(f.bytes), profile: PROFILE, addresses: rvas.map(rva => va(f, rva))});
function reject(f, opts = options(f), bytes = f.bytes) {
  assert.throws(() => inspect(bytes, opts), error => {
    assert.equal(error.message, ERROR);
    assert.equal(String(error), 'Error: ' + ERROR);
    return true;
  });
}
function header(f, rva) {
  f.bytes.fill(0, 0x400 + rva - 0x2000, 0x400 + rva - 0x2000 + 76);
  f.bytes.writeBigUInt64LE(f.target, 0x400 + rva - 0x2000);
}

test('exact neutral header metadata, unresolved requirements and immutable inputs', () => {
  const f = fixture(), before = Buffer.from(f.bytes), opts = options(f), optsBefore = structuredClone(opts);
  assert.deepEqual(inspect(f.bytes, opts), {
    input_sha256: opts.sha256, profile: PROFILE, image_base: '0x140000000', release_qualified: false,
    selected: [{address: '0x140002000', factory_pointer: '0x140001100', opaque_conversion_input: '0x0', opaque_slot_10: '0x0', dependencies: [], property_parameter_pointers: [], interface_records: [], function_link: {pointer: '0x0', count: 0}, parameter_word_48: 0}],
    unresolved_requirements: REQUIREMENTS,
  });
  assert.deepEqual(f.bytes, before);
  assert.deepEqual(opts, optsBefore);
});

test('nonempty arrays expose only exact pointer and raw-record metadata', () => {
  const f = fixture();
  for (const [off, value] of [[8,0x2081n],[16,0x2089n],[24,0x2090n],[32,0x2099n],[40,0x20a0n],[48,0x20b0n]]) f.bytes.writeBigUInt64LE(f.base+value,0x400+off);
  for (const off of [56,60,64,68]) f.bytes.writeInt32LE(1,0x400+off);
  f.bytes.writeBigUInt64LE(f.target+1n,0x490);
  f.bytes.writeBigUInt64LE(f.base+0x20c1n,0x4a0);
  f.bytes.writeBigUInt64LE(f.target+2n,0x4b0);
  f.bytes.writeUInt32LE(0xfedcba98,0x4b8);
  f.bytes[0x4bc]=0xfe;
  f.bytes.writeUInt32LE(0x87654321,0x448);
  const r=inspect(f.bytes,options(f)).selected[0];
  assert.equal(r.opaque_conversion_input,'0x140002081');
  assert.equal(r.opaque_slot_10,'0x140002089');
  assert.deepEqual(r.dependencies,['0x140001101']);
  assert.deepEqual(r.property_parameter_pointers,['0x1400020c1']);
  assert.deepEqual(r.interface_records,[{getter:'0x140001102',word_08:0xfedcba98,raw_byte_0c:254}]);
  assert.equal(r.parameter_word_48,0x87654321);
  assert.deepEqual(r.function_link,{pointer:'0x140002099',count:1});
});

test('zero-count optional slots validate one opaque byte without reading array extents', () => {
  const f=fixture();
  for (const off of [8,16,24,32,40,48]) f.bytes.writeBigUInt64LE(f.base+0x217fn,0x400+off);
  const r=inspect(f.bytes,options(f)).selected[0];
  assert.deepEqual(r.dependencies,[]);
  assert.deepEqual(r.property_parameter_pointers,[]);
  assert.deepEqual(r.interface_records,[]);
  assert.deepEqual(r.function_link,{pointer:'0x14000217f',count:0});
  f.bytes.writeInt32LE(1,0x438); reject(f);
});

test('selected header order, eight-header cap, alignment and exclusive section ends', () => {
  const f=fixture(); header(f,0x2050);
  assert.deepEqual(inspect(f.bytes,options(f,[0x2050,0x2000])).selected.map(r=>r.address),['0x140002050','0x140002000']);
  const end=fixture(); header(end,0x2130); assert.equal(inspect(end.bytes,options(end,[0x2130])).selected.length,1);
  reject(end,options(end,[0x2138])); reject(end,options(end,[0x2001]));
  const wide=fixture(); wide.bytes.writeUInt32LE(0x400,wide.table+40+8); wide.bytes.writeUInt32LE(0x400,wide.table+40+16); wide.bytes.writeUInt32LE(0x800,wide.table+80+20);
  const rvas=Array.from({length:8},(_,i)=>0x2000+i*0x50); for(const rva of rvas)header(wide,rva);
  assert.equal(inspect(wide.bytes,options(wide,rvas)).selected.length,8);
  reject(wide,options(wide,[...rvas,0x2280]));
});

for (const [field, countOffset, pointerOffset, stride, total] of [['properties',64,40,8,'total_properties'],['interfaces',68,48,16,'total_interfaces']]) {
  test(`aggregate ${total} budget rejects two individually admitted selected headers`, () => {
    const f=fixture(); header(f,0x2050);
    for(const start of [0x400,0x450]) {
      f.bytes.writeInt32LE(1,start+countOffset);
      f.bytes.writeBigUInt64LE(f.base+0x20d0n,start+pointerOffset);
    }
    if(stride===8)f.bytes.writeBigUInt64LE(f.base+0x2121n,0x4d0);
    const opts={...options(f,[0x2000,0x2050]),limits:{[field]:1,[total]:2}};
    assert.equal(inspect(f.bytes,opts).selected.length,2);
    reject(f,{...opts,limits:{[field]:1,[total]:1}});
    assert.equal(inspect(f.bytes,{...options(f),limits:{[field]:1,[total]:1}}).selected.length,1);
  });
}

test('public selector strings must be canonical, unique and representable', () => {
  const f=fixture(), opts=options(f);
  for(const addresses of [[],[opts.addresses[0],opts.addresses[0]],[1n],[0x140002000],['0X140002000'],['0x0140002000'],['0x14000200A'],['0x0'],['0x800000000000'],['0x10000000000000000'],[' 0x140002000'],['0x140002000 ']]) reject(f,{...opts,addresses});
});

test('options and lowered limits reject unsupported keys, prototypes and values', () => {
  const f=fixture(), opts=options(f);
  for(const bad of [null,[],new Date(),Object.assign(Object.create({inherited:true}),opts),Object.create(opts),{...opts,unknown:1},{sha256:opts.sha256,addresses:opts.addresses},{profile:PROFILE,addresses:opts.addresses}]) reject(f,bad);
  for(const limits of [null,[],new Date(),Object.assign(Object.create({inherited:true}),{properties:1}),{unknown:1},{properties:4097},{properties:-1},{properties:0.5},{properties:NaN},{properties:Infinity},{properties:'1'},{output_bytes:1}]) reject(f,{...opts,limits});
  reject(f,{...opts,profile:'unknown'});
  assert.equal(inspect(f.bytes,{...opts,limits:{properties:0,total_properties:0,interfaces:0,total_interfaces:0}}).selected.length,1);
});

test('negative and per-header excessive counts reject without a result prefix', () => {
  for(const off of [56,64,68]){const f=fixture();f.bytes.writeInt32LE(-1,0x400+off);reject(f);}
  for(const [off,value]of [[56,65],[60,4097],[64,4097],[68,129]]){const f=fixture();f.bytes.writeUInt32LE(value,0x400+off);reject(f);}
  const f=fixture();header(f,0x2050);f.bytes.writeInt32LE(-1,0x450+64);reject(f,options(f,[0x2000,0x2050]));
});

test('hash mismatch is checked before any PE parse access', () => {
  const f=fixture(); f.bytes.fill(0); let reads=0;
  f.bytes.readUInt16LE=()=>{reads++;throw Error('PE parse must not run');};
  reject(f,{...options(f),sha256:'0'.repeat(64)});
  assert.equal(reads,0);
});

test('same-realm and cross-realm shared backing buffers reject', () => {
  const f=fixture();
  for(const backing of [new SharedArrayBuffer(f.bytes.length),vm.runInNewContext(`new SharedArrayBuffer(${f.bytes.length})`)]) {
    const shared=Buffer.from(backing);f.bytes.copy(shared);reject(f,options(f),shared);
  }
});

test('readonly/code/resource mappings, virtual tails and opaque pointers are bounded', () => {
  for(const pointer of [0n,0x140002000n,0x140001180n,0xfffffffffffffff8n]){const f=fixture();f.bytes.writeBigUInt64LE(pointer,0x400);reject(f);}
  for(const pointer of [0x140001100n,0x140003010n,0x140002180n,0x140004000n]){const f=fixture();f.bytes.writeBigUInt64LE(pointer,0x408);reject(f);}
  for(const flags of [0xc0000040,0x60000020,0x00000040]){const f=fixture();f.bytes.writeUInt32LE(flags,f.table+40+36);reject(f);}
  const short=fixture();short.bytes.writeUInt32LE(75,short.table+40+8);reject(short);
  const cross=fixture();cross.bytes.writeInt32LE(1,0x440);cross.bytes.writeBigUInt64LE(cross.base+0x2178n,0x428);cross.bytes.writeBigUInt64LE(cross.base+0x2180n,0x578);reject(cross);
  const extent=fixture();extent.bytes.writeInt32LE(2,0x440);extent.bytes.writeBigUInt64LE(extent.base+0x2178n,0x428);reject(extent);
  const virtual=fixture();virtual.bytes.writeUInt32LE(0x300,virtual.table+40+8);virtual.bytes.writeBigUInt64LE(virtual.base+0x2201n,0x408);reject(virtual);
  const overlap=fixture();overlap.bytes.writeUInt32LE(0x1000,overlap.table+40+12);reject(overlap);
});

test('opaque and unselected bytes never leak as strings or descriptor interpretation', () => {
  const f=fixture(), marker='AuthoredPrivateMarker';
  f.bytes.write(marker,0x4c1,'ascii');f.bytes.write(marker,0x540,'ascii');
  f.bytes.writeBigUInt64LE(f.base+0x20c1n,0x408);
  f.bytes.writeInt32LE(1,0x440);f.bytes.writeBigUInt64LE(f.base+0x20a0n,0x428);f.bytes.writeBigUInt64LE(f.base+0x20c1n,0x4a0);
  const result=inspect(f.bytes,options(f)), text=JSON.stringify(result);
  assert.equal(text.includes(marker),false);assert.equal(text.includes(Buffer.from(marker).toString('hex')),false);
  assert.equal(result.release_qualified,false);assert.deepEqual(result.unresolved_requirements,REQUIREMENTS);
  assert.deepEqual(result.selected[0].property_parameter_pointers,['0x1400020c1']);
});

test('output byte cap includes the complete metadata object at its exact boundary', () => {
  const f=fixture(),opts=options(f),result=inspect(f.bytes,opts);
  const length=Buffer.byteLength(JSON.stringify(result));
  assert.deepEqual(inspect(f.bytes,{...opts,limits:{output_bytes:length}}),result);
  reject(f,{...opts,limits:{output_bytes:length-1}});
});

test('only Buffer input and lowercase full SHA256 pins are admitted', () => {
  const f=fixture(),opts=options(f);
  reject(f,opts,new Uint8Array(f.bytes));
  for(const sha256 of ['',opts.sha256.toUpperCase(),'g'.repeat(64),'0'.repeat(63),0n])reject(f,{...opts,sha256});
});

test('null-prototype data options/limits work while unknown symbol keys reject', () => {
  const f=fixture(),opts=options(f);
  const plain=Object.assign(Object.create(null),opts,{limits:Object.assign(Object.create(null),{properties:0})});
  assert.equal(inspect(f.bytes,plain).selected.length,1);
  const symbol=Symbol('unknown');
  reject(f,{...opts,[symbol]:true});
  reject(f,{...opts,limits:{[symbol]:true}});
});
