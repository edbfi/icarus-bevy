#!/usr/bin/env node
'use strict';

// Manual integration only; deliberately excluded from the ordinary unit suite.
// Requires public Ghidra 12.2, JAVA_HOME pointing to JDK 25 (or --java-home),
// GNU cc/nm, bubblewrap, util-linux taskset/prlimit and GNU timeout on Linux.
// Usage:
// node tools/tests/trace_bounded_functions.integration.cjs \
//   --case-root /absolute/existing/authorized/work/case \
//   --ghidra-home /absolute/public/ghidra-overlay [--java-home /absolute/jdk25]
//
// The explicit case-root is the operator's authorization for authored-fixture
// analysis there. No original inputs are read and the generated ELF is NEVER
// executed. A new mkdtemp child retains frozen sources/hashes, logs and receipts.
// All subprocesses use CPUs 4,5 and a private network namespace. Ghidra is
// sequential: 180s + 30s kill grace per invocation, 4GiB heap, max-cpu 2, private
// Java home/settings/tmp; the complete runner has a 15-minute watchdog. Namespace
// unavailability is a failure, never an implicit network-enabled fallback/skip.

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');

const REPO = path.resolve(__dirname, '../..');
const FUNCTION_NAMES = [
  'authored_good_gate', 'authored_xbegin_gate', 'authored_override_gate',
  'authored_inline_caller', 'authored_inline_callee', 'authored_mode_gate',
  'authored_many_instructions', 'authored_straddle', 'authored_many_pcode',
  'authored_undefined_override',
];
const SOURCE_FILES = [
  'tools/ghidra/TraceBoundedFunctions.java',
  'tools/ghidra/fixtures/PrepareTraceAdversarial.java',
  'tools/ghidra/fixtures/trace_adversarial.S',
];
const sha256 = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const hex = value => value.toString(16);
let activeProcess;
let interrupted;

function killActive(signal) {
  if (!activeProcess?.pid) return;
  try {
    process.kill(-activeProcess.pid, signal);
  } catch (error) {
    if (error.code !== 'ESRCH') throw error;
  }
}

// GNU timeout also manages its process group; these handlers cover interruption
// of the Node parent and the outer watchdog, including launcher descendants.
for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    interrupted = signal;
    killActive('SIGTERM');
  });
}

function parseArgs(argv) {
  const options = {};
  for (let i = 0; i < argv.length; i += 2) {
    const option = argv[i];
    assert(['--case-root', '--ghidra-home', '--java-home'].includes(option),
      `Unknown option ${option}; required: --case-root DIR --ghidra-home DIR`);
    assert(argv[i + 1] && !argv[i + 1].startsWith('--'), `Missing value for ${option}`);
    assert(!(option in options), `Duplicate option ${option}`);
    options[option] = argv[i + 1];
  }
  for (const required of ['--case-root', '--ghidra-home']) {
    assert(options[required], `Required argument: ${required} DIR (no automatic skip)`);
  }
  options['--java-home'] ||= process.env.JAVA_HOME;
  assert(options['--java-home'], 'Set JAVA_HOME to public JDK 25 or pass --java-home DIR');
  for (const [option, value] of Object.entries(options)) {
    assert(path.isAbsolute(value), `${option} must be an absolute existing directory`);
    options[option] = fs.realpathSync(value);
    assert(fs.statSync(options[option]).isDirectory(), `${option} must be a directory`);
    assert(!/["\r\n;]/.test(options[option]),
      `${option} cannot contain quotes, line breaks or Ghidra script-path separators`);
  }
  const work = fs.realpathSync(path.join(REPO, 'work'));
  const relative = path.relative(work, options['--case-root']);
  assert(relative && !relative.startsWith(`..${path.sep}`) && relative !== '..' &&
    !path.isAbsolute(relative), '--case-root must be an existing case below this checkout/work');
  const parentCase = JSON.parse(fs.readFileSync(path.join(options['--case-root'], 'case.json'), 'utf8'));
  assert.equal(parentCase.state, 'open', '--case-root must have an open case.json');
  assert(parentCase.owner && parentCase.purpose, '--case-root must name its owner and purpose');
  return options;
}

function tsv(file) {
  const text = fs.readFileSync(file, 'utf8');
  assert(text.endsWith('\n'), `Incomplete TSV: ${file}`);
  const lines = text.slice(0, -1).split('\n');
  const headers = lines.shift().split('\t');
  assert.equal(new Set(headers).size, headers.length, `Duplicate columns: ${file}`);
  return lines.map(line => {
    const cells = line.split('\t');
    assert.equal(cells.length, headers.length, `Wrong column count: ${file}: ${line}`);
    return Object.fromEntries(headers.map((header, index) => [header, cells[index]]));
  });
}

function number(row, key) {
  assert.match(row[key], /^\d+$/, `Invalid ${key}: ${row[key]}`);
  const value = Number(row[key]);
  assert(Number.isSafeInteger(value), `Unsafe ${key}`);
  return value;
}

const address = value => {
  assert.match(value, /^(?:0x)?[0-9a-f]+$/i);
  return BigInt(value.startsWith('0x') ? value : `0x${value}`);
};

function snapshot(directory) {
  const entries = [];
  function visit(dir) {
    for (const name of fs.readdirSync(dir).sort()) {
      const file = path.join(dir, name);
      const stat = fs.lstatSync(file);
      const relative = path.relative(directory, file);
      assert(!stat.isSymbolicLink(), `Unexpected symlink in owned output: ${relative}`);
      if (stat.isDirectory()) {
        entries.push({ path: relative, directory: true });
        visit(file);
      } else {
        assert(stat.isFile(), `Unexpected output type: ${relative}`);
        entries.push({ path: relative, bytes: stat.size, sha256: sha256(fs.readFileSync(file)) });
      }
    }
  }
  visit(directory);
  return entries;
}

// Parse only this newly compiled, authored ELF64; no external file/path lookup.
function elfReader(bytes) {
  assert(bytes.length >= 64 && bytes.subarray(0, 4).equals(Buffer.from([0x7f, 69, 76, 70])));
  assert.equal(bytes[4], 2, 'Expected ELF64');
  assert.equal(bytes[5], 1, 'Expected little-endian ELF');
  assert.equal(bytes.readUInt16LE(16), 2, 'Expected fixed-address ET_EXEC');
  assert.equal(bytes.readUInt16LE(18), 62, 'Expected x86-64');
  const phoff = Number(bytes.readBigUInt64LE(32));
  const entrySize = bytes.readUInt16LE(54);
  const count = bytes.readUInt16LE(56);
  assert(Number.isSafeInteger(phoff) && entrySize >= 56 && count > 0 && count < 128);
  assert(phoff + count * entrySize <= bytes.length, 'Program headers outside authored ELF');
  const segments = [];
  for (let i = 0; i < count; i++) {
    const offset = phoff + i * entrySize;
    if (bytes.readUInt32LE(offset) === 1 && (bytes.readUInt32LE(offset + 4) & 1)) {
      segments.push({ file: bytes.readBigUInt64LE(offset + 8),
        start: bytes.readBigUInt64LE(offset + 16), size: bytes.readBigUInt64LE(offset + 32) });
    }
  }
  return (start, end) => {
    const matches = segments.filter(segment => start >= segment.start && end > start &&
      end <= segment.start + segment.size);
    assert.equal(matches.length, 1, 'Authored range must belong to exactly one file-backed executable segment');
    const segment = matches[0];
    const offset = segment.file + start - segment.start;
    const length = end - start;
    assert(offset >= 0n && offset + length <= BigInt(bytes.length));
    return bytes.subarray(Number(offset), Number(offset + length));
  };
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  assert.equal(process.platform, 'linux', 'This integration runner requires Linux');
  const allowedLine = fs.readFileSync('/proc/self/status', 'utf8')
    .split('\n').find(line => line.startsWith('Cpus_allowed_list:'));
  assert(allowedLine, 'Cannot read this process CPU affinity from /proc/self/status');
  const allowed = allowedLine.split(':')[1].trim();
  const includesCpu = cpu => allowed.split(',').some(item => {
    const [start, end = start] = item.split('-').map(Number);
    return Number.isInteger(start) && Number.isInteger(end) && cpu >= start && cpu <= end;
  });
  assert(includesCpu(4) && includesCpu(5),
    `This integration runner requires allowed logical CPUs 4 and 5; Cpus_allowed_list=${allowed}`);
  const ghidra = options['--ghidra-home'];
  const javaHome = options['--java-home'];
  const launcher = path.join(ghidra, 'support/launch.sh');
  fs.accessSync(launcher, fs.constants.X_OK);
  fs.accessSync(path.join(javaHome, 'bin/java'), fs.constants.X_OK);
  const properties = fs.readFileSync(path.join(ghidra, 'Ghidra/application.properties'), 'utf8');
  assert.match(properties, /^application\.version=12\.2(?:\.[0-9]+)*\s*$/m,
    '--ghidra-home must contain public Ghidra 12.2');

  const root = fs.mkdtempSync(path.join(options['--case-root'], 'trace-integration-'));
  const started = Date.now();
  const receipt = { state: 'open', owner: 'trace-bounded-functions-integration',
    purpose: 'Manual public Ghidra 12.2/JDK25 checks on independently authored source only',
    parent: options['--case-root'], ghidraHome: ghidra, javaHome,
    started: new Date(started).toISOString(), commands: [], assertions: [] };
  const record = () => fs.writeFileSync(path.join(root, 'case.json'), `${JSON.stringify(receipt, null, 2)}\n`);
  record();
  console.log(`Authored integration receipts: ${root}`);
  const watchdog = setTimeout(() => {
    interrupted = '15-minute watchdog';
    killActive('SIGKILL');
  }, 15 * 60 * 1000);

  try {
    const sources = path.join(root, 'sources');
    const projects = path.join(root, 'projects');
    const privateHome = path.join(root, 'java-home');
    const temporary = path.join(root, 'java-tmp');
    for (const dir of [sources, projects, privateHome, temporary]) fs.mkdirSync(dir);
    const sourceHashes = [];
    for (const source of SOURCE_FILES) {
      const destination = path.join(sources, path.basename(source));
      fs.copyFileSync(path.join(REPO, source), destination, fs.constants.COPYFILE_EXCL);
      fs.chmodSync(destination, 0o444);
      sourceHashes.push({ source, frozen: destination, sha256: sha256(fs.readFileSync(destination)) });
    }
    fs.writeFileSync(path.join(root, 'sources.sha256'),
      sourceHashes.map(item => `${item.sha256}  sources/${path.basename(item.frozen)}\n`).join(''), { flag: 'wx' });
    receipt.sources = sourceHashes;
    receipt.ghidraPropertiesSha256 = sha256(Buffer.from(properties));
    receipt.launcherSha256 = sha256(fs.readFileSync(launcher));
    record();

    // Do not inherit Java flags, user preferences, proxies, or compiler options.
    const env = { PATH: process.env.PATH || '/usr/bin:/bin', JAVA_HOME: javaHome,
      USER: 'trace-integration', LOGNAME: 'trace-integration',
      LANG: 'C.UTF-8', LC_ALL: 'C.UTF-8', TZ: 'UTC', TMPDIR: temporary,
      XDG_CONFIG_HOME: path.join(privateHome, '.config'),
      XDG_CACHE_HOME: path.join(privateHome, '.cache'),
      GHIDRA_HEADLESS_MAXMEM: '4G',
      JAVA_TOOL_OPTIONS: [`-Duser.home=${privateHome}`, `-Djava.io.tmpdir=${temporary}`,
        `-Dapplication.settingsdir=${path.join(privateHome, 'ghidra-settings')}`,
        `-Djava.util.prefs.userRoot=${path.join(privateHome, 'prefs')}`,
        `-Djava.util.prefs.systemRoot=${path.join(privateHome, 'system-prefs')}`]
        .map(value => `"${value}"`).join(' ') };

    async function command(name, executable, args, seconds = 180) {
      assert(!interrupted, `Interrupted: ${interrupted}`);
      assert(!activeProcess, 'Processes must remain sequential');
      const stdoutPath = path.join(root, `${name}.stdout.log`);
      const stderrPath = path.join(root, `${name}.stderr.log`);
      const stdout = fs.openSync(stdoutPath, 'wx');
      const stderr = fs.openSync(stderrPath, 'wx');
      // Read-only host filesystem; only this new case is writable. No network.
      const wrapped = ['--die-with-parent', '--unshare-net', '--ro-bind', '/', '/',
        '--bind', root, root, '--proc', '/proc', '--dev', '/dev', '--', 'taskset', '-c', '4,5',
        'prlimit', '--fsize=268435456', '--', 'timeout', '--signal=TERM', '--kill-after=30s',
        `${seconds}s`, executable, ...args];
      const item = { name, executable: 'bwrap', args: wrapped, started: new Date().toISOString() };
      receipt.commands.push(item);
      record();
      let child;
      let fallback;
      try {
        child = spawn('bwrap', wrapped, { cwd: root, env, detached: true,
          stdio: ['ignore', stdout, stderr] });
        activeProcess = child;
        fallback = setTimeout(() => killActive('SIGKILL'), (seconds + 35) * 1000);
        const outcome = await new Promise((resolve, reject) => {
          child.once('error', reject);
          child.once('close', (code, signal) => resolve({ code, signal }));
        });
        Object.assign(item, outcome);
      } finally {
        clearTimeout(fallback);
        // No daemon descendants from an owned command may outlive its receipt.
        if (child?.pid) killActive('SIGKILL');
        activeProcess = undefined;
        fs.closeSync(stdout);
        fs.closeSync(stderr);
        item.finished = new Date().toISOString();
        record();
      }
      assert(!interrupted, `Interrupted: ${interrupted}`);
      assert.equal(item.signal, null, `${name} terminated; inspect ${stderrPath}`);
      assert.equal(item.code, 0, `${name} process failed (${item.code}); inspect ${stderrPath}`);
      return fs.readFileSync(stdoutPath, 'utf8') + fs.readFileSync(stderrPath, 'utf8');
    }

    const javaVersion = await command('java-version', path.join(javaHome, 'bin/java'), ['-version'], 15);
    assert.match(javaVersion, /(?:openjdk|java) version "25(?:[."])/, 'JDK 25 is required');
    const elfPath = path.join(root, 'trace-adversarial.elf');
    await command('compile', 'cc', ['-nostdlib', '-no-pie', '-Wl,--build-id=none',
      '-Wl,-e,authored_good_gate', '-o', elfPath, path.join(sources, 'trace_adversarial.S')], 30);
    fs.chmodSync(elfPath, 0o444); // No executable permission; it is only an import input.
    const elf = fs.readFileSync(elfPath);
    const elfHash = sha256(elf);
    const physicalBytes = elfReader(elf);
    receipt.fixture = { path: elfPath, sha256: elfHash, bytes: elf.length };

    const nmText = await command('symbols', 'nm', ['-S', '--defined-only', '--format=posix', elfPath], 15);
    const symbols = new Map();
    for (const line of nmText.trim().split('\n')) {
      const parts = line.trim().split(/\s+/);
      if (!parts[0].startsWith('authored_')) continue;
      assert(parts.length === 3 || parts.length === 4, `Unexpected nm symbol: ${line}`);
      assert.equal(parts[1], 'T', `Fixture symbol must belong to text: ${line}`);
      assert(!symbols.has(parts[0]), `Duplicate nm symbol: ${parts[0]}`);
      symbols.set(parts[0], { start: address(parts[2]), size: parts[3] ? address(parts[3]) : 0n });
    }
    function symbol(name) {
      assert(symbols.has(name), `Missing authored symbol: ${name}`);
      return symbols.get(name);
    }
    const bounds = new Map(FUNCTION_NAMES.map(name => {
      const entry = symbol(name);
      const end = symbol(`${name}_end`).start;
      assert(entry.size > 0n && entry.start + entry.size === end,
        `nm size must equal explicit end marker for ${name}`);
      physicalBytes(entry.start, end);
      return [name, { name, start: entry.start, end }];
    }));
    receipt.symbols = [...symbols].map(([name, entry]) => ({ name,
      start: hex(entry.start), size: hex(entry.size) }));
    record();

    const prepArgs = [elfPath, elfHash, ...FUNCTION_NAMES.map(name => {
      const span = bounds.get(name);
      return `${name}:${hex(span.start)}:${hex(span.end)}`;
    }), hex(symbol('authored_override_gate_jump').start),
    hex(symbol('authored_override_gate_fallthrough').start)];
    const spans = [
      bounds.get('authored_good_gate'), bounds.get('authored_xbegin_gate'),
      bounds.get('authored_override_gate'), bounds.get('authored_inline_caller'),
      bounds.get('authored_mode_gate'), bounds.get('authored_many_instructions'),
      { ...bounds.get('authored_straddle'), end: bounds.get('authored_straddle').end - 2n },
      bounds.get('authored_many_pcode'),
    ];
    const rangeArg = span => `${hex(span.start)}:${hex(span.end)}`;
    async function ghidraRun(name, output, ranges) {
      const log = path.join(root, `${name}.ghidra.log`);
      const scriptlog = path.join(root, `${name}.scripts.log`);
      // Direct public launch.sh invocation makes the 4G heap explicit instead of
      // relying on an analyzeHeadless shell script's hardcoded MAXMEM value.
      const text = await command(name, launcher, ['fg', 'jdk', 'Ghidra-Headless', '4G', '',
        'ghidra.app.util.headless.AnalyzeHeadless', projects, name,
        '-import', elfPath, '-processor', 'x86:LE:64:default', '-noanalysis', '-max-cpu', '2',
        '-scriptPath', sources, '-log', log, '-scriptlog', scriptlog,
        '-preScript', 'PrepareTraceAdversarial.java', ...prepArgs,
        '-postScript', 'TraceBoundedFunctions.java', output, ...ranges, '-deleteProject']);
      const logs = text + [log, scriptlog].map(file => fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : '').join('\n');
      assert(logs.includes(`TRACE_ADVERSARIAL_PREPARED ${elfHash}`),
        `${name}: preparation did not finish; inspect retained logs`);
      return logs;
    }

    const output = path.join(root, 'accepted-output');
    await ghidraRun('accepted', output, spans.map(rangeArg));
    assert(fs.statSync(path.join(output, 'EXPORT_FINISHED.txt')).isFile(), 'Missing export completion marker');
    assert(!fs.existsSync(path.join(output, 'FAILED.txt')), 'Exporter published FAILED.txt');
    const statuses = tsv(path.join(output, 'status.tsv'));
    assert.equal(statuses.length, spans.length, 'Every requested range needs its own status');
    const file = (index, suffix) => path.join(output, `range-${String(index + 1).padStart(2, '0')}${suffix}`);
    const issues = index => tsv(file(index, '.issues.tsv'));
    const noC = index => assert(!fs.existsSync(file(index, '.c')), `${spans[index].name}: unexpected pseudocode`);
    const issue = (index, reason) => assert(issues(index).some(row => row.reason === reason),
      `${spans[index].name}: missing issue ${reason}`);
    const partial = index => {
      assert.equal(statuses[index].flow_complete, 'false', `${spans[index].name}: must be partial`);
      assert(number(statuses[index], 'issues') > 0);
      assert.equal(statuses[index].decompilation, 'disabled_raw_trace_only_no_pseudocode');
      noC(index);
    };
    function passed(name) {
      receipt.assertions.push(name);
      record();
      console.log(`PASS ${name}`);
    }

    // Bind selected memory, individual decoded bytes, body intervals and status
    // to the independently read physical PT_LOAD spans for EVERY fixture.
    for (let i = 0; i < spans.length; i++) {
      const span = spans[i];
      const status = statuses[i];
      assert.equal(status.range, `range-${String(i + 1).padStart(2, '0')}`);
      assert.equal(address(status.start), span.start);
      assert.equal(address(status.end_exclusive), span.end);
      assert.equal(number(status, 'selected_bytes'), Number(span.end - span.start));
      const memory = tsv(file(i, '.memory.tsv'));
      assert.equal(memory.length, 1);
      const expected = physicalBytes(span.start, span.end);
      assert.equal(memory[0].source, 'selected_analysis_memory_not_physical_file');
      assert.equal(address(memory[0].start), span.start);
      assert.equal(address(memory[0].end_exclusive), span.end);
      assert.equal(number(memory[0], 'bytes'), expected.length);
      assert.equal(memory[0].hex, expected.toString('hex'));
      assert.equal(memory[0].sha256, sha256(expected));
      const instructions = tsv(file(i, '.instructions.tsv'));
      assert.equal(instructions.length, number(status, 'instructions'));
      let decoded = 0;
      const covered = new Set();
      for (const instruction of instructions) {
        const at = address(instruction.address);
        const length = number(instruction, 'length');
        assert(length > 0 && at >= span.start && at + BigInt(length) <= span.end);
        assert.equal(instruction.bytes, physicalBytes(at, at + BigInt(length)).toString('hex'));
        for (let byte = at; byte < at + BigInt(length); byte++) {
          assert(!covered.has(byte.toString()), 'Decoded instructions overlap');
          covered.add(byte.toString());
        }
        decoded += length;
      }
      assert.equal(decoded, number(status, 'decoded_instruction_bytes'));
      assert.equal(expected.length - decoded, number(status, 'unvisited_bytes'));
      assert.equal(issues(i).length, number(status, 'issues'));
      assert(number(status, 'instructions') <= 4096);
      assert(number(status, 'pcode_ops_exported') <= 16384);
      const pcode = tsv(file(i, '.pcode.tsv'));
      assert.equal(pcode.length, number(status, 'pcode_ops_exported'));
      for (const row of pcode) {
        assert(covered.has(address(row.instruction).toString()));
        assert.equal(address(row.sequence_address), address(row.instruction));
      }
      const bodyBytes = new Map();
      for (const body of tsv(file(i, '.body.tsv'))) {
        const start = address(body.start);
        const end = address(body.end_exclusive);
        assert(start >= span.start && end <= span.end && end > start);
        assert.equal(number(body, 'bytes'), Number(end - start));
        if (body.kind === 'observed_decoded_instruction_bytes') {
          for (let at = start; at < end; at++) assert(covered.has(at.toString()));
        }
        bodyBytes.set(body.kind, (bodyBytes.get(body.kind) || 0) + Number(end - start));
      }
      assert.equal(bodyBytes.get('observed_decoded_instruction_bytes') || 0, decoded);
      assert.equal(bodyBytes.get('unvisited_not_classified_as_code_or_data') || 0, expected.length - decoded);
    }
    passed('all selected memory hashes and decoded bytes match authored ELF physical spans');

    const good = statuses[0];
    assert.equal(good.flow_complete, 'true');
    assert.equal(number(good, 'issues'), 0);
    assert.equal(number(good, 'unvisited_bytes'), 0);
    assert.equal(number(good, 'pending_addresses'), 0);
    assert.equal(good.body_binding, 'skipped_no_bounded_listing_installation');
    assert.equal(good.decompilation, 'disabled_raw_trace_only_no_pseudocode');
    for (let i = 0; i < statuses.length; i++) noC(i);
    const goodInstructions = tsv(file(0, '.instructions.tsv'));
    assert(goodInstructions.some(row => /MOV EAX,0x64\b/i.test(row.asm)));
    assert(goodInstructions.some(row => /MOV EAX,0xfffffff9\b/i.test(row.asm)));
    const goodPcode = tsv(file(0, '.pcode.tsv'));
    assert.equal(goodPcode.filter(row => row.mnemonic === 'CBRANCH').length, 2);
    assert(!goodPcode.some(row => ['CALL', 'CALLIND', 'CALLOTHER'].includes(row.mnemonic)));
    const jumps = tsv(file(0, '.flows.tsv')).filter(row => row.kind === 'jump');
    assert.equal(jumps.length, 2);
    assert.deepEqual(jumps.map(row => address(row.target)).sort((a, b) => a < b ? -1 : 1),
      [symbol('authored_good_gate_sentinel').start, symbol('authored_good_gate_disabled').start]
        .sort((a, b) => a < b ? -1 : 1));
    assert(jumps.every(row => ['scheduled', 'already_scheduled'].includes(row.status)));
    passed('canonical conditional gate exports both sentinel/disabled branches with no pseudocode');

    partial(1);
    issue(1, 'unaudited_callother_unresolved');
    assert(tsv(file(1, '.pcode.tsv')).some(row => row.mnemonic === 'CALLOTHER'));
    passed('XBEGIN is partial and withholds C');

    partial(2);
    issue(2, 'instruction_reference_override');
    assert(issues(2).some(row => row.detail.includes('primary=true')));
    passed('primary jump reference override is partial and withholds C');

    const caller = statuses[3];
    assert.equal(caller.flow_complete, 'true', 'Call-only blocker permits a complete scoped raw walk');
    assert.equal(number(caller, 'issues'), 0);
    assert.equal(number(caller, 'unvisited_bytes'), 0);
    assert.equal(caller.decompilation, 'disabled_raw_trace_only_no_pseudocode');
    noC(3);
    assert.match(fs.readFileSync(file(3, '.decompiler.txt'), 'utf8'), /disabled/i);
    const calls = tsv(file(3, '.flows.tsv')).filter(row => row.kind === 'call');
    assert.equal(calls.length, 1);
    assert.equal(address(calls[0].target), bounds.get('authored_inline_callee').start);
    assert.equal(calls[0].status, 'not_followed');
    assert(bounds.get('authored_inline_callee').start >= spans[3].end);
    passed('outside inline callee is not followed; complete raw walk still blocks C explicitly');

    partial(4);
    issue(4, 'noncanonical_or_unknown_x86_64_context');
    assert.equal(number(statuses[4], 'instructions'), 0);
    assert.equal(number(statuses[4], 'decoded_instruction_bytes'), 0);
    assert(tsv(file(4, '.context.tsv')).some(row => row.register === 'longMode' &&
      row.value_hex === '0x0' && row.status === 'rejected_unknown_or_noncanonical'));
    passed('wrong execution mode is rejected before instruction decode');

    partial(5);
    issue(5, 'instruction_cap_4096');
    assert.equal(number(statuses[5], 'instructions'), 4096);
    assert.equal(number(statuses[5], 'decoded_instruction_bytes'), 4096);
    assert.equal(number(statuses[5], 'unvisited_bytes'), 905);
    assert(number(statuses[5], 'pending_addresses') > 0);
    passed('4096-instruction cap retains explicit partial status');

    partial(6);
    assert(issues(6).some(row => ['decode_insufficient_bytes_possible_straddling',
      'straddling_instruction'].includes(row.reason)), 'Expected exact-range straddle diagnostic');
    assert.equal(number(statuses[6], 'instructions'), 0);
    assert.equal(number(statuses[6], 'decoded_instruction_bytes'), 0);
    passed('exact-range straddle decodes no outside bytes and withholds C');

    partial(7);
    issue(7, 'pcode_cap_16384');
    assert(number(statuses[7], 'instructions') < 4096, 'Pcode cap must win before instruction cap');
    assert(number(statuses[7], 'pcode_ops_exported') > 0);
    assert(number(statuses[7], 'unvisited_bytes') > 0);
    passed('raw-pcode cap retains explicit partial status before instruction cap');

    const undefinedOutput = path.join(root, 'undefined-reference-output');
    const undefinedSpan = bounds.get('authored_undefined_override');
    await ghidraRun('undefined-reference', undefinedOutput, [rangeArg(undefinedSpan)]);
    assert(fs.existsSync(path.join(undefinedOutput, 'EXPORT_FINISHED.txt')));
    assert(!fs.existsSync(path.join(undefinedOutput, 'FAILED.txt')));
    const undefinedStatuses = tsv(path.join(undefinedOutput, 'status.tsv'));
    assert.equal(undefinedStatuses.length, 1);
    assert.equal(undefinedStatuses[0].flow_complete, 'false');
    assert.equal(number(undefinedStatuses[0], 'instructions'), 0);
    assert.equal(undefinedStatuses[0].decompilation, 'disabled_raw_trace_only_no_pseudocode');
    assert(tsv(path.join(undefinedOutput, 'range-01.issues.tsv'))
      .some(row => row.reason === 'instruction_reference_override' && row.detail.includes('primary=true')));
    const undefinedMemory = tsv(path.join(undefinedOutput, 'range-01.memory.tsv'));
    assert.equal(undefinedMemory.length, 1);
    assert.equal(undefinedMemory[0].hex, physicalBytes(undefinedSpan.start, undefinedSpan.end).toString('hex'));
    assert.equal(undefinedMemory[0].sha256, sha256(physicalBytes(undefinedSpan.start, undefinedSpan.end)));
    assert(!fs.existsSync(path.join(undefinedOutput, 'range-01.c')));
    passed('primary override on undefined RET rejected before raw instruction admission');

    for (const [name, ranges, diagnostic] of [
      ['malformed', ['not-a-range'], /IllegalArgumentException: Malformed start:end_exclusive range/],
      ['overlap', [rangeArg(spans[0]), rangeArg(spans[0])],
        /IllegalArgumentException: Duplicate or overlapping explicit ranges/],
    ]) {
      const rejected = path.join(root, `${name}-output`);
      const logs = await ghidraRun(name, rejected, ranges);
      assert.match(logs, diagnostic, `${name}: intended script rejection must appear in logs`);
      assert(!fs.existsSync(rejected), `${name}: rejected arguments must not create output directory`);
      passed(`${name} range rejected before output directory creation`);
    }

    const before = snapshot(output);
    fs.writeFileSync(path.join(root, 'existing-output.before.json'), `${JSON.stringify(before, null, 2)}\n`, { flag: 'wx' });
    const existingLogs = await ghidraRun('existing-output', output, [rangeArg(spans[0])]);
    assert.match(existingLogs, /FileAlreadyExistsException/, 'Existing output must be rejected explicitly');
    const after = snapshot(output);
    fs.writeFileSync(path.join(root, 'existing-output.after.json'), `${JSON.stringify(after, null, 2)}\n`, { flag: 'wx' });
    assert.deepEqual(after, before, 'Existing output file inventory and every SHA-256 must remain unchanged');
    passed('existing output rejected; complete output hash snapshot unchanged');

    for (const source of sourceHashes) {
      assert.equal(sha256(fs.readFileSync(source.frozen)), source.sha256, 'Frozen source changed');
    }
    assert.equal(sha256(fs.readFileSync(elfPath)), elfHash, 'Authored input ELF changed');
    assert(!interrupted, `Interrupted: ${interrupted}`);
    receipt.success = true;
  } catch (error) {
    receipt.success = false;
    receipt.error = error.stack || String(error);
    throw error;
  } finally {
    clearTimeout(watchdog);
    killActive('SIGKILL');
    receipt.state = 'closed';
    receipt.finished = new Date().toISOString();
    receipt.elapsedMs = Date.now() - started;
    record();
  }
  console.log(`PASS all authored Ghidra integration assertions; preserved ${root}`);
}

main().catch(error => {
  console.error(error.stack || String(error));
  process.exitCode = 1;
});
