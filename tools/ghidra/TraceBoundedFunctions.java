// Trace explicit qualified ranges in an owned analysis database, without auto-analysis.
// Usage: TraceBoundedFunctions.java NEW_DIRECTORY HEX_START:HEX_END_EXCLUSIVE [...]
// Supports the project's byte-addressed x86:LE:64:default language only. No target
// bytes, names, file hashes or owner assumptions are embedded in this source.
// API reference: NationalSecurityAgency/ghidra, tag Ghidra_12.2_build, notably
// Language.parse, ByteMemBufferImpl, PseudoInstruction/PseudoCodeUnit,
// ListingDB.createInstruction -> CodeManager.addInstruction/addReferencesForInstruction,
// InstructionDB, CodeUnitDB, SleighInstructionPrototype, ProgramContext,
// RegisterValue and x86-64.pspec/ia.sinc. No decompiler is used, ever.
// No external JSON dependency is needed.
// @category IcarusResearch

import ghidra.app.script.GhidraScript;
import ghidra.app.util.PseudoInstruction;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressFactory;
import ghidra.program.model.address.AddressOverflowException;
import ghidra.program.model.address.AddressRange;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.address.AddressSpace;
import ghidra.program.model.lang.InstructionPrototype;
import ghidra.program.model.lang.InsufficientBytesException;
import ghidra.program.model.lang.ParserContext;
import ghidra.program.model.lang.ProcessorContextImpl;
import ghidra.program.model.lang.Register;
import ghidra.program.model.lang.RegisterValue;
import ghidra.program.model.lang.UnknownContextException;
import ghidra.program.model.lang.UnknownInstructionException;
import ghidra.program.model.listing.FlowOverride;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.FunctionManager;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.listing.Listing;
import ghidra.program.model.mem.ByteMemBufferImpl;
import ghidra.program.model.mem.MemBuffer;
import ghidra.program.model.mem.MemoryAccessException;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.pcode.PcodeOp;
import ghidra.program.model.pcode.Varnode;
import ghidra.program.model.symbol.FlowType;
import ghidra.program.model.symbol.Reference;
import java.io.IOException;
import java.math.BigInteger;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashSet;
import java.util.HexFormat;
import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;

public class TraceBoundedFunctions extends GhidraScript {
    private static final int MAX_RANGES = 8;
    private static final int MAX_RANGE_BYTES = 65536;
    private static final int MAX_INSTRUCTIONS = 4096;
    private static final int MAX_PCODE = 16384;
    private static final int MAX_X86_INSTRUCTION_BYTES = 15;
    private static final String DECOMPILATION_STATUS = "disabled_raw_trace_only_no_pseudocode";
    private static final int MAX_OUTPUT_BYTES = 8 * 1024 * 1024;
    // Reserve room for fixed run/failure markers even if staged payload hits its cap.
    private static final int MARKER_RESERVE = 1024;

    /**
     * Detached instruction: no Program, no memory fallback, no cross-instruction parser lookup.
     * PseudoCodeUnit normally caches length+3 bytes and can read Program memory outside that cache.
     * Its detached partial-copy implementation also cannot safely service every bulk request.
     * Route decoder callbacks directly to the original bounded buffer instead of either path.
     */
    private static final class BoundedInstruction extends PseudoInstruction {
        private final ByteMemBufferImpl buffer;
        private final ProcessorContextImpl context;
        private ParserContext parserContext;
        private boolean refusedCrossInstructionContext;

        BoundedInstruction(AddressFactory factory, Address at, InstructionPrototype prototype,
                ByteMemBufferImpl buffer, ProcessorContextImpl context) throws AddressOverflowException {
            super(factory, at, prototype, buffer, context);
            this.buffer = buffer;
            this.context = context;
        }

        @Override
        public MemBuffer getMemBuffer() {
            return buffer;
        }

        @Override
        public byte getByte(int offset) throws MemoryAccessException {
            return buffer.getByte(offset);
        }

        @Override
        public int getBytes(byte[] bytes, int offset) {
            return buffer.getBytes(bytes, offset);
        }

        @Override
        public ParserContext getParserContext() throws MemoryAccessException {
            if (parserContext == null) {
                parserContext = getPrototype().getParserContext(buffer, context);
            }
            return parserContext;
        }

        @Override
        public ParserContext getParserContext(Address requested)
                throws UnknownContextException, MemoryAccessException {
            if (!getAddress().equals(requested)) {
                refusedCrossInstructionContext = true;
                throw new UnknownContextException("Raw trace refuses cross-instruction context: " + requested);
            }
            return getParserContext();
        }
    }

    private static final class OutputLimitException extends IOException {
        OutputLimitException() {
            super("Aggregate UTF-8 output would exceed 8 MiB; export failed, not truncated");
        }
    }

    /** Stage all payload before publishing it: an output-cap failure publishes no payload. */
    private static final class Export {
        final Map<String, StringBuilder> files = new LinkedHashMap<>();
        long bytes;

        void add(String name, String text) throws OutputLimitException {
            int length = text.getBytes(StandardCharsets.UTF_8).length;
            if (length > MAX_OUTPUT_BYTES - MARKER_RESERVE - bytes) {
                throw new OutputLimitException();
            }
            files.computeIfAbsent(name, key -> new StringBuilder()).append(text);
            bytes += length;
        }

        void publish(Path directory) throws IOException {
            for (Map.Entry<String, StringBuilder> file : files.entrySet()) {
                writeNew(directory.resolve(file.getKey()), file.getValue().toString());
            }
        }
    }

    private static final class Range {
        final Address start;
        final Address end;
        final int size;
        final AddressSet selected;
        final AddressSet decoded = new AddressSet();
        final String prefix;
        byte[] original;
        int instructions;
        int pcodeOps;
        int issues;
        int pending;
        boolean flowComplete = true;
        Function imported;
        String binding = "skipped_no_bounded_listing_installation";

        Range(Address start, Address end, int index) {
            this.start = start;
            this.end = end;
            size = (int) end.subtract(start);
            selected = new AddressSet(start, end.subtract(1));
            prefix = String.format("range-%02d", index);
        }

        String file(String suffix) {
            return prefix + suffix;
        }
    }

    @Override
    public AnalysisMode getScriptAnalysisMode() {
        // Keep auto-analysis disabled even though the trace performs no listing/body mutations.
        return AnalysisMode.DISABLED;
    }

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2 || args.length > MAX_RANGES + 1 || args[0].isBlank()) {
            throw new IllegalArgumentException(
                "Expected NEW output directory and 1..8 explicit hex start:end_exclusive ranges");
        }
        if (currentProgram == null ||
                !currentProgram.getLanguageID().toString().equals("x86:LE:64:default")) {
            throw new IllegalArgumentException("Requires an owned x86:LE:64:default analysis DB");
        }
        List<Range> ranges = validateRanges(args);
        // Read every original selected analysis-memory span before decoding.
        for (Range range : ranges) {
            range.original = readMemory(range);
        }
        Path output = Path.of(args[0]).toAbsolutePath().normalize();
        // Atomic refusal of existing paths, including symlinks; no createDirectories/overwrite.
        Files.createDirectory(output);
        try {
            writeNew(output.resolve("RUN.txt"),
                "Require EXPORT_FINISHED.txt and no FAILED.txt before consuming this export.\n" +
                "Export completion is separate from each range's flow/decompilation status.\n");
            Export export = new Export();
            export.add("README.txt", interpretation());
            export.add("status.tsv", "range\tstart\tend_exclusive\tselected_bytes\t" +
                "decoded_instruction_bytes\tunvisited_bytes\tinstructions\tpcode_ops_exported\t" +
                "pending_addresses\tissues\tflow_complete\tbody_binding\tdecompilation\n");
            for (Range range : ranges) {
                monitor.checkCancelled();
                prepareFiles(range, export);
                boolean safe = checkImportedFunctions(range, export);
                if (safe) {
                    walk(range, export);
                }
                exportIntervals(range, export);
            }
            for (Range range : ranges) {
                if (!Arrays.equals(range.original, readMemory(range))) {
                    throw new IllegalStateException("Selected analysis-memory bytes changed during trace");
                }
                export.add("status.tsv", range.prefix + "\t" + range.start + "\t" + range.end +
                    "\t" + range.size + "\t" + range.decoded.getNumAddresses() + "\t" +
                    (range.size - range.decoded.getNumAddresses()) + "\t" + range.instructions +
                    "\t" + range.pcodeOps + "\t" + range.pending + "\t" + range.issues + "\t" +
                    range.flowComplete + "\t" + range.binding + "\t" + DECOMPILATION_STATUS + "\n");
            }
            export.publish(output);
            writeNew(output.resolve("EXPORT_FINISHED.txt"),
                "All requested range statuses exported. Consult status.tsv; partial ranges are retained.\n" +
                "Payload UTF-8 bytes: " + export.bytes + "; total including markers <= 8388608.\n");
        } catch (Exception failure) {
            String message = failure instanceof OutputLimitException
                ? "FAILED: aggregate_output_cap_8388608_bytes; payload not published; no truncation.\n"
                : "FAILED: export aborted; any files are incomplete. Consult the raised exception.\n";
            try {
                writeNew(output.resolve("FAILED.txt"), message);
            } catch (IOException markerFailure) {
                failure.addSuppressed(markerFailure);
            }
            throw failure;
        }
        println("Bounded trace exported; inspect per-range status.tsv (no owner/physical-byte proof)");
    }

    private List<Range> validateRanges(String[] args) throws AddressOverflowException {
        List<Range> result = new ArrayList<>();
        AddressSpace space = currentProgram.getAddressFactory().getDefaultAddressSpace();
        if (!space.isMemorySpace() || space.getAddressableUnitSize() != 1) {
            throw new IllegalArgumentException("Requires a byte-addressed default memory space");
        }
        for (int i = 1; i < args.length; i++) {
            String[] parts = args[i].split(":", -1);
            if (parts.length != 2) {
                throw new IllegalArgumentException("Malformed start:end_exclusive range: " + args[i]);
            }
            Address start = parseHex(space, parts[0]);
            Address end = parseHex(space, parts[1]);
            // Ordered same-space subtraction is bounded by the address space (no wrapped addition).
            if (start.compareTo(end) >= 0 || end.subtract(start) <= 0 ||
                    end.subtract(start) > MAX_RANGE_BYTES) {
                throw new IllegalArgumentException("Range must have 1..65536 bytes: " + args[i]);
            }
            Range range = new Range(start, end, i);
            Address at = start;
            while (at.compareTo(end) < 0) {
                MemoryBlock block = currentProgram.getMemory().getBlock(at);
                if (block == null || !block.isExecute() || !block.isInitialized() || !block.isLoaded()) {
                    throw new IllegalArgumentException(
                        "Every selected byte must be loaded initialized executable memory: " + args[i]);
                }
                at = block.contains(end.subtract(1)) ? end : block.getEnd().addNoWrap(1);
            }
            for (Range prior : result) {
                if (range.selected.intersects(prior.selected)) {
                    throw new IllegalArgumentException("Duplicate or overlapping explicit ranges: " + args[i]);
                }
            }
            result.add(range);
        }
        return result;
    }

    private static Address parseHex(AddressSpace space, String value) {
        if (!value.matches("(?:0[xX])?[0-9a-fA-F]{1,16}")) {
            throw new IllegalArgumentException("Expected unsigned hex address: " + value);
        }
        String digits = value.startsWith("0x") || value.startsWith("0X") ? value.substring(2) : value;
        long offset = Long.parseUnsignedLong(digits, 16);
        Address address = space.getAddress(offset);
        if (address.getOffset() != offset) {
            throw new IllegalArgumentException("Address is not representable: " + value);
        }
        return address;
    }

    private byte[] readMemory(Range range) throws Exception {
        byte[] bytes = new byte[range.size];
        if (currentProgram.getMemory().getBytes(range.start, bytes) != bytes.length) {
            throw new IllegalStateException("Short analysis-memory read at " + range.start);
        }
        return bytes;
    }

    private void prepareFiles(Range range, Export export) throws Exception {
        export.add(range.file(".memory.tsv"), "source\tstart\tend_exclusive\tbytes\tsha256\thex\n" +
            "selected_analysis_memory_not_physical_file\t" + range.start + "\t" + range.end +
            "\t" + range.size + "\t" + HexFormat.of().formatHex(
                MessageDigest.getInstance("SHA-256").digest(range.original)) + "\t" +
            HexFormat.of().formatHex(range.original) + "\n");
        export.add(range.file(".instructions.tsv"), "address\tbytes\tlength\tasm\tflow_type\n");
        export.add(range.file(".context.tsv"),
            "address\tregister\tvalue_hex\tmask_hex\texpected_hex\tstatus\n");
        export.add(range.file(".pcode.tsv"),
            "instruction\tindex\tsequence_address\tsequence_time\topcode\tmnemonic\tinputs\n");
        export.add(range.file(".varnodes.tsv"),
            "instruction\top_index\trole\tinput_index\tspace_id\tspace_name\toffset_hex\tsize_bytes\n");
        export.add(range.file(".targets.tsv"),
            "instruction\top_index\tkind\ttarget_space_id\ttarget_space\ttarget_offset_hex\tsize_bytes\n");
        export.add(range.file(".flows.tsv"), "source\tkind\ttarget\tstatus\n");
        export.add(range.file(".issues.tsv"), "address\treason\tdetail\n");
        export.add(range.file(".body.tsv"), "kind\tstart\tend_exclusive\tbytes\n");
        export.add(range.file(".decompiler.txt"),
            DECOMPILATION_STATUS + "\nDecompilation is permanently disabled in this raw trace helper.\n" +
            "No decompiler is opened or run; no pseudocode or C file is produced, even for complete traces.\n");
    }

    private boolean checkImportedFunctions(Range range, Export export) throws Exception {
        FunctionManager manager = currentProgram.getFunctionManager();
        range.imported = manager.getFunctionAt(range.start);
        if (range.imported != null) {
            // Fail closed instead of silently repairing an imported body that needs new qualification.
            if (!range.selected.contains(range.imported.getBody())) {
                issue(range, export, range.start, "imported_body_outside_selected_range",
                    "Refused imported function; no raw decoding for this range");
                range.binding = "rejected_imported_body";
                return false;
            }
            intervals(range, export, "imported_body_before", new AddressSet(range.imported.getBody()));
            if (range.imported.isThunk() || range.imported.isInline()) {
                issue(range, export, range.start, "imported_thunk_or_inline",
                    "Refused imported special function semantics");
                range.binding = "rejected_imported_special_function";
                return false;
            }
        }
        Iterator<Function> overlaps = manager.getFunctionsOverlapping(range.selected);
        while (overlaps.hasNext()) {
            monitor.checkCancelled();
            Function other = overlaps.next();
            if (!other.getEntryPoint().equals(range.start)) {
                issue(range, export, range.start, "function_conflict", "Other entry: " + other.getEntryPoint());
                range.binding = "rejected_function_conflict";
                return false;
            }
        }
        return true;
    }

    private void walk(Range range, Export export) throws Exception {
        ArrayDeque<Address> pending = new ArrayDeque<>();
        Set<Address> scheduled = new HashSet<>();
        pending.add(range.start);
        scheduled.add(range.start);
        while (!pending.isEmpty()) {
            monitor.checkCancelled();
            if (range.instructions == MAX_INSTRUCTIONS) {
                issue(range, export, pending.peek(), "instruction_cap_4096",
                    "Pending addresses: " + pending.size());
                break;
            }
            Address at = pending.removeFirst();
            BoundedInstruction instruction = decodeBounded(range, at, export);
            if (instruction == null) {
                continue;
            }
            range.instructions++;
            range.decoded.add(instruction.getMinAddress(), instruction.getMaxAddress());
            export.add(range.file(".instructions.tsv"), at + "\t" +
                HexFormat.of().formatHex(instruction.getBytes()) + "\t" + instruction.getLength() +
                "\t" + cell(instruction.toString()) + "\t" + instruction.getFlowType() + "\n");
            PcodeOp[] ops = instruction.getPcode(false);
            if (instruction.refusedCrossInstructionContext) {
                issue(range, export, at, "unsupported_cross_instruction_context",
                    "Decoder callback refused; successors not traversed");
                continue;
            }
            if (ops == null || ops.length > MAX_PCODE - range.pcodeOps) {
                issue(range, export, at, ops == null ? "pcode_unavailable" : "pcode_cap_16384",
                    "Entire instruction pcode omitted; generated ops=" + (ops == null ? "null" : ops.length) +
                    "; exported ops=" + range.pcodeOps + "; cap=16384; successors not traversed");
                break;
            }
            boolean validPcode = exportPcode(range, export, instruction, ops);
            if (!validPcode) {
                continue;
            }
            FlowType flow = instruction.getFlowType();
            Address fallthrough = instruction.getDefaultFallThrough();
            Address[] targets = instruction.getDefaultFlows();
            if (instruction.refusedCrossInstructionContext) {
                issue(range, export, at, "unsupported_cross_instruction_context",
                    "Flow callback refused; successors not traversed");
                continue;
            }
            if (fallthrough != null) {
                edge(range, export, at, "fallthrough", fallthrough, pending, scheduled);
            } else if (flow.hasFallthrough()) {
                issue(range, export, at, "unresolved_fallthrough", "Flow type requires a fallthrough");
            }
            if (flow.isCall()) {
                for (Address target : targets) {
                    export.add(range.file(".flows.tsv"), at + "\tcall\t" + target + "\tnot_followed\n");
                }
                if (targets.length == 0) {
                    export.add(range.file(".flows.tsv"), at + "\tcall\t-\tunresolved_not_followed\n");
                }
            } else if (flow.isJump()) {
                for (Address target : targets) {
                    edge(range, export, at, "jump", target, pending, scheduled);
                }
                if (targets.length == 0 || flow.isComputed()) {
                    issue(range, export, at, "unresolved_non_call_jump",
                        "Computed targets are not inferred from imported references or memory");
                    export.add(range.file(".flows.tsv"), at + "\tjump\t-\tunresolved\n");
                }
            } else if (targets.length != 0) {
                issue(range, export, at, "unsupported_non_call_flow", flow.toString());
            }
            if (flow.isTerminal()) {
                export.add(range.file(".flows.tsv"), at + "\tterminal\t-\treturn_or_terminal_stop\n");
            } else if (!flow.isCall() && !flow.isJump() && fallthrough == null) {
                issue(range, export, at, "unresolved_non_call_stop", flow.toString());
            }
        }
        range.pending = pending.size();
    }

    /** Parse only from the bounded snapshot; do not install through memory-backed DB callbacks. */
    private BoundedInstruction decodeBounded(Range range, Address at, Export export) throws Exception {
        // Detached decoding must preserve the overlap refusal formerly enforced by installation.
        if (range.decoded.contains(at)) {
            issue(range, export, at, "observed_offcut_instruction",
                "Reached address is already inside another observed instruction");
            return null;
        }
        Listing listing = currentProgram.getListing();
        Instruction existing = listing.getInstructionContaining(at);
        if (existing != null && (!existing.getAddress().equals(at) ||
                existing.getParsedLength() > range.end.subtract(at))) {
            issue(range, export, at, "existing_straddling_or_offcut_instruction",
                "Existing instruction starts at " + existing.getAddress() + ", parsed length=" + existing.getParsedLength());
            return null;
        }
        int offset = (int) at.subtract(range.start);
        byte[] bytes = Arrays.copyOfRange(range.original, offset,
            Math.min(range.size, offset + MAX_X86_INSTRUCTION_BYTES));
        ByteMemBufferImpl buffer = new ByteMemBufferImpl(at, bytes, false);
        ProcessorContextImpl context = new ProcessorContextImpl(currentProgram.getLanguage());
        // Only processor context may enter raw decoding. Imported tracked ZF/general register
        // values, prototypes and calling-convention injections are never supplied to this context.
        RegisterValue value = currentProgram.getProgramContext().getDisassemblyContext(at);
        if (value != null) {
            context.setRegisterValue(value);
        }
        if (!canonicalContext(range, export, at, context)) {
            return null;
        }
        InstructionPrototype prototype;
        try {
            prototype = currentProgram.getLanguage().parse(buffer, context, false);
        } catch (InsufficientBytesException failure) {
            issue(range, export, at, "decode_insufficient_bytes_possible_straddling",
                "Bounded decoder has " + bytes.length + " bytes; no outside lookahead permitted");
            return null;
        } catch (UnknownInstructionException failure) {
            issue(range, export, at, "decode_error", failure.toString());
            return null;
        }
        int length = prototype.getLength();
        if (length < 1 || length > bytes.length || length > range.end.subtract(at)) {
            issue(range, export, at, "straddling_instruction", "Decoded length=" + length);
            return null;
        }
        if (prototype.getDelaySlotByteCount() != 0 || prototype.hasNext2Dependency()) {
            issue(range, export, at, "unsupported_decoder_dependency", "Delay slots or next2 dependency");
            return null;
        }
        BoundedInstruction raw = new BoundedInstruction(currentProgram.getAddressFactory(),
            at, prototype, buffer, context);
        if (!checkInstructionReferences(range, export, at, raw)) {
            return null;
        }
        Address last = at.addNoWrap(length - 1);
        if (range.decoded.intersects(new AddressSet(at, last))) {
            issue(range, export, at, "observed_instruction_overlap",
                "Bounded decode overlaps previously observed instruction bytes");
            return null;
        }
        for (int i = 0; i < length; i++) {
            Address part = at.addNoWrap(i);
            Instruction overlapping = listing.getInstructionContaining(part);
            if ((overlapping != null && !overlapping.getAddress().equals(at)) ||
                    listing.getDefinedDataContaining(part) != null) {
                issue(range, export, at, "listing_conflict", "Existing code/data at " + part);
                return null;
            }
        }
        if (existing != null && (existing.isLengthOverridden() || existing.isFallThroughOverridden() ||
                existing.getFlowOverride() != FlowOverride.NONE || !existing.getPrototype().equals(prototype))) {
            issue(range, export, at, "existing_instruction_override_or_prototype_conflict",
                "Imported instruction is not equivalent to the bounded raw decode");
            return null;
        }
        if (existing != null && (existing.getParsedLength() != length ||
                !existing.getMaxAddress().equals(last))) {
            issue(range, export, at, "existing_instruction_mismatch", "Raw bounded decode differs from listing");
            return null;
        }
        if (raw.refusedCrossInstructionContext) {
            issue(range, export, at, "unsupported_cross_instruction_context", "Reference-check callback refused");
            return null;
        }
        // Do not call Listing.createInstruction even with this bounded buffer: in Ghidra 12.2,
        // ListingDB -> CodeManager.createCodeUnit -> addInstruction -> addReferencesForInstruction
        // runs prototype callbacks against InstructionDB, whose CodeUnitDB buffer falls back to
        // Program memory. The supplied buffer controls prototype storage, not those later reads.
        // Existing instructions supply metadata/reference conflict checks only. In particular,
        // InstructionDB.getPrototype and SleighInstructionPrototype.equals are metadata-only;
        // never request their parser context, pcode, operand rendering or memory-backed byte cache.
        // No function body is created/rebound when the decoded instructions are detached.
        return raw;
    }

    /** Compare with the x86-64.pspec entry context, without assigning it to the decoder/DB. */
    private boolean canonicalContext(Range range, Export export, Address at,
            ProcessorContextImpl context) throws Exception {
        Register base = currentProgram.getLanguage().getContextBaseRegister();
        RegisterValue effective = context.getRegisterValue(base);
        RegisterValue expected = new RegisterValue(base, BigInteger.ZERO);
        String[] names = {"longMode", "addrsize", "opsize", "rexprefix"};
        int[] values = {1, 2, 1, 0};
        boolean valid = effective != null;
        for (int i = 0; i < names.length; i++) {
            Register register = context.getRegister(names[i]);
            RegisterValue value = register == null ? null : context.getRegisterValue(register);
            BigInteger required = BigInteger.valueOf(values[i]);
            boolean matches = value != null && required.equals(value.getUnsignedValue());
            valid &= matches;
            export.add(range.file(".context.tsv"), at + "\t" + names[i] + "\t" +
                (value == null ? "-" : "0x" + value.getUnsignedValueIgnoreMask().toString(16)) + "\t" +
                (value == null ? "-" : "0x" + value.getValueMask().toString(16)) + "\t0x" +
                required.toString(16) + "\t" + (matches ? "canonical" : "rejected_unknown_or_noncanonical") + "\n");
            if (register != null) {
                expected = expected.combineValues(new RegisterValue(register, required));
            }
        }
        // Other known context bits must also be zero at instruction entry (no injected prefixes).
        // Unspecified bits remain unspecified; Sleigh's zero fallback is not written into context.
        boolean knownBitsMatch = effective != null && effective.getUnsignedValueIgnoreMask()
            .xor(expected.getUnsignedValueIgnoreMask()).and(effective.getValueMask()).signum() == 0;
        valid &= knownBitsMatch;
        export.add(range.file(".context.tsv"), at + "\t" + cell(base.getName()) + "\t" +
            (effective == null ? "-" : "0x" + effective.getUnsignedValueIgnoreMask().toString(16)) + "\t" +
            (effective == null ? "-" : "0x" + effective.getValueMask().toString(16)) + "\t0x" +
            expected.getUnsignedValueIgnoreMask().toString(16) + "\t" +
            (valid ? "canonical" : "rejected_unknown_or_noncanonical") + "\n");
        if (!valid) {
            issue(range, export, at, "noncanonical_or_unknown_x86_64_context",
                "Requires longMode=1, addrsize=2, opsize=1, rexprefix=0 and no other nonzero entry bits; see context.tsv");
        }
        return valid;
    }

    /** InstructionPcodeOverride honors reference overrides independently of FlowOverride. */
    private boolean checkInstructionReferences(Range range, Export export, Address at,
            Instruction raw) throws Exception {
        for (Reference reference : currentProgram.getReferenceManager().getReferencesFrom(at)) {
            monitor.checkCancelled();
            if (reference.getReferenceType().isOverride()) {
                issue(range, export, at, "instruction_reference_override",
                    reference.getReferenceType() + "; primary=" + reference.isPrimary() +
                    "; target=" + reference.getToAddress());
                return false;
            }
            if (reference.isPrimary() && reference.getReferenceType().isCall() &&
                    (!raw.getFlowType().isCall() || raw.getFlowType().isComputed() ||
                    !Arrays.asList(raw.getDefaultFlows()).contains(reference.getToAddress()))) {
                issue(range, export, at, "changed_or_unaudited_primary_call_reference",
                    "Primary call reference does not equal a raw direct-call destination: " + reference.getToAddress());
                return false;
            }
        }
        return true;
    }

    private void edge(Range range, Export export, Address source, String kind, Address target,
            ArrayDeque<Address> pending, Set<Address> scheduled) throws Exception {
        String state;
        if (!range.selected.contains(target)) {
            state = "escaping";
            issue(range, export, source, "escaping_non_call_" + kind, "Target: " + target);
        } else if (scheduled.add(target)) {
            pending.addLast(target);
            state = "scheduled";
        } else {
            state = "already_scheduled";
        }
        export.add(range.file(".flows.tsv"), source + "\t" + kind + "\t" + target + "\t" + state + "\n");
    }

    private boolean exportPcode(Range range, Export export, Instruction instruction, PcodeOp[] ops)
            throws Exception {
        boolean valid = true;
        Address at = instruction.getAddress();
        for (int i = 0; i < ops.length; i++) {
            PcodeOp op = ops[i];
            if (op == null || op.getSeqnum() == null || !op.getSeqnum().getTarget().equals(at) ||
                    op.getOpcode() < PcodeOp.UNIMPLEMENTED || op.getOpcode() >= PcodeOp.PCODE_MAX) {
                issue(range, export, at, "invalid_raw_pcode", "Op index=" + i +
                    "; remaining raw pcode not exported; successors not traversed");
                return false;
            }
            range.pcodeOps++;
            export.add(range.file(".pcode.tsv"), at + "\t" + i + "\t" + op.getSeqnum().getTarget() +
                "\t" + op.getSeqnum().getTime() + "\t" + op.getOpcode() + "\t" + op.getMnemonic() +
                "\t" + op.getNumInputs() + "\n");
            varnode(range, export, at, i, "output", -1, op.getOutput());
            for (int j = 0; j < op.getNumInputs(); j++) {
                varnode(range, export, at, i, "input", j, op.getInput(j));
            }
            int opcode = op.getOpcode();
            if (opcode == PcodeOp.UNIMPLEMENTED) {
                issue(range, export, at, "unimplemented_pcode", "Op index=" + i);
                valid = false;
            }
            if (opcode == PcodeOp.CALLOTHER) {
                issue(range, export, at, "unaudited_callother_unresolved",
                    "Op index=" + i + "; userop may hide escaping native control flow; successors not traversed");
                valid = false;
            }
            if (opcode == PcodeOp.BRANCH || opcode == PcodeOp.CBRANCH || opcode == PcodeOp.CALL ||
                    opcode == PcodeOp.BRANCHIND || opcode == PcodeOp.CALLIND || opcode == PcodeOp.RETURN) {
                Varnode target = op.getNumInputs() == 0 ? null : op.getInput(0);
                String kind = op.getMnemonic();
                if ((opcode == PcodeOp.BRANCH || opcode == PcodeOp.CBRANCH) && target != null && target.isConstant()) {
                    kind += "_relative_pcode_op_offset_not_native_address";
                } else if (opcode == PcodeOp.BRANCHIND || opcode == PcodeOp.CALLIND || opcode == PcodeOp.RETURN) {
                    kind += "_target_expression_not_resolved_address";
                }
                export.add(range.file(".targets.tsv"), at + "\t" + i + "\t" + kind + "\t" + location(target) + "\n");
                if (opcode == PcodeOp.BRANCHIND) {
                    issue(range, export, at, "raw_pcode_unresolved_non_call_branch", "Op index=" + i);
                }
            }
        }
        return valid;
    }

    private void varnode(Range range, Export export, Address at, int index, String role,
            int input, Varnode node) throws Exception {
        export.add(range.file(".varnodes.tsv"), at + "\t" + index + "\t" + role + "\t" + input +
            "\t" + location(node) + "\n");
    }

    private static String location(Varnode node) {
        if (node == null) {
            return "-\t-\t-\t0";
        }
        AddressSpace space = node.getAddress().getAddressSpace();
        return space.getSpaceID() + "\t" + cell(space.getName()) + "\t0x" +
            Long.toUnsignedString(node.getOffset(), 16) + "\t" + node.getSize();
    }

    private void exportIntervals(Range range, Export export) throws Exception {
        intervals(range, export, "observed_decoded_instruction_bytes", range.decoded);
        intervals(range, export, "unvisited_not_classified_as_code_or_data", range.selected.subtract(range.decoded));
    }

    private void intervals(Range range, Export export, String kind, AddressSet addresses) throws Exception {
        for (AddressRange interval : addresses.getAddressRanges()) {
            monitor.checkCancelled();
            export.add(range.file(".body.tsv"), kind + "\t" + interval.getMinAddress() + "\t" +
                interval.getMaxAddress().addNoWrap(1) + "\t" + interval.getLength() + "\n");
        }
    }

    private void issue(Range range, Export export, Address at, String reason, String detail) throws Exception {
        range.flowComplete = false;
        range.issues++;
        export.add(range.file(".issues.tsv"), at + "\t" + reason + "\t" + cell(detail) + "\n");
    }

    /** Reversible TSV escaping, never silent removal/truncation of diagnostics or assembly. */
    private static String cell(String value) {
        return value == null ? "" : value.replace("\\", "\\\\").replace("\t", "\\t")
            .replace("\r", "\\r").replace("\n", "\\n");
    }

    private static void writeNew(Path path, String text) throws IOException {
        Files.writeString(path, text, StandardCharsets.UTF_8, StandardOpenOption.CREATE_NEW,
            StandardOpenOption.WRITE);
    }

    private static String interpretation() {
        return "Explicit selected ranges use hexadecimal default-space byte addresses, end exclusive.\n" +
            "Limits: 8 ranges; 65536 bytes/range; 4096 instructions/range; 16384 raw pcode ops/range; " +
            "8388608 aggregate UTF-8 output bytes including markers.\n" +
            "This script supports only x86:LE:64:default; no delay slots or next2 dependencies.\n" +
            "Before every bounded parse, effective disassembly context is checked against canonical " +
            "64-bit entry mode (longMode=1, addrsize=2, opsize=1, rexprefix=0; other known bits zero). " +
            "context.tsv exports pre-parse values/masks; unknown required fields fail closed. No mode repair occurs. " +
            "Only processor context is copied; imported tracked general/flag register values are not used.\n" +
            "Analysis is disabled. No instructions are installed and no function bodies are created or changed. " +
            "Listing.createInstruction cannot bound its InstructionDB reference-generation callbacks to the " +
            "supplied buffer in Ghidra 12.2; body_binding remains skipped_no_bounded_listing_installation " +
            "unless an imported function is rejected first. Calls are recorded " +
            "but never followed. Return/terminal edges end traversal. Call effects are not analyzed.\n" +
            "flow_complete means only that the bounded entry-reachable non-call walk exhausted its " +
            "worklist without issues; it does not mean the selected range is an entire function.\n" +
            "Observed byte intervals/coverage are separate. Unvisited bytes may be data or padding; " +
            "even full selected-byte coverage does not establish full-function coverage.\n" +
            "Pcode is raw Instruction.getPcode(false), without flow overrides. Varnodes preserve space " +
            "ID/name, unsigned hex offset and byte size; '-' is absent. Relative constant branch " +
            "offsets index pcode operations, not native addresses. Indirect targets remain expressions.\n" +
            "All CALLOTHER userops are unaudited/unresolved and make flow_complete false. " +
            "Instruction reference overrides and changed/unaudited primary call targets are rejected.\n" +
            "An insufficient bounded decode is reported as possible straddling, not claimed to prove " +
            "the instruction's length. Decoding and parser/pcode/flow callbacks use detached, bounded " +
            "snapshot buffers with no Program memory fallback; cross-instruction contexts are refused. " +
            "Sleigh may zero-fill unavailable pattern bytes within its own implementation; these are " +
            "not observed bytes or permission to extend the selected range. Existing listing objects " +
            "are consulted only for metadata/reference conflicts, never for parser contexts, pcode, " +
            "operand rendering or cached bytes. This is a decoder-input boundary, not a guarantee " +
            "against DB/storage read-ahead or reads performed before this script starts. Caps retain explicit " +
            "partial statuses; aggregate output overflow aborts publication rather than truncating.\n" +
            "Imported bodies outside selected bounds and overlapping other functions are rejected. " +
            "Observed instruction intervals are exported without assigning any function body.\n" +
            "Decompilation and pseudocode are permanently disabled. status.tsv keeps its decompilation " +
            "column fixed at disabled_raw_trace_only_no_pseudocode; .decompiler.txt is a fixed disabled " +
            "notice. No C file is produced. Imported prototypes, calling conventions, entry injections " +
            "and tracked register values are not interpreted as native semantics.\n" +
            "The selected_analysis_memory_not_physical_file SHA-256/hex describe loaded analysis " +
            "memory before decoding. A loaded-memory hash does NOT prove physical file bytes or " +
            "official owner semantics. The lead must verify against original physical file spans.\n" +
            "TSV text cells escape backslash, tab, CR and LF as \\\\, \\t, \\r and \\n.\n";
    }
}
