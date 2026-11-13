// Run: node tests/gui/projects.mjs
// Settings > Projects: one row per world, Open stores the pick, the tab hides with Advanced features off.
import assert from 'node:assert';
import fs from 'node:fs';

const node = () => ({ children: [], listeners: {}, attrs: {}, className: '', classList: { add() {}, remove() {} }, closest: () => null, getAttribute(k) { return this.attrs[k] ?? null; }, setAttribute(k, v) { this.attrs[k] = v; }, removeAttribute(k) { delete this.attrs[k]; }, addEventListener(e, f) { this.listeners[e] = f; }, append(...c) { c.forEach((k) => { k.parent = this; }); this.children.push(...c); }, remove() { this.parent.children = this.parent.children.filter((k) => k !== this); this.detaches = (this.detaches || 0) + 1; }, replaceChildren(...c) { this.children.forEach((k) => { k.detaches = (k.detaches || 0) + 1; }); c.forEach((k) => { k.parent = this; }); this.children = c; }, get firstElementChild() { return this.children[0] || null; }, get nextElementSibling() { const sib = this.parent?.children || []; return sib[sib.indexOf(this) + 1] || null; }, after(...c) { const sib = this.parent.children; sib.splice(sib.indexOf(this) + 1, 0, ...c); c.forEach((k) => { k.parent = this.parent; }); }, querySelector(sel) { return this.children.find((c) => sel === `.${c.className}`) || null; } });
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
let running = false;
const during = initProjects({ invoke: async () => worlds, savePath: () => 's', scale: () => 1, open: async () => {}, busy: () => running });
running = true;
await during();
assert.ok(btn(document.getElementById('projects-list').children[0], 'Delete').disabled, 'no Delete while a run holds the worlds');
running = false;
await refresh();

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
assert.ok(paris().children.some((c) => c.className === 'projects-error' && c.textContent === refuse && c.attrs.role === 'alert'), 'the reason shows and is read out');
assert.ok(!btn(paris(), 'Delete').disabled, 'and it can be tried again');
refuse = null;
await press();
await press();
assert.deepStrictEqual(gone, ['Paris'], 'moved: the window forgets it');
assert.strictEqual(asked.length, 2);

worlds.length = 0;
await refresh();
assert.strictEqual(document.getElementById('projects-list').children[0].textContent, 'No worlds yet. Build one with One World on.');

// New One World: the button opens a name card at the head of the list; the engine checks the name.
const made = [];
const checked = [];
let taken = null;
let busyNow = false;
let avail = true;
let save = 's';
const fresh = initProjects({
  invoke: async (cmd, args) => {
    if (cmd !== 'gui_new_one_world_name') return worlds;
    checked.push(args);
    if (taken) throw taken;
    return args.name.trim();
  },
  savePath: () => save, scale: () => 1, open: async () => {}, available: () => avail, busy: () => busyNow,
  created: async (n) => made.push(n),
});
await fresh();
const newBtn = document.getElementById('projects-new');
const listEl = document.getElementById('projects-list');
const isCard = (c) => c?.className === 'projects-row projects-new-row';
const settleNew = () => new Promise((r) => setTimeout(r, 0));
assert.ok(!newBtn.disabled);
await newBtn.listeners.click();
const card = listEl.children[0];
assert.ok(isCard(card) && listEl.children.length === 2, 'the card heads the list');
const input = card.children[0];
assert.strictEqual(input.maxLength, 32, "Minecraft's own name limit");
const [create, cancel] = card.children[1].children;
assert.deepStrictEqual([create.textContent, cancel.textContent], ['Create', 'Cancel']);
await create.listeners.click();
assert.strictEqual(checked.length, 0, 'no name, nothing asked');
taken = 'A world named Rome already exists.';
input.value = 'Rome';
input.listeners.keydown({ key: 'Enter' });
await settleNew();
assert.strictEqual(card.children[2].textContent, taken, 'a refusal shows in the card');
assert.strictEqual(card.children[2].attrs.role, 'alert');
assert.strictEqual(input.attrs['aria-describedby'], card.children[2].id, 'the field names its error');
await fresh();
assert.strictEqual(listEl.children[0], card, 'a refresh keeps the card and what was typed');
assert.ok(!card.detaches, 'without taking it out, so it keeps its focus');
assert.strictEqual(listEl.children.length, 2, 'and the rows under it are replaced, not added');
taken = null;
input.value = '  Tokyo ';
await create.listeners.click();
await settleNew();
assert.deepStrictEqual(checked.at(-1), { savePath: 's', name: 'Tokyo' });
assert.deepStrictEqual(made, ['Tokyo'], 'the window takes the new world');
assert.strictEqual(store.get('arnis-one-world-name'), 'Tokyo');
assert.strictEqual(store.get('arnis-one-world-picked'), 'new');
assert.ok(!isCard(listEl.children[0]), 'and the card closes');
await newBtn.listeners.click();
listEl.children[0].children[0].listeners.keydown({ key: 'Escape' });
assert.ok(!isCard(listEl.children[0]), 'Escape closes it');
await newBtn.listeners.click();
listEl.children[0].children[1].children[1].listeners.click();
assert.ok(!isCard(listEl.children[0]), 'so does Cancel');
assert.strictEqual(made.length, 1, 'neither makes a world');
await newBtn.listeners.click();
busyNow = true;
await fresh();
assert.ok(newBtn.disabled && !isCard(listEl.children[0]), 'a run closes the card and holds the button');
await newBtn.listeners.click();
assert.ok(!isCard(listEl.children[0]), 'a held button opens nothing');
busyNow = false;
avail = false;
await fresh();
assert.ok(newBtn.disabled, 'not without One World');
avail = true;
save = '';
await fresh();
assert.ok(newBtn.disabled, 'not without a saves folder');
save = 's';
await fresh();
assert.ok(!newBtn.disabled);

// Advanced features off hides the tab.
const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const css = fs.readFileSync(new URL('../../src/gui/css/settings.css', import.meta.url), 'utf8');
assert.match(html, /class="settings-nav-item meld-only" data-target="settings-section-projects"/);
assert.match(html, /id="projects-new"[^>]*><svg class="icon" aria-hidden="true"><use href="#i-plus"><\/use><\/svg><span data-localize="projects_new">/);
assert.match(html, /<symbol id="i-plus"/);
assert.match(css, /body\.meld-off \.meld-only\s*\{\s*display: none !important/);
console.log('projects ok');
