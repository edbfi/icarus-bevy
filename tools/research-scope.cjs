'use strict';

// Project-owned offline scope checks. This validates a declared scope; it does
// not grant authorization or extend a session's deadline.
const assert = require('node:assert/strict');

function section(scope, name) {
  const sections = scope.split(/^##\s+/m).slice(1);
  const matches = sections.filter(s => s.split(/\r?\n/, 1)[0].trim() === name);
  assert.equal(matches.length, 1, 'scope section must occur exactly once');
  return matches[0].split(/\r?\n/).slice(1).join('\n');
}

function field(scope, name, key) {
  const lines = section(scope, name).split(/\r?\n/);
  const pattern = new RegExp('^\\s*-\\s+' + key + ':\\s*(.*?)\\s*$');
  const matches = lines.map(line => pattern.exec(line)).filter(Boolean);
  assert.equal(matches.length, 1, 'scope field must occur exactly once');
  return matches[0][1];
}

function validateOfflineScope(scope, session, now = Date.now(), reserveMs = 95000) {
  assert.equal(field(scope, 'auth', 'status'), 'granted');
  assert.equal(field(scope, 'network_profile', 'mode'), 'offline');
  assert.equal(field(scope, 'signoff', 'ready_for_act'), 'true');
  const authority = field(scope, 'auth', 'evidence_of_auth');
  assert(authority.trim().length > 0 && session.authorization_source === authority,
    'session must identify the exact declared authorization source');
  assert(/^-\s*assets:\s*\r?\n(?:\s{2,}-\s+\S.*\r?\n?)+/m.test(section(scope, 'in_scope')),
    'offline scope needs explicit input assets');
  assert(session.reviewed === true, 'session must be reviewed');
  assert(Number.isFinite(now) && Number.isSafeInteger(reserveMs) && reserveMs > 0);
  const timestamp = value => {
    assert(typeof value === 'string' && /(?:Z|[+-]\d{2}:\d{2})$/.test(value),
      'session timestamps must include a timezone');
    const parsed = Date.parse(value);
    assert(Number.isFinite(parsed), 'invalid session timestamp');
    return parsed;
  };
  const start = timestamp(session.started_at), end = timestamp(session.ends_at);
  assert.equal(end - start, 900000, 'session must last exactly fifteen minutes');
  assert(start <= now && end - now >= reserveMs, 'session has insufficient runtime reserve');
}

module.exports = {validateOfflineScope};
