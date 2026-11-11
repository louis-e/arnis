// Run: node tests/gui/projects-v2.mjs
// Projects v2: List / Grid, thumbnails, a world without any of the new fields.
import assert from 'node:assert';

const node = () => ({ children: [], listeners: {}, attrs: {}, className: '', textContent: '', setAttribute(k, v) { this.attrs[k] = v; }, addEventListener(e, f) { this.listeners[e] = f; }, append(...c) { this.children.push(...c); }, replaceChildren(...c) { this.children = c; } });
const nodes = new Map();
globalThis.document = { createElement: node, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
const store = new Map();
globalThis.localStorage = { setItem: (k, v) => store.set(k, v), getItem: (k) => store.get(k) ?? null };
globalThis.window = {};
const { initProjects, placeZoom, placeForRun } = await import('../../src/gui/js/meld-features.js');

const calls = [];
const worlds = [
  { name: 'Old', area_count: 1, scale: 1, locked: false, modified_ms: 1e12 },
  { name: 'Paris', area_count: 2, scale: 1, locked: false, modified_ms: 1e12, world_path: '/s/Paris', place_name: 'Paris', preview: true },
  { name: 'Bare', area_count: 1, scale: 1, locked: false, modified_ms: 1e12, world_path: '/s/Bare', preview: false },
];
const invoke = async (cmd, args) => {
  calls.push([cmd, args]);
  if (cmd === 'gui_list_one_worlds') return worlds;
  if (cmd === 'gui_world_thumbnail') return args.worldPath === '/s/Paris' ? 'data:image/png;base64,AA' : null;
};
const make = () => initProjects({ invoke, savePath: () => '/s', scale: () => 1, open: async () => {} });
const list = document.getElementById('projects-list');

// List is the default; the Grid button switches and remembers.
let refresh = make();
await refresh();
assert.strictEqual(list.className, 'projects-list');
await document.getElementById('projects-view-grid').listeners.click();
assert.strictEqual(list.className, 'projects-list is-grid');
assert.strictEqual(store.get('arnis-projects-view'), 'grid');
assert.strictEqual(document.getElementById('projects-view-grid').attrs['aria-pressed'], 'true');

// A new session starts in Grid.
make();
assert.strictEqual(list.className, 'projects-list is-grid', 'the view outlives the session');
await document.getElementById('projects-view-list').listeners.click();
assert.strictEqual(store.get('arnis-projects-view'), 'list');

// No IntersectionObserver here, so thumbnails load at once. Only a world that may have one is asked.
await new Promise((r) => setTimeout(r, 0));
const asked = calls.filter((c) => c[0] === 'gui_world_thumbnail').map((c) => c[1].worldPath);
assert.ok(asked.includes('/s/Paris') && !asked.includes('/s/Bare'), 'preview:false is never asked');
const rows = list.children;
assert.strictEqual(rows.length, 3, 'a world with no preview and no new field still renders');
assert.ok(rows[1].children[0].className === 'projects-thumb' && rows[1].children[0].children[0].src.startsWith('data:image/png'), 'a thumbnail fills its box');
assert.ok(rows[2].children[0].className.includes('is-empty'), 'no image leaves the placeholder');
assert.ok(rows[0].children[0].className.includes('is-empty'));
const title = (r) => r.children[1].children.map((c) => c.textContent).filter(Boolean);
assert.deepStrictEqual(title(rows[1]), ['Paris', 'Paris'], 'the place name sits beside the name');
assert.deepStrictEqual(title(rows[0]), ['Old'], 'an old world shows the name only');

// The geocode zoom follows the size of the area.
const box = (km) => [44, 26, 44 + km / 111, 26 + km / 111 / Math.cos((44 * Math.PI) / 180)];
assert.deepStrictEqual([3000, 600, 100, 30, 8, 1].map((km) => placeZoom(box(km))), [3, 5, 8, 10, 14, 16]);

// A search result names the build only when its point is inside the area.
const found = { name: 'Bucharest', lat: 44.43, lon: 26.1 };
assert.strictEqual(placeForRun(found, '44.40 26.00 44.50 26.20'), 'Bucharest');
assert.strictEqual(placeForRun(found, '45.00 26.00 45.50 26.20'), '');
assert.strictEqual(placeForRun(null, '44.40 26.00 44.50 26.20'), '');

// A world with place_name null is looked up once at its centre; a failure is not retried.
store.clear();
calls.length = 0;
const gone = [{ name: 'Old', area_count: 1, scale: 1, locked: false, modified_ms: 1, world_path: '/s/Old', place_name: null, bbox: [44, 26, 44.2, 26.2] }, { name: 'Done', area_count: 1, scale: 1, locked: false, modified_ms: 1, world_path: '/s/Done', place_name: 'Rome' }];
const asked2 = [];
const place = (geocode) => initProjects({
  invoke: async (cmd, args) => { calls.push([cmd, args]); return cmd === 'gui_list_one_worlds' ? gone : null; },
  savePath: () => '/s', scale: () => 1, open: async () => {}, geocode, pause: async () => {},
});
const settle = () => new Promise((r) => setTimeout(r, 5));
await place(async (lat, lon, zoom) => { asked2.push([lat, lon, zoom]); return 'Bucharest'; })();
await settle();
assert.strictEqual(asked2.length, 1, 'one lookup, for the world without a name');
assert.ok(Math.abs(asked2[0][0] - 44.1) < 1e-9 && Math.abs(asked2[0][1] - 26.1) < 1e-9 && asked2[0][2] === 10, 'at the centre, city zoom');
assert.ok(!calls.some((c) => c[0] === 'gui_get_one_world_overlays'), 'no overlay files are read');
assert.deepStrictEqual(calls.find((c) => c[0] === 'gui_set_place_name'), ['gui_set_place_name', { worldPath: '/s/Old', placeName: 'Bucharest' }]);
store.clear();
asked2.length = 0;
gone[0].place_name = null;
await place(async () => { asked2.push(1); throw new Error('offline'); })();
await settle();
await place(async () => { asked2.push(1); return 'x'; })();
await settle();
assert.strictEqual(asked2.length, 1, 'a failure is cached: the second open does not ask again');
// An engine without the field sends nothing to look up.
asked2.length = 0;
gone[0].place_name = undefined;
store.clear();
await place(async () => { asked2.push(1); return 'x'; })();
await settle();
assert.strictEqual(asked2.length, 0);

// Resume (unfinished job) and Rebuild (finished area, behind a second click).
store.clear();
const jobs = [
  { name: 'Open', area_count: 1, scale: 1, locked: false, modified_ms: 1, jobs: [{ name: 'j1', settings: ['--threads=2'], done: 1204, total: 61747, finished: false, last_run_ms: 5 }] },
  { name: 'Done', area_count: 1, scale: 1, locked: false, modified_ms: 1, jobs: [{ name: 'j2', settings: ['--seed=3'], done: 9, total: 9, finished: true, last_run_ms: 5 }] },
  { name: 'Plain', area_count: 1, scale: 1, locked: false, modified_ms: 1 },
];
const started = [];
const withJobs = initProjects({ invoke: async (c) => (c === 'gui_list_one_worlds' ? jobs : null), savePath: () => '/s', scale: () => 1, open: async () => {}, resume: async (w, j, r) => started.push([w.name, j.settings, r]) });
await withJobs();
const cards = list.children;
const label = (r) => r.children.map((c) => c.textContent);
assert.ok(label(cards[0]).includes('1,204 / 61,747 cells'), 'the count is formatted');
assert.ok(label(cards[0]).includes('Resume') && !label(cards[0]).includes('Rebuild'));
assert.ok(label(cards[1]).includes('Rebuild') && !label(cards[1]).includes('Resume') && !label(cards[1]).some((x) => x.includes('cells')));
assert.deepStrictEqual(label(cards[2]).filter((x) => /Resume|Rebuild/.test(x)), [], 'a plain world has neither');
const btn = (r, name) => r.children.find((c) => c.textContent === name || c.textContent.startsWith(name));
await btn(cards[0], 'Resume').listeners.click();
assert.deepStrictEqual(started.at(-1), ['Open', ['--threads=2'], false]);
assert.strictEqual(store.get('arnis-one-world-name'), 'Open', 'the world is picked first');
const rebuild = btn(cards[1], 'Rebuild');
await rebuild.listeners.click();
assert.strictEqual(started.length, 1, 'the first click only asks');
assert.strictEqual(rebuild.textContent, 'Rebuild? Click again');
await rebuild.listeners.click();
assert.deepStrictEqual(started.at(-1), ['Done', ['--seed=3'], true]);

// Inline rename: the pencil opens the field, Escape leaves it, Enter renames and refreshes.
store.clear();
const names = [{ name: 'Old', area_count: 1, scale: 1, locked: false, modified_ms: 1 }, { name: 'Busy', area_count: 1, scale: 1, locked: true, modified_ms: 1 }];
const renames = [];
const renamedTo = [];
let lists = 0;
const ren = initProjects({
  invoke: async (c, a) => {
    if (c === 'gui_list_one_worlds') { lists++; return names; }
    if (c === 'gui_rename_world') { renames.push(a); if (a.newName === 'Bad/') throw 'A name cannot hold / characters.'; names[0].name = a.newName; return a.newName; }
  },
  savePath: () => '/s', scale: () => 1, open: async () => {}, renamed: (a, b) => renamedTo.push([a, b]),
});
await ren();
store.set('arnis-one-world-name', 'Old');
const head = () => list.children[0].children[1];
assert.strictEqual(list.children[1].children[1].children[1].disabled, true, 'a world in use cannot be renamed');
head().children[1].listeners.click();
let field = head().children[0];
assert.strictEqual(field.value, 'Old', 'the field starts on the name');
field.listeners.keydown({ key: 'Escape' });
assert.deepStrictEqual(head().children.map((c) => c.textContent).filter(Boolean), ['Old'], 'Escape puts the name back');
assert.strictEqual(renames.length, 0);
head().children[0].listeners.click(); // clicking the name opens it too
field = head().children[0];
field.value = 'Bad/';
field.listeners.keydown({ key: 'Enter' });
await settle();
assert.strictEqual(head().children[1].textContent, 'A name cannot hold / characters.', 'the refusal is shown');
field.value = 'New';
field.listeners.keydown({ key: 'Enter' });
await settle();
assert.deepStrictEqual(renames.at(-1), { savePath: '/s', oldName: 'Old', newName: 'New' });
assert.deepStrictEqual(renamedTo, [['Old', 'New']]);
assert.strictEqual(store.get('arnis-one-world-name'), 'New', 'the picked world follows its new name');
assert.strictEqual(lists, 2, 'the list is read again');
assert.strictEqual(list.children[0].children[1].children[0].textContent, 'New');
console.log('projects-v2 ok');
