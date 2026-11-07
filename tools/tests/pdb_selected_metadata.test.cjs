'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const {spawnSync} = require('node:child_process');
const {createHash} = require('node:crypto');
const {fixture, pack} = require('./pdb_public_symbols_fixtures.cjs');
const {PROFILE, PARAMETER_PROFILE, ENUM_PROFILE} = require('../pdb_selected_types.cjs');
const {run, CAPS} = require('../pdb_selected_metadata.cjs');
// Independently authored empty structure and enum records; shared MSF builder.
function tpi(enums = false) {
    const name = Buffer.from('Authored\0');
    const r = Buffer.alloc(enums ? 28 : 32);
    r.writeUInt16LE(r.length - 2); r.writeUInt16LE(enums ? 0x1507 : 0x1505, 2);
    if (enums) { r.writeUInt32LE(0x75, 8); name.copy(r, 16); r.set([0xf3, 0xf2, 0xf1], 25); }
    else { r.writeUInt16LE(1, 20); name.copy(r, 22); r[31] = 0xf1; }
    const h = Buffer.alloc(56);
    for (const [offset, value] of [[0,20040203],[4,56],[8,0x1000],[12,0x1001],[16,r.length],[24,4],[28,4096]]) h.writeUInt32LE(value,offset);
    h.writeUInt16LE(0xffff,20); h.writeUInt16LE(0xffff,22);
    return Buffer.concat([h,r]);
}
const caseRoot = path.resolve('work/pdb-selected-metadata-authored-tests');
fs.mkdirSync(caseRoot, {recursive:true});
fs.writeFileSync(path.join(caseRoot,'case.json'), JSON.stringify({state:'open',owner:'metadata-runner-tests',purpose:'Authored MSF/TPI integration fixtures only'}));
let sequence = 0;
function setup(enums = false) {
    const dir = fs.mkdtempSync(path.join(caseRoot, `test-${sequence++}-`));
    const f = fixture(); f.streams[2] = tpi(enums);
    const bytes = pack(f).pdb, input = path.join(dir,'input.pdb'); fs.writeFileSync(input,bytes);
    const request = {pdb_path:input,pdb_bytes:bytes.length,pdb_sha256:createHash('sha256').update(bytes).digest('hex'),
        identity:f.identity,profile:enums ? ENUM_PROFILE : PROFILE,names:['Authored'],output_path:path.join(dir,'result.json')};
    return {dir,bytes,request};
}
function rejects(request) { assert.throws(()=>run(request), /^Error: selected metadata request failed$/); assert.equal(fs.existsSync(request.output_path),false); }
test('all supported profiles bind exact authored PDB hash and identity and exclusively publish metadata',()=>{
    for (const profile of [PROFILE, PARAMETER_PROFILE, ENUM_PROFILE]) {
        const {request} = setup(profile === ENUM_PROFILE); request.profile=profile;
        const receipt=run(request), output=fs.readFileSync(request.output_path);
        assert.equal(receipt.output_bytes,output.length);
        const result=JSON.parse(output);assert.equal(result.metadata.profile,profile);assert.equal(result.pdb_sha256,request.pdb_sha256);
        assert.equal(fs.statSync(request.output_path).mode & 0o777,0o600);
        assert.throws(()=>run(request),/selected metadata request failed/);
        assert.deepEqual(fs.readFileSync(request.output_path),output);
    }
});
test('mismatch, truncation, identity, selection, bounds and malformed framing fail without output',()=>{
    const changes=[r=>r.pdb_sha256='0'.repeat(64),r=>r.pdb_bytes--,r=>r.identity={...r.identity,age:2},
        r=>r.names=['PrivateMissingSentinel'],r=>r.max_input_bytes=r.pdb_bytes-1,r=>r.max_output_bytes=1,
        r=>r.max_output_bytes=CAPS.output_bytes+1,r=>r.profile='unknown',r=>r.unexpected=true,
        r=>fs.truncateSync(r.pdb_path,100),r=>{const b=fs.readFileSync(r.pdb_path);b[0]=0;fs.writeFileSync(r.pdb_path,b);r.pdb_sha256=createHash('sha256').update(b).digest('hex');}];
    for(const change of changes){const {request}=setup();change(request);rejects(request);}
});
test('symlink, directory and FIFO input reject promptly; output symlink preserves destination',()=>{
    for(const kind of ['symlink','directory','fifo']){
        const {dir,request}=setup(), special=path.join(dir,'special');
        if(kind==='symlink')fs.symlinkSync(request.pdb_path,special);
        if(kind==='directory')fs.mkdirSync(special);
        if(kind==='fifo'){const child=spawnSync('mkfifo',[special],{timeout:3000});assert.equal(child.status,0);}
        request.pdb_path=special;rejects(request);
    }
    const {dir,request}=setup(), target=path.join(dir,'keep');fs.writeFileSync(target,'keep');fs.symlinkSync(target,request.output_path);
    assert.throws(()=>run(request),/selected metadata request failed/);assert.equal(fs.readFileSync(target,'utf8'),'keep');
});
test('CLI stays silent on success and suppresses private paths and selectors on failure',()=>{
    const {dir,request}=setup(), config=path.join(dir,'request.json');fs.writeFileSync(config,JSON.stringify(request));
    const invoke=()=>spawnSync(process.execPath,[path.resolve('tools/pdb_selected_metadata.cjs'),'--request',config],{encoding:'utf8',timeout:5000});
    const good=invoke();assert.equal(good.status,0);assert.equal(good.stdout,'');assert.equal(good.stderr,'');
    request.names=['PrivateMissingSentinel'];request.output_path=path.join(dir,'private-result');fs.writeFileSync(config,JSON.stringify(request));
    const bad=invoke();assert.equal(bad.status,1);assert.equal(bad.stdout,'');assert.equal(bad.stderr,'selected metadata request failed\n');
    assert.equal(fs.existsSync(request.output_path),false);
});
test.after(()=>{fs.writeFileSync(path.join(caseRoot,'case.json'),JSON.stringify({state:'closed',owner:'metadata-runner-tests',purpose:'Authored MSF/TPI integration fixtures only'}));});
test('API rejects accessors, proxies, symbols and sparse data without caller code execution',()=>{
    for(const mutate of [
        r=>Object.defineProperty(r,'pdb_sha256',{get(){throw Error('private getter invoked');},enumerable:true}),
        r=>r.identity=new Proxy(r.identity,{get(){throw Error('private proxy invoked');}}),
        r=>r[Symbol('private')]=1,
        r=>r.names=new Array(1),
        r=>r.identity.toJSON=()=>{throw Error('private toJSON invoked');},
    ]){const {request}=setup();mutate(request);rejects(request);}
    let invoked=false;const {request}=setup();
    Object.defineProperty(request,'names',{get(){invoked=true;return ['Authored'];},enumerable:true});
    rejects(request);assert.equal(invoked,false);
});
