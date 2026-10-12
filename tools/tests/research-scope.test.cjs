'use strict';
const {test} = require('node:test');
const assert = require('node:assert/strict');
const {validateOfflineScope} = require('../research-scope.cjs');
const scope = `## auth
- status: granted
- evidence_of_auth: execution prompt
## network_profile
- mode: offline
## in_scope
- assets:
  - work/authored-sample
## signoff
- ready_for_act: true
`;
const session = {
  reviewed: true, authorization_source: 'execution prompt',
  started_at: '2026-10-12T21:00:00Z', ends_at: '2026-10-12T21:15:00Z',
};
const now = Date.parse('2026-10-12T21:01:00Z');
test('accepts exact offline scope and bounded reviewed session', () => {
  validateOfflineScope(scope, session, now);
});
test('rejects absent, duplicate or misplaced scope fields and sections', () => {
  for (const invalid of [
    scope.replace('- status: granted', '- status: denied'),
    scope.replace('- status: granted', '- status: granted\n- status: denied'),
    scope.replace('## auth', '## notes'),
    scope + '## auth\n- status: granted\n',
    scope.replace('- mode: offline', '- mode: unrestricted_lab'),
    scope.replace('- ready_for_act: true', '- ready_for_act: false'),
    scope.replace('  - work/authored-sample', ''),
  ]) assert.throws(() => validateOfflineScope(invalid, session, now));
});
test('rejects stale, unreviewed, unbound or timezone-free sessions', () => {
  for (const invalid of [
    {...session, reviewed: false}, {...session, authorization_source: 'other'},
    {...session, started_at: '2026-10-12T21:00:00'},
    {...session, ends_at: '2026-10-12T21:16:00Z'},
    {...session, ends_at: 'invalid'},
  ]) assert.throws(() => validateOfflineScope(scope, invalid, now));
  assert.throws(() => validateOfflineScope(scope, session, now - 120000));
  assert.throws(() => validateOfflineScope(scope, session, now + 800001));
  assert.throws(() => validateOfflineScope(scope, session, now, -1));
});
