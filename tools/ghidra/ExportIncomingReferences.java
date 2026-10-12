// Read-only, bounded incoming-reference export for one explicit existing function.
// Run only against a separately reviewed disposable retained-project copy.
// @category IcarusResearch

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.ReferenceIterator;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.security.MessageDigest;
import java.util.HexFormat;

public class ExportIncomingReferences extends GhidraScript {
    private static final int MAX_REFERENCES = 256;
    private static final int MAX_OUTPUT_BYTES = 64 * 1024;

    @Override
    public void run() throws Exception {
        try {
            export();
        } catch (Exception failure) {
            // Do not include program names, paths, symbols or library diagnostics.
            throw new IllegalStateException("Bound incoming-reference export failed");
        }
    }

    private void export() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 3 || currentProgram == null
                || !args[1].matches("[a-f0-9]{64}")
                || !args[2].matches("(?:0x)?[a-fA-F0-9]{1,16}")) {
            throw new IllegalArgumentException();
        }
        String expected = args[1];
        if (!expected.equals(currentProgram.getExecutableSHA256())) {
            throw new IllegalStateException();
        }
        String hex = args[2].startsWith("0x") ? args[2].substring(2) : args[2];
        Address target = currentProgram.getAddressFactory().getDefaultAddressSpace()
            .getAddress(Long.parseUnsignedLong(hex, 16));
        MemoryBlock block = currentProgram.getMemory().getBlock(target);
        Function selected = currentProgram.getFunctionManager().getFunctionAt(target);
        if (block == null || !block.isInitialized() || !block.isExecute()
                || selected == null || !selected.getEntryPoint().equals(target)) {
            throw new IllegalArgumentException();
        }

        StringBuilder rows = new StringBuilder(
            "target_space\ttarget_offset\tsource_space\tsource_offset\t"
                + "reference_type_id\tsource_type\toperand_index\tis_call\t"
                + "source_function_space\tsource_function_entry\n");
        int count = 0;
        ReferenceIterator references = currentProgram.getReferenceManager().getReferencesTo(target);
        while (references.hasNext()) {
            monitor.checkCancelled();
            if (count == MAX_REFERENCES) {
                throw new IllegalStateException();
            }
            Reference reference = references.next();
            if (reference == null || !target.equals(reference.getToAddress())) {
                throw new IllegalStateException();
            }
            Address from = reference.getFromAddress();
            if (from == null || reference.getReferenceType() == null
                    || reference.getSource() == null) {
                throw new IllegalStateException();
            }
            Function origin = currentProgram.getFunctionManager().getFunctionContaining(from);
            rows.append(space(target)).append('\t').append(offset(target)).append('\t')
                .append(space(from)).append('\t').append(offset(from)).append('\t')
                .append(Byte.toUnsignedInt(reference.getReferenceType().getValue())).append('\t')
                .append(reference.getSource().name()).append('\t')
                .append(reference.getOperandIndex()).append('\t')
                .append(reference.getReferenceType().isCall()).append('\t');
            if (origin == null) {
                rows.append("NONE\tNONE\n");
            } else {
                Address entry = origin.getEntryPoint();
                if (entry == null) {
                    throw new IllegalStateException();
                }
                rows.append(space(entry)).append('\t').append(offset(entry)).append('\n');
            }
            count++;
            if (rows.length() > MAX_OUTPUT_BYTES) {
                throw new IllegalStateException();
            }
        }
        monitor.checkCancelled();
        if (!expected.equals(currentProgram.getExecutableSHA256())) {
            throw new IllegalStateException();
        }
        byte[] referenceBytes = rows.toString().getBytes(StandardCharsets.UTF_8);
        String digest = HexFormat.of().formatHex(
            MessageDigest.getInstance("SHA-256").digest(referenceBytes));
        Address imageBase = currentProgram.getImageBase();
        String manifest = "key\tvalue\n"
            + "program_sha256\t" + expected + "\n"
            + "image_base_space\t" + space(imageBase) + "\n"
            + "image_base_offset\t" + offset(imageBase) + "\n"
            + "target_space\t" + space(target) + "\n"
            + "target_offset\t" + offset(target) + "\n"
            + "selected_existing_functions\t1\n"
            + "reference_count\t" + count + "\n"
            + "reference_limit\t" + MAX_REFERENCES + "\n"
            + "reference_bytes\t" + referenceBytes.length + "\n"
            + "reference_sha256\t" + digest + "\n"
            + "reference_type_encoding\tghidra_RefType_unsigned_byte\n"
            + "coverage\tretained_program_reference_manager_only\n"
            + "program_mutation_requested\tfalse\n"
            + "analysis_requested\tfalse\n"
            + "disassembly_requested\tfalse\n"
            + "decompilation_requested\tfalse\n"
            + "function_names_emitted\tfalse\n"
            + "data_bytes_emitted\tfalse\n"
            + "complete\ttrue\n";
        byte[] manifestBytes = manifest.getBytes(StandardCharsets.UTF_8);
        if (referenceBytes.length + manifestBytes.length > MAX_OUTPUT_BYTES) {
            throw new IllegalStateException();
        }

        // Validate and materialize the complete bounded result before any output.
        // Manifest is written last; consumers must verify it and the reference hash.
        // Filesystem failures may leave incomplete evidence, never an accepted result.
        Path output = Path.of(args[0]).toAbsolutePath().normalize();
        Files.createDirectory(output);
        Files.write(output.resolve("incoming-references.tsv"), referenceBytes,
            StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE);
        Files.write(output.resolve("manifest.tsv"), manifestBytes,
            StandardOpenOption.CREATE_NEW, StandardOpenOption.WRITE);
        println("Bound incoming-reference export completed; references: " + count);
    }

    private static int space(Address address) {
        return address.getAddressSpace().getSpaceID();
    }

    private static String offset(Address address) {
        return "0x" + Long.toUnsignedString(address.getOffset(), 16);
    }
}
