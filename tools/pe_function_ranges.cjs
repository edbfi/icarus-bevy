'use strict';

// Pure library: inspect(bytes, {sha256, addresses}). No file I/O or code scan.
// addresses are 1..24 distinct canonical lowercase hex x86_64 VAs, without
// leading zeroes. They are caller-supplied selectors; public-symbol identity is
// not checked. The SHA-256 pin is checked before any PE byte interpretation.
//
// Conservative profile: inherit pe_candidates.parse's PE32+/8664 executable,
// alignment, section, resource and entry-point checks. Require one contiguous
// file-backed exception directory in a read-only initialized .pdata section,
// with 1..1048576 12-byte RUNTIME_FUNCTION records. Validate the ENTIRE table:
// nonzero BeginAddress < EndAddress, strictly increasing disjoint ranges,
// wholly backed by one nonwritable executable code section; nonzero aligned
// UnwindInfoAddress with a file-backed 4-byte header in read-only initialized
// data. Reject duplicate, overlapping, unsorted and zero-padding records,
// tagged unwind pointers, virtual-only tails, raw padding and split mappings.
// These restrictions deliberately reject some otherwise valid PE images.
//
// All spans use inclusive start / exclusive end. entry_index is zero-based in
// the exception directory, not a function identity. The unwind header span is
// ONLY four mapped bytes: no unwind bytes/flags are interpreted or exported,
// no chains are followed, and no containing-function ownership is inferred.
// Complete framing does not establish that a selector is an instruction start.
// Failure (including any missing selector) throws Error with a stable .code;
// success preserves selector order and exports no unselected table records.
const crypto = require('node:crypto');
const {isSharedArrayBuffer} = require('node:util').types;
const {parse, MAX_FILE_BYTES, MAX_SECTIONS, MAX_OUTPUT_BYTES} = require('./pe_candidates.cjs');

const LIMITS = Object.freeze({
  file_bytes: MAX_FILE_BYTES,
  sections: MAX_SECTIONS,
  selectors: 24,
  entries: 1048576,
  output_bytes: MAX_OUTPUT_BYTES,
});
const hex = value => '0x' + value.toString(16);
const canonical = value => value >= 0n && value <= 0xffffffffffffffffn &&
  (value <= 0x7fffffffffffn || value >= 0xffff800000000000n);
function fail(code, message) {
  const error = new Error(message);
  error.code = code;
  throw error;
}
function check(ok, code, message) {
  if (!ok) fail(code, message);
}

function inspect(bytes, pin) {
  check(Buffer.isBuffer(bytes) && bytes.length >= 64 && bytes.length <= LIMITS.file_bytes,
    'ERR_INPUT_BOUND', 'Buffer must be 64 bytes through 128 MiB before hash');
  // A concurrent writer must not alter bytes between hashing and interpretation.
  check(!isSharedArrayBuffer(bytes.buffer),
    'ERR_INPUT_BOUND', 'Shared Buffer backing storage is unsupported');
  check(pin && typeof pin === 'object' && !Array.isArray(pin) &&
    Object.keys(pin).length === 2 && Object.hasOwn(pin, 'sha256') && Object.hasOwn(pin, 'addresses'),
  'ERR_INPUT_PIN', 'Expected exactly sha256 and addresses');
  const expected = pin.sha256, addresses = pin.addresses;
  check(typeof expected === 'string' && expected.length === 64 && /^[0-9a-f]{64}$/.test(expected),
    'ERR_INPUT_PIN', 'Expected a lowercase SHA-256 pin');
  const selectorCount = Array.isArray(addresses) ? addresses.length : 0;
  check(Number.isInteger(selectorCount) && selectorCount >= 1 && selectorCount <= LIMITS.selectors,
    'ERR_SELECTORS', 'Expected 1..24 distinct canonical VA selectors');
  const selected = [], seen = new Set();
  for (let index = 0; index < selectorCount; index++) {
    const address = addresses[index];
    check(typeof address === 'string' && address.length <= 18 &&
      /^0x(?:0|[1-9a-f][0-9a-f]{0,15})$/.test(address),
    'ERR_SELECTORS', 'Selector must be canonical lowercase hexadecimal');
    const value = BigInt(address);
    check(address === hex(value) && canonical(value), 'ERR_SELECTORS', 'Noncanonical x86_64 VA selector');
    check(!seen.has(address), 'ERR_SELECTORS', 'Duplicate VA selector');
    seen.add(address);
    selected.push({address, value, index});
  }
  check(crypto.createHash('sha256').update(bytes).digest('hex') === expected,
    'ERR_SHA_MISMATCH', 'Input SHA-256 mismatch');

  let image;
  try {
    image = parse(bytes);
  } catch (error) {
    // parse rejects every overlap globally, even outside the selected ranges.
    if (error instanceof Error && /^overlapping (virtual|raw) sections$/.test(error.message)) {
      fail('ERR_MAPPING_AMBIGUOUS', error.message);
    }
    fail('ERR_PE_PROFILE', error instanceof Error ? error.message : 'Invalid PE profile');
  }
  check(canonical(image.base) && canonical(image.base + BigInt(image.imageBytes) - 1n),
    'ERR_PE_PROFILE', 'Image VA extent must be canonical x86_64');
  for (const selector of selected) {
    check(selector.value >= image.base && selector.value < image.base + BigInt(image.imageBytes),
      'ERR_SELECTOR_RANGE', 'Selector outside image: ' + selector.address);
    selector.rva = Number(selector.value - image.base);
  }

  // parse has already rejected overlapping raw AND virtual section extents.
  // Binary search keeps complete-table mapping work bounded by log(sections).
  const sections = [...image.sections].sort((a, b) => a.rva - b.rva);
  const mapped = (rva, size, label) => {
    check(rva > 0 && size > 0 && rva + size <= image.imageBytes,
      'ERR_MAPPING_ABSENT', label + ': outside image');
    let low = 0, high = sections.length;
    while (low < high) {
      const middle = Math.floor((low + high) / 2);
      if (sections[middle].rva <= rva) low = middle + 1;
      else high = middle;
    }
    const section = sections[low - 1];
    check(section && rva >= section.rva && rva + size <= section.rva + section.mappedBytes,
      'ERR_MAPPING_ABSENT', label + ': no single complete file-backed mapping');
    const start = section.raw + rva - section.rva;
    return {section, span: {start, end: start + size}};
  };
  const pe = bytes.readUInt32LE(60), op = pe + 24;
  const directoryCount = bytes.readUInt32LE(op + 108);
  check(directoryCount > 3, 'ERR_EXCEPTION_ABSENT', 'Exception directory absent');
  const directoryRva = bytes.readUInt32LE(op + 136), directoryBytes = bytes.readUInt32LE(op + 140);
  check(directoryRva !== 0 || directoryBytes !== 0, 'ERR_EXCEPTION_ABSENT', 'Exception directory absent');
  check(directoryRva > 0 && directoryRva % 4 === 0 && directoryBytes > 0 &&
    directoryBytes % 12 === 0 && directoryBytes / 12 <= LIMITS.entries &&
    directoryRva + directoryBytes <= image.imageBytes,
  'ERR_EXCEPTION_DIRECTORY', 'Exception directory extent/alignment/record cap');
  const directory = mapped(directoryRva, directoryBytes, 'Exception directory');
  const ds = directory.section;
  const table = op + bytes.readUInt16LE(pe + 20);
  const sectionIndex = image.sections.indexOf(ds);
  check(bytes.subarray(table + sectionIndex * 40, table + sectionIndex * 40 + 8)
    .equals(Buffer.from('.pdata\0\0', 'ascii')) &&
    ds.readable && ds.initialized && !ds.writable && !ds.executable && !ds.resourceOverlap,
  'ERR_EXCEPTION_DIRECTORY', 'Exception directory requires read-only initialized .pdata');

  const entryCount = directoryBytes / 12, ordered = [...selected].sort((a, b) => a.rva - b.rva);
  const matches = new Array(selected.length);
  let previousStart = -1, previousEnd = -1, nextSelector = 0;
  for (let index = 0; index < entryCount; index++) {
    const offset = directory.span.start + index * 12;
    const start = bytes.readUInt32LE(offset), end = bytes.readUInt32LE(offset + 4);
    const unwind = bytes.readUInt32LE(offset + 8);
    check(start > 0 && end > start && end <= image.imageBytes,
      'ERR_RUNTIME_FUNCTION_RANGE', 'Invalid RUNTIME_FUNCTION extent at index ' + index);
    check(start !== previousStart, 'ERR_FUNCTION_AMBIGUOUS', 'Duplicate RUNTIME_FUNCTION start at index ' + index);
    check(start > previousStart, 'ERR_RUNTIME_FUNCTION_ORDER', 'Unsorted RUNTIME_FUNCTION at index ' + index);
    check(start >= previousEnd, 'ERR_FUNCTION_AMBIGUOUS', 'Overlapping RUNTIME_FUNCTION at index ' + index);
    previousStart = start;
    previousEnd = end;
    const code = mapped(start, end - start, 'RUNTIME_FUNCTION index ' + index);
    check(code.section.code && code.section.executable && !code.section.writable && !code.section.resourceOverlap,
      'ERR_RUNTIME_FUNCTION_RANGE', 'RUNTIME_FUNCTION requires nonwritable code at index ' + index);
    check(unwind > 0 && unwind % 4 === 0,
      'ERR_UNWIND_POINTER', 'Zero/unaligned/tagged unwind RVA at index ' + index);
    const header = mapped(unwind, 4, 'Unwind header index ' + index);
    check(header.section.readable && header.section.initialized && !header.section.executable &&
      !header.section.writable && !header.section.resourceOverlap,
    'ERR_UNWIND_POINTER', 'Unwind header requires read-only initialized data at index ' + index);
    while (nextSelector < ordered.length && ordered[nextSelector].rva < end) {
      const selector = ordered[nextSelector++];
      if (selector.rva < start) continue;
      matches[selector.index] = {
        address: selector.address,
        rva: selector.rva,
        entry_index: index,
        start_rva: start,
        end_rva: end,
        unwind_rva: unwind,
        start_address: hex(image.base + BigInt(start)),
        end_address: hex(image.base + BigInt(end)),
        entry_file_span: {start: offset, end: offset + 12},
        function_file_span: code.span,
        unwind_header_file_span: header.span,
      };
    }
  }
  for (const selector of selected) {
    check(matches[selector.index] !== undefined, 'ERR_FUNCTION_ABSENT',
      'No RUNTIME_FUNCTION contains selector: ' + selector.address);
  }
  const result = {
    profile: 'strict_PE64_x64_pdata_containment',
    input_sha256: expected,
    image_base: hex(image.base),
    exception_directory_rva: directoryRva,
    exception_directory_file_span: directory.span,
    runtime_function_entry_count: entryCount,
    complete_exception_table_framing: true,
    interval_convention: 'start_inclusive_end_exclusive',
    matches,
    public_symbol_identity_verified: false,
    instruction_boundaries_confirmed: false,
    function_identity_confirmed: false,
    unwind_chains_expanded: false,
    unwind_headers_interpreted: false,
  };
  check(Buffer.byteLength(JSON.stringify(result) + '\n') <= LIMITS.output_bytes,
    'ERR_OUTPUT_BOUND', 'Output exceeds 64 KiB');
  return result;
}

module.exports = {inspect, LIMITS};
