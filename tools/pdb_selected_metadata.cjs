'use strict';

// Offline source tooling only. Caller owns authorization, case scope and provenance.
// A private bounded snapshot binds every MSF/TPI read to the exact SHA256 input.
// This does not qualify a PE association, runtime semantics or compatibility.
// CLI: node tools/pdb_selected_metadata.cjs --request <bounded JSON request file>
// Request keys match run() below; results go only to an exclusively created file.
const fs = require('node:fs');
const path = require('node:path');
const {isProxy} = require('node:util').types;
const {createHash} = require('node:crypto');
const {msf, LIMITS} = require('./pdb_public_symbols.cjs');
const {inspectSelectedTypes, inspectSelectedEnums, PROFILE, PARAMETER_PROFILE, ENUM_PROFILE} = require('./pdb_selected_types.cjs');
const CAPS = Object.freeze({input_bytes: LIMITS.pdb, output_bytes: 65536, request_bytes: 65536});
const ERROR = 'selected metadata request failed';
const need = ok => { if (!ok) throw Error(ERROR); };
const plain = value => value && typeof value === 'object' && !Array.isArray(value)
    && [Object.prototype, null].includes(Object.getPrototypeOf(value));
const pathValue = value => typeof value === 'string' && value.length > 0 && value.length <= 4096 && !value.includes('\0');
const bound = (value, cap) => Number.isSafeInteger(value) && value > 0 && value <= cap;

// Copy descriptor values only: exported API accepts the same bounded data as JSON.
// Reject accessors/proxies without invoking caller code or retaining its objects.
function requestData(value) {
    let nodes = 0, budget = 0;
    function copy(item, depth = 0) {
        need(++nodes <= 2048 && depth <= 8);
        if (item === null || typeof item === 'boolean') return item;
        if (typeof item === 'number') { need(Number.isSafeInteger(item)); return item; }
        if (typeof item === 'string') {
            budget += Buffer.byteLength(item); need(budget <= CAPS.request_bytes); return item;
        }
        need(typeof item === 'object' && !isProxy(item));
        const array = Array.isArray(item);
        need(array ? Object.getPrototypeOf(item) === Array.prototype : plain(item));
        const keys = Reflect.ownKeys(item);
        need(keys.length <= 2048 && keys.every(key => typeof key === 'string'));
        const result = array ? [] : Object.create(null);
        if (array) {
            const length = Object.getOwnPropertyDescriptor(item, 'length').value;
            need(length <= 2048 && keys.length === length + 1);
            for (let index = 0; index < length; index++) need(Object.hasOwn(item, String(index)));
        }
        for (const key of keys) {
            if (array && key === 'length') continue;
            budget += Buffer.byteLength(key); need(budget <= CAPS.request_bytes);
            const descriptor = Object.getOwnPropertyDescriptor(item, key);
            need(Object.hasOwn(descriptor, 'value') && descriptor.enumerable);
            Object.defineProperty(result, key, {value: copy(descriptor.value, depth + 1), enumerable: true});
        }
        return result;
    }
    return copy(value);
}

function snapshot(path, cap, expectedBytes) {
    need(pathValue(path));
    const fd = fs.openSync(path, fs.constants.O_RDONLY | fs.constants.O_NONBLOCK | fs.constants.O_NOFOLLOW);
    try {
        const before = fs.fstatSync(fd, {bigint: true});
        need(before.isFile() && before.size > 0n && before.size <= BigInt(cap));
        const size = Number(before.size);
        need(expectedBytes === undefined || size === expectedBytes);
        const bytes = Buffer.alloc(size);
        let offset = 0;
        while (offset < size) {
            const count = fs.readSync(fd, bytes, offset, Math.min(size - offset, 1024 * 1024), offset);
            need(count > 0); offset += count;
        }
        const after = fs.fstatSync(fd, {bigint: true});
        need(before.size === after.size && before.mtimeNs === after.mtimeNs && before.ctimeNs === after.ctimeNs);
        return bytes;
    } finally { fs.closeSync(fd); }
}

function run(request) {
    try {
        request = requestData(request);
        need(plain(request));
        const required = ['pdb_path', 'pdb_sha256', 'pdb_bytes', 'identity', 'profile', 'names', 'output_path'];
        need(required.every(key => Object.hasOwn(request, key)));
        need(Reflect.ownKeys(request).every(key => required.includes(key)
            || ['limits', 'max_input_bytes', 'max_output_bytes'].includes(key)));
        const inputCap = request.max_input_bytes ?? CAPS.input_bytes;
        const outputCap = request.max_output_bytes ?? CAPS.output_bytes;
        need(bound(inputCap, CAPS.input_bytes) && bound(outputCap, CAPS.output_bytes));
        need(bound(request.pdb_bytes, inputCap) && /^[0-9a-f]{64}$/.test(request.pdb_sha256)
            && typeof request.pdb_sha256 === 'string' && pathValue(request.output_path));
        const id = request.identity;
        need(plain(id) && Reflect.ownKeys(id).length === 2 && Object.hasOwn(id, 'guid_bytes')
            && typeof id.guid_bytes === 'string' && /^[0-9a-f]{32}$/.test(id.guid_bytes)
            && Object.hasOwn(id, 'age') && Number.isInteger(id.age) && id.age > 0 && id.age <= 0xffffffff);
        need([PROFILE, PARAMETER_PROFILE, ENUM_PROFILE].includes(request.profile));
        const bytes = snapshot(request.pdb_path, inputCap, request.pdb_bytes);
        need(createHash('sha256').update(bytes).digest('hex') === request.pdb_sha256);
        const container = msf({size: bytes.length, read: (offset, length) => bytes.subarray(offset, offset + length)});
        need(container.streams[1].size >= 28 && container.streams[1].size <= 65536);
        const info = container.stream(1, 0, 28);
        need(info.readUInt32LE(0) === 20000404 && info.readUInt32LE(8) === id.age
            && info.subarray(12, 28).toString('hex') === id.guid_bytes);
        const inspect = request.profile === ENUM_PROFILE ? inspectSelectedEnums : inspectSelectedTypes;
        const result = inspect({size: container.streams[2].size,
            read: (offset, length) => container.stream(2, offset, length)},
        {profile: request.profile, names: request.names, ...(request.limits === undefined ? {} : {limits: request.limits})});
        const output = Buffer.from(JSON.stringify({pdb_sha256: request.pdb_sha256, pdb_bytes: bytes.length,
            identity: id, metadata: result}) + '\n');
        need(output.length <= outputCap);
        // Publish only complete, synced bytes; link never replaces an existing
        // path. The private staging directory is on the destination filesystem.
        const staging = fs.mkdtempSync(path.join(path.dirname(request.output_path), '.selected-metadata-'));
        const temporary = path.join(staging, 'report');
        let published = false;
        try {
            const fd = fs.openSync(temporary, fs.constants.O_WRONLY | fs.constants.O_CREAT
                | fs.constants.O_EXCL | fs.constants.O_NOFOLLOW, 0o600);
            try { fs.writeFileSync(fd, output); fs.fsyncSync(fd); }
            finally { fs.closeSync(fd); }
            fs.linkSync(temporary, request.output_path);
            published = true;
        } finally {
            try { fs.rmSync(staging, {recursive: true, force: true}); }
            catch (error) { if (!published) throw error; }
        }
        return {output_bytes: output.length};
    } catch { throw Error(ERROR); }
}

function main(argv) {
    try {
        need(argv.length === 2 && argv[0] === '--request');
        run(JSON.parse(snapshot(argv[1], CAPS.request_bytes).toString('utf8')));
        return 0;
    } catch { process.stderr.write(ERROR + '\n'); return 1; }
}
if (require.main === module) process.exitCode = main(process.argv.slice(2));
module.exports = {run, CAPS};
