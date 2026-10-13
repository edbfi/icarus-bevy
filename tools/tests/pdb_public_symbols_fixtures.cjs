'use strict';
// Independently authored synthetic MSF7/DBI/PUB32 and PE/RSDS fixtures only.
// No target files are opened and no production serializer is used.
const {image}=require('./pe_candidates_fixtures.cjs');
const GUID='f18774cb0f78a849bf3055a023b53562';
function pub(name,segment=1,offset=0x20,flags=2){const n=Buffer.from(name,'ascii'),b=Buffer.alloc(Math.ceil((15+n.length)/4)*4);b.writeUInt16LE(b.length-2);b.writeUInt16LE(0x110e,2);b.writeUInt32LE(flags,4);b.writeUInt32LE(offset,8);b.writeUInt16LE(segment,12);n.copy(b,14);return b;}
function fixture(records=[pub('??0UWorld@@QEAA@XZ'),pub('??_7UWorld@@6BFNetworkNotify@@@',2,0x40,0),pub('?NotifyControlMessage@UPendingNetGame@@UEAAXXZ')]){
 const f=image(),exe=f.bytes;exe.writeUInt32LE(0x2080,f.op+112+6*8);exe.writeUInt32LE(28,f.op+116+6*8);const dr=0x480,cv=0x4a0,n=32;exe.writeUInt32LE(2,dr+12);exe.writeUInt32LE(n,dr+16);exe.writeUInt32LE(0x20a0,dr+20);exe.writeUInt32LE(cv,dr+24);exe.write('RSDS',cv);Buffer.from(GUID,'hex').copy(exe,cv+4);exe.writeUInt32LE(1,cv+20);exe[cv+24]=65;
 const info=Buffer.alloc(28);info.writeUInt32LE(20000404);info.writeUInt32LE(1,8);Buffer.from(GUID,'hex').copy(info,12);const dbi=Buffer.alloc(76);dbi.writeInt32LE(-1);dbi.writeUInt32LE(19990903,4);dbi.writeUInt32LE(1,8);dbi.writeUInt16LE(4,20);dbi.writeInt32LE(12,48);dbi.writeUInt16LE(0x8664,58);for(let i=64;i<76;i+=2)dbi.writeUInt16LE(65535,i);dbi.writeUInt16LE(5,74);
 const streams=[Buffer.alloc(0),info,null,dbi,Buffer.concat(records),Buffer.from(exe.subarray(f.table,f.table+120))];return {...f,exe,streams,identity:{guid_bytes:GUID,age:1}};
}
function pack(f){const bs=4096,dirs=[],entries=[];let page=5;for(const b of f.streams){const pages=[];if(b)for(let i=0;i<Math.ceil(b.length/bs);i++)pages.push(page++);entries.push({size:b?b.length:0xffffffff,pages});}const ds=4+4*entries.length+4*entries.reduce((n,s)=>n+s.pages.length,0),d=Buffer.alloc(ds);d.writeUInt32LE(entries.length);let at=4+entries.length*4;entries.forEach((s,i)=>{d.writeUInt32LE(s.size,4+i*4);s.pages.forEach(p=>{d.writeUInt32LE(p,at);at+=4;});});const pdb=Buffer.alloc(page*bs);Buffer.from('Microsoft C/C++ MSF 7.00\r\n\x1aDS\0\0\0','binary').copy(pdb);for(const [o,n]of [[32,bs],[36,1],[40,page],[44,ds],[52,3]])pdb.writeUInt32LE(n,o);pdb.writeUInt32LE(4,3*bs);d.copy(pdb,4*bs);entries.forEach((s,i)=>s.pages.forEach((p,j)=>f.streams[i].subarray(j*bs,(j+1)*bs).copy(pdb,p*bs)));return {...f,pdb,entries,reader:{size:pdb.length,read:(o,n)=>Buffer.from(pdb.subarray(o,o+n))}};}
module.exports={fixture,pack,pub,GUID};
