'use strict';
// Independently authored PE64 envelopes; no target data or production encoder.
function image(){const b=Buffer.alloc(0xa00),pe=0x80,op=pe+24,table=op+240;
 b.writeUInt16LE(0x5a4d,0);b.writeUInt32LE(pe,60);b.writeUInt32LE(0x4550,pe);b.writeUInt16LE(0x8664,pe+4);b.writeUInt16LE(3,pe+6);b.writeUInt16LE(240,pe+20);b.writeUInt16LE(2,pe+22);b.writeUInt16LE(0x20b,op);b.writeUInt32LE(0x1000,op+16);b.writeBigUInt64LE(0x140000000n,op+24);b.writeUInt32LE(0x1000,op+32);b.writeUInt32LE(0x200,op+36);b.writeUInt32LE(0x4000,op+56);b.writeUInt32LE(0x200,op+60);b.writeUInt32LE(16,op+108);b.writeUInt32LE(0x3000,op+128);b.writeUInt32LE(0x100,op+132);
 for(const [i,rva,raw,virtual,flags]of [[0,0x1000,0x200,0x180,0x60000020],[1,0x2000,0x400,0x180,0x40000040],[2,0x3000,0x600,0x180,0x40000040]]){const at=table+40*i;b.writeUInt32LE(virtual,at+8);b.writeUInt32LE(rva,at+12);b.writeUInt32LE(0x200,at+16);b.writeUInt32LE(raw,at+20);b.writeUInt32LE(flags,at+36);}
 return {bytes:b,pe,op,table,base:0x140000000n,target:0x140001100n};
}
function relative(f,raw,opcode=0xe8){const rva=0x1000+raw-0x200;f.bytes[raw]=opcode;f.bytes.writeInt32LE(Number(f.target-f.base-BigInt(rva+5)),raw+1);}
module.exports={image,relative};
