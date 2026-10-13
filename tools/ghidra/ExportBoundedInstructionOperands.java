// Numeric instruction operands/raw p-code only; source-only script leaf.
// @category IcarusResearch
import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.lang.Register;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.pcode.PcodeOp;
import ghidra.program.model.pcode.Varnode;
import ghidra.program.model.scalar.Scalar;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.util.HexFormat;

public class ExportBoundedInstructionOperands extends GhidraScript {
    private static final String SHA="67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104";
    private static final String[] IDS={"TARGET01","TARGET02"},STARTS={"1439da30e","1439dae70"};
    private static final String[] METADATA_BEGINS={"1439da2e8","1439dae50"};
    private static final int MAX_OUTPUT=65536;
    private static String hex(long n){return "0x"+Long.toUnsignedString(n,16);}
    private Address addr(String n){return currentProgram.getAddressFactory().getDefaultAddressSpace().getAddress(Long.parseUnsignedLong(n,16));}
    private static String location(Address a){return "{\"space\":"+a.getAddressSpace().getSpaceID()+",\"offset\":\""+hex(a.getOffset())+"\"}";}
    private static String entry(Function f){return f==null?"null":location(f.getEntryPoint());}
    private static String hash(byte[] b)throws Exception{return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(b));}
    private static String varnode(Varnode n){if(n==null)return "null";int width=n.getSize();if(width<1||width>16)throw new IllegalStateException();String kind=n.isConstant()?"constant":n.isRegister()?"register":n.isUnique()?"unique":n.isAddress()?"address":null;if(kind==null)throw new IllegalStateException();return "{\"kind\":\""+kind+"\",\"space\":"+n.getAddress().getAddressSpace().getSpaceID()+",\"offset\":\""+hex(n.getOffset())+"\",\"width\":"+width+"}";}
    private static String operand(Object o){
        if(o instanceof Register r){if(r.getBitLength()<1||r.getBitLength()>128)throw new IllegalStateException();return "{\"kind\":\"register\",\"location\":"+location(r.getAddress())+",\"bit_width\":"+r.getBitLength()+"}";}
        if(o instanceof Scalar s){if(s.bitLength()<1||s.bitLength()>64)throw new IllegalStateException();return "{\"kind\":\"scalar\",\"bit_width\":"+s.bitLength()+",\"unsigned\":\""+hex(s.getUnsignedValue())+"\",\"signed\":\""+s.getSignedValue()+"\"}";}
        if(o instanceof Address a)return "{\"kind\":\"address\",\"location\":"+location(a)+"}";
        throw new IllegalStateException();
    }
    @Override public void run()throws Exception{try{export();}catch(Exception failure){throw new IllegalStateException("Bound instruction operand export failed");}}
    private void export()throws Exception{
        String[] args=getScriptArgs();if(args.length!=2||currentProgram==null||!SHA.equals(args[1])||!SHA.equals(currentProgram.getExecutableSHA256())||currentProgram.getImageBase().getOffset()!=0x140000000L||!currentProgram.getLanguageID().toString().equals("x86:LE:64:default"))throw new IllegalArgumentException();
        Path output=Path.of(args[0]).toAbsolutePath().normalize();if(Files.exists(output))throw new IllegalStateException();
        Function[] beforeAt=new Function[2],beforeContaining=new Function[2];
        for(int i=0;i<2;i++){Address a=addr(STARTS[i]);MemoryBlock b=currentProgram.getMemory().getBlock(a);if(b==null||!b.isInitialized()||!b.isExecute()||b.isWrite()||!b.contains(a.add(45)))throw new IllegalStateException();beforeAt[i]=currentProgram.getFunctionManager().getFunctionAt(addr(METADATA_BEGINS[i]));beforeContaining[i]=currentProgram.getFunctionManager().getFunctionContaining(a);}
        StringBuilder json=new StringBuilder("{\"profile\":\"bounded_instruction_operands_pcode\",\"program_sha256\":\""+SHA+"\",\"image_base\":\"0x140000000\",\"window_bytes\":32,\"decoder_lookahead_bytes\":14,\"decoder_span_bytes\":46,\"instruction_limit\":8,\"operand_object_limit\":16,\"pcode_limit\":64,\"varnode_limit\":16,\"decompilation_requested\":false,\"function_creation_requested\":false,\"analysis_requested\":false,\"flow_following_requested\":false,\"owner_confirmed\":false,\"windows\":[");
        for(int wi=0;wi<2;wi++){
            if(wi>0)json.append(',');Address start=addr(STARTS[wi]),end=start.add(32),cursor=start;String stop="instruction_limit";int count=0;
            json.append("{\"id\":\"").append(IDS[wi]).append("\",\"start\":").append(location(start)).append(",\"end_exclusive\":").append(location(end)).append(",\"before_function_at_metadata_begin\":").append(entry(beforeAt[wi])).append(",\"before_function_containing_probe\":").append(entry(beforeContaining[wi])).append(",\"instructions\":[");
            while(count<8&&cursor.compareTo(end)<0){
                monitor.checkCancelled();AddressSet remaining=new AddressSet(cursor,end.subtract(1));DisassembleCommand command=new DisassembleCommand(cursor,remaining,false);command.enableCodeAnalysis(false);command.applyTo(currentProgram,monitor);
                Instruction in=currentProgram.getListing().getInstructionAt(cursor);if(in==null){stop="decode_gap";break;}int length=in.getLength();if(length<1||length>15)throw new IllegalStateException();if(cursor.add(length).compareTo(end)>0){stop="next_instruction_crosses_window";break;}if(count==0&&(length!=7||!in.getMnemonicString().equalsIgnoreCase("LEA")))throw new IllegalStateException();String mnemonic=in.getMnemonicString();if(!mnemonic.matches("[A-Za-z0-9_.]{1,32}"))throw new IllegalStateException();if(in.getNumOperands()<0||in.getNumOperands()>8)throw new IllegalStateException();byte[] bytes=in.getBytes();if(bytes.length!=length)throw new IllegalStateException();
                if(count>0)json.append(',');json.append("{\"address\":").append(location(cursor)).append(",\"length\":").append(length).append(",\"flow_type_id\":").append(Byte.toUnsignedInt(in.getFlowType().getValue())).append(",\"pure_fallthrough\":").append(in.getFlowType().isFallthrough()).append(",\"mnemonic\":\"").append(mnemonic).append("\",\"bytes_sha256\":\"").append(hash(bytes)).append("\",\"operand_objects\":[");int objectCount=0;
                for(int oi=0;oi<in.getNumOperands();oi++){Object[] objects=in.getOpObjects(oi);if(objects==null)throw new IllegalStateException();for(int j=0;j<objects.length;j++){if(++objectCount>16)throw new IllegalStateException();if(objectCount>1)json.append(',');json.append("{\"operand_index\":").append(oi).append(",\"object_index\":").append(j).append(",\"object\":").append(operand(objects[j])).append('}');}}
                json.append("],\"pcode\":[");PcodeOp[] ops=in.getPcode(false);if(ops==null||ops.length>64)throw new IllegalStateException();for(int pi=0;pi<ops.length;pi++){PcodeOp op=ops[pi];if(op==null||!op.getSeqnum().getTarget().equals(cursor)||op.getNumInputs()<0||op.getNumInputs()+(op.getOutput()==null?0:1)>16)throw new IllegalStateException();if(pi>0)json.append(',');json.append("{\"index\":").append(pi).append(",\"origin\":").append(location(op.getSeqnum().getTarget())).append(",\"sequence_time\":").append(op.getSeqnum().getTime()).append(",\"opcode\":").append(op.getOpcode()).append(",\"output\":").append(varnode(op.getOutput())).append(",\"inputs\":[");for(int ni=0;ni<op.getNumInputs();ni++){if(ni>0)json.append(',');json.append(varnode(op.getInput(ni)));}json.append("]}");}
                json.append("]}");count++;cursor=cursor.add(length);if(!in.getFlowType().isFallthrough()){stop="non_fallthrough_control";break;}if(cursor.equals(end))stop="window_end";
                if(json.length()>MAX_OUTPUT)throw new IllegalStateException();
            }
            if(count==0)throw new IllegalStateException();json.append("],\"instruction_count\":").append(count).append(",\"stop_reason\":\"").append(stop).append("\",\"caller_confirmed\":false,\"owner_confirmed\":false}");
        }
        json.append("]}\n");byte[] payload=json.toString().getBytes(StandardCharsets.UTF_8);if(payload.length>MAX_OUTPUT||!SHA.equals(currentProgram.getExecutableSHA256()))throw new IllegalStateException();String manifest="key\tvalue\nprogram_sha256\t"+SHA+"\npayload_bytes\t"+payload.length+"\npayload_sha256\t"+hash(payload)+"\nselected_windows\t2\noutput_limit\t65536\ndecompilation_requested\tfalse\nfunction_creation_requested\tfalse\nanalysis_requested\tfalse\nflow_following_requested\tfalse\nowner_confirmed\tfalse\ncomplete\ttrue\n";byte[] mb=manifest.getBytes(StandardCharsets.UTF_8);if(payload.length+mb.length>MAX_OUTPUT)throw new IllegalStateException();Files.createDirectory(output);Files.write(output.resolve("instructions.json"),payload,StandardOpenOption.CREATE_NEW,StandardOpenOption.WRITE);Files.write(output.resolve("manifest.tsv"),mb,StandardOpenOption.CREATE_NEW,StandardOpenOption.WRITE);println("Bound instruction operand export completed; windows: 2");
    }
}
