// Run: node tests/gui/plan-summary.mjs
// The plan headline: time and size on one line each, the disk line fits / does not fit, and
// the longest strings still fit the panel (there is no DOM here: a character-width model
// stands in for scrollWidth <= clientWidth; the real window was measured at 1000x650 and 1920x1080).
import assert from 'node:assert';

const node = () => ({ value: '', hidden: false, textContent: '', className: '', children: [], classList: { toggle() {}, remove() {} }, setAttribute() {}, addEventListener() {}, append(...c) { this.children.push(...c); } });
globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: () => null, createElement: node };
globalThis.window = { addEventListener() {} };
const { planHead } = await import('../../src/gui/js/meld-features.js');

const huge = { disk_mb: [3.8 * 1048576, 4.6 * 1048576], ram_mb: [552, 704], time_min: [47 * 60 + 50, 139 * 60 + 10], workers: 4 };
const stats = (e) => planHead(e, []).at(0).children.filter((c) => c.className.startsWith('plan-stat')).map((c) => c.textContent);
assert.deepStrictEqual(stats(huge), ['≈ 94 h', '≈ 2.5 GB RAM', '≈ 4.2 TB'], 'one midpoint each, long jobs in hours');
assert.deepStrictEqual(stats({ ...huge, time_min: [3, 40], disk_mb: [300, 450] }), ['≈ 22 min', '≈ 2.5 GB RAM', '≈ 375 MB']);

// Disk line both ways.
const disk = (e) => planHead(e, []).at(1);
const no = disk({ ...huge, free_disk_mb: 48.1 * 1024 });
assert.ok(no.className.includes('is-low') && !no.hidden, 'does not fit: red line');
assert.strictEqual(no.textContent, 'Needs 3.8–4.6 TB, 48.1 GB free on this disk');
const yes = disk({ ...huge, disk_mb: [120 * 1024, 150 * 1024], free_disk_mb: 412 * 1024 });
assert.ok(!yes.className.includes('is-low') && !yes.hidden);
assert.strictEqual(yes.textContent, 'Fits: 412.0 GB free');
assert.ok(disk(huge).hidden, 'free space unknown: no line');

// Fit model: 11px semibold ~6.5px a character, each stat + 16px (icon and gap), the "?" 16px,
// 8px gaps. The panel is 340px wide (1000x650 and up) with 20px padding.
const row = (x) => x.reduce((w, t) => w + t.length * 6.5 + 16, 0) + 16 + 8 * x.length;
assert.ok(row(stats(huge)) <= 320, 'the stats row and its "?" fit on one line');
console.log('plan-summary: ok');
process.exit(0);
