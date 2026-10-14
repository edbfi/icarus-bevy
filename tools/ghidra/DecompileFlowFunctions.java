// Bounded flow decompilation of explicitly selected function starts.
// Each start is disassembled by flow within a 64 KiB window (no auto-analysis)
// and its function body is computed by Ghidra from that flow, so no end address
// is guessed. A start that cannot be defined is recorded and skipped. Changes
// only the analysis database; output must remain in an authorized case.
// @category IcarusResearch

import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.cmd.function.CreateFunctionCmd;
import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.listing.Function;
import ghidra.program.model.mem.MemoryBlock;
import java.io.BufferedWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayList;
import java.util.List;

public class DecompileFlowFunctions extends GhidraScript {
    private static final long MAX_BODY_BYTES = 65536;

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2 || args.length > 25) {
            throw new IllegalArgumentException("Expected new output directory and 1..24 hex starts");
        }
        Path output = Path.of(args[0]);
        // Fail rather than overwrite prior evidence.
        Files.createDirectory(output);
        List<Function> functions = new ArrayList<>();
        try (BufferedWriter bodies = Files.newBufferedWriter(output.resolve("bodies.tsv"), StandardCharsets.UTF_8)) {
            bodies.write("start\tranges\tbytes\n");
            for (int i = 1; i < args.length; i++) {
                monitor.checkCancelled();
                Address start = toAddr(args[i]);
                MemoryBlock block = currentProgram.getMemory().getBlock(start);
                if (block == null || !block.isExecute()) {
                    throw new IllegalArgumentException("Start must be in an executable block: " + args[i]);
                }
                Address windowEnd = start.add(Math.min(MAX_BODY_BYTES - 1, block.getEnd().subtract(start)));
                DisassembleCommand disassemble = new DisassembleCommand(start, new AddressSet(start, windowEnd), true);
                disassemble.enableCodeAnalysis(false);
                disassemble.applyTo(currentProgram, monitor);
                if (getInstructionAt(start) == null) {
                    bodies.write(start + "\tno_instruction\t0\n");
                    continue;
                }
                Function function = getFunctionAt(start);
                if (function == null) {
                    CreateFunctionCmd create = new CreateFunctionCmd(start);
                    if (!create.applyTo(currentProgram, monitor)) {
                        bodies.write(start + "\tcreate_failed\t0\n");
                        continue;
                    }
                    function = getFunctionAt(start);
                }
                AddressSet body = new AddressSet(function.getBody());
                if (body.getNumAddresses() > MAX_BODY_BYTES) {
                    bodies.write(start + "\tbody_over_64KiB\t" + body.getNumAddresses() + "\n");
                    continue;
                }
                bodies.write(start + "\t" + body.getNumAddressRanges() + "\t" + body.getNumAddresses() + "\n");
                functions.add(function);
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
        println("Decompiled selected flow function count: " + functions.size());
    }
}
