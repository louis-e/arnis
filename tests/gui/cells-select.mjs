// Run: node tests/gui/cells-select.mjs
// The cell select tool and menu: what a click or box picks, which actions fit, and what they send.
import assert from 'node:assert';

globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: () => null };
globalThis.window = { addEventListener() {} };
await import('../../src/gui/js/piece-style.js');
const { cellAction, legendRows, removedLine } = await import('../../src/gui/js/meld-features.js');
const C = globalThis.arnisCells;

// A 10 x 10 grid of cells, 1 degree each, index = row * 10 + col; two grey ones beside it.
const grid = [];
for (let r = 0; r < 10; r++) for (let c = 0; c < 10; c++) grid.push({ i: r * 10 + c, state: 'planned', bounds: [r, c, r + 1, c + 1] });
grid.push({ i: 100, state: 'excluded', bounds: [0, 10, 1, 11] }, { i: 101, state: 'room', bounds: [1, 10, 2, 11] });
const sorted = (list) => [...list].sort((a, b) => a - b);

// A box picks every cell whose centre is inside, whether or not it is drawn; the grey ones never.
assert.deepStrictEqual(sorted(C.inBox(grid, [2.2, 3.2, 4.8, 5.8])), [23, 24, 25, 33, 34, 35, 43, 44, 45]);
assert.strictEqual(C.inBox(grid, [4.8, 5.8, 2.2, 3.2]).length, 9, 'a box dragged backwards is the same box');
assert.deepStrictEqual(sorted(C.inBox(grid, [0, 9, 3, 12])), [9, 19, 29]);
assert.deepStrictEqual(C.inBox(grid, [20, 20, 30, 30]), []);
assert.strictEqual(C.at(grid, 3.5, 4.5), 34);
assert.strictEqual(C.at(grid, 0.5, 10.5), -1, 'a grey cell is not picked');
assert.strictEqual(C.at(grid, 50, 50), -1);

// A plain pick replaces the selection (nothing picked clears it); Shift/Ctrl adds, and takes away what is all in.
let sel = C.pick(new Set(), [34], false);
assert.deepStrictEqual([...sel], [34]);
sel = C.pick(sel, [35], true);
assert.deepStrictEqual(sorted(sel), [34, 35]);
sel = C.pick(sel, [34], true);
assert.deepStrictEqual([...sel], [35], 'Shift on a picked cell takes it away');
sel = C.pick(sel, [35, 36], true);
assert.deepStrictEqual(sorted(sel), [35, 36], 'a box with a new cell adds');
sel = C.pick(sel, [35, 36], true);
assert.strictEqual(sel.size, 0, 'a box of picked cells takes them away');
assert.strictEqual(C.pick(new Set([1, 2]), [], false).size, 0, 'a click on nothing clears');
assert.strictEqual(C.key({ key: 'Escape' }, new Set([1, 2])).size, 0, 'Esc clears');
const keep = new Set([1]);
assert.strictEqual(C.key({ key: 'a' }, keep), keep, 'another key leaves it');

// Actions: only what fits every picked cell.
const ids = (cells, running) => C.actions(cells.map((state) => ({ state })), running).map((a) => a.id);
assert.deepStrictEqual(ids(['planned', 'planned'], false), ['skip']);
assert.deepStrictEqual(ids(['omitted'], false), ['include']);
assert.deepStrictEqual(ids(['done', 'done'], false), ['rebuild', 'remove']);
assert.deepStrictEqual(ids(['done', 'failed'], false), [], 'a mixed selection has no common action');
assert.deepStrictEqual(ids(['failed', 'stopped'], false), ['skip', 'retry']);
assert.deepStrictEqual(ids(['queued', 'queued'], true), ['pause', 'next', 'skip']);
assert.deepStrictEqual(ids(['queued', 'held'], true), ['next', 'skip'], 'pause does not fit a held cell');
assert.deepStrictEqual(ids(['held'], true), ['resume', 'next', 'skip']);
assert.deepStrictEqual(ids(['start', 'retry'], true), ['pause', 'pausenow', 'cancel']);
assert.deepStrictEqual(ids(['queued', 'start'], true), ['pause', 'pausenow']);
assert.deepStrictEqual(ids(['failed'], true), ['retry']);
assert.deepStrictEqual(ids(['done', 'queued'], true), []);
// The head cell: hold and cancel are listed with the reason, not greyed without a word.
const head = C.actions([{ state: 'start', head: true }], true);
assert.deepStrictEqual(head.map((a) => a.id), ['pause', 'pausenow', 'cancel']);
assert.ok(head.every((a) => a.why && a.why[0] === 'cell_head_why'));
assert.ok(C.actions([{ state: 'start' }], true).every((a) => !a.why));
assert.ok(C.actions([{ state: 'queued' }, { state: 'start', head: true }], true).every((a) => a.why), 'one head cell in the selection disables them');

// Engine calls.
assert.deepStrictEqual(C.calls('pause', [1, 2], 'w'), [['gui_cells_control', { action: 'hold', cells: [1, 2] }]]);
assert.deepStrictEqual(C.calls('next', [3], 'w'), [['gui_cells_control', { action: 'front', cells: [3] }]]);
assert.deepStrictEqual(C.calls('pausenow', [4], 'w').map(([, a]) => a.action), ['hold', 'cancel']);
assert.deepStrictEqual(C.calls('remove', [5, 6], 'C:/w', 'J'), [['gui_cells_remove', { worldPath: 'C:/w', cells: [5, 6], job: 'J' }]]);
assert.strictEqual(C.skipFlag([7, 2, 7, 10, 1]), '--skip-cells=1,2,7,10');
assert.strictEqual(C.skipFlag([]), '--skip-cells=');

// Remove asks first, and sends the world and the sorted cells only when held.
const sent = [];
const call = async (command, args) => (sent.push([command, args]), { removed: [3], not_built: [9] });
let after = 0;
await cellAction('remove', [9, 3, 3], { call, ask: async () => false, live: false, world: () => 'W', job: () => 'J', after: () => after++ });
assert.deepStrictEqual(sent, [], 'no hold, no call');
const ask = async (action, list) => (assert.deepStrictEqual([action, list], ['remove', [3, 9]]), true);
await cellAction('remove', [9, 3, 3], { call, ask, live: false, world: () => 'W', job: () => 'J', after: () => after++ });
assert.deepStrictEqual(sent, [['gui_cells_remove', { worldPath: 'W', cells: [3, 9], job: 'J' }]], 'the job the cells belong to goes with them');
assert.strictEqual(after, 1);
assert.strictEqual(removedLine({ removed: [3], not_built: [9] }), 'Cells removed: 1. Not built, so not removed: 1.');
assert.strictEqual(removedLine({ removed: [3, 4], not_built: [] }), 'Cells removed: 2.');
// Rebuild removes land first, so it is refused unless a run in cells would build it again.
sent.length = 0;
await cellAction('rebuild', [4], { call, ask: async () => true, live: false, world: () => 'W', job: () => 'J', after: () => after++, runs: () => false });
assert.deepStrictEqual(sent, [], 'no cells run: nothing is removed');
assert.strictEqual(after, 1);
const rebuilt = [];
await cellAction('rebuild', [4], { call, ask: async () => true, live: false, world: () => 'W', job: () => 'J', after: (list, rebuild) => rebuilt.push([list, rebuild]), runs: () => true });
assert.deepStrictEqual(sent, [['gui_cells_remove', { worldPath: 'W', cells: [4], job: 'J' }]]);
assert.deepStrictEqual(rebuilt, [[[4], true]], 'removed, then rebuilt');
// An engine refusal is shown, not thrown, and nothing refreshes.
await cellAction('remove', [1], { call: async () => { throw 'A job is building this world; stop it first.'; }, ask: async () => true, live: false, after: () => after++ });
assert.strictEqual(after, 1);
// A running job: Pause now sends hold, then cancel; neither asks.
sent.length = 0;
await cellAction('pausenow', [2, 1], { call, ask: async () => assert.fail('no question'), live: true });
assert.deepStrictEqual(sent.map(([c, a]) => [c, a.action, a.cells]), [['gui_cells_control', 'hold', [1, 2]], ['gui_cells_control', 'cancel', [1, 2]]]);

// The legend lists the new states.
const rows = legendRows({ cells: [{ state: 'held' }, { state: 'omitted' }], stretch: [], picked: true }).map((r) => r.key);
assert.deepStrictEqual(rows, ['run_state_held', 'run_state_omitted', 'legend_picked']);
console.log('cells-select: ok');
