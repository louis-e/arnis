// Run: node tests/gui/preview-popover.mjs
// The setting preview, with a stub document (no browser): which picture each setting and choice button
// maps to, that the whole tile and keyboard focus show it, that a switch gets both its pictures, a
// missing file, and that every picture the page names exists and every locale has its caption.
import assert from 'node:assert';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const read = (f) => fs.readFileSync(root + f, 'utf8');

const node = (extra = {}) => ({
  dataset: {}, listeners: {}, children: [], style: {}, textContent: '', attrs: {},
  classList: { s: new Set(), add(c) { this.s.add(c); }, remove(c) { this.s.delete(c); }, contains(c) { return this.s.has(c); } },
  addEventListener(t, f) { (this.listeners[t] ||= []).push(f); },
  removeAttribute(k) { delete this.attrs[k]; },
  setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.children.push(c); },
  querySelectorAll: () => [],
  getBoundingClientRect: () => ({ left: 0, right: 0, top: 0, bottom: 0 }),
  ...extra,
});
function fire(n, t, e = {}) { (n.listeners[t] || []).forEach((f) => f(e)); }

// A switch row and a segmented row.
const sw = node({ checked: false });
const swRow = node({ querySelector: (s) => (s === 'input.switch' ? sw : null), querySelectorAll: (s) => (s.includes('tooltip') ? [sw] : [sw]) });
const swIcon = node({ dataset: { previewOn: 'rocks-on', previewOff: 'rocks-off' }, closest: () => swRow });
const segs = ['max', 'clean', 'compact'].map((v) => { const n = node({ dataset: { v } }); n.classList.add('segment'); return n; });
segs[0].classList.add('active');
const segRow = node({ querySelector: () => segs[0], querySelectorAll: (s) => (s === 'input, select' ? [] : segs) });
const segIcon = node({ dataset: { previewSeg: 'road-detail' }, closest: () => segRow });
const body = node();
const doc = node({ body, createElement: () => node(), querySelectorAll: () => [swIcon, segIcon], querySelector: () => null, getElementById: () => null });
globalThis.document = doc;
globalThis.window = { localization: { preview_rocks_on: 'Rocks: on', preview_rocks_off: 'Rocks: off', preview_road_detail_clean: 'Road Detail: Clean' }, addEventListener() {} };
const { previewName, previewFrames, captionKey, initPreviews } = await import('../../src/gui/js/preview-popover.js');

// names
assert.equal(previewName(swIcon, null), 'rocks-off');
assert.deepEqual(previewFrames(swIcon, null).map((f) => f.name), ['rocks-off', 'rocks-on'], 'a switch shows both states, its own first');
sw.checked = true;
assert.equal(previewName(swIcon, null), 'rocks-on');
assert.deepEqual(previewFrames(swIcon, null).map((f) => f.name), ['rocks-on', 'rocks-off']);
assert.equal(previewName(segIcon, null), 'road-detail-max', 'the active button');
assert.equal(previewName(segIcon, segs[1]), 'road-detail-clean', 'the hovered button');
assert.equal(previewFrames(segIcon, segs[1]).length, 1);
assert.equal(captionKey('post-apocalyptic-on'), 'preview_post_apocalyptic_on');
// a picture borrowed from another setting brings its own caption; a switch with one picture shows it in both states
const shared = node({ dataset: { previewOn: 'winter-on', previewOff: 'buildings-on', previewOffCaption: 'preview_winter_off' }, closest: () => node({ querySelector: () => node({ checked: false }) }) });
assert.deepEqual(previewFrames(shared, null), [{ name: 'buildings-on', caption: 'preview_winter_off' }, { name: 'winter-on', caption: 'preview_winter_on' }]);
assert.deepEqual(previewFrames(node({ dataset: { previewOn: 'x-on' }, closest: () => swRow }), null).map((f) => f.name), ['x-on']);

// select, slider and number box rows, and a keyed button group
const sel = node({ value: 'eur' });
const selRow = node({ querySelector: (q) => (q === 'select' ? sel : null) });
assert.equal(previewName(node({ dataset: { previewSel: 'tree-realm' }, closest: () => selRow }), null), 'tree-realm-eur');
const rng = node({ value: '1.3' });
const rngRow = node({ querySelector: (q) => (q.includes('range') ? rng : null) });
const rngIcon = node({ dataset: { previewNum: 'height-multiplier', previewSteps: '0.5,1,2' }, closest: () => rngRow });
assert.equal(previewName(rngIcon, null), 'height-multiplier-1');
rng.value = '1.6';
assert.equal(previewName(rngIcon, null), 'height-multiplier-2');
const keyed = node({ dataset: { maxTreeSize: 'tall' } });
const keyedRow = node({ querySelector: () => keyed });
assert.equal(previewName(node({ dataset: { previewSeg: 'max-tree-size', previewKey: 'maxTreeSize' }, closest: () => keyedRow }), null), 'max-tree-size-tall');
keyed.dataset.nopreview = '';
assert.equal(previewName(node({ dataset: { previewSeg: 'max-tree-size', previewKey: 'maxTreeSize' }, closest: () => keyedRow }), null), null, 'a button without a picture');

// behaviour: the whole tile shows it
initPreviews();
const el = body.children[0];
const img = el.children[0];
assert.equal(img.width * 3, img.height * 4, 'fixed 4:3 box, so nothing shifts when it loads');
fire(swRow, 'mouseenter');
assert.ok(!el.classList.contains('is-visible'), 'not shown until the picture is in');
img.onload();
assert.ok(el.classList.contains('is-visible'));
assert.equal(img.alt, '', 'the caption is the text, not the alt');
assert.equal(swIcon.attrs['aria-describedby'], 'preview-popover');
assert.equal(img.src, 'images/previews3d/rocks-on.webp', 'the state the switch is in comes first');
assert.equal(el.children[1].textContent, 'Rocks: on');
fire(swRow, 'mouseleave');
assert.ok(!el.classList.contains('is-visible'));
assert.ok(!swIcon.attrs['aria-describedby']);
fire(sw, 'focus');
img.onload();
assert.ok(el.classList.contains('is-visible'), 'keyboard focus shows it');
fire(sw, 'blur');
assert.ok(!el.classList.contains('is-visible'));
// a choice button shows its own picture, and leaving it goes back to the chosen one
fire(segRow, 'mouseenter');
fire(segs[1], 'mouseenter');
img.onload();
assert.equal(img.src, 'images/previews3d/road-detail-clean.webp');
fire(segs[1], 'mouseleave');
assert.equal(img.src, 'images/previews3d/road-detail-max.webp');
fire(segRow, 'mouseleave');
assert.ok(!el.classList.contains('is-visible'));
// a picture that arrives after the pointer left stays hidden
fire(swRow, 'mouseenter');
const staleLoad = img.onload;
fire(swRow, 'mouseleave');
staleLoad();
assert.ok(!el.classList.contains('is-visible'), 'a late load does not show it');
// switching the language replaces window.localization; the next hover reads the new text
window.localization = { preview_rocks_on: 'Pietre: activat' };
fire(swRow, 'mouseenter');
img.onload();
assert.equal(el.children[1].textContent, 'Pietre: activat');
img.onerror();
assert.ok(!el.classList.contains('is-visible'), 'a missing picture hides it');

// every picture the page names exists, with a caption in every locale
const html = read('src/gui/index.html');
const names = new Set();
for (const m of html.matchAll(/data-preview-(?:on|off)="([^"]+)"/g)) names.add(m[1]);
const camel = (k) => k.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());
for (const m of html.matchAll(/data-preview-seg="([^"]+)"(?: data-preview-key="([^"]+)")?(?: data-preview-group="([^"]+)")?/g)) {
  const [, stem, key, group] = m;
  const g = new RegExp(`id="${group || `meld-${stem}-group`}"[^>]*>([^]*?)</div>`).exec(html);
  assert.ok(g, `no button group for ${stem}`);
  for (const v of g[1].matchAll(new RegExp(`data-${key ? camel(key) : 'v'}="([^"]+)"`, 'g'))) names.add(`${stem}-${v[1]}`);
}
for (const m of html.matchAll(/data-preview-sel="([^"]+)"/g)) {
  const g = new RegExp(`id="meld-${m[1]}-select"[^>]*>([^]*?)</select>`).exec(html);
  for (const v of g[1].matchAll(/<option value="([^"]+)"/g)) names.add(`${m[1]}-${v[1]}`);
}
for (const m of html.matchAll(/data-preview-num="([^"]+)" data-preview-steps="([^"]+)"/g)) for (const v of m[2].split(',')) names.add(`${m[1]}-${v}`);
assert.ok(names.size >= 50, `${names.size} pictures named`);
const captions = [...html.matchAll(/data-preview-(?:on|off)-caption="([^"]+)"/g)].map((m) => m[1]);
assert.ok(captions.length >= 2);
const locales = fs.readdirSync(root + 'src/gui/locales').filter((f) => f.endsWith('.json') && fs.statSync(`${root}src/gui/locales/${f}`).size > 100);
assert.ok(locales.length >= 20);
for (const k of captions) for (const l of locales) assert.ok(JSON.parse(read(`src/gui/locales/${l}`))[k], `${l} lacks ${k}`);
for (const n of names) {
  const f = fs.statSync(`${root}src/gui/images/previews3d/${n}.webp`);
  assert.ok(f.size > 1000 && f.size <= 60 * 1024, `${n}.webp is ${f.size} bytes`);
  for (const l of locales) assert.ok(JSON.parse(read(`src/gui/locales/${l}`))[captionKey(n)], `${l} lacks ${captionKey(n)}`);
}
// Romanian: every caption present, translated (no English words left) and with its diacritics
const ro = JSON.parse(read('src/gui/locales/ro.json'));
const en = JSON.parse(read('src/gui/locales/en-US.json'));
for (const n of names) {
  const k = captionKey(n);
  assert.ok(typeof ro[k] === 'string' && ro[k].trim(), `ro.json lacks ${k}`);
  assert.notEqual(ro[k], en[k], `${k} is not translated in ro.json`);
  const words = (t) => t.toLowerCase().match(/\p{L}+/gu) || [];
  const english = new Set(words(en[k]));
  const left = words(ro[k]).filter((w) => english.has(w) && !['compact', 'post', 'africa', 'florida', 'vanilla', 'blueprint', 'auto', 'prairie', 'asia', 'america', 'india', 'australasia'].includes(w)); // same word in both languages
  assert.deepEqual(left, [], `English left in ro ${k}: ${ro[k]}`);
  assert.ok(!/\d\.\d/.test(ro[k]), `${k}: Romanian writes decimals with a comma`);
}
assert.ok(ro.preview_caves_on.includes('Peșteri') && ro.preview_river_bed_off.includes('râurilor'));
console.log(`preview-popover: ok (${names.size} pictures, ${locales.length} locales)`);
