// Run: node tests/gui/progress-text.mjs
// An error with `detail` keeps the short message, carries the detail as title and log text; a
// Region Download fallback line becomes the translated notice.
import assert from 'node:assert';
import fs from 'node:fs';

globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: () => null };
globalThis.window = { addEventListener() {}, localization: { region_download_fallback: 'Notice', job_settings_changed: 'Changed: {names}' } };
const { progressText, holdsNotice } = await import('../../src/gui/js/meld-features.js');

const full = 'Error! Could not reach the server after 3 tries: connection refused';
assert.deepEqual(progressText({ message: 'Error! Could not reach', detail: full }), { text: 'Error! Could not reach', title: full, log: full, notice: false });
assert.deepEqual(progressText({ message: 'Done!' }), { text: 'Done!', title: '', log: null, notice: false });
const n = progressText({ message: 'region_download_fallback: http_503' });
assert.equal(n.text, 'Notice');
assert.ok(n.notice);
const c = progressText({ message: 'Settings changed since this job started: --river-bed, --winter' });
assert.equal(c.text, 'Changed: --river-bed, --winter', 'a resume with other settings names them');
assert.ok(c.notice);

assert.equal(progressText({ message: 'Building pieces... 23/100 done' }).text, 'Building cells… 23/100', 'the title counts cells');
assert.equal(progressText({ message: 'Building pieces... 23/100 done. Paused.' }).text, 'Building cells… 23/100 Paused.', 'paused, in the same words');

// A notice outlasts the next stage line for 8 s, not an error or the end of a run.
const ord = progressText({ message: 'Fetching' });
assert.ok(holdsNotice(ord, 'Fetching', 1000, 5000));
assert.ok(!holdsNotice(ord, 'Fetching', 1000, 9500) && !holdsNotice(ord, 'Fetching', 0, 5000));
assert.ok(!holdsNotice(ord, 'Done! x', 1000, 5000) && !holdsNotice(progressText({ message: 'Error! x' }), 'Error! x', 1000, 5000));

assert.ok(!holdsNotice(ord, 'The retried cells are built', 1000, 5000));
const dir = new URL('../../src/gui/locales/', import.meta.url);
for (const f of fs.readdirSync(dir).filter((f) => f.endsWith('.json') && f !== 'en.json')) {
  const loc = JSON.parse(fs.readFileSync(new URL(f, dir), 'utf8'));
  assert.ok(loc.region_download_fallback, `${f} lacks region_download_fallback`);
  assert.ok(loc.job_settings_changed?.includes('{names}'), `${f} lacks job_settings_changed`);
}
console.log('progress-text: ok');
