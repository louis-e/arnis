// Run: node tests/gui/dock-grip.mjs
// The 3D preview card's top-left grip, with stub elements (no browser): a drag by (-100, -62) makes the
// card 100 px wider (its right and bottom do not move: it is docked there, only the width is set), a
// click on the grip never reaches the card, arrow keys resize by 20 px, and the size stays in range.
import assert from 'node:assert';

globalThis.document = { querySelector: () => null, getElementById: () => null, addEventListener() {} };
globalThis.window = { innerWidth: 1000, innerHeight: 650, addEventListener() {} };
const { initDockGrip, dockWidth } = await import('../../src/gui/js/meld-features.js');

const listeners = {};
const grip = { addEventListener: (t, f) => (listeners[t] = f), setPointerCapture() {}, releasePointerCapture() {} };
const dock = { offsetWidth: 300, style: {} };
dock.style = { set width(v) { this._w = v; dock.offsetWidth = parseInt(v, 10); }, get width() { return this._w; } };
initDockGrip(dock, grip);
const ev = (o = {}) => ({ stopped: false, stopPropagation() { this.stopped = true; }, preventDefault() {}, pointerId: 1, ...o });

const down = ev({ clientX: 500, clientY: 300 });
listeners.pointerdown(down);
assert.ok(down.stopped, 'pointerdown does not reach the card');
listeners.pointermove(ev({ clientX: 400, clientY: 238 }));
assert.equal(dock.offsetWidth, 400, 'dragging (-100, -62) grows the card by 100 px');
listeners.pointerup(ev());
listeners.pointermove(ev({ clientX: 0, clientY: 0 }));
assert.equal(dock.offsetWidth, 400, 'no drag after pointerup');
const click = ev();
listeners.click(click);
assert.ok(click.stopped, 'a click on the grip does not open the full preview');

assert.equal(dockWidth(300, 1000, 1920, 1080), 200, 'minimum 200');
assert.equal(dockWidth(300, -1000, 1000, 650), 600, '60 % of the width');
assert.equal(dockWidth(300, -1000, 1920, 400), 384, '60 % of the height at 16:10');
const key = ev({ key: 'ArrowLeft' });
listeners.keydown(key);
assert.equal(dock.offsetWidth, 420, 'arrow left grows by 20 px');
listeners.keydown(ev({ key: 'ArrowDown' }));
assert.equal(dock.offsetWidth, 400);
console.log('dock-grip: ok');
process.exit(0);
