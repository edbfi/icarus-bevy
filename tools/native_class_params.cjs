'use strict';

// Caller must establish input/selection provenance. No release qualification,
// discovery, descriptor interpretation or layouts outside this profile.
const crypto = require('node:crypto');
const {isSharedArrayBuffer} = require('node:util').types;
const {parse, MAX_FILE_BYTES} = require('./pe_candidates.cjs');

const PROFILE = 'native_class_params_header_4c_v1';
const CAPS = Object.freeze({
    dependencies: 64,
    properties: 4096,
    total_properties: 8192,
    interfaces: 128,
    total_interfaces: 512,
    functions: 4096,
    output_bytes: 1048576,
});
const check = condition => {
    if (!condition) throw Error('invalid native class parameters');
};
const hex = pointer => '0x' + pointer.toString(16);
const plainObject = value => value !== null && typeof value === 'object'
    && !Array.isArray(value)
    && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
const canonicalPointer = pointer => typeof pointer === 'bigint'
    && pointer > 0n && pointer <= 0xffffffffffffffffn
    && (pointer <= 0x7fffffffffffn || pointer >= 0xffff800000000000n);

function selectedAddresses(addresses) {
    check(Array.isArray(addresses) && addresses.length >= 1 && addresses.length <= 8);
    const selected = [], seen = new Set();
    for (const address of addresses) {
        check(typeof address === 'string' && /^0x(?:0|[1-9a-f][0-9a-f]{0,15})$/.test(address));
        // Only convert after validating the complete public string grammar.
        const pointer = BigInt(address);
        check(canonicalPointer(pointer) && pointer % 8n === 0n && !seen.has(address));
        seen.add(address);
        selected.push(pointer);
    }
    return selected;
}

function inspectNativeClassParams(bytes, options) {
    try {
        check(Buffer.isBuffer(bytes) && !isSharedArrayBuffer(bytes.buffer) && bytes.length <= MAX_FILE_BYTES);
        check(plainObject(options));
        const keys = Reflect.ownKeys(options);
        check(keys.every(key => typeof key === 'string'
            && ['sha256', 'addresses', 'profile', 'limits'].includes(key)));
        check(['sha256', 'addresses', 'profile'].every(key => Object.hasOwn(options, key)));
        const {sha256, addresses, profile} = options;
        check(typeof sha256 === 'string' && /^[a-f0-9]{64}$/.test(sha256) && profile === PROFILE);
        check(crypto.createHash('sha256').update(bytes).digest('hex') === sha256);

        const limits = {...CAPS};
        if (options.limits !== undefined) {
            check(plainObject(options.limits));
            for (const key of Reflect.ownKeys(options.limits)) {
                check(typeof key === 'string' && Object.hasOwn(CAPS, key));
                const value = options.limits[key];
                check(Number.isSafeInteger(value) && value >= 0 && value <= CAPS[key]);
                limits[key] = value;
            }
        }
        const selected = selectedAddresses(addresses);
        const image = parse(bytes);

        function span(pointer, length, code = false, alignment = 8n) {
            check(Number.isSafeInteger(length) && length > 0);
            check(canonicalPointer(pointer) && pointer % alignment === 0n
                && pointer >= image.base
                && pointer + BigInt(length) <= image.base + BigInt(image.imageBytes));
            const rva = Number(pointer - image.base), found = image.locate(rva);
            check(found.length === 1);
            const section = found[0];
            check(rva + length <= section.rva + section.mappedBytes
                && !section.writable && !section.resourceOverlap);
            check(code
                ? section.code && section.executable
                : section.readable && section.initialized && !section.executable && !section.code);
            return section.raw + rva - section.rva;
        }

        const code = pointer => {
            span(pointer, 1, true, 1n);
            return hex(pointer);
        };
        // Location only: no alignment or descriptor-content assertion follows.
        const opaque = pointer => {
            if (pointer !== 0n) span(pointer, 1, false, 1n);
            return hex(pointer);
        };
        let propertiesTotal = 0, interfacesTotal = 0;
        const records = selected.map(address => {
            const at = span(address, 76);
            const pointer = offset => bytes.readBigUInt64LE(at + offset);
            const dependenciesCount = bytes.readInt32LE(at + 56);
            const functionsCount = bytes.readUInt32LE(at + 60);
            const propertiesCount = bytes.readInt32LE(at + 64);
            const interfacesCount = bytes.readInt32LE(at + 68);
            check(dependenciesCount >= 0 && dependenciesCount <= limits.dependencies
                && functionsCount <= limits.functions
                && propertiesCount >= 0 && propertiesCount <= limits.properties
                && interfacesCount >= 0 && interfacesCount <= limits.interfaces);
            propertiesTotal += propertiesCount;
            interfacesTotal += interfacesCount;
            check(propertiesTotal <= limits.total_properties && interfacesTotal <= limits.total_interfaces);

            function array(arrayPointer, count, stride, read) {
                if (count === 0) {
                    opaque(arrayPointer);
                    return [];
                }
                // Nonempty arrays, unlike opaque pointees, retain alignment8.
                const start = span(arrayPointer, count * stride);
                return Array.from({length: count}, (_, index) => read(start + index * stride));
            }
            const dependencies = array(pointer(24), dependenciesCount, 8,
                offset => code(bytes.readBigUInt64LE(offset)));
            const properties = array(pointer(40), propertiesCount, 8, offset => {
                const value = bytes.readBigUInt64LE(offset);
                check(value !== 0n);
                return opaque(value);
            });
            const interfaces = array(pointer(48), interfacesCount, 16, offset => {
                const getter = bytes.readBigUInt64LE(offset);
                return {
                    getter: getter !== 0n ? code(getter) : '0x0',
                    word_08: bytes.readUInt32LE(offset + 8),
                    raw_byte_0c: bytes[offset + 12],
                };
            });
            const link = pointer(32);
            if (functionsCount !== 0) check(link !== 0n);
            opaque(link);
            return {
                address: hex(address),
                factory_pointer: code(pointer(0)),
                opaque_conversion_input: opaque(pointer(8)),
                opaque_slot_10: opaque(pointer(16)),
                dependencies,
                property_parameter_pointers: properties,
                interface_records: interfaces,
                function_link: {pointer: hex(link), count: functionsCount},
                parameter_word_48: bytes.readUInt32LE(at + 72),
            };
        });
        const result = {
            input_sha256: sha256,
            profile,
            image_base: hex(image.base),
            release_qualified: false,
            selected: records,
            unresolved_requirements: [
                'Caller must establish input and selected-address provenance.',
                'Function-link stride and extent remain unresolved; entries are not read.',
                'Opaque slots, property descriptors, interface meanings and layouts outside this profile remain unresolved.',
                'Runtime bindings and release compatibility remain unqualified.',
            ],
        };
        check(Buffer.byteLength(JSON.stringify(result)) <= limits.output_bytes);
        return result;
    } catch {
        throw Error('invalid native class parameters');
    }
}

module.exports = {inspectNativeClassParams, PROFILE, CAPS};
