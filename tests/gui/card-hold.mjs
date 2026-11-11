// Run: node tests/gui/card-hold.mjs
// A failed piece's card (Open log) stays while the pointer is on it, on the piece, or just left
// the piece, so a neighbour's card cannot take its place on the way.
import assert from 'node:assert';

await import('../../src/gui/js/piece-style.js');
const hold = globalThis.arnisCardHold;
assert.equal(hold(true, 0, 99999), true, 'over the card');
assert.equal(hold(false, null, 99999), true, 'on the piece');
assert.equal(hold(false, 1000, 1500), true, 'just left, still travelling');
assert.equal(hold(false, 1000, 2000), false, 'left a while ago');
assert.equal(hold(false, undefined, 2000), false, 'never entered');
// A layer removed with its card open: no poll left, no hold, and it is no longer the held card.
const item = { leftAt: null, closing: setTimeout(() => assert.fail('poll still running'), 50) };
assert.equal(globalThis.arnisCardDrop(item, item), null);
assert.equal(item.leftAt, 0);
assert.equal(hold(false, item.leftAt, 2000), false);
const other = {};
assert.equal(globalThis.arnisCardDrop({ leftAt: null }, other), other, 'another piece stays held');
// Projects Open on the world already shown refits the map; the same overlays posted again do not.
const fit = globalThis.arnisShouldFit;
assert.equal(fit('w', 'w', false), false, 'same world, plain refresh');
assert.equal(fit('w', 'w', true), true, 'same world, Open');
assert.equal(fit('v', 'w', false), true, 'another world');
console.log('card-hold ok');
