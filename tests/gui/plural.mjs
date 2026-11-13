// Run: node tests/gui/plural.mjs
// "1 cell" not "1 cells": the singular string is used for 1 when the language has one.
import assert from 'node:assert';

globalThis.document = { querySelector: () => null, getElementById: () => null };
globalThis.window = { addEventListener() {}, localization: {} };
const { tn } = await import('../../src/gui/js/meld-features.js');

assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 1), '1 cell');
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 30), '30 cells');
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 0), '0 cells');
window.localization = { plan_pieces: '{n} celule', plan_pieces_one: '{n} celulă' };
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 1), '1 celulă');
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 2), '2 celule');
window.localization = { plan_pieces: '{n} 个单元格' }; // no singular string: the plain one
assert.equal(tn('plan_pieces', '{n} cells', '{n} cell', 1), '1 个单元格');
window.localization = { plan_regions: '{n} regiuni', plan_regions_one: '{n} regiune' };
assert.equal(tn('plan_regions', '{n} regions', '{n} region', 1), '1 regiune');
assert.equal(tn('plan_regions', '{n} regions', '{n} region', 3), '3 regiuni');
// run_failed: one / few / other from the language's own plural rule; every locale carries all three.
import fs from 'node:fs';
const loc = (f) => JSON.parse(fs.readFileSync(new URL(`../../src/gui/locales/${f}.json`, import.meta.url), 'utf8'));
const { tp } = await import('../../src/gui/js/meld-features.js');
window.localization = loc('ro');
const ro = (n) => tp('run_failed', '{n} failed', n, 'ro');
assert.deepEqual([1, 2, 19, 20, 21, 100, 101].map(ro), ['1 eșuată', '2 eșuate', '19 eșuate', '20 de eșuate', '21 de eșuate', '100 de eșuate', '101 eșuate']);
window.localization = {};
assert.equal(tp('run_failed', '{n} failed', 3, 'en'), '3 failed');
// Ukrainian is filed as "ua", which Intl does not know: it takes uk's rule, whose many (5 to 20) is _other.
window.localization = { run_failed_one: 'one', run_failed_few: 'few', run_failed_other: 'other' };
assert.deepEqual([1, 2, 5, 21].map((n) => tp('run_failed', '{n} failed', n, 'ua')), ['one', 'few', 'other', 'one']);
for (const f of fs.readdirSync(new URL('../../src/gui/locales/', import.meta.url)).filter((f) => f.endsWith('.json') && f !== 'en.json')) {
  const l = loc(f.slice(0, -5));
  for (const c of ['one', 'few', 'other']) assert.ok(l[`run_failed_${c}`], `${f} lacks run_failed_${c}`);
}
console.log('plural ok');
