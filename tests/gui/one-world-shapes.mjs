// Run: node tests/gui/one-world-shapes.mjs
// With Advanced features, One World not in pieces builds the rectangle around the drawn shapes:
// the run carries the shapes' bbox and no shapes, and the legend names the rectangle. Off, One
// World takes rectangles only, as before.
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

// Advanced off: Start refuses shapes under One World or Live, and the map locks the shape tool.
const main = fs.readFileSync(new URL('../../src/gui/js/main.js', import.meta.url), 'utf8');
assert.ok(main.includes('if (selectedShapes && (isLiveMode() || isOneWorldEnabled()) && !meldOn())'), 'Start refuses shapes only with Advanced off');
assert.ok(main.includes("mode: !meld ? 'locked'"), 'the map is told to lock the tool with Advanced off');
assert.ok(fs.readFileSync(new URL('../../src/gui/js/bbox.js', import.meta.url), 'utf8').includes("setShapesLocked(_shapesMode === 'locked')"));

const rows = legendRows({ cells: [], stretch: [], shape: true, rect: true });
assert.deepStrictEqual(rows.map((r) => r.key), ['legend_shape_rect'], 'the shape row is for pieces only');
