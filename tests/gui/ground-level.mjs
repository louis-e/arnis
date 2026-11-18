// Run: node tests/gui/ground-level.mjs
// Ground Level in Advanced features: what a run is sent, when the field greys and why, the snap, and the quiet row beside Lowest Y.
import assert from 'node:assert';
import fs from 'node:fs';

// The field, so the snap the module wires up on import can be driven.
const fakeField = () => ({ value: '', disabled: false, listeners: [], addEventListener(type, fn) { if (type === 'change') this.listeners.push(fn); }, dispatchEvent() { this.listeners.forEach((fn) => fn()); } });
const field = fakeField();
const lowest = fakeField();
globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: (id) => ({ 'meld-ground-level-input': field, 'meld-min-y-input': lowest })[id] || null };
globalThis.window = { addEventListener() {} };
const { typedGroundLevel, groundLevelFor, DEFAULT_GROUND_LEVEL } = await import('../../src/gui/js/meld-features.js');

assert.strictEqual(DEFAULT_GROUND_LEVEL, -62);
for (const [typed, level] of [['', -62], ['63', 63], ['0', 0], [' 12 ', 12], ['12.7', 12], ['-100', -62], ['400', 304], ['abc', -62]]) {
  assert.strictEqual(typedGroundLevel(typed), level, `"${typed}"`);
}

const plain = { advanced: true, typed: '63', earth: true, oneWorld: false, terrain: true, world: null };
const choice = (o) => groundLevelFor({ ...plain, ...o });
// Untouched, or with Advanced features off, a run is sent the default.
assert.deepStrictEqual(choice({ typed: '' }), { level: -62, why: null });
assert.deepStrictEqual(choice({ advanced: false }), { level: -62, why: null });
assert.deepStrictEqual(choice({}), { level: 63, why: null });
// Off Earth it changes nothing, so the field is greyed and the default sent.
assert.deepStrictEqual(choice({ earth: false }), { level: -62, why: 'reason_earth' });
// A new One World: terrain has heights of its own, a flat world takes the level.
assert.deepStrictEqual(choice({ oneWorld: true }), { level: -62, why: 'reason_not_one_world' });
assert.deepStrictEqual(choice({ oneWorld: true, terrain: false }), { level: 63, why: null });
// An existing One World keeps its own, Advanced features on or off.
const world = { exists: true, ground_level: 10 };
assert.deepStrictEqual(choice({ oneWorld: true, world }), { level: 10, why: 'reason_world_set' });
assert.deepStrictEqual(choice({ oneWorld: true, world, advanced: false, terrain: false }), { level: 10, why: 'reason_world_set' });
// That world means nothing to a run with One World off.
assert.deepStrictEqual(choice({ world }), { level: 63, why: null });

// Typed values snap into range; a greyed field shows a One World's own value untouched.
const type = (v) => { field.value = v; field.dispatchEvent(); return field.value; };
assert.strictEqual(type('400'), '304');
assert.strictEqual(type('-100'), '-62');
assert.strictEqual(type('63'), '63');
assert.strictEqual(type('abc'), '');
field.disabled = true;
assert.strictEqual(type('-500'), '-500');
// Lowest Y still snaps to a section line while greyed, as it always did.
lowest.disabled = true;
lowest.value = '-100';
lowest.dispatchEvent();
assert.strictEqual(lowest.value, '-96');

// In the World group beside Lowest Y, with Advanced features only, no "New!" badge, the label every locale has, and stored like its neighbours.
const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const at = html.indexOf('id="meld-ground-level-input"');
const rowStart = html.lastIndexOf('<div class="settings-row', at);
const row = html.slice(rowStart, html.indexOf('<div class="settings-row', at));
assert.ok(html.slice(rowStart, rowStart + 80).includes('meld-only'), 'shown with Advanced features only');
assert.ok(!row.includes('new-badge'), 'no New! badge');
assert.ok(row.includes('data-localize="ground_level"'), 'reuses the ground_level label');
const group = html.lastIndexOf('settings-group-title', at);
assert.ok(html.slice(group, group + 120).includes('tuning_group_world'), 'in the World group');
assert.ok(html.indexOf('id="meld-min-y-input"') > at && html.indexOf('id="meld-min-y-input"') - at < 2000, 'next to Lowest Y');
const store = fs.readFileSync(new URL('../../src/gui/js/settings-store.js', import.meta.url), 'utf8');
assert.ok(store.includes("{ id: 'meld-ground-level-input', kind: 'text', store: OWN }"), 'saved and restored');
for (const f of fs.readdirSync(new URL('../../src/gui/locales/', import.meta.url))) {
  const loc = JSON.parse(fs.readFileSync(new URL(`../../src/gui/locales/${f}`, import.meta.url), 'utf8'));
  for (const key of ['ground_level', 'ground_level_desc', 'ground_level_tip']) assert.ok(loc[key], `${f} lacks ${key}`);
}
console.log('ground-level: ok');
