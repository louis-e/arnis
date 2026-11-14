// Run: node tests/gui/piece-overlay.mjs
// The piece overlay's state, with a stub window and map (no browser): what the map is sent for
// each state, the failed line under the bar, the Show overlap button, the retry of one piece, and stale pieces dropped.
import assert from 'node:assert';

const nodes = new Map();
const node = () => ({ value: '', checked: false, hidden: false, textContent: '', style: {}, dataset: {}, listeners: {}, classList: { toggle() {}, remove() {}, contains: () => false }, replaceChildren() {}, attrs: {}, setAttribute(k, v) { this.attrs[k] = v; }, getAttribute(k) { return this.attrs[k]; }, addEventListener(t, f) { this.listeners[t] = f; } });
globalThis.document = { querySelector: () => null, addEventListener() {}, getElementById: (id) => (nodes.has(id) ? nodes.get(id) : (nodes.set(id, node()), nodes.get(id))) };
const winListeners = [];
globalThis.window = { addEventListener: (t, f) => winListeners.push([t, f]) };
await import('../../src/gui/js/piece-style.js');
const { notePiece, clearPieces, dropStalePieces, showRunControls, initRunControls, initJobPlan, failedLine, meldFlags, cellAction } =
  await import('../../src/gui/js/meld-features.js');

const sent = [];
let starts = 0;
const runs = [];
let key = 'bbox-a|';
const mapWin = {};
initRunControls({ key: () => key, map: () => mapWin, invoke: async () => {}, post: (m) => sent.push(m), start: async () => { starts++; runs.push(meldFlags({}).filter((f) => f.startsWith('--only-pieces'))); }, localize: () => {} });
const last = () => sent.at(-1).pieces;
const bounds = [44.4, 26.1, 44.5, 26.2];
const rec = (piece, state, extra = {}) => notePiece({ piece, of: 4, state, bounds, ...extra });
const message = (data, source = mapWin) => winListeners.filter(([t]) => t === 'message').forEach(([, f]) => f({ data, source }));
document.getElementById('advanced-meld-features-toggle').checked = true;
// Whether Start would run a job in cells (main.js: One World on, not Live, Parallel cells set).
let cellsJob = true;
initJobPlan({ request: () => (cellsJob ? { savePath: 's', worldName: 'w', bboxText: '1 2 3 4', worldScale: 1 } : null) });

clearPieces();
for (let i = 0; i < 4; i++) rec(i, 'queued');
showRunControls(true);
assert.equal(failedLine(), '', 'no failed cell: no line');
assert.equal(document.getElementById('run-failed').textContent, '');
rec(0, 'start', { worker: 1, pct: 0 });
rec(0, 'start', { worker: 1, pct: 40 });
rec(1, 'start', { worker: 2, pct: 0 });
showRunControls(true);
assert.equal(last()[0].pct, 40);
assert.equal(last()[0].text.worker, 'Worker 1');
assert.ok(last()[0].since, 'a running piece has a clock');
assert.equal(last()[0].text.hint, '', 'no retry hint while running');
rec(0, 'done', { pct: 100 });
rec(1, 'failed', { worker: 2, error: 'exit 1; last output:\nline one\nline two' });
rec(2, 'stopped');
showRunControls(true); // main.js does this after every piece record
assert.equal(failedLine(), '1 failed');
assert.equal(document.getElementById('run-failed').textContent, '1 failed', 'the red line under the bar');
assert.equal(last()[1].text.error, 'exit 1; last output:'.replace('; last output:', '; last output:'), 'first line only');
assert.equal(last()[1].text.state, 'Failed');
assert.ok(last()[1].end, 'a finished piece has an end');
assert.equal(last()[0].text.state, 'Done');
// The map has no language of its own: a language change sends the outlines again in the new words.
window.localization = { run_state_failed: 'Fehlgeschlagen' };
winListeners.filter(([t]) => t === 'arnis-localized').forEach(([, f]) => f());
assert.equal(last()[1].text.state, 'Fehlgeschlagen');
delete window.localization;

// A message from anything but the map is ignored.
showRunControls(false);
message({ type: 'retryPiece', piece: 1 }, {});
assert.equal(starts, 0);
showRunControls(true);

// While running, a click on a failed piece does nothing; afterwards it retries that piece alone.
message({ type: 'retryPiece', piece: 1 });
assert.equal(starts, 0);
showRunControls(false);
assert.equal(last()[1].text.hint, 'Click to retry this cell');
assert.equal(last()[0].text.hint, '', 'a done piece is not retried by a click');
message({ type: 'retryPiece', piece: 0 });
assert.equal(starts, 0);
message({ type: 'retryPiece', piece: 1 });
assert.equal(starts, 1);
assert.deepEqual(runs[0], ['--only-pieces=2']);
await new Promise((r) => setTimeout(r, 0));
assert.deepEqual(meldFlags({}).filter((f) => f.startsWith('--only-pieces')), [], 'the retry list is spent');

// Retry-all keeps the finished pieces and the same job.
document.getElementById('run-retry-button').listeners.click();
assert.equal(starts, 2);
assert.deepEqual(runs[1], ['--only-pieces=2,3,4']); // failed, stopped and still queued

// Another selection makes the pieces stale; the same one does not.
dropStalePieces();
assert.equal(last().length, 4);

// A retry is only for a job in cells: where Start would run something else (Live, say), it is
// refused and says so, and nothing starts.
cellsJob = false;
message({ type: 'retryPiece', piece: 1 });
document.getElementById('run-retry-button').listeners.click();
assert.equal(starts, 2, 'no start that would build the whole area');
assert.equal(document.getElementById('progress-info').textContent, '3 could not be changed.');
assert.equal(last().length, 4, 'the cells stay');
cellsJob = true;

// A run that ends after its job went stale (Advanced or Parallel cells turned off meanwhile)
// takes its cells with it: nothing is left to retry as a whole-area run.
showRunControls(true);
key = 'bbox-a|off';
showRunControls(false);
assert.deepEqual(last(), [], 'no cells are left on the map');
assert.equal(document.getElementById('run-controls').hidden, true);
key = 'bbox-a|';
clearPieces();
for (let i = 0; i < 4; i++) rec(i, 'queued');
rec(1, 'failed');
showRunControls(true);
showRunControls(false);
assert.equal(last().length, 4, 'an unchanged job keeps them');

// Finding 1: the selection (or world) changed by a path that never called dropStalePieces:
// a retry, one piece or all, is refused and the outlines go; no --only-pieces is sent.
key = 'bbox-b|';
const before = starts;
message({ type: 'retryPiece', piece: 1 });
assert.equal(starts, before);
assert.deepEqual(last(), []);
clearPieces();
for (let i = 0; i < 2; i++) rec(i, 'queued');
rec(0, 'failed');
showRunControls(false);
key = 'bbox-b|world-2';
document.getElementById('run-retry-button').listeners.click();
assert.equal(starts, before);
assert.deepEqual(last(), []);
// And meldFlags itself never sends it for another job.
key = 'bbox-b|world-2';
clearPieces();
rec(0, 'failed');
const guarded = [];
initRunControls({ key: () => key, map: () => mapWin, invoke: async () => {}, post: (m) => sent.push(m), start: async () => { key = 'x'; guarded.push(meldFlags({}).filter((f) => f.startsWith('--only-pieces'))); }, localize: () => {} });
showRunControls(false);
message({ type: 'retryPiece', piece: 0 });
assert.deepEqual(guarded.at(-1), [], 'a start that changed the area gets no --only-pieces');
key = 'bbox-a|';
clearPieces();
for (let i = 0; i < 4; i++) rec(i, 'queued');
dropStalePieces();
key = 'bbox-b|';
dropStalePieces();
assert.deepEqual(last(), []);
assert.equal(document.getElementById('run-controls').hidden, true);

// The halo toggle is passed on to the map.
const halo = document.getElementById('run-halo-toggle');
const press = () => halo.listeners.click({ currentTarget: halo });
press();
assert.deepEqual(sent.at(-1), { type: 'pieceHalo', on: true });
assert.equal(halo.attrs['aria-pressed'], 'true');
press();
assert.deepEqual(sent.at(-1), { type: 'pieceHalo', on: false });
assert.equal(halo.attrs['aria-pressed'], 'false');
// G8: a failed piece's card offers "Open log"; the map's openLog message shows that file.
{
  const shown = [];
  initRunControls({ key: () => 'bbox-g8|', map: () => mapWin, invoke: async (c, a) => { shown.push([c, a]); }, post: (m) => sent.push(m), start: async () => {}, localize: () => {} });
  clearPieces();
  rec(0, 'start', { worker: 1, pct: 0 });
  rec(1, 'failed', { error: 'exit 1', log: 'C:/j/piece-2.log' });
  assert.equal(last()[1].text.openLog, 'Open log');
  assert.equal(last()[1].log, 'C:/j/piece-2.log');
  assert.equal(last()[0].text.openLog, '', 'only a failed piece with a log');
  message({ type: 'openLog', piece: 1 }, {});
  assert.equal(shown.length, 0, 'ignored from anything but the map');
  message({ type: 'openLog', piece: 0 });
  assert.equal(shown.length, 0, 'a running piece has no log to open');
  message({ type: 'openLog', piece: 1 });
  // initRunControls ran more than once in this file, so each listener answers.
  assert.ok(shown.length > 0);
  shown.forEach((c) => assert.deepEqual(c, ['gui_show_in_folder', { path: 'C:/j/piece-2.log' }]));
  console.log('piece-overlay G8 ok');
}

// The cell menu's Rebuild removes land before it builds: while Start would refuse (a bake, a
// precompute) or would not build in cells, nothing is removed.
{
  const removed = [];
  let ready = false;
  let rebuilds = 0;
  initRunControls({ key: () => 'bbox-r|', map: () => mapWin, invoke: async (c, a) => { removed.push([c, a.cells]); return { removed: a.cells }; }, post: (m) => sent.push(m), start: async () => { rebuilds++; }, localize: () => {}, ready: () => ready });
  clearPieces();
  rec(0, 'done');
  showRunControls(false);
  const rebuild = () => cellAction('rebuild', [0], { ask: async () => true });
  await rebuild();
  cellsJob = false;
  ready = true;
  await rebuild();
  assert.deepEqual([removed, rebuilds], [[], 0], 'refused before anything is removed');
  cellsJob = true;
  await rebuild();
  assert.deepEqual([removed, rebuilds], [[['gui_cells_remove', [0]]], 1], 'removed, then built again');
}

// Many records in one frame reach the map once, with the newest state.
{
  const frames = [];
  globalThis.requestAnimationFrame = (f) => frames.push(f);
  const before = sent.length;
  for (let pct = 0; pct < 50; pct++) rec(0, 'start', { worker: 1, pct });
  assert.equal(sent.length, before, 'nothing before the frame');
  assert.equal(frames.length, 1, 'one frame asked for');
  frames.shift()();
  assert.equal(sent.length, before + 1);
  assert.equal(last()[0].pct, 49);
  delete globalThis.requestAnimationFrame;
}

console.log('piece-overlay: ok');
// The run line's one-second tick would keep node alive.
process.exit(0);
