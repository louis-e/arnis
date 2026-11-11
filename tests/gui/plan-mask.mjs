// Run: node tests/gui/plan-mask.mjs
// An outline from the map: written by the backend, passed as --plan-mask to a job in pieces only,
// dropped with null; the plan's pieces outside it are drawn grey and unnumbered.
import assert from 'node:assert';

const nodes = new Map();
const node = () => ({ value: '', checked: false, hidden: false, textContent: '', dataset: {}, classList: { toggle() {}, remove() {}, contains: () => false }, setAttribute() {}, addEventListener() {}, append() {}, replaceChildren() {} });
globalThis.document = { querySelector: () => null, addEventListener() {}, createElement: node, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
globalThis.window = { addEventListener() {} };
const { initRunControls, initJobPlan, meldFlags, setPlanMask, hasPlanMask } = await import('../../src/gui/js/meld-features.js');

const sent = [];
const calls = [];
const plan = { units: [{ index: 0, size: 4, regions: 16, chunks: 1024, existing_chunks: 0, bbox: '1,2,3,4' }], estimate: { workers: 1, disk_mb: [1, 2], ram_mb: [1, 2], time_min: [1, 2] }, excluded: ['5,6,7,8'] };
const invoke = async (cmd, a) => { calls.push([cmd, a]); return cmd === 'gui_write_plan_mask' ? 'C:\Temp\arnis-plan-mask-1.geojson' : plan; };
initRunControls({ key: () => 'k', map: () => ({}), invoke, post: (m) => sent.push(m), start: async () => {}, localize: () => {} });
initJobPlan({ changed() {}, request: () => ({ savePath: 's', worldName: 'w', bboxText: '1 2 3 4', worldScale: 1 }) });
document.getElementById('advanced-meld-features-toggle').checked = true;

const square = { type: 'Polygon', coordinates: [[[2, 1], [4, 1], [4, 3], [2, 1]]] };
await setPlanMask(square);
assert.ok(hasPlanMask());
assert.deepStrictEqual(JSON.parse(calls[0][1].geojson), square, 'the backend gets the outline as sent');
// Only a job in pieces (One World with workers) takes it.
assert.ok(!meldFlags({ oneWorld: true, java: true }).some((f) => f.startsWith('--plan-mask')));
document.getElementById('meld-workers-select').value = 'auto';
assert.ok(meldFlags({ oneWorld: true, java: true }).includes('--plan-mask=C:\Temp\arnis-plan-mask-1.geojson'));
assert.ok(!meldFlags({ oneWorld: false, java: true }).some((f) => f.startsWith('--plan-mask')));

// The plan: one numbered piece, one grey piece outside the outline.
await new Promise((r) => setTimeout(r, 400));
const pieces = sent.filter((m) => m.type === 'pieceOutlines').at(-1).pieces;
assert.deepStrictEqual(pieces.map((p) => [p.state, p.n]), [['planned', 1], ['excluded', '']]);
assert.deepStrictEqual(pieces[1].bounds, [5, 6, 7, 8]);

await setPlanMask(null);
assert.ok(!hasPlanMask());
assert.ok(!meldFlags({ oneWorld: true, java: true }).some((f) => f.startsWith('--plan-mask')));
// A write that fails returns the reason and leaves no mask.
initRunControls({ key: () => 'k', map: () => ({}), invoke: async () => { throw new Error('too big'); }, post() {}, start: async () => {}, localize: () => {} });
assert.match(await setPlanMask(square), /too big/);
assert.ok(!hasPlanMask());
console.log('plan-mask: ok');
process.exit(0);
