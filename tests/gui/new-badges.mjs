// Run: node tests/gui/new-badges.mjs
// "New!" badges: three starts each, gone once the feature is used, sidebar marks, the heading row class.
import assert from 'node:assert';
import fs from 'node:fs';
import { initNewBadges } from '../../src/gui/js/new-badges.js';

const classes = () => {
  const set = new Set();
  return { set, add: (c) => set.add(c), remove: (c) => set.delete(c), has: (c) => set.has(c) };
};

// Two badges: Pedestrians in a setting heading of section "world", Live in a segment of section "general".
function page(state) {
  const listeners = new Map();
  const rows = { pedestrians: { classList: classes() }, live: { classList: classes() } };
  const badges = [];
  const badge = (key, when, section, heading) => {
    const b = {
      dataset: { new: key, newWhen: when },
      section,
      gone: false,
      cloneNode() { return { dataset: { ...this.dataset }, cloneNode: this.cloneNode, remove() { this.gone = true; } }; },
      closest: (sel) => (sel === '.setting-heading' ? (heading ? { closest: () => rows[key] } : null) : null),
      remove() { this.gone = true; },
    };
    badges.push(b);
    return b;
  };
  badge('pedestrians', '#pedestrians-toggle:checked', 'world', true);
  badge('live', '#live-on', 'general', false);
  const nav = ['world', 'general', 'other'].map((target) => {
    const kids = [];
    return {
      dataset: { target },
      kids,
      querySelector: (sel) => (sel === '.new-badge' ? kids.find((k) => !k.gone) || null : null),
      insertBefore(b) { kids.push(b); },
    };
  });
  const live = () => badges.filter((b) => !b.gone);
  const doc = {
    querySelectorAll(sel) {
      if (sel === '.new-badge[data-new]') return live();
      if (sel === '.settings-nav-item') return nav;
      const m = sel.match(/data-new="([^"]+)"/);
      return m ? live().filter((b) => b.dataset.new === m[1]) : [];
    },
    querySelector: (sel) => (state[sel] ? {} : null),
    getElementById: (id) => ({ querySelector: () => live().find((b) => b.section === id) || null }),
    addEventListener: (ev, f) => listeners.set(ev, f),
    removeEventListener: (ev, f) => { if (listeners.get(ev) === f) listeners.delete(ev); },
  };
  const marks = () => nav.map((n) => n.kids.filter((k) => !k.gone).length);
  return { doc, badges, rows, nav, listeners, marks };
}

const store = new Map();
const local = { getItem: (k) => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) };
const tick = () => new Promise((r) => setTimeout(r, 5));

// First start: both badges up, the heading row marked, both sections marked once.
const state = {};
let p = page(state);
initNewBadges(p.doc, local);
assert.deepStrictEqual(p.badges.map((b) => b.gone), [false, false]);
assert.ok(p.rows.pedestrians.classList.has('has-new-badge'), 'the heading row hides its unused revert button');
assert.ok(!p.rows.live.classList.has('has-new-badge'), 'a badge outside a heading marks no row');
assert.deepStrictEqual(p.marks(), [1, 1, 0]);
assert.ok(!p.nav[0].kids[0].dataset.new, 'a sidebar mark is not a badge of its own');
assert.strictEqual(JSON.parse(store.get('arnis-new-pedestrians')).starts, 1);

// Using a feature drops its badge, its row class and its section mark at once, and is kept.
state['#pedestrians-toggle:checked'] = true;
p.listeners.get('change')();
await tick();
assert.ok(p.badges[0].gone && !p.badges[1].gone);
assert.ok(!p.rows.pedestrians.classList.has('has-new-badge'));
assert.deepStrictEqual(p.marks(), [0, 1, 0]);
assert.strictEqual(JSON.parse(store.get('arnis-new-pedestrians')).used, true);
state['#pedestrians-toggle:checked'] = false;

// Once every badge is gone nothing listens any more.
state['#live-on'] = true;
p.listeners.get('click')();
await tick();
assert.strictEqual(p.listeners.size, 0, 'the listeners go with the last badge');
state['#live-on'] = false;

// A used feature never shows again; an untried one shows on starts 2 and 3 and not on the 4th.
store.delete('arnis-new-live');
for (const start of [1, 2, 3]) {
  p = page(state);
  initNewBadges(p.doc, local);
  assert.ok(p.badges[0].gone, `start ${start}: used stays gone`);
  assert.ok(!p.badges[1].gone, `start ${start}: untried still shows`);
}
p = page(state);
initNewBadges(p.doc, local);
assert.ok(p.badges[1].gone && p.listeners.size === 0, 'three starts, then gone');
assert.deepStrictEqual(p.marks(), [0, 0, 0]);

// Already in use at start (restored setting): counted as tried.
store.clear();
p = page({ '#pedestrians-toggle:checked': true });
initNewBadges(p.doc, local);
assert.ok(p.badges[0].gone && JSON.parse(store.get('arnis-new-pedestrians')).used);

// Broken storage: shown, not thrown.
p = page({});
initNewBadges(p.doc, { getItem: () => '{oops', setItem: () => { throw new Error('full'); } });
assert.ok(!p.badges[0].gone);

// The markup and the styles the module relies on.
const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const css = fs.readFileSync(new URL('../../src/gui/css/settings.css', import.meta.url), 'utf8');
const store_js = fs.readFileSync(new URL('../../src/gui/js/settings-store.js', import.meta.url), 'utf8');
for (const key of ['live', 'pedestrians', 'advanced']) assert.match(html, new RegExp(`class="new-badge"[^>]*data-new="${key}" data-new-when="`));
assert.match(css, /\.settings-row\.has-new-badge:not\(\.is-modified\) \.setting-heading > \.setting-revert \{\s*display: none;/);
assert.match(store_js, /slot\.insertBefore\(button, slot\.querySelector\(':scope > \.new-badge'\)\)/, 'the revert button goes before a badge');
console.log('new-badges ok');
