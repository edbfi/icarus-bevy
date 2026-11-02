const assert = require('node:assert/strict');
const path = require('node:path');
const test = require('node:test');
const { domains, helpers, parseMapping, validateLayout, checkRepository } = require('../tools/check_test_harnesses.cjs');
const { recordedBaseline, enumerate, compare } = require('./harness-inventory.cjs');

const header = 'old_target\told_source\tnew_target\tnew_source\tmodule_prefix\tisolation\n';
const row = (name, domain) => `${name}\ttests/${name}.rs\t${domain}\ttests/${domain}/${name}.rs\t${name}::\tin-process\n`;
function layout() {
  const result = { mappingText: header, manifest: '[package]\nname = "fixture"\n', files: [], roots: {} };
  domains.forEach((domain, i) => {
    result.mappingText += row(`suite_${i}`, domain);
    result.files.push(`tests/${domain}.rs`, `tests/${domain}/suite_${i}.rs`);
    result.roots[domain] = `//! Domain fixture\n#[path = "${domain}/suite_${i}.rs"]\nmod suite_${i};\n`;
    for (const [name, source] of Object.entries(helpers[domain])) {
      if (!result.files.includes(`tests/${source}`)) result.files.push(`tests/${source}`);
      result.roots[domain] += `#[path = "${source}"]\nmod ${name};\n`;
    }
  });
  return result;
}

test('repository layout passes without invoking Cargo', () => {
  const checked = checkRepository(path.join(__dirname, '..'));
  assert.ok(checked.source_suites > checked.integration_targets);
});

test('missing module declarations, missing sources, orphan sources and wrong paths fail', () => {
  for (const mutate of [
    (x) => { x.roots.protocol = ''; },
    (x) => { x.files = x.files.filter((name) => name !== 'tests/protocol/suite_1.rs'); },
    (x) => { x.files.push('tests/protocol/orphan.rs'); },
    (x) => { x.files.push('tests/protocol/nested/mod.rs'); },
    (x) => { x.roots.protocol = x.roots.protocol.replace('protocol/suite_1.rs', 'protocol/other.rs'); },
    (x) => { x.roots.protocol += '#[path = "protocol/orphan.rs"] mod orphan;'; },
    (x) => { x.roots.protocol += x.roots.protocol; },
    (x) => { x.roots.native_domain = x.roots.native_domain.replace('mod scalar;', 'mod wrong;'); },
  ]) {
    const input = layout();
    mutate(input);
    assert.throws(() => validateLayout(input));
  }
});

test('conditional declarations and commented-out declarations cannot hide uncompiled suites', () => {
  for (const prefix of ['#[cfg(any())]\n', '#![cfg(any())]\n', '//']) {
    const input = layout();
    input.roots.protocol = prefix + input.roots.protocol.replace('//! Domain fixture\n', '');
    assert.throws(() => validateLayout(input));
  }
});

test('automatic discovery rejects surprise roots, directory main and explicit target overrides', () => {
  for (const mutate of [
    (x) => { x.files.push('tests/surprise.rs'); },
    (x) => { x.files.push('tests/surprise/main.rs'); },
    (x) => { x.files.push('tests/protocol/main.rs'); },
    (x) => { x.manifest += 'autotests = false\n'; },
    (x) => { x.manifest += '[[test]]\nname = "hidden"\n'; },
  ]) {
    const input = layout();
    mutate(input);
    assert.throws(() => validateLayout(input));
  }
  const input = layout();
  input.manifest += 'autotests = true # still automatic\n';
  validateLayout(input);
});

test('reviewed mapping additions allow new grouped suites and isolated targets', () => {
  const input = layout();
  input.mappingText += row('new_suite', 'protocol');
  input.files.push('tests/protocol/new_suite.rs');
  input.roots.protocol += '#[path = "protocol/new_suite.rs"] mod new_suite;\n';
  input.mappingText += 'new_cli\ttests/new_cli.rs\tnew_cli\ttests/new_cli.rs\t\tsubprocess\n';
  input.files.push('tests/new_cli.rs');
  assert.deepEqual(validateLayout(input), { source_suites: 6, integration_targets: 5 });
});

test('mapping rejects duplicate, traversal, ambiguous prefix and malformed rows', () => {
  const input = layout();
  for (const text of [
    input.mappingText + row('suite_1', 'protocol'),
    input.mappingText.replace('tests/protocol/suite_1.rs', 'tests/protocol/../suite_1.rs'),
    input.mappingText.replace('suite_1::', 'suite_'),
    input.mappingText.replace('\tin-process', '\tunknown'),
    input.mappingText.replace('old_target', 'wrong'),
    input.mappingText + 'bad\n',
  ]) assert.throws(() => parseMapping(text));
});

const mapping = parseMapping(header + row('one', 'protocol') + row('two', 'protocol'));
const names = ['nested::a: test', 'b: test'];
function baseline() {
  return {
    inventory: [{ target: 'one', lists: [names, ['b: test']] }, { target: 'two', lists: [[], []] }],
    receipts: [
      { target: 'one', executable: '/unread/hash1', bytes: 12, inventory: names, ignored_inventory: ['b: test'] },
      { target: 'one', executable: '/unread/hash2', bytes: 13, inventory: [...names].reverse(), ignored_inventory: ['b: test'] },
      { target: 'two', executable: '/unread/hash3', bytes: 14, inventory: [], ignored_inventory: [] },
    ],
  };
}
function candidate() {
  return { phase: 'after', binary_count: 1, executable_bytes: 20,
    inventory: ['one:nested::a: test', 'one:b: test'], ignored_inventory: ['one:b: test'] };
}

test('recorded baseline compares all variants without executable access', () => {
  assert.deepEqual(compare(baseline(), candidate(), mapping), {
    tests: 2, ignored: 1,
    baseline: { target_count: 2, recorded_variants: 3, recorded_variant_bytes: 39 },
    candidate: { binary_count: 1, executable_bytes: 20 },
  });
});

test('baseline variant disagreement and missing target/receipt coverage fail closed', () => {
  for (const mutate of [
    (x) => { x.receipts[1].inventory = ['different: test']; },
    (x) => { x.receipts[1].ignored_inventory = []; },
    (x) => { x.receipts.pop(); },
    (x) => { x.inventory.pop(); },
    (x) => { x.inventory.push(x.inventory[0]); },
    (x) => { x.receipts[1].executable = x.receipts[0].executable; },
    (x) => { x.receipts[0].bytes = -1; },
  ]) {
    const input = baseline();
    mutate(input);
    assert.throws(() => recordedBaseline(input, mapping));
  }
});

test('comparison rejects dropped/added/duplicate tests and changed ignored flags', () => {
  for (const mutate of [
    (x) => { x.inventory.pop(); },
    (x) => { x.inventory.push('one:extra: test'); },
    (x) => { x.inventory.push(x.inventory[0]); },
    (x) => { x.ignored_inventory = []; },
    (x) => { x.ignored_inventory = ['one:unknown: test']; },
    (x) => { x.binary_count++; },
  ]) {
    const input = candidate();
    mutate(input);
    assert.throws(() => compare(baseline(), input, mapping));
  }
  assert.equal(compare({ inventory: candidate().inventory, ignored_inventory: candidate().ignored_inventory,
    binary_count: 2 }, candidate(), mapping).tests, 2);
});

const artifact = (target, executable) => JSON.stringify({ reason: 'compiler-artifact',
  profile: { test: true }, executable, target: { kind: ['test'], name: target } });
test('enumeration uses bounded list-only calls, keeps nested names and ignored state', () => {
  const calls = [];
  const result = enumerate('after', artifact('protocol', '/never-run'), mapping, (exe, args, options) => {
    calls.push({ exe, args, options });
    return { status: 0, stdout: args.includes('--ignored') ? 'one::b: test\n'
      : 'one::nested::a: test\none::b: test\n\n2 tests, 0 benchmarks\n' };
  }, () => ({ size: 20 }));
  assert.equal(calls.length, 2);
  for (const call of calls) {
    assert.deepEqual(call.args.slice(0, 3), ['--list', '--format', 'terse']);
    assert.equal(call.options.timeout, 60_000);
    assert.equal(call.options.maxBuffer, 16 * 1024 * 1024);
  }
  assert.deepEqual(result.inventory, ['one:b: test', 'one:nested::a: test']);
  assert.deepEqual(result.ignored_inventory, ['one:b: test']);
  assert.equal(compare(baseline(), result, mapping).tests, 2);
});

test('enumeration rejects missing/duplicate targets, unrecognized modules and child failures', () => {
  const failRun = () => assert.fail('must reject artifacts before attempting execution');
  assert.throws(() => enumerate('after', '', mapping, failRun));
  assert.throws(() => enumerate('after', artifact('unexpected', '/never-run'), mapping, failRun));
  assert.throws(() => enumerate('after', [artifact('protocol', '/a'), artifact('protocol', '/b')].join('\n'), mapping, failRun));
  const text = artifact('protocol', '/never-run');
  assert.throws(() => enumerate('after', text, mapping, () => ({ status: 0, stdout: 'unmapped::a: test\n' }), () => ({ size: 1 })));
  assert.throws(() => enumerate('after', text, mapping, () => ({ status: null, error: new Error('timeout') }), () => ({ size: 1 })));
});
