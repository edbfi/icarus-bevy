// Disable symbol-server analysis before automatic analysis of a local sample.
// @category IcarusResearch

import ghidra.app.script.GhidraScript;
import java.util.Map;

public class OfflineAnalysis extends GhidraScript {
    @Override
    public void run() throws Exception {
        Map<String, String> options = getCurrentAnalysisOptionsAndValues(currentProgram);
        for (String name : options.keySet()) {
            if (name.equals("PDB") || name.equals("PDB Universal")) {
                setAnalysisOption(currentProgram, name, "false");
                println("Disabled external symbols: " + name);
            }
        }
    }
}
