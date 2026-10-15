// Bounded flow listing of explicitly selected function starts.
// Each start is disassembled by flow within a 64 KiB window (no auto-analysis);
// the listing follows fallthrough and jump flows from the start, never calls,
// stops at 4096 instructions, and writes address, bytes and Ghidra's
// instruction text in address order. Changes only the analysis database;
// output must remain in an authorized case.
// @category IcarusResearch

import ghidra.app.cmd.disassemble.DisassembleCommand;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.address.AddressSet;
import ghidra.program.model.listing.Instruction;
import ghidra.program.model.mem.MemoryBlock;
import java.io.BufferedWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.util.ArrayDeque;
import java.util.HexFormat;
import java.util.TreeMap;

public class ListFlowInstructions extends GhidraScript {
    private static final long WINDOW_BYTES = 65536;
    private static final int MAX_INSTRUCTIONS = 4096;

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2 || args.length > 9) {
            throw new IllegalArgumentException("Expected new output directory and 1..8 hex starts");
        }
        Path output = Path.of(args[0]);
        // Fail rather than overwrite prior evidence.
        Files.createDirectory(output);
        try (BufferedWriter status = Files.newBufferedWriter(output.resolve("status.tsv"), StandardCharsets.UTF_8)) {
            status.write("start\tinstructions\tcomplete\n");
            for (int i = 1; i < args.length; i++) {
                monitor.checkCancelled();
                Address start = toAddr(args[i]);
                MemoryBlock block = currentProgram.getMemory().getBlock(start);
                if (block == null || !block.isExecute()) {
                    throw new IllegalArgumentException("Start must be in an executable block: " + args[i]);
                }
                Address windowEnd = start.add(Math.min(WINDOW_BYTES - 1, block.getEnd().subtract(start)));
                AddressSet window = new AddressSet(start, windowEnd);
                DisassembleCommand disassemble = new DisassembleCommand(start, window, true);
                disassemble.enableCodeAnalysis(false);
                disassemble.applyTo(currentProgram, monitor);
                TreeMap<Address, Instruction> listing = new TreeMap<>();
                ArrayDeque<Address> pending = new ArrayDeque<>();
                pending.add(start);
                boolean complete = true;
                while (!pending.isEmpty()) {
                    Address at = pending.pop();
                    if (listing.containsKey(at) || !window.contains(at)) {
                        continue;
                    }
                    Instruction instruction = getInstructionAt(at);
                    if (instruction == null) {
                        complete = false;
                        continue;
                    }
                    if (listing.size() == MAX_INSTRUCTIONS) {
                        complete = false;
                        break;
                    }
                    listing.put(at, instruction);
                    Address fallthrough = instruction.getFallThrough();
                    if (fallthrough != null) {
                        pending.push(fallthrough);
                    }
                    if (!instruction.getFlowType().isCall()) {
                        for (Address flow : instruction.getFlows()) {
                            pending.push(flow);
                        }
                    }
                }
                StringBuilder text = new StringBuilder();
                for (Instruction instruction : listing.values()) {
                    text.append(instruction.getAddress()).append('\t')
                        .append(HexFormat.of().formatHex(instruction.getBytes())).append('\t')
                        .append(instruction).append('\n');
                }
                Files.writeString(output.resolve(start + ".asm"), text.toString(), StandardCharsets.UTF_8,
                    StandardOpenOption.CREATE_NEW);
                status.write(start + "\t" + listing.size() + "\t" + complete + "\n");
                status.flush();
            }
        }
        println("Listed selected flow starts: " + (args.length - 1));
    }
}
