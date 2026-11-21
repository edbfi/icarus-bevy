'use strict';

const assert = require('node:assert/strict');
const {spawn, spawnSync} = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {test} = require('node:test');

const launcher = path.resolve(__dirname, '../hindsight_runtime');

function fixture(t, commands) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'icarus-hindsight-runtime-'));
  t.after(() => fs.rmSync(directory, {recursive: true}));
  for (const [name, body] of Object.entries(commands)) {
    fs.writeFileSync(path.join(directory, name), '#!/bin/bash\n' + body, {mode: 0o700});
  }
  return (args, overrides = {}) => {
    const env = {
      PATH: `${directory}:/usr/bin:/bin`,
      ...overrides,
    };
    const result = spawnSync('/bin/bash', [launcher, ...args], {env, encoding: 'utf8', timeout: 5000});
    assert.ifError(result.error);
    return result;
  };
}

test('the supported wheel lock pins one SHA-256 per unique distribution', () => {
  const lock = fs.readFileSync(`${launcher}.requirements.txt`, 'utf8');
  const distributions = new Map();
  for (const line of lock.split('\n').filter(line => line && !line.startsWith('#'))) {
    const match = /^([a-z0-9-]+)==([^ ]+) --hash=sha256:([a-f0-9]{64})  # ([^ ]+\.whl)$/.exec(line);
    assert.ok(match, `Expected a wheel and a SHA-256 digest for ${line}`);
    const [, distribution, version, , wheel] = match;
    assert.ok(!distributions.has(distribution), `Duplicate dependency ${distribution}`);
    assert.ok(wheel.startsWith(`${distribution.replaceAll('-', '_')}-${version}-`), wheel);
    assert.match(wheel, /(?:none-any|x86_64)\.whl$/);
    distributions.set(distribution, version);
  }
  assert.equal(distributions.get('hindsight-api-slim'), '1.2.3');
  assert.ok(distributions.has('onnxruntime'));
  assert.ok(distributions.has('pg0-embedded'));
});

test('install refuses while a memory service holds the shared assets lock', t => {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'icarus-hindsight-locked-'));
  t.after(() => fs.rmSync(repo, {recursive: true}));
  const assets = path.join(repo, 'work', 'alternate');
  fs.mkdirSync(assets, {recursive: true, mode: 0o700});
  const lock = path.join(assets, 'assets.lock');
  fs.writeFileSync(lock, '');
  const holder = spawn('/usr/bin/flock', ['--shared', lock, 'sleep', '30'], {stdio: 'ignore'});
  t.after(() => holder.kill());
  while (spawnSync('/usr/bin/flock', ['--nonblock', '--exclusive', lock, 'true']).status === 0) {
    spawnSync('sleep', ['0.05']);
  }
  const run = fixture(t, {
    git: 'printf "%s\\n" "$FAKE_COMMON_DIR"',
    df: 'printf "%s\\n" Available 99999999999',
  });
  const result = run(['install'], {
    FAKE_COMMON_DIR: path.join(repo, '.git'), ICARUS_HINDSIGHT_ASSETS_ROOT: assets, ICARUS_HINDSIGHT_STATE_ROOT: assets,
  });
  assert.equal(result.status, 1);
  assert.match(result.stderr, /Stop the running project instance before installing/);
});

test('only the guide and install remain', t => {
  const run = fixture(t, {});
  for (const command of ['serve', 'health', 'codex', 'claude']) {
    const result = run([command]);
    assert.equal(result.status, 2, command);
    assert.match(result.stderr, /Usage: tools\/hindsight_runtime guide\|install/);
  }
  const guide = run(['guide']);
  assert.equal(guide.status, 0, guide.stderr);
  assert.match(guide.stdout, /python3 -m tools\.memory service/);
});

test('install provisions PostgreSQL by a throwaway pg0 start and refuses a build that is not the pinned one', t => {
  if (!fs.existsSync('/usr/bin/python3.14')) {
    t.skip('the wheel lock needs /usr/bin/python3.14');
    return;
  }
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'icarus-hindsight-postgres-'));
  t.after(() => fs.rmSync(repo, {recursive: true}));
  const assets = path.join(repo, 'work', 'alternate');
  fs.mkdirSync(assets, {recursive: true, mode: 0o700});
  const calls = path.join(repo, 'pg0-calls.txt');
  const pg0 = path.join(assets, 'venv', 'lib', 'python3.14', 'site-packages', 'pg0', 'bin', 'pg0');
  fs.mkdirSync(path.dirname(pg0), {recursive: true});
  fs.mkdirSync(path.join(assets, 'venv', 'bin'), {recursive: true});
  fs.writeFileSync(path.join(assets, 'venv', 'bin', 'python'), '#!/bin/bash\n', {mode: 0o700});
  // A stand-in pg0: logs its arguments and, on start, "extracts" a build whose binaries report 18.1.
  fs.writeFileSync(pg0, `#!/bin/bash
printf '%s\\n' "$*" >> ${JSON.stringify(calls)}
if [[ $1 == start ]]; then
  mkdir -p "$PG0_INSTALLATION_DIR/18.1.0/bin"
  for name in postgres pg_dump pg_restore psql; do
    printf '#!/bin/bash\\necho "postgres (PostgreSQL) 18.1"\\n' > "$PG0_INSTALLATION_DIR/18.1.0/bin/$name"
    chmod 700 "$PG0_INSTALLATION_DIR/18.1.0/bin/$name"
  done
fi
`, {mode: 0o700});
  const hub = path.join(assets, '.cache', 'hf', 'hub');
  for (const [repoName, revision, files] of [
    ['models--intfloat--multilingual-e5-small', '614241f622f53c4eeff9890bdc4f31cfecc418b3',
      ['config.json', 'tokenizer.json', 'tokenizer_config.json', 'special_tokens_map.json', 'sentencepiece.bpe.model', 'onnx/model.onnx']],
    ['models--hindsight-ai--hs-rerank-1', '927f5be31280298a51cf29af8a29cb111ed812c5',
      ['config.json', 'tokenizer.json', 'tokenizer_config.json', 'onnx/model.onnx']],
  ]) {
    for (const file of files) {
      const target = path.join(hub, repoName, 'snapshots', revision, file);
      fs.mkdirSync(path.dirname(target), {recursive: true});
      fs.writeFileSync(target, 'x');
    }
  }
  const run = fixture(t, {
    git: 'printf "%s\\n" "$FAKE_COMMON_DIR"',
    df: 'printf "%s\\n" Available 99999999999',
    uv: 'exit 0',
  });
  const result = run(['install'], {FAKE_COMMON_DIR: path.join(repo, '.git'), ICARUS_HINDSIGHT_ASSETS_ROOT: assets, ICARUS_HINDSIGHT_STATE_ROOT: assets});
  assert.equal(result.status, 1, result.stdout);
  assert.match(result.stderr, /The extracted PostgreSQL 18\.1\.0 does not match the pinned build/);
  const logged = fs.readFileSync(calls, 'utf8').trim().split('\n').map(line => line.split(' ').slice(0, 3).join(' '));
  assert.deepEqual(logged, ['start --name provision', 'stop --name provision', 'drop --name provision']);
  assert.equal(fs.existsSync(path.join(assets, 'wheels.sha256')), false, 'no install stamp without a verified PostgreSQL');
});
