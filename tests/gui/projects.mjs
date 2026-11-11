// Run: node tests/gui/projects.mjs
// Settings > Projects: one row per world, Open stores the pick, the tab hides with Advanced features off.
import assert from 'node:assert';
import fs from 'node:fs';

const node = () => ({ children: [], listeners: {}, setAttribute() {}, addEventListener(e, f) { this.listeners[e] = f; }, append(...c) { this.children.push(...c); }, replaceChildren(...c) { this.children = c; } });
const nodes = new Map();
globalThis.document = { createElement: node, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
const store = new Map();
globalThis.localStorage = { setItem: (k, v) => store.set(k, v), getItem: (k) => store.get(k) ?? null };
globalThis.window = {};
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
assert.strictEqual(rows.length, 3, 'three entries, three rows');
const cells = (r) => r.children.map((c) => c.textContent);
assert.ok(cells(rows[0]).includes('Areas: 3'));
assert.ok(cells(rows[1]).includes('Scale 1:2'));
assert.ok(rows[1].children.some((c) => c.className.includes('is-warn')), 'a different scale is flagged');
assert.ok(cells(rows[2]).includes('In use') && rows[2].children.at(-1).disabled, 'a locked world cannot be opened');

await rows[1].children.at(-2).listeners.click();
assert.strictEqual(store.get('arnis-one-world-name'), 'Paris');
assert.deepStrictEqual(opened.at(-1), ['Paris', false]);
await rows[0].children.at(-1).listeners.click();
assert.deepStrictEqual(opened.at(-1), ['Arnis One World', true], 'Expand opens then expands');

let ok = false;
const off = initProjects({ invoke: async () => worlds, savePath: () => 's', scale: () => 1, open: async () => {}, available: () => ok });
await off();
assert.ok(document.getElementById('projects-list').children[0].children.at(-1).disabled, 'no One World here: Open is disabled');

worlds.length = 0;
await refresh();
assert.strictEqual(document.getElementById('projects-list').children[0].textContent, 'No worlds yet. Build one with One World on.');

// Advanced features off hides the tab.
const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const css = fs.readFileSync(new URL('../../src/gui/css/settings.css', import.meta.url), 'utf8');
assert.match(html, /class="settings-nav-item meld-only" data-target="settings-section-projects"/);
assert.match(css, /body\.meld-off \.meld-only\s*\{\s*display: none !important/);
console.log('projects ok');
