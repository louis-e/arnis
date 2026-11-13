// Run: node tests/gui/projects.mjs
// Settings > Projects: one row per world, Open stores the pick, the tab hides with Advanced features off.
import assert from 'node:assert';
import fs from 'node:fs';

const node = () => ({ children: [], listeners: {}, attrs: {}, className: '', classList: { add() {}, remove() {} }, closest: () => null, getAttribute(k) { return this.attrs[k] ?? null; }, setAttribute(k, v) { this.attrs[k] = v; }, removeAttribute(k) { delete this.attrs[k]; }, addEventListener(e, f) { this.listeners[e] = f; }, append(...c) { c.forEach((k) => { k.parent = this; }); this.children.push(...c); }, remove() { this.parent.children = this.parent.children.filter((k) => k !== this); }, replaceChildren(...c) { this.children = c; }, querySelector(sel) { return this.children.find((c) => sel === `.${c.className}`) || null; } });
const nodes = new Map();
globalThis.document = { createElement: node, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
const store = new Map();
globalThis.localStorage = { setItem: (k, v) => store.set(k, v), getItem: (k) => store.get(k) ?? null };
globalThis.window = {};
await import('../../src/gui/js/piece-style.js');
const { initProjects } = await import('../../src/gui/js/meld-features.js');

const worlds = [
  { name: 'Arnis One World', area_count: 3, scale: 1, locked: false, modified_ms: 1e12 },
  { name: 'Paris', area_count: 1, scale: 0.5, locked: false, modified_ms: 1e12 },
  { name: 'Busy', area_count: 2, scale: 1, locked: true, modified_ms: null },
];
const opened = [];
const refresh = initProjects({ invoke: async () => worlds, savePath: () => 's', scale: () => 1, open: async (n, x) => opened.push([n, x]) });
await refresh();
const rows = document.getElementById('projects-list').children;
const flat = (r) => r.children.flatMap((c) => (c.className === 'projects-actions' ? c.children : [c])); // the buttons sit in one box
const btn = (r, label) => flat(r).find((c) => c.textContent === label);
assert.strictEqual(rows.length, 3, 'three entries, three rows');
const cells = (r) => r.children.map((c) => c.textContent);
assert.ok(cells(rows[0]).includes('Areas: 3'));
assert.ok(cells(rows[1]).includes('Scale 1:2'));
assert.ok(rows[1].children.some((c) => c.className.includes('is-warn')), 'a different scale is flagged');
assert.ok(cells(rows[2]).includes('In use') && btn(rows[2], 'Open').disabled && btn(rows[2], 'Delete').disabled, 'a locked world cannot be opened or deleted');

await btn(rows[1], 'Open').listeners.click();
assert.strictEqual(store.get('arnis-one-world-name'), 'Paris');
assert.deepStrictEqual(opened.at(-1), ['Paris', false]);
await btn(rows[0], 'Expand').listeners.click();
assert.deepStrictEqual(opened.at(-1), ['Arnis One World', true], 'Expand opens then expands');

let ok = false;
const off = initProjects({ invoke: async () => worlds, savePath: () => 's', scale: () => 1, open: async () => {}, available: () => ok });
await off();
const first = document.getElementById('projects-list').children[0];
assert.ok(btn(first, 'Expand').disabled, 'no One World here: Open is disabled');
assert.ok(!btn(first, 'Delete').disabled, 'a world can still be deleted');

// Delete: two presses (arm, confirm) move the world to the trash; a refusal shows in its row.
const asked = [];
const gone = [];
let refuse = 'The world is in use; close it in Minecraft or wait for the run.';
const del = initProjects({
  invoke: async (cmd, args) => {
    if (cmd !== 'gui_trash_one_world') return worlds;
    asked.push(args);
    if (refuse) throw refuse;
  },
  savePath: () => 's', scale: () => 1, open: async () => {}, trashed: (n) => gone.push(n),
});
await del();
const paris = () => document.getElementById('projects-list').children[1];
const press = async () => { await btn(paris(), 'Delete').listeners.click({ detail: 0 }); await new Promise((r) => setTimeout(r, 0)); };
await press();
assert.strictEqual(asked.length, 0, 'one press only arms it');
await press();
assert.deepStrictEqual(asked, [{ savePath: 's', worldName: 'Paris' }]);
assert.deepStrictEqual(gone, [], 'refused: the window keeps the world');
assert.ok(paris().children.some((c) => c.className === 'projects-error' && c.textContent === refuse), 'the reason shows');
assert.ok(!btn(paris(), 'Delete').disabled, 'and it can be tried again');
refuse = null;
await press();
await press();
assert.deepStrictEqual(gone, ['Paris'], 'moved: the window forgets it');
assert.strictEqual(asked.length, 2);

worlds.length = 0;
await refresh();
assert.strictEqual(document.getElementById('projects-list').children[0].textContent, 'No worlds yet. Build one with One World on.');

// Advanced features off hides the tab.
const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const css = fs.readFileSync(new URL('../../src/gui/css/settings.css', import.meta.url), 'utf8');
assert.match(html, /class="settings-nav-item meld-only" data-target="settings-section-projects"/);
assert.match(css, /body\.meld-off \.meld-only\s*\{\s*display: none !important/);
console.log('projects ok');
