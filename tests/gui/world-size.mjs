// Run: node tests/gui/world-size.mjs
// The size estimate on the selection line against worlds the 3.4 build made (MiB, 1 km² at scale 1 unless said), and how it reads.
import assert from 'node:assert';
import fs from 'node:fs';
import { worldSizeMb, formatMb, formatSizeRange, ringsKm2 } from '../../src/gui/js/world-size.js';

const within = (measured, [low, high], slack, what) =>
  assert.ok(measured >= low * (1 - slack) && measured <= high * (1 + slack), `${what}: ${measured} MiB is not about ${low.toFixed(1)}-${high.toFixed(1)}`);

// The window's defaults, 30% either way: the landscape is not known before the run.
for (const [what, mib] of [['farmland', 15.6], ['sea', 15.6], ['suburb', 16.0], ['Alps', 16.3], ['Munich', 16.7], ['Manhattan', 18.8], ['forest', 21.7]]) {
  within(mib, worldSizeMb({ km2: 1 }), 0.3, what);
}
// Larger areas and other scales: blocks grow with the scale squared.
within(66.5, worldSizeMb({ km2: 4 }), 0.1, 'Munich 4 km²');
within(146, worldSizeMb({ km2: 9 }), 0.1, 'Munich 9 km²');
within(1307, worldSizeMb({ km2: 85 }), 0.15, 'Munich 85 km²');
within(16.4, worldSizeMb({ km2: 0.25, scale: 2 }), 0.1, 'Munich 0.25 km² at scale 2');
within(18.8, worldSizeMb({ km2: 4, scale: 0.5 }), 0.15, 'Munich 4 km² at scale 0.5');
// Options a run measured on the same area.
within(21.4, worldSizeMb({ km2: 1, interior: true }), 0.35, 'Munich interiors');
within(35.4, worldSizeMb({ km2: 1, interior: true }), 0.35, 'Manhattan interiors');
within(28.1, worldSizeMb({ km2: 1, light: true }), 0.1, 'Munich baked light');
within(39.7, worldSizeMb({ km2: 1, voxy: true }), 0.1, 'Munich Voxy');
within(36.2, worldSizeMb({ km2: 1, dh: true }), 0.1, 'Munich Distant Horizons');
// Filled ground and caves: flat land at the low end, mountains at the high end.
within(17.0, worldSizeMb({ km2: 1, fill: true }), 0.1, 'Munich filled');
within(19.8, worldSizeMb({ km2: 1, caves: true }), 0.1, 'Munich caves');
within(18.3, worldSizeMb({ km2: 1, fill: true, groundLevel: 0 }), 0.1, 'Munich filled at 0');
within(33.3, worldSizeMb({ km2: 1, caves: true, groundLevel: 0 }), 0.1, 'Munich caves at 0');
within(19.3, worldSizeMb({ km2: 1, fill: true, groundLevel: 63 }), 0.1, 'Munich filled at 63');
within(34.9, worldSizeMb({ km2: 1, caves: true, groundLevel: 63 }), 0.1, 'Munich caves at 63');
within(32.5, worldSizeMb({ km2: 1, caves: true, groundLevel: 63 }), 0.1, 'suburb caves at 63');
within(29.2, worldSizeMb({ km2: 1, fill: true }), 0, 'forest filled');
within(42.5, worldSizeMb({ km2: 1, caves: true }), 0, 'forest caves');
within(36.6, worldSizeMb({ km2: 1, fill: true }), 0, 'Alps filled');
within(63.9, worldSizeMb({ km2: 1, caves: true }), 0, 'Alps caves');
within(24.3, worldSizeMb({ km2: 1, fill: true, tall: false }), 0, 'Alps filled at vanilla height');
within(41.7, worldSizeMb({ km2: 1, caves: true, tall: false }), 0, 'Alps caves at vanilla height');
// Bedrock's .mcworld, and the Moon and Mars at their own scale (5 by 5 degrees about the equator).
within(6.4, worldSizeMb({ km2: 1, format: 'bedrock' }), 0.2, 'Munich Bedrock');
within(9.3, worldSizeMb({ km2: 1, format: 'bedrock' }), 0.25, 'Manhattan Bedrock');
within(18.8, worldSizeMb({ km2: 1, format: 'bedrock', caves: true, groundLevel: 63 }), 0.2, 'Munich Bedrock caves at 63');
within(9.5, worldSizeMb({ km2: 22988, body: 'moon' }), 0.1, 'Moon');
within(5.9, worldSizeMb({ km2: 87827, body: 'mars' }), 0.1, 'Mars');
// Off Earth there are no caves, interiors or extended height; the scale slider does not apply.
assert.deepStrictEqual(worldSizeMb({ km2: 22988, body: 'moon', caves: true, interior: true, scale: 3 }), worldSizeMb({ km2: 22988, body: 'moon' }));
// Formats nobody measured, and no area, say nothing.
for (const format of ['luanti_mineclonia', 'hytale']) assert.strictEqual(worldSizeMb({ km2: 1, format }), null);
assert.strictEqual(worldSizeMb({ km2: 0 }), null);

// Two significant figures, in the unit Windows shows.
assert.strictEqual(formatMb(146), '150 MB');
assert.strictEqual(formatMb(399), '400 MB');
assert.strictEqual(formatMb(38.2), '38 MB');
assert.strictEqual(formatMb(7.4), '7 MB');
assert.strictEqual(formatMb(0.2), '1 MB');
assert.strictEqual(formatMb(999), '1.0 GB');
assert.strictEqual(formatMb(1307), '1.3 GB');
assert.strictEqual(formatMb(23000), '22 GB');
assert.strictEqual(formatMb(2.5e6), '2.4 TB');
assert.strictEqual(formatSizeRange([17, 17.2]), '17 MB');
assert.strictEqual(formatSizeRange([18.5, 65]), '19–65 MB');
assert.strictEqual(formatSizeRange([900, 1400]), '900 MB–1.4 GB');
assert.strictEqual(formatSizeRange(worldSizeMb({ km2: 9 })), '150 MB');
assert.strictEqual(formatSizeRange(worldSizeMb({ km2: 25 })), '430 MB');

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
