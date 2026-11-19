// Run: node tests/gui/setting-heading.mjs
// A setting's name keeps exactly the room its icons take, with no markup space before them, so it stays on one line when it fits.
import assert from 'node:assert';
import fs from 'node:fs';

const html = fs.readFileSync(new URL('../../src/gui/index.html', import.meta.url), 'utf8');
const css = fs.readFileSync(new URL('../../src/gui/css/settings.css', import.meta.url), 'utf8').replace(/\r\n/g, '\n');

const spaced = html.match(/<span class="setting-heading">\s*<span data-localize="[^"]+">[^<]*<\/span>\s+<span class="tooltip-icon"/g) || [];
assert.deepStrictEqual(spaced, [], 'a space between a name and its tooltip lets the tooltip wrap alone');

// A property of the rule with exactly this selector, wherever it sits.
const px = (sel, prop) => {
  for (const block of css.split(`\n${sel} {`).slice(1)) {
    const m = block.slice(0, block.indexOf('}')).match(new RegExp(`${prop}: (-?\\d+)px`));
    if (m) return parseFloat(m[1]);
  }
  assert.fail(`${sel} has no ${prop}`);
};
const name = '.setting-heading > [data-localize]';
const gap = px('.setting-heading > .tooltip-icon,\n.setting-heading > .setting-revert', 'margin-left');
const icon = 16;
// The first icon is pulled back into the room to sit one gap after the name, and the last one ends where the room ends.
for (const [first, last, icons] of [['.tooltip-icon', '.tooltip-icon', 1], ['.tooltip-icon', '.setting-revert', 2], ['.setting-revert', '.setting-revert', 1]]) {
  const has = first === last ? first : `${first} + ${last}`;
  const room = px(`${name}:has(+ ${has})`, 'padding-right');
  const pull = px(`${name}:has(+ ${has}) + ${first}`, 'margin-left');
  assert.strictEqual(room + pull, gap, `${has}: the first icon sits one gap after the name`);
  assert.strictEqual(pull + icons * icon + (icons - 1) * gap, 0, `${has}: the icons end where the room does`);
}

// A "New!" badge in a heading follows its tooltip with no space, and while it shows the room holds both.
const headings = html.match(/<span class="setting-heading">[\s\S]*?<\/span>\s*(?:<span class="setting-desc|<\/label>)/g);
for (const h of headings.filter((x) => x.includes('new-badge'))) {
  assert.match(h, /\?<\/span><img class="new-badge"/, h.slice(0, 120));
}
const badge = px('.setting-heading > .new-badge', 'width');
const shown = `.settings-row.has-new-badge:not(.is-modified) ${name}:has(+ .tooltip-icon)`;
const room = px(shown, 'padding-right');
const pull = px(`${shown} + .tooltip-icon`, 'margin-left');
assert.strictEqual(room + pull, gap, 'with a badge the tooltip still sits one gap after the name');
assert.strictEqual(pull + icon + gap + badge, 0, 'the badge ends where the room does');

// Headings fill their row, so display scaling can't break a short name.
const rule = (sel) => {
  const at = css.indexOf(`\n${sel} {`);
  assert.ok(at >= 0, `${sel} has a rule`);
  return css.slice(at, css.indexOf('}', at));
};
assert.match(rule('.setting-text'), /flex-grow: 1;/, 'a heading in a tile takes the room the row leaves it');
assert.match(rule('.setting-choice > .setting-label > .setting-heading'), /width: 100%;/, 'a choice row heading takes the whole row');
assert.match(rule('.setting-choice .setting-heading > [data-localize]'), /white-space: nowrap;/, 'a choice row name never breaks');
console.log('setting-heading: ok');
