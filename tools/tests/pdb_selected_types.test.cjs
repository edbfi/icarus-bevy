'use strict';
// Independently authored synthetic TPI records only. No original inputs.
const test = require('node:test');
const assert = require('node:assert/strict');
const vm = require('node:vm');
const {inspectSelectedTypes, PROFILE, CAPS, inspectSelectedEnums, ENUM_PROFILE} = require('../pdb_selected_types.cjs');

const TARGET = 'Authored::Parameters';
const u8 = n => Buffer.from([n]);
const u16 = n => { const b = Buffer.alloc(2); b.writeUInt16LE(n); return b; };
const u32 = n => { const b = Buffer.alloc(4); b.writeUInt32LE(n); return b; };
const z = name => Buffer.from(name + '\0', 'ascii');
function padded(bytes) {
    const n = (4 - bytes.length % 4) % 4;
    return Buffer.concat([bytes, Buffer.from(Array.from({length: n}, (_, index) => 0xf0 + n - index))]);
}
function record(leaf, ...parts) {
    const b = padded(Buffer.concat([Buffer.alloc(2), u16(leaf), ...parts]));
    b.writeUInt16LE(b.length - 2); return b;
}
function member(name, type = 0x75, offset = 0) {
    return padded(Buffer.concat([u16(0x150d), u16(3), u32(type), u16(offset), z(name)]));
}
const continuation = index => Buffer.concat([u16(0x1404), u16(0), u32(index)]);
const list = (...members) => record(0x1203, ...members);
function udt({name = TARGET, fields = 0x1000, count = 1, size = 4, flags = 0,
    unique = null, leaf = 0x1505, sizeEncoding = null} = {}) {
    const options = flags | (unique === null ? 0 : 0x200);
    return record(leaf, u16(count), u16(options), u32(fields), u32(0), u32(0),
        sizeEncoding === null ? u16(size) : sizeEncoding, z(name),
        unique === null ? Buffer.alloc(0) : z(unique));
}
function stream(records) {
    const data = Buffer.concat(records), h = Buffer.alloc(56);
    h.writeUInt32LE(20040203, 0); h.writeUInt32LE(56, 4);
    h.writeUInt32LE(0x1000, 8); h.writeUInt32LE(0x1000 + records.length, 12);
    h.writeUInt32LE(data.length, 16); h.writeUInt16LE(0xffff, 20); h.writeUInt16LE(0xffff, 22);
    h.writeUInt32LE(4, 24); h.writeUInt32LE(4096, 28);
    return Buffer.concat([h, data]);
}
const reader = bytes => ({size: bytes.length, read: (offset, length) => bytes.subarray(offset, offset + length)});
const options = (limits = {}) => ({profile: PROFILE, names: [TARGET], limits});
const fixture = () => stream([list(member('Count')), udt()]);
const inspect = (bytes, limits = {}) => inspectSelectedTypes(reader(bytes), options(limits));
function fails(fn) {
    assert.throws(fn, error => error instanceof Error
        && error.message === 'invalid or unsupported selected TPI types');
}

test('exact selected layout reports offsets and widths without qualification claims', () => {
    const result = inspect(fixture());
    assert.equal(result.records_framed, 2);
    assert.equal(result.selected[0].size_bytes, 4);
    assert.deepEqual(result.selected[0].fields, [{name: 'Count', attributes: 3,
        type_index: 0x75, offset_bytes: 0, kind: 'primitive', storage_kind: 'integer', width_bytes: 4, signed: false}]);
    for (const key of ['caller_provenance_verified', 'original_layout_qualified',
        'release_qualified', 'runtime_behavior_proven', 'hash_tables_validated']) assert.equal(result[key], false);
});

test('selection is exact and ordered; unrelated record payloads remain opaque', () => {
    const bytes = stream([list(member('A')), udt(), udt({name: TARGET + 'Extra'}),
        udt({name: 'Other::Parameters'}), record(0x7777, Buffer.from([0xde, 0xad, 0xbe, 0xef]))]);
    const result = inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: ['Other::Parameters', TARGET]});
    assert.deepEqual(result.selected.map(value => value.name), ['Other::Parameters', TARGET]);
    assert.equal(result.unselected_payloads_validated, false);
    fails(() => inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: ['Parameters']}));
});

test('unselected UDT semantic tails and size values remain opaque', () => {
    const name = 'Other::Malformed';
    const trailing = udt({name});
    const malformed = Buffer.concat([trailing, Buffer.from([1, 2, 3, 4])]);
    malformed.writeUInt16LE(malformed.length - 2);
    const negative = udt({name, sizeEncoding: Buffer.from([0, 0x80, 0xff])});
    for (const unrelated of [malformed, negative]) {
        const bytes = stream([list(member('Count')), udt(), unrelated]);
        assert.equal(inspect(bytes).selected[0].name, TARGET);
        fails(() => inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: [TARGET, name]}));
    }
});

test('forward declarations are retained and bind one complete definition', () => {
    const unique = '.?AUParameters@Authored@@';
    const bytes = stream([list(member('Count')), udt({fields: 0, count: 0, size: 0, flags: 0x80, unique}), udt({unique})]);
    const result = inspect(bytes).selected[0];
    assert.equal(result.type_index, 0x1002);
    assert.equal(result.forward_declarations.length, 1);
    assert.equal(result.forward_declarations[0].type_index, 0x1001);
    assert.equal(result.unique_name, unique);
});

test('missing, forward-only, duplicate and unique-name-conflicting definitions fail', () => {
    fails(() => inspect(stream([udt({name: 'PrivateSentinel', fields: 0, count: 0})])));
    fails(() => inspect(stream([udt({fields: 0, count: 0, size: 0, flags: 0x80})])));
    fails(() => inspect(stream([list(member('Count')), udt(), udt()])));
    fails(() => inspect(stream([list(member('Count')), udt({fields: 0, count: 0, size: 0, flags: 0x80, unique: 'Wrong'}), udt({unique: 'Right'})])));
});

test('ordinary64 pointer, modifier, enum and bitfield dependencies resolve widths', () => {
    const bytes = stream([
        record(0x1002, u32(3), u32(0x1000c)),
        record(0x1001, u32(0x1000), u16(1)),
        record(0x1205, u32(0x75), u8(3), u8(5)),
        record(0x1507, u16(0), u16(0), u32(0x75), u32(0), z('AuthoredEnum')),
        list(member('Pointer', 0x1001, 0), member('Bits', 0x1002, 8), member('Enum', 0x1003, 12)),
        udt({fields: 0x1004, count: 3, size: 16}),
    ]);
    const fields = inspect(bytes).selected[0].fields;
    assert.deepEqual(fields.map(value => value.width_bytes), [8, 4, 4]);
    assert.equal(fields[0].kind, 'modifier');
    assert.equal(fields[1].bit_size, 3); assert.equal(fields[1].bit_offset, 5);
    assert.equal(fields[2].enumerators_interpreted, false);
});

test('direct pointer attributes preserve legacy output and remain distinct in parameter layouts', () => {
    const bytes = stream([record(0x1002, u32(3), u32(0x1000c)),
        list(member('Pointer', 0x1000)), udt({fields: 0x1001, size: 8})]);
    assert.deepEqual(inspect(bytes).selected[0].fields[0], {
        name: 'Pointer', attributes: 0x1000c, type_index: 0x1000, offset_bytes: 0,
        kind: 'pointer', storage_kind: 'pointer', width_bytes: 8, referent_type_index: 3,
    });
    const field = inspectSelectedTypes(reader(bytes), {
        profile: PARAMETER_PROFILE, names: [TARGET],
    }).selected[0].fields[0];
    assert.equal(field.name, 'Pointer');
    assert.equal(field.attributes, 3);
    assert.equal(field.pointer_attributes, 0x1000c);
});

test('primitive64 pointers do not require standalone pointer records', () => {
    const result = inspect(stream([list(member('Pointer', 0x603)), udt({size: 8})]));
    assert.equal(result.selected[0].fields[0].width_bytes, 8);
    assert.equal(result.selected[0].fields[0].referent_type_index, 3);
});

test('fieldlist continuation preserves field order across lists', () => {
    const bytes = stream([list(member('A'), continuation(0x1001)), list(member('B', 0x75, 4)),
        udt({fields: 0x1000, count: 2, size: 8})]);
    assert.deepEqual(inspect(bytes).selected[0].fields.map(value => [value.name, value.offset_bytes]), [['A', 0], ['B', 4]]);
    fails(() => inspect(bytes, {continuations: 1}));
});

test('continuation cycles, nonterminal continuation and wrong target kinds reject', () => {
    fails(() => inspect(stream([list(continuation(0x1001)), list(continuation(0x1000)), udt({count: 0})])));
    fails(() => inspect(stream([list(continuation(0x1001), member('A')), list(), udt()])));
    fails(() => inspect(stream([list(continuation(0x1001)), udt({count: 0})])));
    const bad = continuation(0x1001); bad.writeUInt16LE(1, 2);
    fails(() => inspect(stream([list(bad), list(), udt({count: 0})])));
});

test('selected unsupported member kind, type kind, inheritance and bad field extent reject', () => {
    fails(() => inspect(stream([list(Buffer.concat([u16(0x150f), Buffer.alloc(6)])), udt()])));
    fails(() => inspect(stream([list(member('Opaque', 0x1001)), record(0x7777), udt({fields: 0x1000})])));
    fails(() => inspect(stream([list(member('TooFar', 0x75, 1)), udt()])));
    const inherited = udt(); inherited.writeUInt32LE(0x1000, 12);
    fails(() => inspect(stream([list(member('Count')), inherited])));
    fails(() => inspect(stream([list(member('Count')), udt({count: 2})])));
});

test('invalid pointer modes, bitfield extents and dependent type cycles reject', () => {
    fails(() => inspect(stream([record(0x1002, u32(3), u32(0x1004c)), list(member('P', 0x1000)), udt({fields: 0x1001, size: 8})])));
    fails(() => inspect(stream([record(0x1205, u32(0x75), u8(32), u8(1)), list(member('B', 0x1000)), udt({fields: 0x1001})])));
    fails(() => inspect(stream([record(0x1001, u32(0x1001), u16(1)), record(0x1001, u32(0x1000), u16(1)), list(member('C', 0x1000)), udt({fields: 0x1002})])));
    fails(() => inspect(stream([list(member('C', 0x80001000)), udt()])));
});

test('supported numeric encodings are explicit and negative or oversized values reject', () => {
    for (const encoded of [Buffer.concat([u16(0x8002), u16(4)]), Buffer.concat([u16(0x8004), u32(4)]),
        Buffer.concat([u16(0x800a), Buffer.from([4, 0, 0, 0, 0, 0, 0, 0])])]) {
        assert.equal(inspect(stream([list(member('Count')), udt({sizeEncoding: encoded})])).selected[0].size_bytes, 4);
    }
    fails(() => inspect(stream([list(member('Count')), udt({sizeEncoding: Buffer.concat([u16(0x8001), Buffer.from([0xff, 0xff])])})])));
    fails(() => inspect(stream([list(member('Count')), udt({sizeEncoding: Buffer.concat([u16(0x800a), Buffer.alloc(8, 0xff)])})])));
    fails(() => inspect(stream([list(member('Count')), udt({sizeEncoding: u16(0x8005)})])));
});

test('header profile, count, record and exact extent mutations fail closed', () => {
    for (const [offset, value] of [[0, 19990903], [4, 52], [8, 4095], [12, 0x1003],
        [16, 0], [24, 8], [28, 0], [28, 4095], [36, 4], [44, 1], [52, 1]]) {
        const bytes = fixture(); bytes.writeUInt32LE(value, offset);
        fails(() => inspect(bytes));
    }
    const bytes = fixture(); bytes.writeUInt16LE(1, 56);
    fails(() => inspect(bytes));
    fails(() => inspect(Buffer.concat([fixture(), Buffer.from([0])])));
    fails(() => inspect(fixture().subarray(0, fixture().length - 1)));
});

test('selected padding, names, unknown options and limits are strictly bounded', () => {
    const badPadding = udt({name: TARGET + 'X'});
    // Append another complete record byte instead of relying on a NUL scan to
    // silently consume any tail: exact semantic consumption rejects it.
    const overlong = Buffer.concat([badPadding, Buffer.from([0xf0, 0xf0, 0xf0, 0xf0])]);
    overlong.writeUInt16LE(overlong.length - 2);
    fails(() => inspectSelectedTypes(reader(stream([list(member('Count')), overlong])), {profile: PROFILE, names: [TARGET + 'X']}));
    for (const limits of [{records: 1}, {record_bytes: 4}, {read_bytes: 55}, {fields: 0},
        {retained_bytes: 0}, {output_bytes: 1}, {candidates: 0}, {udt_bytes: 3},
        {fields: CAPS.fields + 1}, {unknown: 1}]) fails(() => inspect(fixture(), limits));
    fails(() => inspectSelectedTypes(reader(fixture()), {...options(), extra: 'PRIVATE'}));
    fails(() => inspectSelectedTypes(reader(fixture()), {...options(), names: [TARGET, TARGET]}));
    fails(() => inspectSelectedTypes(reader(fixture()), {...options(), names: ['Not\u0000Allowed']}));
});

test('cache crossing stays within lowered read cap and preserves exact result', () => {
    const bytes = fixture(), reads = [];
    const input = {size: bytes.length, read(offset, length) {
        reads.push([offset, length]); return bytes.subarray(offset, offset + length);
    }};
    assert.equal(inspectSelectedTypes(input, options({read_bytes: 64})).selected[0].fields[0].name, 'Count');
    assert.ok(reads.every(([offset, length]) => length <= 64 && offset + length <= bytes.length));
});

test('short reads and same/cross-realm shared backing are refused', () => {
    const bytes = fixture();
    fails(() => inspectSelectedTypes({size: bytes.length, read: () => Buffer.alloc(1)}, options()));
    for (const backing of [new SharedArrayBuffer(bytes.length), vm.runInNewContext(`new SharedArrayBuffer(${bytes.length})`)]) {
        const shared = Buffer.from(backing); bytes.copy(shared);
        fails(() => inspectSelectedTypes(reader(shared), options()));
    }
});

test('late unsupported selected result is never returned as a successful prefix', () => {
    const bytes = stream([list(member('Count')), udt(), list(Buffer.concat([u16(0x150f), Buffer.alloc(6)])),
        udt({name: 'PrivateSentinel', fields: 0x1002})]);
    let returned = false;
    fails(() => { inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: [TARGET, 'PrivateSentinel']}); returned = true; });
    assert.equal(returned, false);
    try { inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: ['MissingPrivateSentinel']}); }
    catch (error) { assert.equal(error.message, 'invalid or unsupported selected TPI types'); }
});

test('dependency count and recursion caps lower without changing successful layouts', () => {
    const bytes = stream([record(0x1002, u32(3), u32(0x1000c)), record(0x1001, u32(0x1000), u16(1)),
        list(member('P', 0x1001)), udt({fields: 0x1002, size: 8})]);
    const result = inspect(bytes, {dependencies: 2, depth: 1});
    assert.deepEqual(result.dependent_types.map(value => [value.type_index, value.width_bytes]), [[0x1000, 8], [0x1001, 8]]);
    fails(() => inspect(bytes, {dependencies: 1}));
    fails(() => inspect(bytes, {depth: 0}));
});

test('exact output and retained-byte budgets pass while one lower rejects atomically', () => {
    const bytes = fixture(), expected = inspect(bytes);
    const outputBytes = Buffer.byteLength(JSON.stringify(expected));
    assert.deepEqual(inspect(bytes, {output_bytes: outputBytes}), expected);
    fails(() => inspect(bytes, {output_bytes: outputBytes - 1}));
    assert.deepEqual(inspect(bytes, {retained_bytes: expected.charged_retained_bytes}), expected);
    fails(() => inspect(bytes, {retained_bytes: expected.charged_retained_bytes - 1}));
});

// Enum extension fixtures are independently authored. LF_ENUMERATE is the
// modern leaf: attributes, encoded integer, zero-terminated name, CV padding.
const ENUM_NAME = 'Authored::Flags';
function enumNumber(value, leaf = 0x800a) {
    const widths = new Map([[0x8000,1],[0x8001,2],[0x8002,2],[0x8003,4],[0x8004,4],[0x8009,8],[0x800a,8]]);
    const bytes = Buffer.alloc(widths.get(leaf));
    if (leaf === 0x8009) bytes.writeBigInt64LE(BigInt(value));
    else if (leaf === 0x800a) bytes.writeBigUInt64LE(BigInt(value));
    else if ([0x8000,0x8001,0x8003].includes(leaf)) bytes.writeIntLE(Number(value),0,bytes.length);
    else bytes.writeUIntLE(Number(value),0,bytes.length);
    return Buffer.concat([u16(leaf),bytes]);
}
const enumerator = (name,value,encoding=null,attributes=3) => padded(Buffer.concat([
    u16(0x1502),u16(attributes),encoding===null?enumNumber(value):encoding,z(name)]));
function enumTag({name=ENUM_NAME, count=1, fields=0x1000, underlying=0x77,
    flags=0,unique=null}={}) {
    return record(0x1507,u16(count),u16(flags|(unique===null?0:0x200)),u32(underlying),
        u32(fields),z(name),unique===null?Buffer.alloc(0):z(unique));
}
const enumInspect = (bytes,limits={},names=[ENUM_NAME]) => inspectSelectedEnums(reader(bytes),{profile:ENUM_PROFILE,names,limits});
const enumFixture = () => stream([list(enumerator('High',1n<<63n),enumerator('Maximum',(1n<<64n)-1n)),enumTag({count:2})]);

test('enum selectors snapshot changing getters and ignore custom infinite iterators', () => {
    const bytes = enumFixture(), names = [ENUM_NAME];
    let getterReads = 0, iteratorCalls = 0;
    names[Symbol.iterator] = function* () {
        iteratorCalls++;
        while (true) yield 'UnselectedPrivateSentinel';
    };
    const result = inspectSelectedEnums(reader(bytes), {profile: ENUM_PROFILE, limits: {selectors: 1},
        get names() { return ++getterReads === 1 ? names : ['UnselectedPrivateSentinel']; },
    });
    assert.deepEqual(result.selected.map(value => value.name), [ENUM_NAME]);
    assert.equal(getterReads, 1); assert.equal(iteratorCalls, 0);
    const inherited = new Array(1);
    Object.setPrototypeOf(inherited, Object.assign(Object.create(Array.prototype), {0: ENUM_NAME}));
    for (const invalid of [new Array(1), inherited, [ENUM_NAME, ENUM_NAME]]) {
        fails(() => enumInspect(bytes, {selectors: 1}, invalid));
    }
});

test('enum unsigned 64-bit values remain decimal strings with declaration-only limits',()=>{
    const r=enumInspect(enumFixture()),e=r.selected[0];
    assert.equal(r.profile,ENUM_PROFILE);assert.equal(e.width_bytes,8);assert.equal(e.signed,false);
    assert.equal(e.declared_enumerator_count,2);
    assert.deepEqual(e.enumerators,[{name:'High',attributes:3,value_decimal:'9223372036854775808'},
        {name:'Maximum',attributes:3,value_decimal:'18446744073709551615'}]);
    assert.equal(r.release_qualified,false);assert.equal(r.runtime_behavior_proven,false);
    assert.equal(r.original_layout_qualified,false);assert.equal(r.caller_provenance_verified,false);
    assert.equal(r.unselected_payloads_validated,false);assert.equal(r.hash_tables_validated,false);
    assert.doesNotThrow(()=>JSON.stringify(r));
});

test('enum signed 64-bit minima/maxima and negative values preserve exact signs',()=>{
    const bytes=stream([list(enumerator('Min',0,enumNumber(-(1n<<63n),0x8009)),
        enumerator('MinusOne',0,enumNumber(-1,0x8000)),enumerator('Max',0,enumNumber((1n<<63n)-1n,0x8009))),
        enumTag({underlying:0x76,count:3})]);
    const e=enumInspect(bytes).selected[0];
    assert.equal(e.signed,true);assert.deepEqual(e.enumerators.map(x=>x.value_decimal),['-9223372036854775808','-1','9223372036854775807']);
});

test('enum encoded integer widths and direct immediate values are handled independently',()=>{
    const encodings=[u16(0),u16(0x7fff),enumNumber(-128,0x8000),enumNumber(-32768,0x8001),
        enumNumber(65535,0x8002),enumNumber(-2147483648,0x8003),enumNumber(4294967295,0x8004)];
    const values=['0','32767','-128','-32768','65535','-2147483648','4294967295'];
    const bytes=stream([list(...encodings.map((enc,i)=>enumerator('Value'+i,0,enc))),enumTag({underlying:0x76,count:encodings.length})]);
    assert.deepEqual(enumInspect(bytes).selected[0].enumerators.map(x=>x.value_decimal),values);
});

test('enum continuation order and equal-value aliases are retained completely',()=>{
    const bytes=stream([list(enumerator('First',1),continuation(0x1001)),
        list(enumerator('Alias',1),enumerator('Last',2)),enumTag({count:3})]);
    assert.deepEqual(enumInspect(bytes).selected[0].enumerators.map(x=>x.name),['First','Alias','Last']);
    fails(()=>enumInspect(bytes,{continuations:1}));
    const cycle=stream([list(enumerator('First',1),continuation(0x1000)),enumTag()]);
    fails(()=>enumInspect(cycle));
});

test('enum exact selection ignores UDT names and minimally discovers unrelated enums',()=>{
    const unrelated=enumTag({name:'Unrelated',flags:0x200,unique:'A'});
    unrelated[unrelated.length-1]=0x7f; // invalid unselected suffix remains uninterpreted
    const bytes=stream([list(enumerator('First',1)),enumTag(),udt({name:ENUM_NAME}),unrelated,
        enumTag({name:'Other',underlying:0x77})]);
    const r=enumInspect(bytes,{},['Other',ENUM_NAME]);
    assert.deepEqual(r.selected.map(x=>x.name),['Other',ENUM_NAME]);
    assert.equal(r.unselected_payloads_validated,false);
    fails(()=>enumInspect(bytes,{},[ENUM_NAME+'Extra']));
});

test('enum forward declarations require one matching unambiguous definition',()=>{
    const bytes=stream([list(enumerator('First',1)),enumTag({flags:0x80,count:0,fields:0}),enumTag()]);
    assert.equal(enumInspect(bytes).selected[0].forward_declarations.length,1);
    fails(()=>enumInspect(stream([list(enumerator('First',1)),enumTag(),enumTag()])));
    fails(()=>enumInspect(stream([enumTag({flags:0x80,count:0,fields:0})])));
    fails(()=>enumInspect(stream([list(enumerator('First',1)),enumTag({flags:0x80,count:0,fields:0,underlying:0x76}),enumTag()])));
    fails(()=>enumInspect(stream([list(enumerator('First',1)),enumTag({flags:0x80,count:0,fields:0,unique:'One'}),enumTag({unique:'Two'})])));
    fails(()=>enumInspect(bytes,{candidates:1}));
});

test('enum incomplete counts, unsupported members and out-of-range integers fail atomically',()=>{
    for(const bytes of [stream([list(enumerator('First',1)),enumTag({count:2})]),
        stream([list(enumerator('First',1)),enumTag({count:0})]),
        stream([list(member('NotEnumerator')),enumTag()]),
        stream([list(enumerator('Negative',0,enumNumber(-1,0x8009))),enumTag()]),
        stream([list(enumerator('TooLarge',256)),enumTag({underlying:0x69})]),
        stream([list(enumerator('BadFloat',0,Buffer.concat([u16(0x8005),Buffer.alloc(4)]))),enumTag()]),
        stream([list(enumerator('Repeat',1),enumerator('Repeat',2)),enumTag({count:2})])])fails(()=>enumInspect(bytes));
    const bytes=stream([list(enumerator('First',1)),enumTag(),list(member('LateWrongKind')),enumTag({name:'Other',fields:0x1002})]);
    fails(()=>enumInspect(bytes,{},[ENUM_NAME,'Other']));
});

test('enum underlying integer modifiers are bounded; pointer/bool/enum bases reject',()=>{
    const modifier=record(0x1001,u32(0x77),u16(1));
    const bytes=stream([list(enumerator('First',1)),modifier,enumTag({underlying:0x1001})]);
    assert.equal(enumInspect(bytes).selected[0].width_bytes,8);
    assert.equal(enumInspect(bytes).dependent_types.length,1);
    fails(()=>enumInspect(bytes,{dependencies:0}));
    fails(()=>enumInspect(bytes,{depth:0}));
    for(const underlying of [0x30,0x677,0,0x1001]){
        const bad=stream([list(enumerator('First',1)),enumTag({underlying})]);fails(()=>enumInspect(bad));
    }
});

test('enum lower-only aggregate enumerator/output limits preserve complete admission',()=>{
    const bytes=enumFixture(),r=enumInspect(bytes),size=Buffer.byteLength(JSON.stringify(r));
    assert.equal(enumInspect(bytes,{enumerators:2}).selected[0].enumerators.length,2);
    fails(()=>enumInspect(bytes,{enumerators:1}));
    // Lowering output cap alone does not change charged_retained_bytes.
    assert.deepEqual(enumInspect(bytes,{output_bytes:size}),r);
    fails(()=>enumInspect(bytes,{output_bytes:size-1}));
    const multiple=stream([list(enumerator('First',1)),enumTag(),enumTag({name:'Other'})]);
    fails(()=>enumInspect(multiple,{enumerators:1},[ENUM_NAME,'Other']));
    for(const limits of [{enumerators:4097},{enumerators:-1},{enumerators:1.5},{candidates:0},{output_bytes:0}])fails(()=>enumInspect(bytes,limits));
    fails(()=>inspectSelectedEnums(reader(bytes),{profile:PROFILE,names:[ENUM_NAME]}));
});

const {PARAMETER_PROFILE} = require('../pdb_selected_types.cjs');
const parameterInspect = (bytes, limits = {}) => inspectSelectedTypes(reader(bytes), {
    profile: PARAMETER_PROFILE, names: [TARGET], limits,
});
const baseEntry = (type, offset = 0) => padded(Buffer.concat([u16(0x1400), u16(3), u32(type), u16(offset)]));
test('parameter profile preserves bases, nested arrays and separate flattened paths', () => {
    const bytes = stream([list(member('Value')), udt({name: 'Base'}),
        record(0x1503, u32(0x1001), u32(0x75), u16(8), z('')),
        list(baseEntry(0x1001), member('Items', 0x1002, 4)), udt({fields: 0x1003, count: 2, size: 12})]);
    const value = parameterInspect(bytes).selected[0];
    assert.equal(value.fields[0].entry_kind, 'base');
    assert.equal(value.fields[1].element_count, 2);
    assert.deepEqual(value.flattened_fields.map(x => x.offset_bytes), [0, 4, 8]);
    fails(() => inspect(bytes));
    fails(() => parameterInspect(bytes, {dependencies: 1}));
    fails(() => parameterInspect(bytes, {depth: 0}));
    fails(() => parameterInspect(bytes, {fields: 3}));
});
test('parameter profile rejects overlap, virtual bases, cycles and fractional arrays', () => {
    fails(() => parameterInspect(stream([list(member('A'), member('B')), udt({count: 2})])));
    fails(() => parameterInspect(stream([list(Buffer.concat([u16(0x1401), Buffer.alloc(18)])), udt()])));
    fails(() => parameterInspect(stream([list(member('Self', 0x1001)), udt()])));
    fails(() => parameterInspect(stream([record(0x1503, u32(0x75), u32(0x75), u16(5), z('')),
        list(member('Bad', 0x1000)), udt({fields: 0x1001, size: 8})])));
});

test('parameter multilevel bases and dependency continuations retain absolute paths', () => {
    const bytes = stream([
        list(member('First'), continuation(0x1001)), list(member('Second', 0x75, 4)),
        udt({name: 'Root', count: 2, size: 8}),
        list(baseEntry(0x1002), member('Middle', 0x75, 8)),
        udt({name: 'Middle', fields: 0x1003, count: 2, size: 12}),
        list(baseEntry(0x1004, 4), continuation(0x1006)), list(member('Last', 0x75, 16)),
        udt({fields: 0x1005, count: 2, size: 20}),
    ]);
    const result = parameterInspect(bytes);
    assert.deepEqual(result.selected[0].flattened_fields, [
        {path: [{base_type_index: 0x1004}, {base_type_index: 0x1002}, 'First'], offset_bytes: 4, width_bytes: 4},
        {path: [{base_type_index: 0x1004}, {base_type_index: 0x1002}, 'Second'], offset_bytes: 8, width_bytes: 4},
        {path: [{base_type_index: 0x1004}, 'Middle'], offset_bytes: 12, width_bytes: 4},
        {path: ['Last'], offset_bytes: 16, width_bytes: 4},
    ]);
    assert.equal(parameterInspect(bytes, {dependencies: 2, fields: 7, continuations: 2}).selected.length, 1);
    fails(() => parameterInspect(bytes, {dependencies: 1}));
    fails(() => parameterInspect(bytes, {fields: 6}));
    fails(() => parameterInspect(bytes, {continuations: 1}));
});

const forwardDependencyFixture = (forward = {}, extras = []) => stream([
    udt({name: 'Dependency', fields: 0, count: 0, size: 0, flags: 0x80, unique: 'AuthoredDependency', ...forward}),
    list(member('Value')), udt({name: 'Dependency', fields: 0x1001, unique: 'AuthoredDependency'}),
    list(baseEntry(0x1000)), udt({fields: 0x1003}), ...extras,
]);
test('parameter dependency forwards require one exact complete definition and valid declaration', () => {
    const result = parameterInspect(forwardDependencyFixture(), {dependencies: 2, candidates: 2});
    assert.equal(result.selected[0].fields[0].resolved_type_index, 0x1002);
    assert.deepEqual(result.selected[0].flattened_fields[0].path, [{base_type_index: 0x1000}, 'Value']);
    fails(() => parameterInspect(forwardDependencyFixture(), {dependencies: 1}));
    fails(() => parameterInspect(forwardDependencyFixture(), {candidates: 1}));
    for (const forward of [{count: 1}, {fields: 0x1001}, {size: 4}, {unique: 'Mismatch'}]) {
        fails(() => parameterInspect(forwardDependencyFixture(forward)));
    }
    fails(() => parameterInspect(forwardDependencyFixture({}, [udt({name: 'Dependency', fields: 0x1001, unique: 'AuthoredDependency'})])));
    fails(() => parameterInspect(stream([udt({name: 'Dependency', fields: 0, count: 0, size: 0, flags: 0x80}),
        list(baseEntry(0x1000)), udt({fields: 0x1001})])));
});

test('parameter modifier aggregates expand and repeated arrays obey expansion bounds', () => {
    const bytes = stream([list(member('Value')), udt({name: 'Element'}),
        record(0x1001, u32(0x1001), u16(1)),
        record(0x1503, u32(0x1002), u32(0x75), u16(8), z('')),
        list(member('Items', 0x1003)), udt({fields: 0x1004, size: 8})]);
    assert.deepEqual(parameterInspect(bytes).selected[0].flattened_fields, [
        {path: ['Items', 0, 'Value'], offset_bytes: 0, width_bytes: 4},
        {path: ['Items', 1, 'Value'], offset_bytes: 4, width_bytes: 4},
    ]);
    assert.equal(parameterInspect(bytes, {fields: 8, dependencies: 3}).selected.length, 1);
    fails(() => parameterInspect(bytes, {fields: 7}));
    fails(() => parameterInspect(bytes, {dependencies: 2}));
    fails(() => parameterInspect(bytes, {depth: 3}));
});

test('parameter shared dependencies retain exact output and retained-byte admission', () => {
    const bytes = forwardDependencyFixture();
    const result = parameterInspect(bytes);
    const outputBytes = Buffer.byteLength(JSON.stringify(result));
    assert.deepEqual(parameterInspect(bytes, {output_bytes: outputBytes}), result);
    fails(() => parameterInspect(bytes, {output_bytes: outputBytes - 1}));
    assert.deepEqual(parameterInspect(bytes, {retained_bytes: result.charged_retained_bytes}), result);
    fails(() => parameterInspect(bytes, {retained_bytes: result.charged_retained_bytes - 1}));
});

const {DECLARATION_PROFILE, DECLARATION_CAPS} = require('../pdb_selected_types.cjs');
const DECL_ROW = 'Authored::Row', DECL_BASE = 'Authored::MetaBase';
const declarationInspect = (bytes, limits = {}, names = [DECL_BASE, DECL_ROW]) =>
    inspectSelectedTypes(reader(bytes), {profile: DECLARATION_PROFILE, names, limits});
const oneMethod = (name, type, attributes = 3, extra = Buffer.alloc(0)) =>
    padded(Buffer.concat([u16(0x1511), u16(attributes), u32(type), extra, z(name)]));
const methodGroup = (name, type, count = 2) => padded(Buffer.concat([u16(0x150f), u16(count), u32(type), z(name)]));
const nestedType = (name, type, reserved = 0) => padded(Buffer.concat([u16(0x1510), u16(reserved), u32(type), z(name)]));
const argList = (args = [], count = args.length) => record(0x1201, u32(count), ...args.map(u32));
const methodList = (methods = [[3, 0x1008], [0x103, 0x1009]], reserved = 0) =>
    record(0x1206, ...methods.map(([attributes, type]) => Buffer.concat([u16(attributes), u16(reserved), u32(type)])));
function methodSignature({returns = 3, owner = 0x1006, thisType = 0x1000, cc = 0, options = 0,
    count = 0, args = 0x1001, adjustment = 0} = {}) {
    const adjust = Buffer.alloc(4); adjust.writeInt32LE(adjustment);
    return record(0x1009, u32(returns), u32(owner), u32(thisType), u8(cc), u8(options), u16(count), u32(args), adjust);
}
// Indices are chosen solely for these authored records. Unknown referent bodies
// must stay opaque, including a signature's return/class/this/argument types.
function declarationRecords() {
    return [record(0x7777, u32(0xdecaf)), // opaque synthetic identity, not a layout
        argList(), argList([0x1000, 0x41]),
        methodSignature({owner: 0x1006, thisType: 0}),
        methodSignature({returns: 0x1000, owner: 0x100c, options: 1}),
        list(oneMethod('Describe', 0x1003, 0xb)),
        udt({name: DECL_BASE, fields: 0x1005, size: 1, unique: 'AuthoredMetaIdentity'}),
        methodList(),
        methodSignature({owner: 0x100c, options: 2}),
        methodSignature({owner: 0x100c, count: 2, args: 0x1002}),
        list(baseEntry(0x1006), member('Value', 0x77), nestedType('Alias', 0x1000),
            oneMethod('operator struct Authored Value', 0x1004, 0x103), methodGroup('Choose', 0x1007)),
        udt({name: DECL_ROW, fields: 0, count: 0, size: 0, flags: 0x80, unique: 'AuthoredRowIdentity'}),
        udt({name: DECL_ROW, fields: 0x100a, count: 6, size: 8, unique: 'AuthoredRowIdentity'}),
    ];
}
function declarationChange(index, replacement, limits = {}, names) {
    const records = declarationRecords(); records[index] = replacement;
    // Most field-list refusal probes replace the row with one declaration.
    // Keep its count otherwise valid so a malformed payload is the blocker.
    if (index === 10) records[12] = udt({name: DECL_ROW, fields: 0x100a,
        count: replacement.readUInt16LE(4) === 0x150f ? replacement.readUInt16LE(6) : 1,
        size: 8, unique: 'AuthoredRowIdentity'});
    return declarationInspect(stream(records), limits, names);
}

test('declarations preserve identity, weighted counts, methods and coincident base/member offsets without layouts', () => {
    const bytes = stream(declarationRecords()), result = declarationInspect(bytes), [base, row] = result.selected;
    assert.equal(result.profile, DECLARATION_PROFILE);
    assert.equal(base.name, DECL_BASE); assert.equal(base.size_bytes, 1);
    assert.equal(base.unique_name, 'AuthoredMetaIdentity'); assert.equal(base.fields.length, 0);
    assert.equal(row.forward_declarations[0].type_index, 0x100b);
    assert.equal(row.physical_member_count, 5); assert.equal(row.weighted_member_count, 6);
    assert.equal(row.declared_member_count, 6); assert.equal(row.method_count, 3);
    assert.deepEqual(row.declarations.map(x => x.entry_kind), ['base', 'member', 'nested_type', 'method', 'overload_group']);
    assert.equal(row.declarations[0].offset_bytes, 0); assert.equal(row.fields[0].offset_bytes, 0);
    assert.equal(row.fields.length, 1); assert.equal(row.fields[0].type_index, 0x77);
    for (const value of [base, row, result]) assert.equal(value.storage_layout_complete, false);
    for (const value of [...base.declarations, ...row.declarations]) {
        assert.equal(Object.hasOwn(value, 'width_bytes'), false);
        assert.equal(Object.hasOwn(value, 'occupied_storage'), false);
        if (['method', 'nested_type', 'overload_group'].includes(value.entry_kind)) assert.equal(Object.hasOwn(value, 'offset_bytes'), false);
        assert.ok(value.entry_offset >= 4); assert.ok(value.field_list_type_index >= 0x1000);
    }
    assert.equal(row.declarations[3].name, 'operator struct Authored Value');
    assert.equal(row.declarations[3].attributes, 0x103);
    assert.deepEqual(result.dependent_types, []);
    assert.deepEqual(result.direct_references.filter(x => x.leaf === 0x1009).map(x => x.options), [0, 1, 2, 0]);
    assert.equal(result.direct_references.find(x => x.type_index === 0x1003).this_type_index, 0);
    assert.equal(result.direct_references.find(x => x.type_index === 0x1000).interpretation, 'opaque');
    assert.equal(result.direct_references.find(x => x.type_index === 0x41).interpretation, 'opaque');
    for (const profile of [PROFILE, PARAMETER_PROFILE]) {
        fails(() => inspectSelectedTypes(reader(bytes), {profile, names: [DECL_BASE, DECL_ROW]}));
    }
    for (const key of ['original_layout_qualified', 'release_qualified', 'runtime_behavior_proven',
        'caller_provenance_verified', 'unselected_payloads_validated']) assert.equal(result[key], false);
});

test('declaration references are memoized while shared overload occurrences consume their full weights', () => {
    const records = declarationRecords();
    records[10] = list(methodGroup('First', 0x1007), methodGroup('Second', 0x1007));
    records[12] = udt({name: DECL_ROW, fields: 0x100a, count: 4, size: 8, unique: 'AuthoredRowIdentity'});
    const bytes = stream(records), result = declarationInspect(bytes);
    assert.equal(result.selected[1].method_count, 4);
    assert.equal(result.direct_references.filter(x => x.type_index === 0x1007).length, 1);
    assert.equal(declarationInspect(bytes, {methods: 5}).selected.length, 2);
    fails(() => declarationInspect(bytes, {methods: 4})); // static base method also counts
    assert.equal(declarationInspect(bytes, {dependencies: 6}).selected.length, 2);
    fails(() => declarationInspect(bytes, {dependencies: 5}));
});

test('declaration signatures and direct lists reject truncated, trailing, wrong-leaf and invalid metadata', () => {
    for (const change of [{cc: 1}, {options: 3}, {options: 4}, {options: 0x80}, {adjustment: 1},
        {adjustment: -1}, {count: 1}, {count: 257}, {args: 0x1007}, {args: 0x2000},
        {returns: 0}, {returns: 0x2000}, {returns: 0x80000000}, {returns: 0x999},
        {owner: 0}, {owner: 3}, {thisType: 0}, {thisType: 0x2000}, {thisType: 0x603}]) {
        fails(() => declarationChange(4, methodSignature(change)));
    }
    fails(() => declarationChange(3, methodSignature({thisType: 0x1000})));
    for (const index of [1, 2, 3, 4, 7, 8, 9]) {
        const original = declarationRecords()[index];
        const cut = Buffer.from(original.subarray(0, original.length - 4)); cut.writeUInt16LE(cut.length - 2);
        fails(() => declarationChange(index, cut));
        const extra = Buffer.concat([original, Buffer.alloc(4)]); extra.writeUInt16LE(extra.length - 2);
        fails(() => declarationChange(index, extra));
        const wrong = Buffer.from(original); wrong.writeUInt16LE(0x7777, 2);
        fails(() => declarationChange(index, wrong));
    }
    for (const args of [argList([0x2000, 0x75]), argList([0, 0x75]), argList([0x75], 2), argList([], 0xffffffff)]) {
        fails(() => declarationChange(2, args));
    }
    fails(() => declarationChange(7, methodList(undefined, 1)));
    fails(() => declarationChange(7, methodList([[3, 0x1008]])));
    fails(() => declarationChange(7, methodList([[3, 0x1008], [3, 0x1008], [3, 0x1008]])));
    fails(() => declarationChange(7, methodList([[3, 0x1007], [3, 0x1009]])));
});

test('declaration attributes refuse virtual, friend, introducing, reserved and unsupported option bits', () => {
    for (const attributes of [0, 4, 7, 0xf, 0x13, 0x17, 0x1b, 0x1f, 0x23, 0x43, 0x83, 0x203, 0x8003]) {
        fails(() => declarationChange(10, list(oneMethod('Invalid', 0x1004, attributes))));
        fails(() => declarationChange(7, methodList([[attributes, 0x1008], [3, 0x1009]])));
    }
    // Neither missing nor extra VFTableOffset is guessed for introducing forms.
    fails(() => declarationChange(10, list(oneMethod('Invalid', 0x1004, 0x13, u32(0)))));
    fails(() => declarationChange(10, list(oneMethod('Invalid', 0x1004, 3, u32(0)))));
    fails(() => declarationChange(10, list(nestedType('Alias', 0x1000, 1))));
    fails(() => declarationChange(10, list(methodGroup('Choose', 0x1007, 0))));
    fails(() => declarationChange(10, list(member('Valid'), record(0x7777))));
    for (const leaf of [0x1401, 0x1402, 0x150e]) fails(() => declarationChange(10, list(u16(leaf))));
});

test('declaration method names permit printable spaces with bounded termination and exact padding', () => {
    for (const name of ['', 'Bad\nName', 'Bad\x7fName', 'Bad\x01Name', 'x'.repeat(257)]) {
        fails(() => declarationChange(10, list(oneMethod(name, 0x1004))));
        fails(() => declarationChange(10, list(methodGroup(name, 0x1007))));
    }
    for (const entry of [oneMethod('A', 0x1004), methodGroup('A', 0x1007), nestedType('A', 0x1000), member('AB')]) {
        const bad = Buffer.from(entry); bad[bad.length - 1] = 0;
        fails(() => declarationChange(10, list(bad)));
        const unterminated = Buffer.from(entry); unterminated.fill(0x41, 8);
        fails(() => declarationChange(10, list(unterminated)));
    }
    for (const entry of [member('Bad Name'), nestedType('Bad Alias', 0x1000)]) {
        fails(() => declarationChange(10, list(entry)));
    }
    const bytes = stream([argList(), methodSignature({owner: 0x1003, args: 0x1000}),
        list(oneMethod('A B', 0x1001)), udt({fields: 0x1002})]);
    assert.equal(declarationInspect(bytes, {}, [TARGET]).selected[0].declarations[0].name, 'A B');
    fails(() => declarationInspect(bytes, {name_bytes: TARGET.length - 1}, [TARGET]));
});

test('declaration counts, exact forwards, ambiguity, continuations and selected base cycles fail closed', () => {
    for (const count of [0, 1, 5, 7]) fails(() => declarationChange(12,
        udt({name: DECL_ROW, fields: 0x100a, count, size: 8, unique: 'AuthoredRowIdentity'})));
    for (const forward of [{count: 1}, {fields: 0x100a}, {size: 8}, {unique: 'Conflict'}, {unique: null}, {leaf: 0x1504}]) {
        fails(() => declarationChange(11, udt({name: DECL_ROW, fields: 0, count: 0, size: 0,
            flags: 0x80, unique: 'AuthoredRowIdentity', ...forward})));
    }
    const records = declarationRecords();
    for (const extra of [records[11], records[12]]) fails(() => declarationInspect(stream([...records, extra])));
    fails(() => declarationInspect(stream(records.slice(0, -1))));
    const bytes = stream([list(member('First'), continuation(0x1001)), list(nestedType('Alias', 0x75)),
        udt({fields: 0x1000, count: 2})]);
    const value = declarationInspect(bytes, {}, [TARGET]).selected[0];
    assert.equal(value.physical_member_count, 2); assert.equal(value.field_lists.length, 2);
    assert.equal(value.field_lists[0].continuation_type_index, 0x1001);
    fails(() => declarationInspect(bytes, {continuations: 1}, [TARGET]));
    for (const tail of [continuation(0x1000), continuation(0x1001), continuation(0x2000),
        Buffer.concat([u16(0x1404), u16(1), u32(0x1000)]), Buffer.concat([continuation(0x1000), member('After')])]) {
        fails(() => declarationInspect(stream([list(member('First'), tail), udt({count: 2})]), {}, [TARGET]));
    }
    const cyclic = stream([list(baseEntry(0x1003)), udt({name: 'First'}),
        list(baseEntry(0x1001)), udt({name: 'Second', fields: 0x1002})]);
    fails(() => declarationInspect(cyclic, {}, ['First', 'Second']));
    fails(() => declarationInspect(stream([list(baseEntry(0x1001)), udt()]), {}, [TARGET]));
});

test('declaration independent budgets admit exact boundaries and never return a valid prefix', () => {
    const bytes = stream(declarationRecords()), result = declarationInspect(bytes);
    const boundaries = {declarations: 6, weighted_members: 7, methods: 4,
        references: result.direct_references.length, dependencies: 7, parameters: 2, argument_types: 2,
        output_bytes: Buffer.byteLength(JSON.stringify(result)), retained_bytes: result.charged_retained_bytes};
    for (const [key, value] of Object.entries(boundaries)) {
        assert.deepEqual(declarationInspect(bytes, {[key]: value}), result, key);
        fails(() => declarationInspect(bytes, {[key]: value - 1}));
    }
    fails(() => declarationInspect(bytes, {depth: 0}));
    for (const key of ['declarations', 'weighted_members', 'methods', 'references', 'parameters', 'argument_types']) {
        fails(() => declarationInspect(bytes, {[key]: DECLARATION_CAPS[key] + 1}));
        fails(() => declarationInspect(bytes, {[key]: -1}));
        fails(() => inspectSelectedTypes(reader(bytes), {profile: PROFILE, names: [DECL_ROW], limits: {[key]: 1}}));
    }
    const broken = declarationRecords(); broken[10] = list(member('Valid'), u16(0x7777));
    let published;
    fails(() => { published = declarationInspect(stream(broken)); });
    assert.equal(published, undefined);
});

test('declaration offsets give no empty-base privilege to opaque or data-bearing bases', () => {
    const bytes = stream([list(member('Stored')), udt({name: 'OpaqueBase'}),
        list(baseEntry(0x1001), member('SameOffset')), udt({fields: 0x1002, count: 2})]);
    const result = declarationInspect(bytes, {}, [TARGET]);
    assert.equal(result.selected[0].storage_layout_complete, false);
    assert.equal(result.direct_references.find(x => x.type_index === 0x1001).interpretation, 'opaque');
    assert.equal(result.selected[0].declarations[0].type_index, 0x1001);
    assert.equal(Object.hasOwn(result.selected[0].declarations[0], 'width_bytes'), false);
    fails(() => parameterInspect(bytes)); // ordinary base/member overlap unchanged
    fails(() => parameterInspect(stream([list(member('A'), member('B')), udt({count: 2})])));
    fails(() => declarationInspect(stream([list(baseEntry(0x75)), udt()]), {}, [TARGET]));
});

test('declaration truncated field payloads and malformed storage metadata refuse independently of counts', () => {
    for (const entry of [baseEntry(0x1006), member('Value', 0x77), nestedType('Alias', 0x1000),
        oneMethod('Describe', 0x1004), methodGroup('Choose', 0x1007)]) {
        let payloadLength = entry.length;
        while (entry[payloadLength - 1] >= 0xf1) payloadLength--;
        for (let length = 2; length < payloadLength; length += 4) {
            const truncated = record(0x1203, entry.subarray(0, length));
            fails(() => declarationChange(10, truncated));
        }
    }
    const badAttributes = member('Value'); badAttributes.writeUInt16LE(0x103, 2);
    fails(() => declarationChange(10, list(badAttributes)));
    fails(() => declarationChange(10, list(member('Value', 0x2000))));
    fails(() => declarationChange(10, list(member('Value', 0x77, 9))));
    fails(() => declarationChange(10, list(nestedType('Alias', 0x2000))));
    const self = stream([list(member('Self', 0x1001)), udt()]);
    fails(() => declarationInspect(self, {}, [TARGET]));
});

test('memoized direct signatures preserve depth admission across single and overloaded occurrences', () => {
    const records = declarationRecords();
    records[10] = list(oneMethod('First', 0x1008), oneMethod('Second', 0x1009), methodGroup('Overloads', 0x1007));
    records[12] = udt({name: DECL_ROW, fields: 0x100a, count: 4, size: 8, unique: 'AuthoredRowIdentity'});
    const bytes = stream(records);
    assert.equal(declarationInspect(bytes, {depth: 2}).selected.length, 2);
    fails(() => declarationInspect(bytes, {depth: 1}));
});
