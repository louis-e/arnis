// Run: node tests/gui/meld-flags.mjs
// The Meld features switch off, and on with every control at its default, send no flags. The defaults are read from index.html itself.
import assert from 'node:assert';
import fs from 'node:fs';
import { meldFlags, generationTarget } from '../../src/gui/js/meld-features.js';

const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const state = new Map(); // id -> element, so a test can change one

function fromHtml(id) {
  const tag = html.match(new RegExp(`<(input|select)[^>]* id="${id}"[^>]*>`));
  if (!tag) return null;
  const attr = (name) => (tag[0].match(new RegExp(` ${name}="([^"]*)"`)) || [])[1];
  let value = attr('value') ?? '';
  if (tag[1] === 'select') {
    const block = html.slice(tag.index, html.indexOf('</select>', tag.index));
    const chosen = block.match(/<option value="([^"]*)" selected/) || block.match(/<option value="([^"]*)"/);
    value = chosen ? chosen[1] : '';
  }
  return { value, checked: /\bchecked\b/.test(tag[0]), closest: () => null };
}

globalThis.window = {};
globalThis.document = {
  getElementById(id) {
    if (!state.has(id)) state.set(id, fromHtml(id));
    return state.get(id);
  },
  querySelector(selector) {
    const id = selector.match(/^#([\w-]+) \.segment\.active$/)[1];
    const start = html.indexOf(`id="${id}"`);
    const block = html.slice(start, html.indexOf('</div>', start));
    const active = block.match(/class="segment active"[^>]*data-v="([^"]*)"/);
    return active ? { dataset: { v: active[1] } } : null;
  },
};

const contexts = [
  { oneWorld: false, java: true, caves: false, extendHeight: false, lods: false },
  { oneWorld: true, java: true, caves: true, extendHeight: true, lods: true },
  { oneWorld: false, java: false, caves: true, extendHeight: false, lods: false },
];
const toggle = document.getElementById('advanced-meld-features-toggle');
for (const ctx of contexts) {
  toggle.checked = false;
  assert.deepStrictEqual(meldFlags(ctx), [], 'switch off');
  toggle.checked = true;
  assert.deepStrictEqual(meldFlags(ctx), [], 'switch on, defaults');
}

const ctx = contexts[1];
document.getElementById('meld-seed-input').value = '7';
assert.deepStrictEqual(meldFlags(ctx), ['--seed=7']);
// A job in pieces: its defaults send nothing new; each control is its own flag.
document.getElementById('meld-workers-select').value = '2';
assert.deepStrictEqual(meldFlags(ctx), ['--one-world-workers=2', '--seed=7']);
document.getElementById('meld-margin-input').value = '2';
document.getElementById('meld-piece-order-select').value = 'west';
document.getElementById('meld-skip-built-toggle').checked = true;
document.getElementById('meld-stitch-map-toggle').checked = true;
assert.deepStrictEqual(meldFlags(ctx), ['--one-world-workers=2', '--margin-regions=2', '--piece-order=west', '--skip-built', '--stitch-map', '--seed=7']);
// Not without One World.
assert.ok(!meldFlags({ ...ctx, oneWorld: false }).some((f) => f.includes('margin')));
// A prewarm has no world: never the last built one, never a new-world run (the backend deletes those at the end).
assert.deepStrictEqual(generationTarget({ prewarm: true, oneWorld: false, savePath: 's', worldPath: 'w' }), { selectedWorld: '', isNewWorld: false });
assert.deepStrictEqual(generationTarget({ prewarm: false, oneWorld: false, savePath: 's', worldPath: 'w' }), { selectedWorld: 'w', isNewWorld: true });
assert.deepStrictEqual(generationTarget({ prewarm: false, oneWorld: true, savePath: 's', worldPath: 'w' }), { selectedWorld: 's', isNewWorld: true });
// A prewarm with One World on still names the save folder (the backend resolves the world from it), never a new world.
assert.deepStrictEqual(generationTarget({ prewarm: true, oneWorld: true, savePath: 's', worldPath: 'w' }), { selectedWorld: 's', isNewWorld: false });
// Download now with Offline Mode on: the CLI refuses --offline beside --prewarm, and the download is the point.
document.getElementById('meld-offline-toggle').checked = true;
assert.ok(meldFlags(ctx).includes('--offline'), 'a run stays offline');
assert.ok(!meldFlags({ ...ctx, prewarm: true }).includes('--offline'), 'a prewarm does not');
console.log('meld-flags: ok');
