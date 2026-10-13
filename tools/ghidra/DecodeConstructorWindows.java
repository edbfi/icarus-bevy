// Independently authored bounded x86-64 Sleigh decoder; no Program/DB/import.
import ghidra.GhidraApplicationLayout;
import ghidra.framework.Application;
import ghidra.framework.ApplicationConfiguration;
import ghidra.app.util.PseudoInstruction;
import ghidra.program.model.address.Address;
import ghidra.program.model.lang.Language;
import ghidra.program.model.lang.LanguageID;
import ghidra.program.model.lang.InstructionPrototype;
import ghidra.program.model.lang.ProcessorContextImpl;
import ghidra.program.model.lang.Register;
import ghidra.program.model.lang.RegisterValue;
import ghidra.program.model.listing.DefaultProgramContext;
import ghidra.program.model.mem.ByteMemBufferImpl;
import ghidra.program.model.pcode.PcodeOp;
import ghidra.program.model.pcode.Varnode;
import ghidra.program.model.scalar.Scalar;
import ghidra.program.model.symbol.FlowType;
import ghidra.program.util.DefaultLanguageService;
import java.io.File;
import java.nio.ByteBuffer;
import java.nio.channels.FileChannel;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.nio.file.attribute.BasicFileAttributes;
import java.security.MessageDigest;
import java.util.Arrays;
import java.util.HexFormat;

public final class DecodeConstructorWindows {
    public static final int MAX_OUTPUT=65536, ATTEMPTS=16;
    private static final long[] START={0x143ca5a40L,0x143a58650L};
    private static final int[] OUTPUT={8,57}, LOOKAHEAD={0,14}, SPAN={8,71};
    private static final String[] ID={"TARGET01","TARGET02"};
    private static String hex(long n){return "0x"+Long.toUnsignedString(n,16);}
    private static String hash(byte[] b)throws Exception{return HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256").digest(b));}
    private static String location(Address a){return "{\"space\":"+a.getAddressSpace().getSpaceID()+",\"offset\":\""+hex(a.getOffset())+"\"}";}
    private static String spaces(Language l){var f=l.getAddressFactory();var ram=f.getDefaultAddressSpace();var constant=f.getConstantSpace();var register=f.getRegisterSpace();var unique=f.getUniqueSpace();if(ram.getType()!=1||constant.getType()!=0||register.getType()!=4||unique.getType()!=3)throw new IllegalStateException();return "{\"address\":"+ram.getSpaceID()+",\"constant\":"+constant.getSpaceID()+",\"register\":"+register.getSpaceID()+",\"unique\":"+unique.getSpaceID()+"}";}
    private static String varnode(Varnode n){if(n==null)return "null";if(n.getSize()<1||n.getSize()>16)throw new IllegalStateException();String kind=n.isConstant()?"constant":n.isRegister()?"register":n.isUnique()?"unique":n.isAddress()?"address":null;if(kind==null)throw new IllegalStateException();return "{\"kind\":\""+kind+"\",\"space\":"+n.getAddress().getAddressSpace().getSpaceID()+",\"offset\":\""+hex(n.getOffset())+"\",\"width\":"+n.getSize()+"}";}
    private static String operand(Object o){if(o instanceof Register r){if(r.getBitLength()<1||r.getBitLength()>128)throw new IllegalStateException();return "{\"kind\":\"register\",\"location\":"+location(r.getAddress())+",\"bit_width\":"+r.getBitLength()+"}";}if(o instanceof Scalar s){if(s.bitLength()<1||s.bitLength()>64)throw new IllegalStateException();return "{\"kind\":\"scalar\",\"bit_width\":"+s.bitLength()+",\"unsigned\":\""+hex(s.getUnsignedValue())+"\",\"signed\":\""+s.getSignedValue()+"\"}";}if(o instanceof Address a)return "{\"kind\":\"address\",\"location\":"+location(a)+"}";throw new IllegalStateException();}
    // Only processor-context defaults from the installed x86-64 language are
    // applied. Ordinary register values/ABI/receiver provenance remain unknown.
    private static ProcessorContextImpl context(Language language,Address at){
        ProcessorContextImpl context=new ProcessorContextImpl(language);
        language.applyContextSettings(new DefaultProgramContext(){
            public void setDefaultValue(RegisterValue value,Address first,Address last){if(value.getRegister().isProcessorContext()&&at.compareTo(first)>=0&&at.compareTo(last)<=0)context.setRegisterValue(value);}
            public RegisterValue getDefaultValue(Register register,Address address){throw new UnsupportedOperationException();}
        });
        for(String name:new String[]{"addrsize","opsize","rexprefix","longMode"}){Register r=language.getRegister(name);if(r==null||!r.isProcessorContext()||context.getValue(r,false)==null)throw new IllegalStateException();}
        if(context.getValue(language.getRegister("addrsize"),false).intValueExact()!=2||context.getValue(language.getRegister("opsize"),false).intValueExact()!=1||context.getValue(language.getRegister("rexprefix"),false).intValueExact()!=0||context.getValue(language.getRegister("longMode"),false).intValueExact()!=1)throw new IllegalStateException();
        return context;
    }
    public static String decode(Language language,byte[][] windows)throws Exception{
        if(language==null||!language.getLanguageID().toString().equals("x86:LE:64:default")||language.isBigEndian()||windows==null||windows.length!=2)throw new IllegalArgumentException();
        for(int i=0;i<2;i++)if(windows[i]==null||windows[i].length!=SPAN[i])throw new IllegalArgumentException();
        StringBuilder json=new StringBuilder("{\"profile\":\"minimal_x64_constructor_windows\",\"language_id\":\"x86:LE:64:default\",\"program_created\":false,\"pe_imported\":false,\"analysis_requested\":false,\"function_creation_requested\":false,\"decompilation_requested\":false,\"flow_following_requested\":false,\"abi_receiver_assumed\":false,\"older_owner_confirmed\":false,\"pdata_may_be_fragment\":true,\"decoder_internal_zero_padding_possible\":true,\"accepted_instruction_bytes_within_input\":true,\"operand_representation_complete\":false,\"space_inventory\":"+spaces(language)+",\"instruction_limit\":16,\"operand_object_limit\":16,\"pcode_limit\":64,\"varnode_limit\":16,\"windows\":[");
        for(int wi=0;wi<2;wi++){
            if(wi>0)json.append(',');Address start=language.getAddressFactory().getDefaultAddressSpace().getAddress(START[wi]);int pos=0,count=0,attempts=0;String stop="instruction_limit";
            json.append("{\"id\":\"").append(ID[wi]).append("\",\"start\":").append(location(start)).append(",\"output_bytes\":").append(OUTPUT[wi]).append(",\"lookahead_bytes\":").append(LOOKAHEAD[wi]).append(",\"input_bytes\":").append(SPAN[wi]).append(",\"input_sha256\":\"").append(hash(windows[wi])).append("\",\"instructions\":[");
            while(attempts<ATTEMPTS&&pos<OUTPUT[wi]){
                Address at=start.add(pos);byte[] remaining=Arrays.copyOfRange(windows[wi],pos,SPAN[wi]);ByteMemBufferImpl buffer=new ByteMemBufferImpl(at,remaining,false);ProcessorContextImpl ctx=context(language,at);InstructionPrototype proto;attempts++;
                try{proto=language.parse(buffer,ctx,false);}catch(ghidra.program.model.lang.InsufficientBytesException|ghidra.program.model.lang.UnknownInstructionException error){stop="undecodable";break;}
                if(proto.hasDelaySlots()||proto.hasNext2Dependency()||proto.hasCrossBuildDependency())throw new IllegalStateException();int length=proto.getLength();if(length<1||length>15)throw new IllegalStateException();if(pos+length>OUTPUT[wi]){stop="instruction_crosses_output_window";break;}
                PseudoInstruction in=new PseudoInstruction(language.getAddressFactory(),at,proto,buffer,ctx);String mnemonic=in.getMnemonicString();if(mnemonic.equals("UNKNOWN")||!mnemonic.matches("[A-Za-z0-9_.]{1,32}"))throw new IllegalStateException();byte[] raw=in.getBytes();if(raw.length!=length||!Arrays.equals(raw,Arrays.copyOfRange(windows[wi],pos,pos+length)))throw new IllegalStateException();int operands=in.getNumOperands();if(operands<0||operands>8)throw new IllegalStateException();FlowType flow=in.getFlowType();
                if(count>0)json.append(',');json.append("{\"address\":").append(location(at)).append(",\"length\":").append(length).append(",\"mnemonic\":\"").append(mnemonic).append("\",\"bytes_sha256\":\"").append(hash(raw)).append("\",\"flow_type_id\":").append(Byte.toUnsignedInt(flow.getValue())).append(",\"call\":").append(flow.isCall()).append(",\"terminal\":").append(flow.isTerminal()).append(",\"pure_fallthrough\":").append(flow.isFallthrough()).append(",\"operand_objects\":[");int objects=0;
                for(int oi=0;oi<operands;oi++){Object[] values=in.getOpObjects(oi);if(values==null)throw new IllegalStateException();for(int j=0;j<values.length;j++){if(++objects>16)throw new IllegalStateException();if(objects>1)json.append(',');json.append("{\"operand_index\":").append(oi).append(",\"object_index\":").append(j).append(",\"object\":").append(operand(values[j])).append('}');}}
                json.append("],\"pcode\":[");PcodeOp[] ops=in.getPcode(false);if(ops==null||ops.length>64)throw new IllegalStateException();for(int pi=0;pi<ops.length;pi++){PcodeOp op=ops[pi];if(op==null||op.getOpcode()<=PcodeOp.UNIMPLEMENTED||op.getOpcode()>=PcodeOp.PCODE_MAX||!op.getSeqnum().getTarget().equals(at)||op.getNumInputs()<0||op.getNumInputs()+(op.getOutput()==null?0:1)>16)throw new IllegalStateException();if(pi>0)json.append(',');json.append("{\"index\":").append(pi).append(",\"origin\":").append(location(op.getSeqnum().getTarget())).append(",\"sequence_time\":").append(op.getSeqnum().getTime()).append(",\"opcode\":").append(op.getOpcode()).append(",\"output\":").append(varnode(op.getOutput())).append(",\"inputs\":[");for(int ni=0;ni<op.getNumInputs();ni++){if(ni>0)json.append(',');json.append(varnode(op.getInput(ni)));}json.append("]}");}
                json.append("]}");count++;pos+=length;if(flow.isCall()){stop="call_barrier";break;}if(flow.isTerminal()){stop="terminal_control";break;}if(!flow.isFallthrough()){stop="non_fallthrough_control";break;}if(pos==OUTPUT[wi])stop="output_window_end";if(json.length()>MAX_OUTPUT)throw new IllegalStateException();
            }
            json.append("],\"instruction_count\":").append(count).append(",\"decode_attempts\":").append(attempts).append(",\"consumed_bytes\":").append(pos).append(",\"stop_reason\":\"").append(stop).append("\",\"receiver_origin_proven\":false}");
        }
        json.append("]}\n");String result=json.toString();if(result.getBytes(StandardCharsets.UTF_8).length>MAX_OUTPUT)throw new IllegalStateException();return result;
    }
    public static Language initialize(String installation)throws Exception{if(Application.isInitialized())throw new IllegalStateException();ApplicationConfiguration configuration=new ApplicationConfiguration();configuration.setInitializeLogging(false);Application.initializeApplication(new GhidraApplicationLayout(new File(installation)),configuration);return DefaultLanguageService.getLanguageService().getLanguage(new LanguageID("x86:LE:64:default"));}
    private static byte[] bounded(Path p,int size,String expected)throws Exception{if(!expected.matches("[a-f0-9]{64}"))throw new IllegalArgumentException();BasicFileAttributes before=Files.readAttributes(p,BasicFileAttributes.class,LinkOption.NOFOLLOW_LINKS);if(!before.isRegularFile()||before.size()!=size)throw new IllegalStateException();try(FileChannel channel=FileChannel.open(p,StandardOpenOption.READ,LinkOption.NOFOLLOW_LINKS)){if(channel.size()!=size)throw new IllegalStateException();ByteBuffer buffer=ByteBuffer.allocate(size);while(buffer.hasRemaining())if(channel.read(buffer)<1)throw new IllegalStateException();BasicFileAttributes after=Files.readAttributes(p,BasicFileAttributes.class,LinkOption.NOFOLLOW_LINKS);if(channel.size()!=size||after.size()!=size||!after.isRegularFile()||!java.util.Objects.equals(before.fileKey(),after.fileKey())||!before.lastModifiedTime().equals(after.lastModifiedTime())||!hash(buffer.array()).equals(expected))throw new IllegalStateException();return buffer.array();}}
    public static void main(String[] args)throws Exception{try{
        if(args.length!=6)throw new IllegalArgumentException();byte[][] windows={bounded(Path.of(args[1]),8,args[2]),bounded(Path.of(args[3]),71,args[4])};Path output=Path.of(args[5]).toAbsolutePath().normalize();if(Files.exists(output))throw new IllegalStateException();Language language=initialize(args[0]);byte[] payload=decode(language,windows).getBytes(StandardCharsets.UTF_8);Files.write(output,payload,StandardOpenOption.CREATE_NEW,StandardOpenOption.WRITE);
    }catch(Exception error){throw new IllegalStateException("Bound minimal constructor-window decode failed");}}
}
