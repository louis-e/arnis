// Run: node tests/gui/map-legend.mjs
// The map legend is drawn from the tables the map uses, and shows a row only for what is on the map.
import assert from 'node:assert';

globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: () => null };
globalThis.window = { addEventListener() {} };
await import('../../src/gui/js/piece-style.js');
const { legendRows } = await import('../../src/gui/js/meld-features.js');
const { colors, dash, selection } = globalThis.arnisPieceStyle;

const none = { cells: [], stretch: [] };
assert.deepStrictEqual(legendRows(none), [], 'an empty map has no legend');

const rows = legendRows({
  ...none,
  cells: [{ state: 'planned' }, { state: 'start' }, { state: 'failed' }, { state: 'room' }],
  stretch: [0.05],
});
assert.deepStrictEqual(rows.map((r) => r.key), ['plan_state', 'run_state_running', 'run_state_failed', 'plan_state_room', 'legend_stretch_5']);
assert.deepStrictEqual(rows.slice(0, 4).map((r) => r.swatch.color), [colors.planned, colors.start, colors.failed, colors.room]);
assert.strictEqual(rows[1].swatch.dash, dash.start);
assert.strictEqual(rows[3].swatch.dash, dash.room);
assert.strictEqual(rows[4].swatch.color, '#fecc44');
assert.ok(rows[4].swatch.line);

// A margin piece is the ring row, not a planned one; the selection and drawing rows follow the map.
const ring = legendRows({ ...none, cells: [{ state: 'planned', ring: true }], selection: true, drawing: true });
assert.deepStrictEqual(ring.map((r) => r.key), ['legend_drawing', 'plan_legend_selection', 'plan_legend_ring']);
assert.strictEqual(ring[1].swatch.color, selection);
assert.strictEqual(ring[0].swatch.dash, globalThis.arnisPieceStyle.selectionDash);

// Done and already-built pieces share one row.
assert.strictEqual(legendRows({ ...none, cells: [{ state: 'done' }, { state: 'skipped' }] }).length, 1);
console.log('map-legend: ok');
