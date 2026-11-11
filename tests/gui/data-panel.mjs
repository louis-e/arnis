// Run: node tests/gui/data-panel.mjs
// Download Plan: rows use the translated source names, a row with nothing to count is not
// drawn as "0 of 0", and the cache folder is named with a button to open it.
import assert from 'node:assert';

const nodes = new Map();
const node = () => ({ value: '', checked: false, hidden: false, textContent: '', children: [], listeners: {}, addEventListener(e, f) { this.listeners[e] = f; }, append(...c) { this.children.push(...c); }, replaceChildren(...c) { this.children = c; } });
globalThis.document = { createElement: node, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
const onWindow = {};
globalThis.window = { localization: { data_source_elevation: 'Altitudine' }, addEventListener: (e, f) => { onWindow[e] = f; } };
const { initDataPanel, mb } = await import('../../src/gui/js/meld-features.js');

const calls = [];
const plan = { cache_root: 'D:\arnis\cache', free_bytes: null, items: [
  { source: 'osm', cached: 0, total: 0, missing_bytes: null },
  { source: 'elevation', cached: 1, total: 4, missing_bytes: 3e6 },
] };
const invoke = async (cmd, args) => { calls.push([cmd, args]); return cmd === 'gui_data_plan' ? plan : undefined; };
let prefetched = 0;
initDataPanel({ invoke, area: () => '1 2 3 4', plan: () => ({}), prefetch: () => { prefetched++; } });

await document.getElementById('meld-plan-show').listeners.click();
const text = document.getElementById('meld-plan-out').children.map((l) => l.children.map((c) => c.textContent).join(' | '));
assert.ok(!text.some((l) => l.includes('0 of 0')), text.join('\n'));
assert.ok(text.some((l) => l.startsWith('Map data | ')), text.join('\n'));
assert.ok(text.some((l) => l.startsWith('Altitudine: 1 of 4 cached')), text.join('\n'));
assert.equal(document.getElementById('meld-plan-path').textContent, 'Files are kept in D:\arnis\cache');
assert.equal(document.getElementById('meld-plan-where').hidden, false);
// A language switch redraws the path line without a new Check.
window.localization.meld_cache_path = 'Fisiere in {path}';
onWindow['arnis-localized']();
assert.equal(document.getElementById('meld-plan-path').textContent, `Fisiere in ${plan.cache_root}`);
window.localization.meld_cache_path = undefined;

document.getElementById('meld-plan-open').listeners.click();
assert.deepEqual(calls.at(-1), ['gui_show_in_folder', { path: 'D:\arnis\cache' }]);
await document.getElementById('meld-plan-download').listeners.click();
assert.equal(prefetched, 1);
// Nothing missing prints nothing; a little prints 1 MB; terabytes get a TB step.
assert.deepEqual([mb(0), mb(1), mb(3e6), mb(4.5e12)], ['0 MB', '1 MB', '3 MB', '4.5 TB']);
// Start refused (prefetch returns false): the panel goes back to the plan, not "Downloading".
let started = false;
initDataPanel({ invoke, area: () => '1 2 3 4', plan: () => ({}), prefetch: async () => started });
await document.getElementById('meld-plan-download').listeners.click();
assert.ok(!document.getElementById('meld-plan-out').children[0].children[0].textContent.startsWith('Downloading'));
console.log('data-panel ok');
