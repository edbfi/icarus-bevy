'use strict';
// Pure authored PE64 reader/candidate search. Never opens files or runs tools.
const crypto=require('node:crypto');
const MAX_FILE_BYTES=128*1024*1024,MAX_SECTIONS=96,MAX_CANDIDATES=16,MAX_OUTPUT_BYTES=65536;
const ensure=(ok,message)=>{if(!ok)throw Error(message);};
function parse(bytes){
 ensure(Buffer.isBuffer(bytes)&&bytes.length>=64&&bytes.length<=MAX_FILE_BYTES,'file bound');
 const span=(at,n)=>{ensure(Number.isSafeInteger(at)&&Number.isSafeInteger(n)&&at>=0&&n>=0&&at+n<=bytes.length,'file range');return bytes.subarray(at,at+n);};
 const u16=o=>span(o,2).readUInt16LE(),u32=o=>span(o,4).readUInt32LE();
 ensure(u16(0)===0x5a4d,'DOS signature');const pe=u32(60);ensure(pe>=64,'PE offset');ensure(u32(pe)===0x00004550,'PE signature');ensure(u16(pe+4)===0x8664,'x64 machine');
 const count=u16(pe+6),optionalBytes=u16(pe+20),coff=u16(pe+22),op=pe+24;ensure(count>0&&count<=MAX_SECTIONS&&(coff&2)!==0&&(coff&0x2000)===0,'executable sections');
 ensure(optionalBytes>=112&&optionalBytes<=4096,'optional header bound');span(op,optionalBytes);ensure(u16(op)===0x20b,'PE64 optional magic');const base=span(op+24,8).readBigUInt64LE(),imageBytes=u32(op+56),headerBytes=u32(op+60),directories=u32(op+108);
 ensure(base>0n&&base+BigInt(imageBytes)<=0xffffffffffffffffn&&imageBytes>0&&headerBytes>0&&headerBytes<=bytes.length,'image/header bound');
 ensure(directories<=16&&112+directories*8<=optionalBytes,'directory bound');
 const entry=u32(op+16),table=op+optionalBytes;span(table,count*40);ensure(table+count*40<=headerBytes,'section table header range');
 const resource=directories>2?{rva:u32(op+112+2*8),size:u32(op+116+2*8)}:{rva:0,size:0};ensure((resource.rva===0)===(resource.size===0)&&resource.rva+resource.size<=imageBytes,'resource directory range');
 const sectionAlignment=u32(op+32),fileAlignment=u32(op+36);const powerOfTwo=n=>n>0&&(n&(n-1))===0;ensure(powerOfTwo(sectionAlignment)&&powerOfTwo(fileAlignment)&&sectionAlignment>=4096&&fileAlignment>=512&&fileAlignment<=65536&&sectionAlignment>=fileAlignment,'standard alignment policy');ensure(headerBytes%fileAlignment===0&&imageBytes%sectionAlignment===0&&base%65536n===0n,'image/header/base alignment');
 const sections=[];
 for(let index=0;index<count;index++){
  const at=table+40*index,virtualBytes=u32(at+8),rva=u32(at+12),rawBytes=u32(at+16),raw=u32(at+20),flags=u32(at+36),memoryBytes=Math.max(virtualBytes,rawBytes),mappedBytes=Math.min(rawBytes,virtualBytes||rawBytes);
  ensure(rva%sectionAlignment===0&&rawBytes%fileAlignment===0&&(rawBytes===0||raw%fileAlignment===0),'section alignment');
  ensure(virtualBytes>0&&memoryBytes>0&&rva>=headerBytes&&rva+memoryBytes<=imageBytes,'section virtual range');
  if(rawBytes){ensure(raw>=headerBytes,'section/header raw overlap');span(raw,rawBytes);}
  for(const s of sections){ensure(rva>=s.rva+s.memoryBytes||s.rva>=rva+memoryBytes,'overlapping virtual sections');if(rawBytes&&s.rawBytes)ensure(raw>=s.raw+s.rawBytes||s.raw>=raw+rawBytes,'overlapping raw sections');}
  const code=(flags&0x20)!==0,executable=(flags&0x20000000)!==0,writable=(flags&0x80000000)!==0,readable=(flags&0x40000000)!==0,initialized=(flags&0x40)!==0,resourceOverlap=resource.size>0&&rva<resource.rva+resource.size&&resource.rva<rva+memoryBytes;
  sections.push({id:'SEC'+String(index+1).padStart(2,'0'),rva,raw,rawBytes,memoryBytes,mappedBytes,code,executable,writable,readable,initialized,resourceOverlap});
 }
 if(resource.size)ensure(sections.filter(s=>resource.rva>=s.rva&&resource.rva+resource.size<=s.rva+s.mappedBytes).length===1,'resource file-backed section mapping');
 const locate=rva=>sections.filter(s=>rva>=s.rva&&rva<s.rva+s.mappedBytes);
 ensure(locate(entry).length===1&&locate(entry)[0].executable,'executable file-backed image entry');
 const maxOffsets=sections.filter(s=>(s.code&&s.executable&&!s.writable&&!s.resourceOverlap)||(s.readable&&s.initialized&&!s.writable&&!s.executable&&!s.resourceOverlap)).reduce((n,s)=>n+s.mappedBytes,0);ensure(maxOffsets<=MAX_FILE_BYTES,'scan-offset ceiling');
 return {base,imageBytes,entry,sections,maxOffsets,locate};
}
function scan(bytes,pin){
 ensure(Buffer.isBuffer(bytes)&&bytes.length>=64&&bytes.length<=MAX_FILE_BYTES,'file bound before hash');
 ensure(pin&&/^[a-f0-9]{64}$/.test(pin.sha256)&&typeof pin.target==='bigint','explicit input pin');
 ensure(crypto.createHash('sha256').update(bytes).digest('hex')===pin.sha256,'input SHA mismatch');
 const image=parse(bytes);ensure(pin.target>=image.base&&pin.target<image.base+BigInt(image.imageBytes),'target image range');const targetRva=Number(pin.target-image.base),target=image.locate(targetRva);ensure(target.length===1&&target[0].code&&target[0].executable&&!target[0].writable&&!target[0].resourceOverlap,'target executable mapping');
 const pointer=Buffer.alloc(8);pointer.writeBigUInt64LE(pin.target);const candidates=[];let examined=0;
 const add=(kind,s,offset)=>{ensure(candidates.length<MAX_CANDIDATES,'candidate cap exceeded');candidates.push({id:'CAND'+String(candidates.length+1).padStart(2,'0'),pattern:kind,section_id:s.id,section_class:s.executable?'executable':'readonly_initialized_nonresource',file_offset:offset,rva:s.rva+offset-s.raw,address:'0x'+(image.base+BigInt(s.rva+offset-s.raw)).toString(16),target:'0x'+pin.target.toString(16),instruction_boundary_confirmed:false,function_identity_confirmed:false});};
 for(const s of image.sections){
  if(s.code&&s.executable&&!s.writable&&!s.resourceOverlap){for(let n=0;n+5<=s.mappedBytes;n++){examined++;const at=s.raw+n;if((bytes[at]===0xe8||bytes[at]===0xe9)&&image.base+BigInt(s.rva+n+5)+BigInt(bytes.readInt32LE(at+1))===pin.target)add(bytes[at]===0xe8?'candidate_E8_rel32':'candidate_E9_rel32',s,at);}}
  else if(s.readable&&s.initialized&&!s.executable&&!s.writable&&!s.resourceOverlap){for(let n=0;n+8<=s.mappedBytes;n++){examined++;const at=s.raw+n;if(bytes[at]!==pointer[0])continue;let same=true;for(let i=1;i<8;i++)if(bytes[at+i]!==pointer[i]){same=false;break;}if(same)add('candidate_absolute64_pointer',s,at);}}
 }
 ensure(examined<=image.maxOffsets,'complete offset bound');
 const result={input_sha256:pin.sha256,profile:'PE64_x64_executable_candidate_patterns_only',image_base:'0x'+image.base.toString(16),target:'0x'+pin.target.toString(16),sections:image.sections.length,examined_offsets:examined,offset_ceiling:image.maxOffsets,candidate_count:candidates.length,candidate_limit:MAX_CANDIDATES,candidates,complete_within_declared_section_patterns:true,confirmed_callers:false,indirect_dispatch_coverage:false,resource_sections_excluded:true};
 const json=JSON.stringify(result)+'\n';ensure(Buffer.byteLength(json)<=MAX_OUTPUT_BYTES,'output byte cap');return result;
}
module.exports={parse,scan,MAX_FILE_BYTES,MAX_SECTIONS,MAX_CANDIDATES,MAX_OUTPUT_BYTES};
