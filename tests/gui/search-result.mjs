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
console.log('search-result ok');
