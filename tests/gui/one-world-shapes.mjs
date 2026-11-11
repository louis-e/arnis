// Run: node tests/gui/one-world-shapes.mjs
// One World not in pieces builds the rectangle around the drawn shapes: Start is not blocked,
// the run carries the shapes' bbox and no shapes, and the legend names the rectangle.
import assert from 'node:assert';
import fs from 'node:fs';

globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: () => null };
globalThis.window = { addEventListener() {} };
await import('../../src/gui/js/piece-style.js');
const { generationShapes, legendRows } = await import('../../src/gui/js/meld-features.js');

const shapes = [[[44.44, 26.10], [44.45, 26.11], [44.44, 26.12]], [[44.46, 26.09], [44.47, 26.10], [44.46, 26.10]]];
assert.deepStrictEqual(globalThis.arnisShapesRect(shapes), [44.44, 26.09, 44.47, 26.12], 'the rectangle around every shape');
assert.strictEqual(generationShapes({ oneWorld: true, shapes }), null, 'One World sends no shapes');
assert.strictEqual(generationShapes({ oneWorld: false, shapes }), shapes, 'another run keeps them');

// Start no longer refuses shapes under One World or Live, and the tool is never locked.
const main = fs.readFileSync(new URL('../../src/gui/js/main.js', import.meta.url), 'utf8');
assert.ok(!main.includes('"shapes_one_world", "#fa7878"'), 'the note is not an error');
assert.ok(!/selectedShapes && \(/.test(main), 'Start does not return early for shapes');
assert.ok(!fs.readFileSync(new URL('../../src/gui/js/bbox.js', import.meta.url), 'utf8').includes('setShapesLocked'));

const rows = legendRows({ cells: [], stretch: [], shape: true, rect: true });
assert.deepStrictEqual(rows.map((r) => r.key), ['legend_shape_rect'], 'the shape row is for pieces only');
