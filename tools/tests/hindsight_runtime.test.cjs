'use strict';

const assert = require('node:assert/strict');
const {spawnSync} = require('node:child_process');
const {createHash} = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {test} = require('node:test');

const launcher = path.resolve(__dirname, '../hindsight_runtime');
const tenant = 'invented-example-key-for-test-only';
const bank = 'icarus-catalog-fixture';

function fixture(t, commands) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'icarus-hindsight-runtime-'));
  t.after(() => fs.rmSync(directory, {recursive: true}));
  for (const [name, body] of Object.entries(commands)) {
    fs.writeFileSync(path.join(directory, name), '#!/bin/bash\n' + body, {mode: 0o700});
  }
  return (args, overrides = {}) => {
    const env = {
      PATH: `${directory}:/usr/bin:/bin`,
      HINDSIGHT_API_KEY: tenant,
      ICARUS_HINDSIGHT_PORT: '18481',
      ...overrides,
    };
    const result = spawnSync('/bin/bash', [launcher, ...args], {env, encoding: 'utf8', timeout: 5000});
    assert.ifError(result.error);
    return result;
  };
}

test('client wrappers carry the trusted loopback bank and reference the token environment', t => {
  const printArgs = 'printf "%s\\n" "$@"';
  const run = fixture(t, {codex: printArgs, claude: printArgs});
  const url = `http://127.0.0.1:18481/mcp/${bank}/`;
  const codex = run(['codex', bank, 'exec', 'verify citations']);
  assert.equal(codex.status, 0, codex.stderr);
  assert.deepEqual(codex.stdout.trimEnd().split('\n'), [
    '-c', `mcp_servers.icarus_hindsight.url="${url}"`,
    '-c', 'mcp_servers.icarus_hindsight.bearer_token_env_var="HINDSIGHT_API_KEY"',
    '-c', 'mcp_servers.icarus_hindsight.enabled=true',
    '-c', 'mcp_servers.icarus_hindsight.enabled_tools=["recall","reflect"]',
    'exec', 'verify citations',
  ]);
  const claude = run(['claude', bank, '--print', 'verify citations']);
  assert.equal(claude.status, 0, claude.stderr);
  const [print, prompt, option, config] = claude.stdout.trimEnd().split('\n');
  assert.equal(option, '--mcp-config');
  assert.deepEqual(JSON.parse(config), {
    mcpServers: {
      icarus_hindsight: {
        type: 'http',
        url,
        headers: {Authorization: 'Bearer ${HINDSIGHT_API_KEY}'},
      },
    },
  });
  assert.deepEqual([print, prompt], ['--print', 'verify citations']);
  assert.equal(codex.stdout.includes(tenant) || claude.stdout.includes(tenant), false);
});

test('client wrappers reject unsafe banks and insufficient bearer keys', t => {
  const run = fixture(t, {codex: 'exit 99', claude: 'exit 99'});
  for (const result of [
    run(['codex', '../other-scope']),
    run(['claude', 'bank?alternate']),
    run(['claude', bank], {HINDSIGHT_API_KEY: 'short'}),
    run(['codex', bank], {ICARUS_HINDSIGHT_PORT: '99999'}),
  ]) {
    assert.equal(result.status, 1);
    assert.match(result.stderr, /^hindsight:/);
  }
});

test('health requires readiness, anonymous denial, and authenticated access', t => {
  const run = fixture(t, {curl: `
set -euo pipefail
[[ "$1" == --disable ]] || exit 1
case "${'${!#}'}" in
  */health/ready) exit 0 ;;
  */v1/default/banks)
    case " $* " in
      *" --write-out "*) printf "%s" "${'${ANON_STATUS:-401}'}" ;;
      *" --config - "*)
        read -r config
        [[ $config == "header = \\"Authorization: Bearer $HINDSIGHT_API_KEY\\"" ]]
        [[ $* != *"$HINDSIGHT_API_KEY"* && "$*" == *"--noproxy *"* ]] ;;
      *) exit 2 ;;
    esac ;;
  *) exit 3 ;;
esac
`});
  const healthy = run(['health']);
  assert.equal(healthy.status, 0, healthy.stderr);
  assert.equal(healthy.stdout.trim(), 'Hindsight ready at http://127.0.0.1:18481');
  const unsecured = run(['health'], {ANON_STATUS: '200'});
  assert.equal(unsecured.status, 1);
});

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

test('alternate assets require a matching verification stamp before serving', t => {
  const repo = fs.mkdtempSync(path.join(os.tmpdir(), 'icarus-hindsight-assets-'));
  t.after(() => fs.rmSync(repo, {recursive: true}));
  const assets = path.join(repo, 'work', 'alternate');
  const state = path.join(repo, 'work', 'state');
  for (const directory of [assets, state]) fs.mkdirSync(directory, {recursive: true, mode: 0o700});
  fs.writeFileSync(path.join(assets, 'assets.lock'), '');
  const run = fixture(t, {
    git: 'printf "%s\\n" "$FAKE_COMMON_DIR"',
    df: 'printf "%s\\n" Available 99999999999',
  });
  const env = {
    FAKE_COMMON_DIR: path.join(repo, '.git'),
    ICARUS_HINDSIGHT_ASSETS_ROOT: assets,
    ICARUS_HINDSIGHT_STATE_ROOT: state,
    ICARUS_HINDSIGHT_PROXY_BASE_URL: 'https://fixture.example/v1',
    ICARUS_HINDSIGHT_PROXY_API_KEY: 'invented-provider-key',
  };
  const stamp = path.join(assets, 'wheels.sha256');
  const missing = run(['serve'], env);
  fs.writeFileSync(stamp, 'incorrect digest\n');
  const mismatched = run(['serve'], env);
  for (const result of [missing, mismatched]) {
    assert.equal(result.status, 1);
    assert.match(result.stderr, /install to verify the selected environment/);
    assert.equal(result.stdout, '');
  }

  const digest = createHash('sha256').update(fs.readFileSync(`${launcher}.requirements.txt`)).digest('hex');
  fs.writeFileSync(stamp, `${digest}\n`);
  const verified = run(['serve'], env);
  assert.equal(verified.status, 1);
  assert.match(verified.stderr, /install first/);
});
