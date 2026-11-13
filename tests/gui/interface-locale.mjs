// Run: node tests/gui/interface-locale.mjs
// Dates, numbers and plurals follow the language the window shows, in the browser's region when it is the same language.
import assert from 'node:assert';

const offered = ['en', 'de', 'ua', 'vi', 'zh-CN'];
const store = {};
globalThis.localStorage = { getItem: (k) => store[k] ?? null };
globalThis.document = { getElementById: (id) => (id === 'language-select' ? { options: offered.map((value) => ({ value })) } : null) };
const { interfaceLocale } = await import('../../src/gui/js/language.js');

const at = (browser, saved = null) => {
  Object.defineProperty(globalThis, 'navigator', { value: { language: browser }, configurable: true });
  store['arnis-language'] = saved;
  return interfaceLocale();
};

assert.equal(at('en-GB'), 'en-GB');
assert.equal(at('de-AT'), 'de-AT');
assert.equal(at('uk-UA'), 'uk-UA');
assert.equal(at('vi-VN'), 'vi-VN');
// A language the window does not offer shows English, so its dates are English too.
assert.equal(at('zh-TW'), 'en');
assert.equal(at('cs-CZ'), 'en');
// A saved language wins over the browser's; Ukrainian's code becomes its tag.
assert.equal(at('en-US', 'de'), 'de');
assert.equal(at('en-US', 'ua'), 'uk');
assert.equal(at('uk-UA', 'ua'), 'uk-UA');
// A saved language no longer offered is ignored, as at startup.
assert.equal(at('de-DE', 'xx'), 'de-DE');
// Storage that throws is no saved language.
globalThis.localStorage = { getItem: () => { throw new Error('denied'); } };
assert.equal(at('vi-VN'), 'vi-VN');
console.log('interface locale ok');
