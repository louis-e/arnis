// Run: node tests/gui/plan-rows.mjs
// Plan drawn before a run: a built piece reads "Already built" only with Skip Built on;
// the size warning waits while the plan is recomputed.
import assert from 'node:assert';

const nodes = new Map();
const node = () => ({ value: '', checked: false, hidden: false, textContent: '', dataset: {}, classList: { toggle() {}, remove() {}, contains: () => false }, setAttribute() {}, addEventListener() {}, append() {}, replaceChildren() {} });
globalThis.document = { querySelector: () => null, addEventListener() {}, createElement: node, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
globalThis.window = { addEventListener() {} };
const { initRunControls, initJobPlan, schedulePlan, planPending, planPieces, overlapCells } = await import('../../src/gui/js/meld-features.js');

const sent = [];
const plan = { units: [{ index: 0, size: 4, regions: 16, chunks: 1024, existing_chunks: 1024, bbox: '1,2,3,4' }], estimate: { workers: 1, disk_mb: [1, 2], ram_mb: [1, 2], time_min: [1, 2] } };
const invoke = async () => plan;
initRunControls({ key: () => 'k', map: () => ({}), invoke, post: (m) => sent.push(m), start: async () => {}, localize: () => {} });
initJobPlan({ changed() {}, request: () => ({ savePath: 's', worldName: 'w', bboxText: '1 2 3 4', worldScale: 1 }) });
document.getElementById('advanced-meld-features-toggle').checked = true;
document.getElementById('meld-workers-select').value = 'auto';

const states = async (skip) => {
  document.getElementById('meld-skip-built-toggle').checked = skip;
  schedulePlan(true);
  await new Promise((r) => setTimeout(r, 400));
  return sent.filter((m) => m.type === 'pieceOutlines').at(-1).pieces.map((p) => p.state);
};
assert.deepStrictEqual(await states(false), ['planned'], 'a built piece is drawn as planned with Skip Built off');
assert.deepStrictEqual(await states(true), ['skipped'], 'and as already built with it on');

// The hold's overlap line counts the cells generated again over built land, as the engine decides:
// a partly built cell is built again even with Skip Built on (Expand), a finished or left-out one never.
const cell = (existing_chunks, more = {}) => ({ chunks: 1024, existing_chunks, ...more });
const cells = [cell(1024), cell(100), cell(0), cell(1024, { done: true }), cell(300, { skip: true })];
assert.strictEqual(overlapCells(cells, false), 2, 'built and partly built cells are built again');
assert.strictEqual(overlapCells(cells, true), 1, 'Skip Built leaves out only the cell with every chunk');
assert.strictEqual(overlapCells([cell(1024, { done: true }), cell(0)], false), 0, 'a Resume rebuilds nothing');
plan.units.push({ index: 1, size: 4, regions: 16, chunks: 1024, existing_chunks: 7, bbox: '1,2,3,4' });
await states(true);
assert.strictEqual(planPieces().overlap, 1, 'the plan carries the count with Skip Built on');
plan.single = true;
await states(false);
assert.strictEqual(planPieces().overlap, 0, 'one plain run has no cells: main asks its own question');
plan.single = false;
plan.units.pop();

// A pending plan: asked for, not yet answered.
let release;
const slow = new Promise((r) => { release = r; });
initRunControls({ key: () => 'k', map: () => ({}), invoke: () => slow, post: () => {}, start: async () => {}, localize: () => {} });
document.getElementById('meld-workers-select').value = '4';
schedulePlan(true);
assert.ok(planPending(), 'pending as soon as the request changes');
await new Promise((r) => setTimeout(r, 400));
assert.ok(planPending(), 'still pending while the answer is on its way');
release(plan);
await new Promise((r) => setTimeout(r, 20));
assert.ok(!planPending(), 'not pending once answered');

// A plan that fails says so with the translated prefix.
window.localization = { plan_error: 'Impossible de planifier : {error}' };
const shown = [];
nodes.set('job-plan', { ...node(), replaceChildren: (...c) => shown.push(c.map((x) => x.textContent ?? String(x)).join('|')) });
initRunControls({ key: () => 'k', map: () => ({}), invoke: async () => { throw 'boom'; }, post: () => {}, start: async () => {}, localize: () => {} });
document.getElementById('meld-workers-select').value = '2';
schedulePlan(true);
await new Promise((r) => setTimeout(r, 400));
assert.ok(shown.some((x) => x.includes('Impossible de planifier : boom')), shown.join(';'));
console.log('plan-rows: ok');
process.exit(0);
