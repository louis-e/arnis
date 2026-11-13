// Run: node tests/gui/stretch-toggle.mjs
// The stretch readout (the plan's "Stretch here" line, the 5% / 10% lines and labels on the map,
// their legend rows) shows only with "Show Map Stretch" in Advanced settings; off by default.
import assert from 'node:assert';

const nodes = new Map();
const node = () => ({ value: '', checked: false, hidden: false, textContent: '', className: '', dataset: {}, children: [], classList: { toggle() {}, remove() {}, contains: () => false }, setAttribute() {}, addEventListener() {}, append() {}, replaceChildren(...c) { this.children = c; } });
globalThis.document = { querySelector: () => null, addEventListener() {}, createElement: node, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
globalThis.window = { addEventListener() {} };
const { initRunControls, initJobPlan, schedulePlan, showRunControls } = await import('../../src/gui/js/meld-features.js');

const sent = [];
initRunControls({ key: () => 'k', map: () => ({}), invoke: async () => ({ units: [], estimate: {} }), post: (m) => sent.push(m), start: async () => {}, localize: () => {} });
initJobPlan({ changed() {}, request: () => null, stretch: () => ({ origin: 44.4, knee: null, south: 44.4, north: 50, lon: 26 }) });
const lines = () => sent.filter((m) => m.type === 'stretchLines').at(-1)?.lines;
const hasRow = () => document.getElementById('job-plan').children.some((c) => c.className === 'plan-stretch');
const features = document.getElementById('advanced-meld-features-toggle');
const stretch = document.getElementById('meld-stretch-toggle');

features.checked = true;
schedulePlan();
assert.ok(!lines()?.length, 'toggle off: no lines or labels on the map');
assert.ok(!hasRow(), 'toggle off: no stretch line in the plan');

stretch.checked = true;
schedulePlan();
assert.ok(lines().length > 0 && lines().every((l) => l.label.endsWith('%')), 'toggle on: lines with their % labels');
assert.ok(hasRow(), 'toggle on: the stretch line is shown');

showRunControls(true);
schedulePlan();
assert.ok(!hasRow(), 'during a run the stretch line is hidden');
showRunControls(false);
schedulePlan();
assert.ok(hasRow(), 'and back after it');

stretch.checked = false;
schedulePlan();
assert.deepStrictEqual(lines(), [], 'toggle off again: the map is cleared');
assert.ok(!hasRow());
console.log('stretch-toggle: ok');
process.exit(0);
