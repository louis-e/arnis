// Run: node tests/gui/search-result.mjs
// A search result carries its place as data-lat/data-lon numbers (a click flies there).
import assert from 'node:assert';
import fs from 'node:fs';
import vm from 'node:vm';

let html = '';
const chain = new Proxy(function () {}, {
  get: (_, k) => (k === 'html' ? (h) => ((html = h), chain) : () => chain),
});
const ctx = {
  $: () => chain,
  window: { mapText: (_, d) => d },
  document: { createElement: () => ({ set textContent(t) { this.innerHTML = t; } }) },
};
vm.runInNewContext(fs.readFileSync(new URL('../../src/gui/js/search.js', import.meta.url), 'utf8'), ctx);
ctx.citySearch.displayResults([{ display_name: 'Bucharest, Romania', lat: '44.4361', lon: '26.1027' }]);

const tag = html.match(/<div class="search-result-item"[^>]*>/)[0];
const attrs = Object.fromEntries([...tag.matchAll(/([\w-]+)="([^"]*)"/g)].map((m) => [m[1], m[2]]));
assert.strictEqual(parseFloat(attrs['data-lat']), 44.4361, tag);
assert.strictEqual(parseFloat(attrs['data-lon']), 26.1027, tag);
assert.match(tag, /^<div( [\w-]+="[^"]*")*>$/, 'stray text in ' + tag);

// "Select this area" is offered only with Advanced features (never to a new user), and only for a
// place with an area; a shop's box is a few metres.
const city = { display_name: 'Berlin, Germany', lat: '52.5', lon: '13.4', boundingbox: ['52.33', '52.68', '13.08', '13.76'] };
const shop = { display_name: 'Bakery, Berlin', lat: '52.5', lon: '13.4', boundingbox: ['52.50000', '52.50005', '13.40000', '13.40008'] };
assert.ok(ctx.citySearch.selectable(city) && !ctx.citySearch.selectable(shop) && !ctx.citySearch.selectable({}));
const offered = (flags) => {
  Object.assign(ctx.window, { arnisMeldOn: false, arnisMapFirst: false }, flags);
  ctx.citySearch.displayResults([city, shop]);
  return (html.match(/class="search-select-area"/g) || []).length;
};
assert.strictEqual(offered({}), 0, 'Advanced off, Classic: as before, no button');
assert.strictEqual(offered({ arnisMapFirst: true }), 0, 'map-first alone: no button either');
assert.strictEqual(offered({ arnisMeldOn: true }), 1, 'Advanced: the city only');
console.log('search-result ok');
