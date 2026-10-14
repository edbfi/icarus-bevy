// Bounded offline analysis of explicitly selected executable address ranges.
// Changes only the analysis database; output must remain in an authorized case.
// @category IcarusResearch

import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.listing.Function;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.mem.MemoryBlock;
import ghidra.program.model.symbol.Reference;
import ghidra.program.model.symbol.SourceType;
import java.io.BufferedWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.List;

public class ExportSelectedFunctions extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2 || args.length > 25) {
            throw new IllegalArgumentException("Expected new output directory and 1..24 start:end ranges (hex, end exclusive)");
        }
        Path output = Path.of(args[0]);
        // Fail rather than overwrite prior evidence.
        Files.createDirectory(output);
        List<Function> functions = new ArrayList<>();
        try (BufferedWriter references = Files.newBufferedWriter(output.resolve("references.tsv"), StandardCharsets.UTF_8)) {
            references.write("function\tinstruction\ttarget\ttype\n");
            for (int i = 1; i < args.length; i++) {
                monitor.checkCancelled();
                String[] range = args[i].split(":", -1);
                if (range.length != 2) {
                    throw new IllegalArgumentException("Invalid range: " + args[i]);
                }
                Address start = toAddr(range[0]);
                Address end = toAddr(range[1]);
                long size = end.subtract(start);
                MemoryBlock block = currentProgram.getMemory().getBlock(start);
                if (size <= 0 || size > 65536 || block == null || !block.isExecute()
                        || !block.contains(end.subtract(1))) {
                    throw new IllegalArgumentException("Range must be <=64 KiB in one executable block: " + args[i]);
                }
                AddressSet body = new AddressSet(start, end.subtract(1));
                DisassembleCommand command = new DisassembleCommand(start, body, true);
                command.enableCodeAnalysis(false);
                command.applyTo(currentProgram, monitor);
                if (getInstructionAt(start) == null) {
                    throw new IllegalStateException("No instruction at " + start + ": " + command.getStatusMsg());
                }
                Function function = getFunctionAt(start);
                if (function == null) {
                    function = currentProgram.getFunctionManager().createFunction(
                        "candidate_" + start, start, body, SourceType.USER_DEFINED);
                } else {
                    function.setBody(body);
                }
                if (function == null) {
                    throw new IllegalStateException("Could not define selected function at " + start);
                }
                functions.add(function);
                for (Instruction instruction : currentProgram.getListing().getInstructions(body, true)) {
                    for (Reference reference : instruction.getReferencesFrom()) {
                        references.write(start + "\t" + instruction.getAddress() + "\t"
                            + reference.getToAddress() + "\t" + reference.getReferenceType() + "\n");
                    }
                }
            }
        }
        DecompInterface decompiler = new DecompInterface();
        try (BufferedWriter status = Files.newBufferedWriter(output.resolve("status.tsv"), StandardCharsets.UTF_8)) {
            if (!decompiler.openProgram(currentProgram)) {
                throw new IllegalStateException("Decompiler could not open program");
            }
            status.write("function\tcompleted\twarnings\n");
            for (Function function : functions) {
                monitor.checkCancelled();
                DecompileResults result = decompiler.decompileFunction(function, 30, monitor);
                String message = result.getErrorMessage();
                String warning = message == null ? "" : message.replace('\n', ' ').replace('\t', ' ');
                status.write(function.getEntryPoint() + "\t" + result.decompileCompleted() + "\t" + warning + "\n");
                if (result.decompileCompleted()) {
                    Files.writeString(output.resolve(function.getEntryPoint() + ".c"),
                        result.getDecompiledFunction().getC(), StandardCharsets.UTF_8,
                        StandardOpenOption.CREATE_NEW);
                }
                status.flush();
            }
        } finally {
            decompiler.dispose();
        }
        println("Inspected selected function count: " + functions.size());
    }
}
