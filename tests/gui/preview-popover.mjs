// Run: node tests/gui/preview-popover.mjs
// The setting preview popover, with a stub document (no browser): which picture each setting and
// choice button maps to, hover/focus/Escape, a missing file, and that every
// picture the page names exists and every locale has its caption.
import assert from 'node:assert';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const read = (f) => fs.readFileSync(root + f, 'utf8');

const node = (extra = {}) => ({
  dataset: {}, listeners: {}, children: [], style: {}, textContent: '', attrs: {},
  classList: { s: new Set(), add(c) { this.s.add(c); }, remove(c) { this.s.delete(c); }, contains(c) { return this.s.has(c); } },
  addEventListener(t, f, cap) { (this.listeners[t] ||= []).push(f); },
  dispatchEvent(e) { fire(this, e.type); },
  removeAttribute(k) { delete this.attrs[k]; },
  setAttribute(k, v) { this.attrs[k] = v; }, appendChild(c) { this.children.push(c); },
  getBoundingClientRect() { return this.rect || { left: 0, right: 0, top: 0, bottom: 0, width: 362, height: 300 }; },
  ...extra,
});
function fire(n, t, e = {}) { (n.listeners[t] || []).forEach((f) => f(e)); }

// A switch row (left half of a 1000 px window) and a segmented row (right half).
const sw = node({ checked: false });
const swRow = node({ rect: { left: 40, right: 480, top: 300, bottom: 360 }, querySelector: (s) => (s === 'input.switch' ? sw : null) });
const swIcon = node({ dataset: { previewOn: 'rocks-on', previewOff: 'rocks-off' }, closest: () => swRow });
const segs = ['max', 'clean', 'compact'].map((v, i) => node({ dataset: { v }, classList: node().classList }));
segs[0].classList.add('active');
const segRow = node({ rect: { left: 520, right: 960, top: 100, bottom: 160 }, querySelector: () => segs[0], querySelectorAll: () => segs });
const segIcon = node({ dataset: { previewSeg: 'road-detail' }, closest: () => segRow });
const body = node();
const doc = node({
  body, createElement: () => node(),
  querySelectorAll: () => [swIcon, segIcon],
  querySelector: () => null,
});
globalThis.document = doc;
globalThis.window = { innerWidth: 1000, innerHeight: 650, localization: { preview_rocks_on: 'Rocks: on', preview_road_detail_clean: 'Road Detail: Clean' }, addEventListener() {} };
const { previewName, captionKey, initPreviews, placePopover } = await import('../../src/gui/js/preview-popover.js');

// names
assert.equal(previewName(swIcon, null), 'rocks-off');
sw.checked = true;
assert.equal(previewName(swIcon, null), 'rocks-on');
assert.equal(previewName(segIcon, null), 'road-detail-max', 'the active button');
assert.equal(previewName(segIcon, segs[1]), 'road-detail-clean', 'the hovered button');
assert.equal(captionKey('post-apocalyptic-on'), 'preview_post_apocalyptic_on');

// placement: beside the control, inside the window, never over it; RTL mirrors the side
const hit = (a, b) => a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom;
for (const [vw, vh] of [[1000, 650], [1920, 1080]]) {
  const col = vw === 1000 ? { left: 250, right: 1000 } : { left: 595, right: 1575 };
  for (const rtl of [false, true]) {
    for (const ctrl of [{ left: 880, right: 920, top: 300, bottom: 324 }, { left: 300, right: 340, top: 20, bottom: 44 }, { left: 700, right: 740, top: 620, bottom: 644 }]) {
      const at = placePopover({ obstacle: ctrl, panel: col, vw, vh, w: 362, h: 300, rtl });
      const r = { left: at.left, right: at.left + 362, top: at.top, bottom: at.top + 300 };
      assert.ok(!hit(r, ctrl), `${vw} ${rtl} ${JSON.stringify(ctrl)}: covers the control`);
      assert.ok(r.left >= 0 && r.top >= 0 && r.right <= vw && r.bottom <= vh, `${vw} ${rtl}: outside the window`);
    }
  }
}
const c1 = { left: 880, right: 920, top: 300, bottom: 324 };
const ltr = placePopover({ obstacle: c1, panel: { left: 600, right: 1320 }, vw: 1920, vh: 1080, w: 362, h: 300, rtl: false });
const rtl = placePopover({ obstacle: c1, panel: { left: 600, right: 1320 }, vw: 1920, vh: 1080, w: 362, h: 300, rtl: true });
assert.ok(ltr.left < 600 && rtl.left > 1320, 'LTR takes the left of the panel, RTL the right');

// No free side (1000 px: the sidebar, then rows whose text runs across): the popover shrinks into the
// free space (>= 220 wide) and covers no text, only the hovered control's neighbours' blank parts.
{
  const text = [];
  for (let y = 100; y < 640; y += 60) { text.push({ left: 271, right: 520, top: y, bottom: y + 14 }, { left: 271, right: 560, top: y + 18, bottom: y + 32 }, { left: 665, right: 900, top: y, bottom: y + 32 }); }
  const ctrl = { left: 549, right: 591, top: 350, bottom: 374 };
  for (const rtl of [false, true]) {
    const at = placePopover({ obstacle: ctrl, panel: { left: 232, right: 990 }, vw: 1000, vh: 650, w: 362, h: 300, rtl, avoid: text });
    const h = 300 - (362 - at.w) * 0.75 + 18;
    const r = { left: at.left, right: at.left + at.w, top: at.top, bottom: at.top + h };
    assert.ok(at.w >= 220 && at.w <= 362, `width ${at.w}`);
    assert.ok(!hit(r, ctrl) && !text.some((a) => hit(r, a)), 'covers a control or text');
    assert.ok(r.left >= 8 && r.right <= 992 && r.top >= 8 && r.bottom <= 642, 'inside the window');
  }
  const roomy = placePopover({ obstacle: { left: 880, right: 920, top: 300, bottom: 324 }, panel: { left: 600, right: 1320 }, vw: 1920, vh: 1080, w: 362, h: 300, rtl: false, avoid: [{ left: 650, right: 900, top: 290, bottom: 330 }] });
  assert.equal(roomy.w, 362, 'with room it keeps the full size');
  assert.ok(roomy.fit && !placePopover({ obstacle: ctrl, panel: { left: 232, right: 990 }, vw: 1000, vh: 650, w: 362, h: 300, avoid: [{ left: 0, right: 1000, top: 0, bottom: 650 }] }).fit, '`fit` says whether a clear spot was found');
}

// switch, select, slider and number box rows, and a row that carries the data itself
const sel = node({ value: 'x' });
const selRow = node({ querySelector: (q) => (q === 'select' ? sel : null) });
sel.value = 'eur';
assert.equal(previewName(node({ dataset: { previewSel: 'tree-realm' }, closest: () => selRow }), null), 'tree-realm-eur');
const rng = node({ value: '1.3' });
const rngRow = node({ querySelector: (q) => (q.includes('range') ? rng : null) });
const rngIcon = node({ dataset: { previewNum: 'height-multiplier', previewSteps: '0.5,1,2' }, closest: () => rngRow });
assert.equal(previewName(rngIcon, null), 'height-multiplier-1');
rng.value = '1.6';
assert.equal(previewName(rngIcon, null), 'height-multiplier-2');
rng.value = '-45';
assert.equal(previewName(node({ dataset: { previewNum: 'rotation', previewSteps: '0,45' }, closest: () => rngRow }), null), 'rotation-45', 'a negative angle is shown by its size');
const keyed = node({ dataset: { maxTreeSize: 'tall' } });
const keyedRow = node({ querySelector: () => keyed });
assert.equal(previewName(node({ dataset: { previewSeg: 'max-tree-size', previewKey: 'maxTreeSize' }, closest: () => keyedRow }), null), 'max-tree-size-tall');
keyed.dataset.nopreview = '';
assert.equal(previewName(node({ dataset: { previewSeg: 'max-tree-size', previewKey: 'maxTreeSize' }, closest: () => keyedRow }), null), null, 'a button without a picture');

// behaviour
initPreviews();
const el = body.children[0];
const img = el.children[0];
assert.equal(img.width * 3, img.height * 4, 'fixed 4:3 box, so nothing shifts when it loads');
assert.ok(!el.classList.contains('is-visible'));
fire(swIcon, 'mouseenter');
assert.ok(!el.classList.contains('is-visible'), 'not shown until the picture is in');
img.onload();
assert.ok(el.classList.contains('is-visible'));
assert.equal(img.alt, '', 'the caption is the text, not the alt');
assert.equal(swIcon.attrs['aria-describedby'], 'preview-popover');
assert.equal(img.src, 'images/previews3d/rocks-on.webp');
assert.equal(el.children[1].textContent, 'Rocks: on');
fire(swIcon, 'mouseleave');
assert.ok(!el.classList.contains('is-visible'));
assert.ok(!swIcon.attrs['aria-describedby']);
fire(swIcon, 'focus');
img.onload();
assert.ok(el.classList.contains('is-visible'), 'keyboard focus shows it');
const esc = doc.listeners.keydown[0];
let stopped = false;
esc({ key: 'a', stopPropagation() { stopped = true; } });
assert.ok(el.classList.contains('is-visible') && !stopped);
let tipClosed = 0;
swIcon.listeners.mouseleave.push(() => tipClosed++);
esc({ key: 'Escape', stopPropagation() { stopped = true; } });
assert.equal(tipClosed, 1, "Escape also closes the icon's own text tooltip");
assert.ok(!el.classList.contains('is-visible') && stopped, 'Escape closes it and is not passed on to the settings page');
stopped = false;
esc({ key: 'Escape', stopPropagation() { stopped = true; } });
assert.ok(!stopped, 'with nothing open Escape goes on to close the page');
fire(segs[1], 'mouseenter');
img.onload();
assert.equal(img.src, 'images/previews3d/road-detail-clean.webp');
fire(segs[1], 'mouseleave');
// a picture that arrives after the pointer left stays hidden; the old picture is cleared first
fire(swIcon, 'mouseenter');
const staleLoad = img.onload;
assert.equal(img.src, 'images/previews3d/rocks-on.webp');
fire(swIcon, 'mouseleave');
staleLoad();
assert.ok(!el.classList.contains('is-visible'), 'a late load does not show it');

// switching the language replaces window.localization; the next hover reads the new text
window.localization = { preview_rocks_on: 'Pietre: activat' };
fire(swIcon, 'mouseenter');
assert.equal(el.children[1].textContent, 'Pietre: activat');
fire(swIcon, 'mouseleave');

fire(swIcon, 'mouseenter');
img.onload();
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
assert.ok(names.size >= 55, `${names.size} pictures named`);
const locales = fs.readdirSync(root + 'src/gui/locales').filter((f) => f.endsWith('.json') && fs.statSync(`${root}src/gui/locales/${f}`).size > 100);
assert.ok(locales.length >= 20);
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
