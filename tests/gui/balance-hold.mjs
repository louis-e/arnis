// Run: node tests/gui/balance-hold.mjs
// The map's balance slider (-100..100) and the held Clear selection button.
import assert from 'node:assert';

await import('../../src/gui/js/piece-style.js');

// Slider: -100 fades the world overlay out and leaves the cells; +100 fades the cells out and
// leaves the world overlay at its default; 0 is both at their defaults.
const b = globalThis.arnisBalance;
assert.deepStrictEqual(b(-100), { world: 0, cells: 1 });
assert.deepStrictEqual(b(100), { world: 0.7, cells: 0 });
assert.deepStrictEqual(b(0), { world: 0.7, cells: 1 });
assert.equal(b(-50).world, 0.35);
assert.equal(b(50).cells, 0.5);
assert.equal(b(-50).cells, 1, 'left never touches the cells');
assert.equal(b(50).world, 0.7, 'right never touches the world overlay');
assert.equal(b(-100, 1).world, 0);
assert.equal(b(0, 1).world, 1, 'Classic One World default: opaque');

// Hold: a short click does not fire, a hold of the full time does.
const listeners = {};
const classes = new Set();
const attrs = {};
const button = { disabled: false, closest: () => null, classList: { add: (c) => classes.add(c), remove: (c) => classes.delete(c) }, addEventListener: (t, f) => { listeners[t] = f; }, getAttribute: (k) => attrs[k] ?? null, setAttribute: (k, v) => { attrs[k] = v; }, removeAttribute: (k) => { delete attrs[k]; } };
let fired = 0;
const MS = globalThis.arnisClearHoldMs;
assert.equal(MS, 800);
globalThis.arnisHold(button, () => fired++, MS);
listeners.pointerdown({ button: 0 });
assert.ok(classes.has('is-holding'), 'the fill runs while held');
await new Promise((r) => setTimeout(r, 100));
listeners.pointerup();
assert.ok(!classes.has('is-holding'));
await new Promise((r) => setTimeout(r, MS + 100));
assert.equal(fired, 0, 'a short click does not clear');
listeners.pointerdown({ button: 0 });
await new Promise((r) => setTimeout(r, MS + 100));
assert.equal(fired, 1, 'a full hold clears');
assert.ok(!classes.has('is-holding'));
// A held key works the same; an early keyup does not fire; a blocked button never starts.
const press = () => { listeners.keydown({ key: 'Enter', repeat: false, preventDefault() {} }); listeners.keyup({ key: 'Enter', preventDefault() {} }); };
press();
await new Promise((r) => setTimeout(r, MS + 100));
assert.equal(fired, 1);
assert.ok(classes.has('is-armed') && attrs['aria-label'] === 'Press again to confirm', 'a short press asks for a second');
press();
assert.equal(fired, 2, 'a second press within a few seconds confirms');
assert.ok(!classes.has('is-armed') && !('aria-label' in attrs));
// A click from assistive technology (no pointer: detail 0) does the same; a mouse click never.
listeners.click({ detail: 1 });
listeners.click({ detail: 1 });
assert.equal(fired, 2);
listeners.click({ detail: 0 });
listeners.click({ detail: 0 });
assert.equal(fired, 3);
listeners.keydown({ key: ' ', repeat: false, preventDefault() {} });
await new Promise((r) => setTimeout(r, MS + 100));
assert.equal(fired, 4, 'Space held');
let blocked = true;
globalThis.arnisHold(button, () => fired++, 50, () => blocked);
listeners.pointerdown({ button: 0 });
await new Promise((r) => setTimeout(r, 120));
listeners.click({ detail: 0 });
listeners.click({ detail: 0 });
assert.equal(fired, 4, 'nothing selected: nothing to clear');
console.log('balance-hold: ok');
