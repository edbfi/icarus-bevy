// Fast source-layout gate. No Cargo invocation or test executable access.
const fs = require('node:fs');
const path = require('node:path');

const domains = ['native_domain', 'protocol', 'replication_domain', 'save_format'];
const helpers = {
  native_domain: { scalar: 'common/c001_scalar_owners.rs', source: 'common/c001_inventory_source.rs', inventory_fixture: 'common/c001_inventory_owners.rs', owner_fixture: 'common/c001_owners.rs' },
  protocol: {},
  replication_domain: { pak_writer: 'pak_writer/mod.rs' },
  save_format: { scalar: 'common/c001_scalar_owners.rs' },
};
const header = 'old_target\told_source\tnew_target\tnew_source\tmodule_prefix\tisolation';

function requireThat(condition, message) {
  if (!condition) throw new Error(message);
}

function parseMapping(text) {
  const [first, ...lines] = text.trimEnd().split(/\r?\n/);
  requireThat(first === header, 'Invalid harness source-map header');
  const rows = lines.map((line, index) => {
    const columns = line.split('\t');
    requireThat(columns.length === 6, `Invalid source-map row ${index + 2}`);
    const [oldTarget, oldSource, newTarget, newSource, prefix, isolation] = columns;
    const name = /^[a-z][a-z0-9_]*$/;
    requireThat(name.test(oldTarget) && name.test(newTarget), `Invalid target in row ${index + 2}`);
    requireThat(oldSource === `tests/${oldTarget}.rs`, `Invalid old source: ${oldSource}`);
    if (isolation === 'in-process') {
      requireThat(domains.includes(newTarget), `Unknown domain: ${newTarget}`);
      requireThat(newSource === `tests/${newTarget}/${oldTarget}.rs`
        && prefix === `${oldTarget}::`, `Invalid module mapping: ${oldTarget}`);
    } else {
      requireThat(isolation === 'subprocess' && oldTarget === newTarget
        && oldSource === newSource && prefix === '' && !domains.includes(newTarget),
      `Invalid isolated target mapping: ${oldTarget}`);
    }
    return { oldTarget, oldSource, newTarget, newSource, prefix, isolation };
  });
  for (const key of ['oldTarget', 'newSource']) {
    requireThat(new Set(rows.map((row) => row[key])).size === rows.length, `Duplicate ${key} in source map`);
  }
  return rows;
}

function sameSet(actual, expected, label) {
  const missing = [...expected].filter((item) => !actual.has(item));
  const extra = [...actual].filter((item) => !expected.has(item));
  requireThat(!missing.length && !extra.length,
    `${label}: missing [${missing.join(', ')}]; unexpected [${extra.join(', ')}]`);
}

function validateLayout({ mappingText, manifest, files, roots }) {
  const mapping = parseMapping(mappingText);
  // Keep Cargo's automatic discovery: additions require a reviewed mapping row.
  const cargo = manifest.replace(/#.*$/gm, '');
  requireThat([...cargo.matchAll(/^\s*autotests\s*=([^\n]*)/gm)].every((match) => match[1].trim() === 'true')
    && !/^\s*\[\[\s*test\s*\]\]/m.test(cargo), 'Keep automatic test discovery without explicit [[test]] targets');
  const expectedTargets = new Set([...domains.map((name) => `tests/${name}.rs`),
    ...mapping.filter((row) => row.isolation === 'subprocess').map((row) => row.newSource)]);
  const discovered = new Set(files.filter((file) => /^tests\/[^/]+\.rs$/.test(file)
    || /^tests\/[^/]+\/main\.rs$/.test(file)));
  sameSet(discovered, expectedTargets, 'Auto-discovered test roots');
  for (const domain of domains) {
    const rows = mapping.filter((row) => row.newTarget === domain);
    requireThat(rows.length > 0, `Empty domain mapping: ${domain}`);
    const expectedModules = new Map(rows.map((row) => [row.oldTarget, row.newSource.slice(6)]));
    for (const [name, source] of Object.entries(helpers[domain])) {
      expectedModules.set(name, source);
      requireThat(files.includes(`tests/${source}`), `Missing helper: ${source}`);
    }
    // Intentionally small root grammar: unconditional explicit path + mod only.
    // Unknown attributes/items fail closed, including cfg that could drop tests.
    const source = roots[domain].replace(/\/\/[^\n]*/g, '');
    const declaration = /\s*#\[path\s*=\s*"([^"\n]+)"\s*\]\s*mod\s+([a-z][a-z0-9_]*)\s*;/gy;
    const actual = new Map();
    let offset = 0;
    while (source.slice(offset).trim()) {
      declaration.lastIndex = offset;
      const match = declaration.exec(source);
      requireThat(match, `Unsupported or conditional declaration in tests/${domain}.rs near ${source.slice(offset, offset + 100).trim()}`);
      const [, modulePath, name] = match;
      requireThat(!actual.has(name), `Duplicate module: ${domain}::${name}`);
      requireThat(expectedModules.get(name) === modulePath, `Unmapped module/path: ${domain}::${name} -> ${modulePath}`);
      actual.set(name, modulePath);
      offset = declaration.lastIndex;
    }
    sameSet(new Set(actual.keys()), new Set(expectedModules.keys()), `${domain} module declarations`);
    sameSet(new Set(files.filter((file) => file.startsWith(`tests/${domain}/`) && file.endsWith('.rs'))),
      new Set(rows.map((row) => row.newSource)), `${domain} source files`);
  }
  return { source_suites: mapping.length, integration_targets: expectedTargets.size };
}

function checkRepository(root) {
  const files = [];
  function walk(directory) {
    for (const entry of fs.readdirSync(path.join(root, directory), { withFileTypes: true })) {
      const name = `${directory}/${entry.name}`;
      if (entry.isDirectory()) walk(name);
      else files.push(name);
    }
  }
  walk('tests');
  return validateLayout({
    mappingText: fs.readFileSync(path.join(root, 'tests/harness-source-map.tsv'), 'utf8'),
    manifest: fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8'),
    files,
    roots: Object.fromEntries(domains.map((domain) => [domain,
      fs.readFileSync(path.join(root, `tests/${domain}.rs`), 'utf8')])),
  });
}

module.exports = { domains, helpers, parseMapping, sameSet, validateLayout, checkRepository };
if (require.main === module) {
  requireThat(process.argv.length === 2, 'Usage: node tools/check_test_harnesses.cjs');
  console.log(JSON.stringify(checkRepository(path.join(__dirname, '..'))));
}
