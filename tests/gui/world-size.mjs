// Run: node tests/gui/world-size.mjs
// The size estimate on the selection line: what the backend is asked (its figures are tested in world_utils.rs), and how the answer reads.
import assert from 'node:assert';
import fs from 'node:fs';
import { sizeRequest, formatMb, formatSizeRange, rangeText, ringsKm2 } from '../../src/gui/js/world-size.js';

// The window's defaults, and the scale squared.
const plain = sizeRequest({ km2: 2 });
assert.deepStrictEqual(plain, {
  blocksKm2: 2, worldFormat: 'java', interior: false, bakeLighting: false, voxyLod: false, dhLod: false,
  caves: false, fillground: false, tall: true, groundLevel: -62,
});
assert.strictEqual(sizeRequest({ km2: 0.25, scale: 2 }).blocksKm2, 1);
// Caves fill the ground; Disasters fill it too and raise it to -40, but leave a higher level alone.
assert.strictEqual(sizeRequest({ km2: 1, caves: true }).fillground, true);
assert.deepStrictEqual([sizeRequest({ km2: 1, disasters: true }).fillground, sizeRequest({ km2: 1, disasters: true }).groundLevel], [true, -40]);
assert.strictEqual(sizeRequest({ km2: 1, disasters: true, groundLevel: 63 }).groundLevel, 63);
// Interiors only where buildings stand.
for (const [mode, built] of [['geo-terrain', true], ['geo-only', true], ['terrain-only', false], ['blueprint', false]]) {
  assert.strictEqual(sizeRequest({ km2: 1, interior: true, mode }).interior, built, mode);
}
// The Moon and Mars at their own block size, without caves, interiors, Disasters or the extended height.
const moon = sizeRequest({ km2: 22988, body: 'moon', scale: 3, caves: true, interior: true, disasters: true });
assert.ok(Math.abs(moon.blocksKm2 - 0.5747) < 0.001);
assert.deepStrictEqual([moon.caves, moon.interior, moon.fillground, moon.tall, moon.groundLevel], [false, false, false, false, -62]);
assert.ok(Math.abs(sizeRequest({ km2: 87827, body: 'mars' }).blocksKm2 - 0.3513) < 0.001);
// Bedrock is asked; Luanti, Hytale and no area are not.
assert.strictEqual(sizeRequest({ km2: 1, format: 'bedrock' }).worldFormat, 'bedrock');
assert.strictEqual(sizeRequest({ km2: 1, format: 'bedrock', caves: true }).tall, false, 'Bedrock does not build deeper');
for (const format of ['luanti_mineclonia', 'hytale']) assert.strictEqual(sizeRequest({ km2: 1, format }), null);
assert.strictEqual(sizeRequest({ km2: 0 }), null);

// Two significant figures, in the unit Windows shows.
for (const [mib, text] of [[146, '150 MB'], [399, '400 MB'], [38.2, '38 MB'], [7.4, '7 MB'], [9.7, '10 MB'], [0.2, '1 MB'], [999, '1.0 GB'],
  [1307, '1.3 GB'], [10230, '10 GB'], [23000, '22 GB'], [2.5e6, '2.4 TB']]) {
  assert.strictEqual(formatMb(mib), text, String(mib));
}
assert.strictEqual(formatSizeRange([17, 17.2]), '17 MB');
assert.strictEqual(formatSizeRange([18.5, 65]), '19–65 MB');
assert.strictEqual(formatSizeRange([900, 1400]), '900 MB–1.4 GB');
// The cells plan joins its ranges with the same helper.
assert.strictEqual(rangeText([327, 388], (v) => `${v} MB`), '327–388 MB');

// A drawn square of 1 km by 1 km over Munich, and two of them.
const dLat = 1000 / (6371000 * Math.PI / 180);
const dLng = dLat / Math.cos(48.137 * Math.PI / 180);
const square = [[48.137, 11.57], [48.137 + dLat, 11.57], [48.137 + dLat, 11.57 + dLng], [48.137, 11.57 + dLng]];
assert.ok(Math.abs(ringsKm2([square], 6371000) - 1) < 0.01);
assert.ok(Math.abs(ringsKm2([square, square.slice().reverse()], 6371000) - 2) < 0.02);

// Every language says it, with the figure in it.
for (const f of fs.readdirSync(new URL('../../src/gui/locales/', import.meta.url))) {
  const loc = JSON.parse(fs.readFileSync(new URL(`../../src/gui/locales/${f}`, import.meta.url), 'utf8'));
  assert.ok(loc.size_estimate?.includes('{size}') && loc.size_estimate_tip, `${f} lacks the size estimate`);
}
console.log('world-size: ok');
