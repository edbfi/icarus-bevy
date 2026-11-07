'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const {joinNativeFieldCatalog: join, PROFILE} = require('../native_field_catalog.cjs');
const {PROFILE: H} = require('../native_class_params.cjs');
const {PROFILE: I} = require('../native_field_inputs.cjs');
const {PROFILE: N} = require('../native_field_names.cjs');
function fixture() {
    const identity = {input_sha256: 'a'.repeat(64), image_base: '0x140000000'};
    const h = {...identity, profile: H, selected: []};
    const i = {...identity, profile: I, header_profile: H, selected: []};
    const n = {...identity, profile: N, selected: [{address: '0x400', name: 'Authored'}]};
    for (const [address, pointer] of [['0x100', '0x300'], ['0x200', '0x301']]) {
        h.selected.push({address, property_parameter_pointers: [pointer], function_link: {pointer: '0x500', count: 1}});
        i.selected.push({address, declared_property_count: 1, declared_function_link_count: 1,
            source_function_link_pointer: '0x500', property_inputs: [{address: pointer, ordinal: 0,
                name_pointer: '0x400', rep_notify_pointer: '0x0', property_flags: '0xffffffffffffffff',
                gen_flags: 4294967295, object_flags: 0, array_dim: -1}],
            function_links: [{ordinal: 0, factory_pointer: '0x600', name_pointer: '0x400'}]});
    }
    return [h, i, n, {profile: PROFILE, addresses: ['0x200', '0x100']}];
}
const reject = f => assert.throws(() => join(...f), {message: 'invalid or unsupported native field catalog'});
test('exact joins preserve selection, opaque inputs and explicit blockers without mutation', () => {
    const f = fixture(), before = structuredClone(f), r = join(...f);
    assert.deepEqual(f, before);
    assert.deepEqual(r.selected.map(c => c.address), f[3].addresses);
    assert.equal(r.selected[0].properties[0].property_flags, '0xffffffffffffffff');
    assert.equal(r.selected[0].properties[0].array_dim, -1);
    assert.equal(r.selected[0].properties[0].rep_notify_name, null);
    assert.equal(r.ready, false); assert.equal(r.release_qualified, false);
    assert.ok(r.blockers.includes('runtime_offsets_and_cpp_types_unbound'));
    f[0].selected.reverse(); f[1].selected.reverse();
    assert.deepEqual(join(...f), r);
});
test('identity, profiles, ownership, missing bindings and selection fail closed', () => {
    for (const mutate of [
        f => { f[1].input_sha256 = 'b'.repeat(64); },
        f => { f[2].image_base = '0x999'; },
        f => { f[0].profile = 'unknown'; },
        f => { f[2].selected = []; },
        f => { f[1].selected[0].property_inputs[0].ordinal = 1; },
        f => { f[1].selected[0].source_function_link_pointer = '0x999'; },
        f => { f[3].addresses = ['0x100', '0x100']; },
        f => { f[0].selected[1].property_parameter_pointers[0] = '0x300'; f[1].selected[1].property_inputs[0].address = '0x300'; },
        f => { f[2].selected.push({address: '0x400', name: 'Collision'}); },
        f => { f[1].selected.pop(); },
    ]) { const f = fixture(); mutate(f); reject(f); }
});
test('limits and output boundary are enforced', () => {
    const f = fixture(), size = Buffer.byteLength(JSON.stringify(join(...f)));
    f[3].limits = {output_bytes: size}; assert.ok(join(...f));
    for (const limits of [{output_bytes: size - 1}, {classes: 1}, {fields: 3}, {names: 0}, {fields: 16385}, {unknown: 1}]) {
        f[3].limits = limits; reject(f);
    }
});
test('accessors, cyclic reports and sparse arrays are rejected', () => {
    const getter = fixture(); Object.defineProperty(getter[0], 'selected', {get() { throw Error('private'); }}); reject(getter);
    const cycle = fixture(); cycle[0].cycle = cycle[0]; reject(cycle);
    const sparse = fixture(); sparse[3].addresses = new Array(2); reject(sparse);
});
test('duplicate descriptors within one owner and duplicate names retain ordinals', () => {
    const f = fixture(), h = f[0].selected[0], i = f[1].selected[0];
    h.property_parameter_pointers.push(h.property_parameter_pointers[0]);
    i.property_inputs.push({...i.property_inputs[0], ordinal: 1});
    i.declared_property_count = 2;
    f[2].selected.push({...f[2].selected[0], ordinal: 1});
    const r = join(...f).selected[1];
    assert.deepEqual(r.properties.map(p => p.ordinal), [0, 1]);
    assert.equal(r.properties[0].address, r.properties[1].address);
});
test('holes cannot be hidden by extra array properties', () => {
    const f = fixture(), h = f[0].selected[0], i = f[1].selected[0];
    h.property_parameter_pointers = new Array(2);
    h.property_parameter_pointers[1] = '0x300';
    h.property_parameter_pointers.extra = 'hidden hole';
    i.property_inputs = new Array(2);
    i.property_inputs[1] = {...fixture()[1].selected[0].property_inputs[0], ordinal: 1};
    i.property_inputs.extra = 'hidden hole';
    i.declared_property_count = 2;
    reject(f);
});
test('duplicate descriptors cannot disagree on input content', () => {
    for (const [key, value] of Object.entries({name_pointer: '0x401', rep_notify_pointer: '0x400',
        property_flags: '0x0000000000000000', gen_flags: 0, object_flags: 1, array_dim: 1})) {
        const f = fixture(), h = f[0].selected[0], i = f[1].selected[0];
        f[2].selected.push({address: '0x401', name: 'OtherAuthored'});
        h.property_parameter_pointers.push(h.property_parameter_pointers[0]);
        i.property_inputs.push({...i.property_inputs[0], ordinal: 1, [key]: value});
        i.declared_property_count = 2;
        reject(f);
    }
});
test('admission includes object key bytes in its budget', () => {
    const f = fixture();
    f[0]['x'.repeat(8 * 1024 * 1024)] = 0;
    reject(f);
});
test('empty field sets remain blocked and extra class selections reject', () => {
    const f = fixture();
    for (const h of f[0].selected) { h.property_parameter_pointers = []; h.function_link.count = 0; }
    for (const i of f[1].selected) {
        i.property_inputs = []; i.function_links = [];
        i.declared_property_count = 0; i.declared_function_link_count = 0;
    }
    f[2].selected = [];
    assert.equal(join(...f).ready, false);
    f[3].addresses.push('0x999'); reject(f);
});
