// Run: node tests/gui/stretch.mjs
// A One World's blocks per metre are fixed at its origin's latitude: the 5% and 10% lines on
// the map, and the stretch the plan panel reads out.
import assert from 'node:assert';

globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: () => null };
globalThis.window = { addEventListener() {} };
const { stretchAt, stretchLines, worstStretch } = await import('../../src/gui/js/meld-features.js');

const near = (a, b) => assert.ok(Math.abs(a - b) < 0.1, `${a} is not about ${b}`);
// Bucharest: wider north of it, narrower south.
const [south, north] = stretchLines(44.43, 0.05);
near(north, 47.1);
near(south, 41.25);
near(stretchAt(44.43, north), 0.05);
near(stretchAt(44.43, south), -0.05);
// At the equator nothing is narrower: both lines are the widening one, mirrored.
const [s0, n0] = stretchLines(0, 0.05);
near(n0, 17.75);
near(s0, -17.75);
assert.ok(stretchAt(0, s0) > 0 && stretchAt(0, n0) > 0, 'no narrowing line at the equator');
// Far south of the equator the nearer line south is the narrowing one.
assert.ok(stretchLines(-60, 0.1).every((l) => l !== null));
// The worst of a selection is at its edge furthest from none.
near(worstStretch(44.43, 44, 52), stretchAt(44.43, 52));
near(worstStretch(10, -1, 1), stretchAt(10, 0));
console.log('stretch: ok');
