// Export callers of explicitly selected diagnostic strings, bounded to six functions.
// Generated disassembly/decompilation stays in the ignored case directory.
// @category IcarusResearch

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;
import java.io.BufferedWriter;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.LinkedHashMap;
import java.util.Map;

public class ExportCompatibilityAnchors extends GhidraScript {
    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length < 2) {
            throw new IllegalArgumentException("Expected output directory and string addresses");
        }
        Path output = Path.of(args[0]);
        Files.createDirectories(output);
        Map<Address, Function> functions = new LinkedHashMap<>();
        try (BufferedWriter writer = Files.newBufferedWriter(output.resolve("references.tsv"), StandardCharsets.UTF_8)) {
            writer.write("anchor\treference\tfunction\n");
            for (int i = 1; i < args.length; i++) {
                monitor.checkCancelled();
                Address anchor = toAddr(args[i]);
                Reference[] references = getReferencesTo(anchor);
                if (references.length == 0) {
                    writer.write(anchor + "\tNONE\tUNKNOWN\n");
                }
                for (Reference reference : references) {
                    Function function = getFunctionContaining(reference.getFromAddress());
                    writer.write(anchor + "\t" + reference.getFromAddress() + "\t" +
                        (function == null ? "UNKNOWN" : function.getEntryPoint()) + "\n");
                    if (function != null && functions.size() < 6) {
                        functions.put(function.getEntryPoint(), function);
                    }
                }
            }
        }
        DecompInterface decompiler = new DecompInterface();
        try {
            if (!decompiler.openProgram(currentProgram)) {
                throw new IllegalStateException("Decompiler could not open program");
            }
            for (Function function : functions.values()) {
                monitor.checkCancelled();
                DecompileResults result = decompiler.decompileFunction(function, 15, monitor);
                String content = result.decompileCompleted()
                    ? result.getDecompiledFunction().getC()
                    : "Decompilation incomplete: " + result.getErrorMessage();
                Files.writeString(output.resolve(function.getEntryPoint() + ".txt"), content, StandardCharsets.UTF_8);
            }
        } finally {
            decompiler.dispose();
        }
        println("Exported candidate function count: " + functions.size());
    }
}
