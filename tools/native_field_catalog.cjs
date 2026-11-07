'use strict';

// Joins reader reports only. No file access, target interpretation or activation.
const {PROFILE: HEADER} = require('./native_class_params.cjs');
const {PROFILE: INPUTS} = require('./native_field_inputs.cjs');
const {PROFILE: NAMES} = require('./native_field_names.cjs');
const PROFILE = 'native_selected_field_catalog_v1';
const CAPS = Object.freeze({classes: 8, fields: 16384, names: 16384, output_bytes: 4 * 1024 * 1024});
const need = value => { if (!value) throw Error('invalid or unsupported native field catalog'); };
const address = value => typeof value === 'string' && /^0x[1-9a-f][0-9a-f]{0,15}$/.test(value);
const uint = value => Number.isInteger(value) && value >= 0 && value <= 0xffffffff;
function joinNativeFieldCatalog(headers, inputs, names, options) {
    try {
        // Snapshot JSON data once; accessors, sparse arrays and exotic objects are
        // refused rather than trusted as reader-owned immutable reports.
        function snapshot(value) {
            if (value === null || ['string', 'boolean'].includes(typeof value)) return value;
            if (typeof value === 'number') { need(Number.isSafeInteger(value)); return value; }
            need(value && typeof value === 'object');
            const array = Array.isArray(value);
            need(array || [Object.prototype, null].includes(Object.getPrototypeOf(value)));
            const result = array ? [] : {};
            const keys = Reflect.ownKeys(value);
            if (array) {
                need(value.length <= CAPS.fields && keys.length === value.length + 1);
                for (let index = 0; index < value.length; index++) need(Object.hasOwn(value, index));
            }
            for (const key of keys) {
                if (array && key === 'length') continue;
                need(typeof key === 'string' && key !== '__proto__');
                const descriptor = Object.getOwnPropertyDescriptor(value, key);
                need(descriptor && Object.hasOwn(descriptor, 'value'));
                result[key] = snapshot(descriptor.value);
            }
            return result;
        }
        // Bound serialized admission before recursively copying caller data.
        let nodes = 0, stringBytes = 0;
        function bounded(value, depth = 0) {
            need(++nodes <= 300000 && depth <= 16);
            if (typeof value === 'string') { stringBytes += Buffer.byteLength(value); need(stringBytes <= 8 * 1024 * 1024); }
            if (value && typeof value === 'object') {
                const keys = Reflect.ownKeys(value); need(keys.length <= CAPS.fields + 1);
                for (const key of keys) {
                    need(typeof key === 'string');
                    stringBytes += Buffer.byteLength(key); need(stringBytes <= 8 * 1024 * 1024);
                    const d = Object.getOwnPropertyDescriptor(value, key);
                    need(d && Object.hasOwn(d, 'value')); bounded(d.value, depth + 1);
                }
            }
        }
        bounded([headers, inputs, names, options]);
        [headers, inputs, names, options] = snapshot([headers, inputs, names, options]);
        need(options && options.profile === PROFILE);
        need(Object.keys(options).every(key => ['profile', 'addresses', 'limits'].includes(key)));
        const limits = {...CAPS};
        if (options.limits !== undefined) {
            need(options.limits && !Array.isArray(options.limits));
            for (const [key, value] of Object.entries(options.limits)) {
                need(Object.hasOwn(CAPS, key) && Number.isSafeInteger(value) && value >= 0 && value <= CAPS[key]);
                limits[key] = value;
            }
        }
        need(headers.profile === HEADER && inputs.profile === INPUTS && inputs.header_profile === HEADER && names.profile === NAMES);
        need(/^[a-f0-9]{64}$/.test(headers.input_sha256) && address(headers.image_base));
        for (const report of [inputs, names]) need(report.input_sha256 === headers.input_sha256 && report.image_base === headers.image_base);
        function index(rows, cap) {
            need(Array.isArray(rows) && rows.length <= cap);
            const map = new Map();
            for (const row of rows) { need(address(row.address) && !map.has(row.address)); map.set(row.address, row); }
            return map;
        }
        const hs = index(headers.selected, limits.classes), ins = index(inputs.selected, limits.classes);
        need(hs.size > 0 && hs.size === ins.size && [...hs.keys()].every(key => ins.has(key)));
        need(Array.isArray(names.selected) && names.selected.length <= limits.names);
        const ns = new Map();
        for (const row of names.selected) {
            need(address(row.address) && typeof row.name === 'string' && /^[!-~]{1,1024}$/.test(row.name));
            need(!ns.has(row.address) || ns.get(row.address) === row.name); ns.set(row.address, row.name);
        }
        need(Array.isArray(options.addresses) && options.addresses.length > 0 && options.addresses.length <= limits.classes);
        const selection = new Set(options.addresses);
        need(selection.size === options.addresses.length && selection.size === hs.size && [...selection].every(key => hs.has(key)));
        const owners = new Map(), descriptors = new Map(); let total = 0;
        function name(pointer, nullable = false) {
            if (nullable && pointer === '0x0') return null;
            need(ns.has(pointer)); return ns.get(pointer);
        }
        const selected = options.addresses.map(key => {
            const h = hs.get(key), i = ins.get(key);
            need(Array.isArray(h.property_parameter_pointers) && Array.isArray(i.property_inputs) && Array.isArray(i.function_links));
            need(i.declared_property_count === i.property_inputs.length && i.property_inputs.length === h.property_parameter_pointers.length);
            need(i.declared_function_link_count === i.function_links.length && h.function_link.count === i.function_links.length && h.function_link.pointer === i.source_function_link_pointer);
            total += i.property_inputs.length + i.function_links.length; need(total <= limits.fields);
            const properties = i.property_inputs.map((p, ordinal) => {
                need(p.ordinal === ordinal && address(p.address) && p.address === h.property_parameter_pointers[ordinal]);
                need(!owners.has(p.address) || owners.get(p.address) === key); owners.set(p.address, key);
                need(/^0x[0-9a-f]{16}$/.test(p.property_flags) && uint(p.gen_flags) && uint(p.object_flags));
                need(Number.isInteger(p.array_dim) && p.array_dim >= -2147483648 && p.array_dim <= 2147483647);
                const descriptor = JSON.stringify([p.name_pointer, p.rep_notify_pointer,
                    p.property_flags, p.gen_flags, p.object_flags, p.array_dim]);
                need(!descriptors.has(p.address) || descriptors.get(p.address) === descriptor);
                descriptors.set(p.address, descriptor);
                return {...p, name: name(p.name_pointer), rep_notify_name: name(p.rep_notify_pointer, true)};
            });
            const functions = i.function_links.map((f, ordinal) => {
                need(f.ordinal === ordinal && address(f.factory_pointer));
                return {...f, name: name(f.name_pointer)};
            });
            return {address: key, properties, functions};
        });
        const result = {profile: PROFILE, input_sha256: headers.input_sha256, image_base: headers.image_base,
            selected, release_qualified: false, net_field_order_qualified: false, ready: false,
            blockers: ['caller_consumer_provenance_unverified', 'ancestry_unbound', 'replication_selection_unbound',
                'runtime_offsets_and_cpp_types_unbound', 'net_order_unbound', 'release_profile_incomplete']};
        need(Buffer.byteLength(JSON.stringify(result)) <= limits.output_bytes);
        return result;
    } catch { throw Error('invalid or unsupported native field catalog'); }
}
module.exports = {joinNativeFieldCatalog, PROFILE, CAPS};
