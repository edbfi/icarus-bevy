'use strict';

const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawnSync} = require('node:child_process');

const launcher = path.resolve(__dirname, '../../.agents/rea/rea.sh');

function fixture(t, {lines = [], minimum = '21', javaHome} = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'icarus-rea-launcher-'));
  t.after(() => fs.rmSync(root, {recursive: true, force: true}));
  function write(relative, content, executable = false) {
    const file = path.join(root, relative);
    fs.mkdirSync(path.dirname(file), {recursive: true});
    fs.writeFileSync(file, content, {mode: executable ? 0o755 : 0o644});
  }
  fs.mkdirSync(path.join(root, '.agents/rea'), {recursive: true});
  fs.copyFileSync(launcher, path.join(root, '.agents/rea/rea.sh'));
  write('.agents/rea/node_modules/rea-agents/scripts/rea.mjs', '');
  write('brew/bin/brew', '#!/bin/sh\nexit 1\n', true);
  write('brew/bin/node', '#!/bin/sh\nprintf "%s\\n" "${JAVA_HOME:-unset}" "$@"\n', true);
  write('ghidra/support/analyzeHeadless', '');
  write('ghidra/Ghidra/application.properties', `application.java.min=${minimum}\n`);
  for (const line of lines) write(`brew/opt/${line}/libexec/bin/java`, '', true);
  const env = {...process.env, PATH: `${root}/brew/bin:/usr/bin:/bin`,
    GHIDRA_INSTALL_DIR: path.join(root, 'ghidra')};
  delete env.JAVA_HOME;
  if (javaHome !== undefined) env.JAVA_HOME = javaHome;
  const result = spawnSync('/bin/bash', [path.join(root, '.agents/rea/rea.sh'), 'doctor'],
    {env, encoding: 'utf8', timeout: 5000});
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  const output = result.stdout.trim().split('\n');
  assert.equal(output[1], path.join(root, '.agents/rea/node_modules/rea-agents/scripts/rea.mjs'));
  assert.equal(output[2], 'doctor');
  return {root, selected: output[0]};
}

test('REA launcher uses an installed newer versioned JDK when exact and unversioned are absent', t => {
  const {root, selected} = fixture(t, {lines: ['openjdk@17', 'openjdk@25', 'openjdk@26']});
  assert.equal(selected, path.join(root, 'brew/opt/openjdk@25/libexec'));
});

test('REA launcher prefers Ghidra minimum and then the unversioned Homebrew JDK', t => {
  let result = fixture(t, {lines: ['openjdk@21', 'openjdk', 'openjdk@25']});
  assert.equal(result.selected, path.join(result.root, 'brew/opt/openjdk@21/libexec'));
  result = fixture(t, {lines: ['openjdk', 'openjdk@25']});
  assert.equal(result.selected, path.join(result.root, 'brew/opt/openjdk/libexec'));
});

test('REA launcher preserves explicit JAVA_HOME', t => {
  assert.equal(fixture(t, {lines: ['openjdk@25'], javaHome: '/authored/jdk'}).selected,
    '/authored/jdk');
});

test('REA launcher leaves missing or too-old JDKs for provider diagnosis', t => {
  for (const options of [{}, {lines: ['openjdk@17']}, {lines: ['openjdk@25'], minimum: 'invalid'}]) {
    assert.equal(fixture(t, options).selected, 'unset');
  }
});
