// Lead-only enumeration of Cargo --no-run artifacts, or JSON-only comparison.
// Never builds or removes files. Comparison never accesses executable paths.
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const { parseMapping, sameSet } = require('../tools/check_test_harnesses.cjs');

function sortedUnique(values, label) {
  assert.ok(Array.isArray(values) && values.every((value) => typeof value === 'string'), `Invalid ${label}`);
  assert.equal(new Set(values).size, values.length, `Duplicate ${label}`);
  return [...values].sort();
}

function canonicalNames(target, lines, phase, mapping) {
  return lines.map((line) => {
    const match = /^(.*): (test|benchmark)$/.exec(line);
    assert.ok(match, `Invalid libtest entry: ${line}`);
    const [, name, kind] = match;
    const rows = mapping.filter((row) => phase === 'before'
      ? row.oldTarget === target : row.newTarget === target && name.startsWith(row.prefix));
    assert.equal(rows.length, 1, `Ambiguous or unmapped test ${target}:${name}`);
    const row = rows[0];
    const originalName = phase === 'before' ? name : name.slice(row.prefix.length);
    assert.ok(originalName, `Empty test name: ${target}:${name}`);
    return `${row.oldTarget}:${originalName}: ${kind}`;
  });
}

function inventories(inventory, ignoredInventory) {
  const normal = sortedUnique(inventory, 'inventory');
  const ignored = sortedUnique(ignoredInventory, 'ignored inventory');
  const names = new Set(normal);
  assert.ok(ignored.every((entry) => names.has(entry)), 'Ignored inventory is not a subset of inventory');
  return { inventory: normal, ignored_inventory: ignored };
}

function recordedBaseline(record, mapping) {
  // Recovered schema: one target with [all, ignored] lists, plus per-hash receipts.
  assert.ok(Array.isArray(record.inventory) && Array.isArray(record.receipts), 'Invalid recorded baseline');
  const expected = new Set(mapping.map((row) => row.oldTarget));
  const targets = new Map();
  for (const entry of record.inventory) {
    assert.ok(!targets.has(entry.target), `Duplicate baseline target: ${entry.target}`);
    assert.ok(Array.isArray(entry.lists) && entry.lists.length === 2, 'Invalid baseline lists');
    targets.set(entry.target, inventories(...entry.lists));
  }
  sameSet(new Set(targets.keys()), expected, 'Recorded baseline targets');
  const seen = new Set();
  const executables = new Set();
  let bytes = 0;
  for (const receipt of record.receipts) {
    assert.ok(targets.has(receipt.target), `Unmapped baseline receipt: ${receipt.target}`);
    assert.deepEqual(inventories(receipt.inventory, receipt.ignored_inventory), targets.get(receipt.target),
      `Baseline variants disagree: ${receipt.target}`);
    assert.ok(typeof receipt.executable === 'string' && !executables.has(receipt.executable), 'Duplicate or invalid receipt executable');
    assert.ok(Number.isSafeInteger(receipt.bytes) && receipt.bytes >= 0, 'Invalid receipt size');
    executables.add(receipt.executable);
    bytes += receipt.bytes;
    seen.add(receipt.target);
  }
  sameSet(seen, expected, 'Baseline receipt coverage');
  const inventory = [];
  const ignored = [];
  for (const [target, lists] of targets) {
    inventory.push(...canonicalNames(target, lists.inventory, 'before', mapping));
    ignored.push(...canonicalNames(target, lists.ignored_inventory, 'before', mapping));
  }
  return {
    ...inventories(inventory, ignored), target_count: targets.size,
    recorded_variants: record.receipts.length, recorded_variant_bytes: bytes,
  };
}

function enumerate(phase, artifactText, mapping, run = spawnSync, stat = fs.statSync) {
  assert.ok(['before', 'after'].includes(phase), 'Invalid inventory phase');
  const artifacts = new Map();
  for (const line of artifactText.split('\n')) {
    if (!line.trim()) continue;
    const item = JSON.parse(line);
    if (item.reason !== 'compiler-artifact' || !item.profile.test || !item.executable
        || !item.target.kind.includes('test')) continue;
    const target = item.target.name;
    assert.ok(!artifacts.has(target) || artifacts.get(target) === item.executable,
      `Multiple executables for integration target ${target}`);
    artifacts.set(target, item.executable);
  }
  const expected = new Set(mapping.map((row) => phase === 'before' ? row.oldTarget : row.newTarget));
  sameSet(new Set(artifacts.keys()), expected, 'Cargo integration artifacts');
  const binaries = [];
  const inventory = [];
  const ignoredInventory = [];
  for (const [target, executable] of [...artifacts].sort(([a], [b]) => a.localeCompare(b))) {
    binaries.push({ target, executable, bytes: stat(executable).size });
    for (const ignored of [false, true]) {
      const result = run(executable, ['--list', '--format', 'terse', ...(ignored ? ['--ignored'] : [])], {
        encoding: 'utf8', timeout: 60_000, maxBuffer: 16 * 1024 * 1024,
      });
      assert.ok(!result.error && result.status === 0, `Cannot enumerate ${target}: ${result.error || result.stderr}`);
      const lines = result.stdout.split('\n').filter((line) => /: (test|benchmark)$/.test(line));
      (ignored ? ignoredInventory : inventory).push(...canonicalNames(target, lines, phase, mapping));
    }
  }
  return {
    phase, binary_count: binaries.length,
    executable_bytes: binaries.reduce((sum, binary) => sum + binary.bytes, 0),
    binaries, ...inventories(inventory, ignoredInventory),
  };
}

function compare(baseline, candidate, mapping) {
  const before = Array.isArray(baseline.receipts)
    ? recordedBaseline(baseline, mapping) : inventories(baseline.inventory, baseline.ignored_inventory);
  assert.equal(candidate.phase, 'after', 'Candidate must be an after inventory');
  assert.equal(candidate.binary_count, new Set(mapping.map((row) => row.newTarget)).size, 'Candidate target count');
  const after = inventories(candidate.inventory, candidate.ignored_inventory);
  assert.deepEqual(after.inventory, before.inventory, 'Test inventory changed');
  assert.deepEqual(after.ignored_inventory, before.ignored_inventory, 'Ignored test inventory changed');
  return {
    tests: after.inventory.length, ignored: after.ignored_inventory.length,
    baseline: { target_count: before.target_count ?? baseline.binary_count,
      recorded_variants: before.recorded_variants, recorded_variant_bytes: before.recorded_variant_bytes },
    candidate: { binary_count: candidate.binary_count, executable_bytes: candidate.executable_bytes },
  };
}

module.exports = { canonicalNames, recordedBaseline, enumerate, compare };
if (require.main === module) {
  const [phase, first, second, ...extra] = process.argv.slice(2);
  assert.ok(first && !extra.length && (phase === 'compare' ? second : ['before', 'after'].includes(phase) && !second),
    'Usage: node tests/harness-inventory.cjs before|after BUILD.jsonl | compare BASELINE.json CANDIDATE.json');
  const mapping = parseMapping(fs.readFileSync(path.join(__dirname, 'harness-source-map.tsv'), 'utf8'));
  const output = phase === 'compare'
    ? compare(JSON.parse(fs.readFileSync(first, 'utf8')), JSON.parse(fs.readFileSync(second, 'utf8')), mapping)
    : enumerate(phase, fs.readFileSync(first, 'utf8'), mapping);
  process.stdout.write(`${JSON.stringify(output, null, 2)}\n`);
}
