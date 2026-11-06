'use strict';

// Pure caller-selected name reader. This explicit ASCII/NUL profile is an
// authored restriction, not a claim about an original runtime's name encoding.
// The caller supplies immutable bytes, provenance and independently bound
// addresses. No files are opened and no native flag meanings are inferred.
const crypto = require('node:crypto');
const {isSharedArrayBuffer} = require('node:util').types;
const {parse, MAX_FILE_BYTES} = require('./pe_candidates.cjs');
const PROFILE = 'native_selected_readonly_ascii_names_v1';
const CAPS = Object.freeze({selectors: 16384, name_bytes: 1024,
    total_name_bytes: 4 * 1024 * 1024, output_bytes: 4 * 1024 * 1024});
const ERROR = 'invalid or unsupported native field names';
const need = condition => { if (!condition) throw Error(ERROR); };
const plain = value => value !== null && typeof value === 'object' && !Array.isArray(value)
    && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);

function inspectNativeFieldNames(bytes, options) {
    try { return inspectImpl(bytes, options); }
    catch { throw Error(ERROR); }
}
function inspectImpl(bytes, options) {
    need(plain(options));
    need(Reflect.ownKeys(options).every(key => typeof key === 'string'
        && ['sha256', 'addresses', 'profile', 'limits'].includes(key)));
    need(['sha256', 'addresses', 'profile'].every(key => Object.hasOwn(options, key)));
    need(options.profile === PROFILE);
    const limits = {...CAPS};
    if (options.limits !== undefined) {
        need(plain(options.limits));
        for (const key of Reflect.ownKeys(options.limits)) {
            need(typeof key === 'string' && Object.hasOwn(CAPS, key));
            const value = options.limits[key];
            need(Number.isSafeInteger(value) && value >= 0 && value <= CAPS[key]);
            limits[key] = value;
        }
    }
    need(Buffer.isBuffer(bytes) && bytes.length >= 64 && bytes.length <= MAX_FILE_BYTES
        && !isSharedArrayBuffer(bytes.buffer));
    const sha256 = options.sha256;
    need(typeof sha256 === 'string' && /^[a-f0-9]{64}$/.test(sha256));
    need(crypto.createHash('sha256').update(bytes).digest('hex') === sha256);
    const selectedAddresses = options.addresses;
    need(Array.isArray(selectedAddresses));
    const selectorCount = selectedAddresses.length;
    need(Number.isSafeInteger(selectorCount) && selectorCount > 0 && selectorCount <= limits.selectors);
    const addresses = [];
    for (let index = 0; index < selectorCount; index++) {
        need(Object.hasOwn(selectedAddresses, index));
        const address = selectedAddresses[index];
        need(typeof address === 'string' && /^0x[1-9a-f][0-9a-f]{0,15}$/.test(address));
        const pointer = BigInt(address);
        need(pointer <= 0x7fffffffffffn || pointer >= 0xffff800000000000n);
        addresses.push({address, pointer});
    }
    const image = parse(bytes), memo = new Map();
    let examined = 0;
    const selected = addresses.map(({address, pointer}, ordinal) => {
        if (!memo.has(address)) {
            need(pointer >= image.base && pointer < image.base + BigInt(image.imageBytes));
            const rva = Number(pointer - image.base), matches = image.locate(rva);
            need(matches.length === 1);
            const section = matches[0];
            need(section.readable && section.initialized && !section.writable
                && !section.code && !section.executable && !section.resourceOverlap);
            const offset = section.raw + rva - section.rva;
            const available = Math.min(section.rva + section.mappedBytes - rva, limits.name_bytes + 1);
            let length = 0, terminated = false;
            while (length < available) {
                need(examined < limits.total_name_bytes);
                const value = bytes[offset + length]; examined++;
                if (value === 0) { terminated = true; break; }
                need(value >= 0x21 && value <= 0x7e && length < limits.name_bytes);
                length++;
            }
            need(terminated && length > 0);
            memo.set(address, {address, name: bytes.toString('ascii', offset, offset + length),
                section_id: section.id, rva, file_offset: offset, bytes_including_nul: length + 1});
        }
        return {ordinal, ...memo.get(address)};
    });
    const result = {
        input_sha256: sha256, profile: PROFILE, image_base: '0x' + image.base.toString(16),
        selected, unique_addresses: memo.size, examined_name_bytes: examined,
        caller_provenance_verified: false, original_encoding_qualified: false,
        release_qualified: false, runtime_behavior_proven: false,
        net_field_counts_qualified: false, net_field_order_qualified: false,
        limitations: [
            'Caller must bind immutable input bytes, provenance and selected addresses independently.',
            'Only nonempty selected printable ASCII names terminated within one readonly initialized nonresource section are admitted.',
            'Repeated addresses preserve selector order and share one bounded name scan; distinct overlapping addresses are scanned separately.',
            'Names do not establish owners, property semantics, native consumers, replication filtering or ordering.',
        ],
    };
    need(Buffer.byteLength(JSON.stringify(result)) <= limits.output_bytes);
    return result;
}
module.exports = {inspectNativeFieldNames, PROFILE, CAPS};
