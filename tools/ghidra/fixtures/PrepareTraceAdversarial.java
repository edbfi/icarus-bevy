// Authored-fixture-only setup; never use with original programs.
// Args: EXACT_ELF SHA256 then name:hex_start:hex_end_exclusive for every
// FUNCTION_NAMES entry, then override jump and fallthrough hex addresses.
// The runner obtains all locations from nm -S, not a linker address assumption.
// @category IcarusResearch

import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.listing.Function;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.symbol.RefType;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.SourceType;
import ghidra.program.model.symbol.Symbol;
import java.math.BigInteger;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.MessageDigest;
import java.util.HexFormat;
import java.util.LinkedHashMap;
import java.util.Map;

public class PrepareTraceAdversarial extends GhidraScript {
    private static final String[] FUNCTION_NAMES = {
        "authored_good_gate", "authored_xbegin_gate", "authored_override_gate",
        "authored_inline_caller", "authored_inline_callee", "authored_mode_gate",
        "authored_many_instructions", "authored_straddle", "authored_many_pcode",
        "authored_undefined_override"
    };

    @Override
    public AnalysisMode getScriptAnalysisMode() {
        return AnalysisMode.DISABLED;
    }

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != FUNCTION_NAMES.length + 4 || currentProgram == null ||
                !currentProgram.getLanguageID().toString().equals("x86:LE:64:default")) {
            throw new IllegalArgumentException("Expected exact authored ELF, hash, symbol ranges and two markers");
        }
        Path fixture = Path.of(args[0]);
        if (!fixture.isAbsolute() || !fixture.equals(fixture.toRealPath()) ||
                !fixture.getFileName().toString().equals("trace-adversarial.elf") ||
                !fixture.equals(Path.of(currentProgram.getExecutablePath()).toRealPath()) ||
                !args[1].matches("[0-9a-f]{64}") ||
                !args[1].equals(HexFormat.of().formatHex(MessageDigest.getInstance("SHA-256")
                    .digest(Files.readAllBytes(fixture))))) {
            throw new IllegalArgumentException("Authored fixture path/hash mismatch");
        }
        Map<String, Address[]> ranges = new LinkedHashMap<>();
        AddressSet all = new AddressSet();
        for (int i = 0; i < FUNCTION_NAMES.length; i++) {
            String name = FUNCTION_NAMES[i];
            String[] parts = args[i + 2].split(":", -1);
            if (parts.length != 3 || !parts[0].equals(name)) {
                throw new IllegalArgumentException("Expected symbol range: " + name);
            }
            Address start = address(parts[1]);
            Address end = address(parts[2]);
            requireSymbol(name, start);
            requireSymbol(name + "_end", end);
            if (start.compareTo(end) >= 0 || end.subtract(start) > 65536) {
                throw new IllegalArgumentException("Invalid fixture bounds: " + name);
            }
            AddressSet span = new AddressSet(start, end.subtract(1));
            MemoryBlock block = currentProgram.getMemory().getBlock(start);
            if (block == null || !block.isExecute() || !block.isInitialized() || !block.isLoaded() ||
                    !block.contains(end.subtract(1)) || all.intersects(span)) {
                throw new IllegalArgumentException("Fixture ranges overlap or leave executable memory");
            }
            all.add(span);
            ranges.put(name, new Address[]{start, end});
        }
        Address jump = address(args[args.length - 2]);
        Address fallthrough = address(args[args.length - 1]);
        requireSymbol("authored_override_gate_jump", jump);
        requireSymbol("authored_override_gate_fallthrough", fallthrough);
        Address[] overrideRange = ranges.get("authored_override_gate");
        if (jump.compareTo(overrideRange[0]) <= 0 || fallthrough.compareTo(jump) <= 0 ||
                fallthrough.compareTo(overrideRange[1]) >= 0) {
            throw new IllegalArgumentException("Override markers must be inside the override function");
        }
        Address[] mode = ranges.get("authored_mode_gate");
        Address[] callee = ranges.get("authored_inline_callee");
        requireBytes(mode[0], mode[1], "4889e5c3");
        requireBytes(callee[0], callee[1], "b807000000c3");

        // All bindings above are checked before making any database changes.
        for (String name : new String[]{"longMode", "addrsize", "opsize"}) {
            if (currentProgram.getRegister(name) == null) {
                throw new IllegalArgumentException("Fixture language lacks required register: " + name);
            }
            currentProgram.getProgramContext().setValue(currentProgram.getRegister(name),
                mode[0], mode[1].subtract(1), BigInteger.valueOf(name.equals("longMode") ? 0 : 1));
        }
        disassembleSpan(overrideRange[0], overrideRange[1].subtract(1));
        if (currentProgram.getListing().getInstructionAt(jump) == null ||
                !fallthrough.equals(currentProgram.getListing().getInstructionAt(jump).getDefaultFallThrough())) {
            throw new IllegalStateException("Expected conditional jump at authored override marker");
        }
        Reference override = currentProgram.getReferenceManager().addMemoryReference(jump,
            fallthrough, RefType.JUMP_OVERRIDE_UNCONDITIONAL, SourceType.USER_DEFINED, -1);
        currentProgram.getReferenceManager().setPrimary(override, true);
        disassembleSpan(callee[0], callee[1].subtract(1));
        AddressSet body = new AddressSet(callee[0], callee[1].subtract(1));
        Function function = currentProgram.getFunctionManager().getFunctionAt(callee[0]);
        if (function == null) {
            function = currentProgram.getFunctionManager().createFunction("authored_inline_callee",
                callee[0], body, SourceType.USER_DEFINED);
        } else {
            function.setBody(body);
        }
        if (function == null) {
            throw new IllegalStateException("Could not create authored inline callee");
        }
        function.setInline(true);
        Address[] undefined = ranges.get("authored_undefined_override");
        requireBytes(undefined[0], undefined[1], "c3");
        if (currentProgram.getListing().getInstructionAt(undefined[0]) != null) {
            throw new IllegalStateException("Undefined override fixture was unexpectedly disassembled");
        }
        Reference undefinedOverride = currentProgram.getReferenceManager().addMemoryReference(
            undefined[0], undefined[0], RefType.JUMP_OVERRIDE_UNCONDITIONAL, SourceType.USER_DEFINED, -1);
        currentProgram.getReferenceManager().setPrimary(undefinedOverride, true);
        println("TRACE_ADVERSARIAL_PREPARED " + args[1]);
    }

    private Address address(String hex) {
        if (!hex.matches("[0-9a-fA-F]{1,16}")) {
            throw new IllegalArgumentException("Expected unsigned hexadecimal fixture address");
        }
        long offset = Long.parseUnsignedLong(hex, 16);
        Address address = currentProgram.getAddressFactory().getDefaultAddressSpace().getAddress(offset);
        if (address.getOffset() != offset) {
            throw new IllegalArgumentException("Fixture address is not representable");
        }
        return address;
    }

    private void requireSymbol(String name, Address expected) {
        java.util.List<Symbol> symbols = currentProgram.getSymbolTable().getGlobalSymbols(name);
        if (symbols.size() != 1 || !symbols.get(0).getAddress().equals(expected)) {
            throw new IllegalArgumentException("Authored symbol/address mismatch: " + name);
        }
    }

    private void requireBytes(Address start, Address end, String expected) throws Exception {
        byte[] bytes = new byte[(int) end.subtract(start)];
        if (currentProgram.getMemory().getBytes(start, bytes) != bytes.length ||
                !HexFormat.of().formatHex(bytes).equals(expected)) {
            throw new IllegalArgumentException("Unexpected authored bytes at " + start);
        }
    }

    private void disassembleSpan(Address start, Address end) throws Exception {
        DisassembleCommand command = new DisassembleCommand(start, new AddressSet(start, end), true);
        command.enableCodeAnalysis(false);
        if (!command.applyTo(currentProgram, monitor)) {
            throw new IllegalStateException("Authored preparation failed: " + command.getStatusMsg());
        }
    }
}
