// Run: node tests/gui/select-ring.mjs
// "Select this area" and Expand: saved settings change only in a One World, and the window says which.
import assert from 'node:assert';

const nodes = new Map();
const changes = [];
const node = (id) => ({ value: id === 'meld-margin-input' ? '0' : '', checked: false, textContent: '', style: {}, addEventListener() {}, dispatchEvent() { changes.push(id); } });
globalThis.document = { querySelector: () => null, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node(id)), nodes.get(id))) };
globalThis.window = { addEventListener() {} };
const { initRunControls, selectWithRing } = await import('../../src/gui/js/meld-features.js');

const sent = [];
initRunControls({ invoke: async () => {}, post: (m) => sent.push(m), start: async () => {}, localize: () => {} });
const el = (id) => document.getElementById(id);
const bounds = [47.2, 9.5, 50.6, 13.9];

// One World off: the area is selected and nothing else changes.
assert.deepStrictEqual(selectWithRing(bounds, false), []);
assert.deepStrictEqual(sent.at(-1), { type: 'setBbox', bounds });
assert.deepStrictEqual([el('meld-workers-select').value, el('meld-margin-input').value, changes.length], ['', '0', 0]);
assert.strictEqual(el('progress-info').textContent, '', 'nothing to say');

// One World on: Parallel cells and the margin are set, and the status line names them.
assert.deepStrictEqual(selectWithRing(bounds, true), ['Parallel cells', 'Margin Regions']);
assert.deepStrictEqual([el('meld-workers-select').value, el('meld-margin-input').value], ['auto', '1']);
assert.deepStrictEqual(changes, ['meld-workers-select', 'meld-margin-input'], 'the settings store hears of both');
assert.strictEqual(el('progress-info').textContent, 'Settings changed: Parallel cells, Margin Regions');

// Expand also turns Skip Built on; what is set already is not named again.
el('meld-workers-select').value = '4';
el('meld-margin-input').value = '2';
assert.deepStrictEqual(selectWithRing(bounds, true, true), ['Skip Built Cells']);
assert.deepStrictEqual([el('meld-workers-select').value, el('meld-margin-input').value, el('meld-skip-built-toggle').checked], ['4', '2', true]);
window.localization = { settings_changed: 'Geänderte Einstellungen: {list}', meld_skip_built: 'Gebaute Zellen überspringen' };
el('meld-skip-built-toggle').checked = false;
selectWithRing(bounds, true, true);
assert.strictEqual(el('progress-info').textContent, 'Geänderte Einstellungen: Gebaute Zellen überspringen');
console.log('select-ring: ok');
process.exit(0);
