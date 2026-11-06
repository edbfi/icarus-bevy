'use strict';

// Pure selected-input reader, independently authored. The caller must bind
// immutable EXE bytes and selected class addresses to independent current
// consumer/TPI evidence. This profile is not discovered from input contents.
// Names are opaque locations: no string bytes or function parameters are read.
const {inspectNativeClassParams, PROFILE: HEADER_PROFILE} = require('./native_class_params.cjs');
const {parse} = require('./pe_candidates.cjs');

const PROFILE = 'native_property_base_24_function_links_10_inputs_v1';
const CAPS = Object.freeze({
    properties: 4096,
    total_properties: 8192,
    functions: 4096,
    total_functions: 8192,
    output_bytes: 2 * 1024 * 1024,
});
const ERROR = 'invalid or unsupported native field inputs';
const need = condition => { if (!condition) throw Error(ERROR); };
const hex = value => '0x' + value.toString(16);
const plain = value => value !== null && typeof value === 'object' && !Array.isArray(value)
    && (Object.getPrototypeOf(value) === Object.prototype || Object.getPrototypeOf(value) === null);
const canonical = value => typeof value === 'bigint' && value > 0n && value <= 0xffffffffffffffffn
    && (value <= 0x7fffffffffffn || value >= 0xffff800000000000n);

function inspectNativeFieldInputs(bytes, options) {
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
    // Keep the header reader's eight-selector ceiling while avoiding caller
    // iterators and repeated getter reads at this API boundary. The header
    // reader still validates each copied address's grammar and uniqueness.
    const selectedAddresses = options.addresses;
    need(Array.isArray(selectedAddresses));
    const selectorCount = selectedAddresses.length;
    need(Number.isSafeInteger(selectorCount) && selectorCount > 0 && selectorCount <= 8);
    const addresses = [];
    for (let index = 0; index < selectorCount; index++) {
        need(Object.hasOwn(selectedAddresses, index));
        addresses.push(selectedAddresses[index]);
    }
    // Header admission checks the EXE hash BEFORE interpreting PE bytes and
    // rejects shared backing, malformed selectors, unsupported mappings/counts.
    // Its existing dependency/interface caps remain in force without expansion.
    const headers = inspectNativeClassParams(bytes, {
        sha256: options.sha256,
        addresses,
        profile: HEADER_PROFILE,
        limits: {
            properties: limits.properties,
            total_properties: limits.total_properties,
            functions: limits.functions,
        },
    });
    const image = parse(bytes);
    function span(pointer, length, code = false) {
        need(canonical(pointer) && Number.isSafeInteger(length) && length > 0);
        need(pointer >= image.base && pointer + BigInt(length) <= image.base + BigInt(image.imageBytes));
        const rva = Number(pointer - image.base), matches = image.locate(rva);
        need(matches.length === 1);
        const section = matches[0];
        need(rva + length <= section.rva + section.mappedBytes
            && !section.writable && !section.resourceOverlap);
        need(code ? section.code && section.executable
            : section.readable && section.initialized && !section.executable && !section.code);
        return section.raw + rva - section.rva;
    }
    function opaque(pointer, nullable) {
        if (pointer === 0n) { need(nullable); return '0x0'; }
        span(pointer, 1);
        return hex(pointer);
    }
    let propertyTotal = 0, functionTotal = 0;
    // Admit aggregate counts before allocating or reading the new link arrays.
    for (const header of headers.selected) {
        propertyTotal += header.property_parameter_pointers.length;
        functionTotal += header.function_link.count;
        need(propertyTotal <= limits.total_properties && functionTotal <= limits.total_functions);
    }
    const selected = headers.selected.map(header => {
        const propertyInputs = header.property_parameter_pointers.map((address, ordinal) => {
            // An opaque descriptor need not be aligned. Only the exact 36-byte
            // base input prefix is admitted; the declared trailing size/padding
            // and property-kind tails are not read or inferred.
            const at = span(BigInt(address), 36);
            return {
                ordinal,
                address,
                name_pointer: opaque(bytes.readBigUInt64LE(at), false),
                rep_notify_pointer: opaque(bytes.readBigUInt64LE(at + 8), true),
                property_flags: '0x' + bytes.readBigUInt64LE(at + 16).toString(16).padStart(16, '0'),
                gen_flags: bytes.readUInt32LE(at + 24),
                object_flags: bytes.readUInt32LE(at + 28),
                array_dim: bytes.readInt32LE(at + 32),
            };
        });
        const {pointer, count} = header.function_link;
        let functionLinks = [];
        if (count !== 0) {
            // The profile qualifies a 16-byte stride, not an additional base
            // alignment rule. All accesses use bounded byte-buffer reads.
            const start = span(BigInt(pointer), count * 16);
            functionLinks = Array.from({length: count}, (_, ordinal) => {
                const at = start + ordinal * 16;
                const factory = bytes.readBigUInt64LE(at);
                span(factory, 1, true);
                return {
                    ordinal,
                    factory_pointer: hex(factory),
                    name_pointer: opaque(bytes.readBigUInt64LE(at + 8), false),
                };
            });
        }
        return {
            address: header.address,
            declared_property_count: propertyInputs.length,
            declared_function_link_count: count,
            source_function_link_pointer: pointer,
            property_inputs: propertyInputs,
            function_links: functionLinks,
        };
    });
    const result = {
        // Report the pin admitted by the header reader, rather than reading
        // caller-owned options again after validation.
        input_sha256: headers.input_sha256,
        profile: PROFILE,
        header_profile: HEADER_PROFILE,
        image_base: headers.image_base,
        selected,
        total_property_inputs: propertyTotal,
        total_function_links: functionTotal,
        caller_provenance_verified: false,
        release_qualified: false,
        net_field_counts_qualified: false,
        net_field_order_qualified: false,
        instruction_boundaries_confirmed: false,
        unresolved_requirements: [
            'Caller must bind this profile, input hash and selected addresses to independent current consumer/TPI evidence.',
            'Required property/function names are nonnull opaque readonly locations; rep-notify locations may be null. Strings are not validated.',
            'ArrayDim is retained as signed input, including zero/negative values, without validation of runtime meaning.',
            'Property flags, GenFlags and object flags have no interpreted enum/filter meaning; no replicated counts or order follow.',
            'Property-kind tails, generated factory bindings, function parameters, inheritance and native net ordering remain unresolved.',
            'Function pointers identify mapped code bytes only, not instruction/function boundaries or callable behavior.',
        ],
    };
    need(Buffer.byteLength(JSON.stringify(result)) <= limits.output_bytes);
    return result;
}

module.exports = {inspectNativeFieldInputs, PROFILE, CAPS};
