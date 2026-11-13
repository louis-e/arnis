// Run: node tests/gui/job-key.mjs
// The real job key (cellsKey, as main.js wires it), not a stub: Skip before a run sets the skip
// list, and the next Start's flags carry it once, for that job only, without recursing.
import assert from 'node:assert';
import fs from 'node:fs';

const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const nodes = new Map();
const element = (id) => {
  const tag = html.match(new RegExp(`<(input|select)[^>]* id="${id}"[^>]*>`));
  let value = (tag && (tag[0].match(/ value="([^"]*)"/) || [])[1]) || '';
  if (tag && tag[1] === 'select') {
    const block = html.slice(tag.index, html.indexOf('</select>', tag.index));
    value = ((block.match(/<option value="([^"]*)" selected/) || block.match(/<option value="([^"]*)"/)) || [])[1] || '';
  }
  return { value, checked: Boolean(tag && /\bchecked\b/.test(tag[0])), hidden: false, textContent: '', dataset: {}, style: {}, classList: { toggle() {}, add() {}, remove() {}, contains: () => false }, setAttribute() {}, removeAttribute() {}, addEventListener() {}, closest: () => null };
};
globalThis.document = {
  getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, element(id)), nodes.get(id))),
  querySelector(selector) {
    const id = (selector.match(/^#([\w-]+) \.segment\.active$/) || [])[1];
    if (!id) return null;
    const start = html.indexOf(`id="${id}"`);
    const active = html.slice(start, html.indexOf('</div>', start)).match(/class="segment active"[^>]*data-v="([^"]*)"/);
    return active ? { dataset: { v: active[1] } } : null;
  },
  addEventListener() {},
};
globalThis.window = { addEventListener() {} };
await import('../../src/gui/js/piece-style.js');
const { cellsKey, meldFlags, initRunControls, cellAction } = await import('../../src/gui/js/meld-features.js');

document.getElementById('advanced-meld-features-toggle').checked = true;
document.getElementById('meld-workers-select').value = 'auto';
let bbox = '44.1 26.0 44.5 26.9';
initRunControls({ invoke: async () => {}, post: () => {}, start: async () => {}, localize: () => {}, key: () => cellsKey(bbox, 'W') });

const run = () => meldFlags({ oneWorld: true, java: true });
assert.match(cellsKey(bbox, 'W'), /^44\.1 26\.0 44\.5 26\.9\|W\|/);
await cellAction('skip', [4, 2], { live: false });
assert.deepStrictEqual(meldFlags({ oneWorld: true, java: true, plan: true }).filter((f) => f.startsWith('--skip-cells')), ['--skip-cells=2,4'], 'the plan sees the skipped cells');
assert.deepStrictEqual(run().filter((f) => f.startsWith('--skip-cells')), ['--skip-cells=2,4'], 'Start sends them');
assert.deepStrictEqual(run().filter((f) => f.startsWith('--skip-cells')), [], 'once');
// Another selection is another job: its run never gets this skip list.
await cellAction('skip', [1], { live: false });
bbox = '45 26 45.5 26.5';
assert.deepStrictEqual(run().filter((f) => f.startsWith('--skip-cells')), []);
// The flags that cut the job are part of the key.
const before = cellsKey(bbox, 'W');
document.getElementById('meld-margin-input').value = '2';
assert.notStrictEqual(cellsKey(bbox, 'W'), before);
// So is whether the run builds in cells at all: Parallel cells Off or Advanced off makes the cells stale.
const cut = cellsKey(bbox, 'W');
document.getElementById('meld-workers-select').value = '';
assert.notStrictEqual(cellsKey(bbox, 'W'), cut, 'Parallel cells Off');
document.getElementById('meld-workers-select').value = '4';
assert.strictEqual(cellsKey(bbox, 'W'), cut, 'another worker count cuts the same cells');
document.getElementById('advanced-meld-features-toggle').checked = false;
assert.notStrictEqual(cellsKey(bbox, 'W'), cut, 'Advanced off');
console.log('job-key: ok');
process.exit(0);
