// Run: node tests/gui/snap-drag.mjs
// With Snap to Pieces on, a dragged selection changes in whole regions.
import assert from 'node:assert';

await import('../../src/gui/js/piece-style.js');
const { span, shift } = globalThis.arnisSnap;
const near = (a, b) => assert.ok(Math.abs(a - b) < 1e-9, `${a} is not ${b}`);

near(shift(0.4, 1), 0);
near(shift(0.6, 1), 1);
near(shift(-2.4, 1), -2);
near(span(2.6, 1), 3);
near(span(0.2, 1), 1); // never thinner than one region
near(span(-0.2, 1), -1); // dragged past the fixed edge: the sign is kept
near(span(-3.4, 0.5), -3.5);
console.log('snap-drag: ok');
