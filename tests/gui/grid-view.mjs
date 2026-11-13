// Run: node tests/gui/grid-view.mjs
// The cell grid on the map: zoomed in, only the cells inside the view are drawn (never more than
// the cap), so a job of tens of thousands of cells stays cheap; zoomed out, the job's outline.
import assert from 'node:assert';

await import('../../src/gui/js/piece-style.js');
const inView = globalThis.arnisCellsInView;
const extent = globalThis.arnisCellsExtent;

// 247 x 250 = 61,750 cells of 0.001 degrees, like a large One World job.
const cells = [];
for (let r = 0; r < 247; r++) for (let c = 0; c < 250; c++) {
  cells.push({ i: cells.length, state: 'planned', bounds: [r * 0.001, c * 0.001, (r + 1) * 0.001, (c + 1) * 0.001] });
}
const t0 = Date.now();
const zoomedIn = inView(cells, [0.1005, 0.1005, 0.1085, 0.1085], 2000);
assert.equal(zoomedIn.over, false);
assert.ok(zoomedIn.cells.length > 0 && zoomedIn.cells.length <= 100, `only the view: ${zoomedIn.cells.length}`);
assert.ok(zoomedIn.cells.every((p) => p.bounds[2] >= 0.1005 && p.bounds[0] <= 0.1085));
const zoomedOut = inView(cells, [-90, -180, 90, 180], 2000);
assert.equal(zoomedOut.over, true, 'more in view than the cap');
assert.equal(zoomedOut.cells.length, 2000, 'never more than the cap');
assert.ok(Date.now() - t0 < 1000, 'cheap for 61,750 cells');
assert.deepEqual(inView(cells, [50, 50, 51, 51]).cells, [], 'nothing in view');

// The outline is the built land: grey cells outside it and the room to grow do not widen it.
const e = extent([...cells.slice(0, 3), { state: 'excluded', bounds: [-5, -5, 5, 5] }, { state: 'room', bounds: [-6, -6, 6, 6] }]);
assert.deepEqual(e, [0, 0, 0.001, 0.003]);
assert.equal(extent([]), null);
console.log('grid-view ok');
