// Run: node tests/gui/resume.mjs
// A job's flags go back into the controls and come out of meldFlags unchanged.
import assert from 'node:assert';

const defaults = { 'meld-margin-input': '0', 'meld-piece-order-select': 'centre', 'meld-tree-realm-select': 'auto', 'meld-cave-style-select': 'all-mix', 'meld-rock-density-input': '0.02', 'meld-bush-density-input': '0.05', 'meld-blinear-level-input': '6' };
const groups = { 'meld-snap-group': 'off', 'meld-piece-size-group': '4', 'meld-road-detail-group': 'max', 'meld-props-group': 'auto', 'meld-tree-pack-mode-group': 'add', 'meld-cave-ores-group': 'normal', 'meld-river-bed-group': 'off', 'meld-water-detail-group': 'default', 'meld-climate-mode-group': 'origin', 'meld-osm-source-group': 'default', 'meld-region-format-group': 'mca' };
const controls = new Map();
const reset = () => {
  controls.clear();
  Object.assign(groups, { 'meld-snap-group': 'off', 'meld-piece-size-group': '4', 'meld-road-detail-group': 'max', 'meld-props-group': 'auto', 'meld-cave-ores-group': 'normal', 'meld-river-bed-group': 'off', 'meld-water-detail-group': 'default', 'meld-climate-mode-group': 'origin', 'meld-osm-source-group': 'default', 'meld-region-format-group': 'mca' });
};
const control = (id) => {
  if (!controls.has(id)) controls.set(id, { value: defaults[id] ?? '', checked: id === 'meld-buildings-toggle' || id === 'advanced-meld-features-toggle', dispatchEvent() {} });
  return controls.get(id);
};
globalThis.document = {
  getElementById: control,
  querySelector(sel) {
    let m = /^#(\S+) \.segment\.active$/.exec(sel);
    if (m) return { dataset: { v: groups[m[1]] } };
    m = /^#(\S+) \.segment\[data-v="(.*)"\]$/.exec(sel);
    if (m) return { click() { groups[m[1]] = m[2]; } };
    return null;
  },
};
globalThis.window = {};
const { applyFlags, meldFlags } = await import('../../src/gui/js/meld-features.js');
const out = () => meldFlags({ oneWorld: true, java: true, caves: true, extendHeight: true, lods: false }).sort();

const job = ['--one-world-workers=4', '--snap-regions=cover', '--margin-regions=1', '--unit-regions=8', '--piece-order=spiral', '--skip-built', '--stitch-map', '--threads=5', '--ram-budget-mb=8000', '--seed=7', '--min-y=-100', '--max-y=500', '--road-detail=low', '--surface-patches', '--no-buildings', '--props=off', '--rocks', '--rock-density=0.05', '--bushes', '--tree-realm=forest', '--cave-style=lush', '--cave-ores=rich', '--river-bed=deep', '--water-detail=high', '--climate-mode=local', '--overpass-url=http://x', '--offline', '--origin=44.4', '--world-border', '--region-format=blinear', '--blinear-level=7', '--osm-pbf=geofabrik', '--osm-pbf-url=http://y'];
// meldFlags builds a Region Format flag only for a world that is not One World: check it apart.
reset();
applyFlags(job);
const flags = meldFlags({ oneWorld: false, java: true, caves: true, extendHeight: true, lods: false });
const oneWorld = meldFlags({ oneWorld: true, java: true, caves: true, extendHeight: true, lods: false });
const got = new Set([...flags, ...oneWorld]);
for (const f of job) assert.ok(got.has(f), `${f} survives the round trip`);
assert.strictEqual([...got].length, job.length, 'and nothing else appears');

// Flags a job did not send put the controls back to their defaults.
applyFlags([]);
assert.deepStrictEqual(out(), [], 'no flags: every control is at its default');

// Rebuild does not skip what is built.
applyFlags(['--one-world-workers=2', '--skip-built', '--threads=2'], true);
assert.deepStrictEqual(out(), ['--one-world-workers=2', '--threads=2']);
applyFlags(['--one-world-workers=2', '--skip-built']);
assert.deepStrictEqual(out(), ['--one-world-workers=2', '--skip-built']);
applyFlags(undefined);
assert.deepStrictEqual(out(), [], 'a job without settings changes nothing but the defaults');
console.log('resume ok');

// Resume / Rebuild select the job's own area, [south, west, north, east], then start.
const { resumeJob } = await import('../../src/gui/js/meld-features.js');
const run = async (bbox, matches = true) => {
  const log = [];
  let sel = '';
  await resumeJob({ settings: ['--threads=3'], bbox }, false, {
    select: (b) => { log.push(['select', b]); if (matches) sel = b.join(' '); },
    current: () => sel,
    start: () => log.push(['start']),
    noArea: () => log.push(['noArea']),
    wait: async () => {},
  });
  return log;
};
assert.deepStrictEqual(await run([44.1, 26.0, 44.5, 26.9]), [['select', [44.1, 26.0, 44.5, 26.9]], ['start']], 'non-square area, S/W/N/E order, then start');
assert.deepStrictEqual(await run(null), [['noArea']], 'no area: settings restored, said once, nothing starts');
assert.deepStrictEqual(await run([1, 2, 3]), [['noArea']], 'a malformed area counts as unknown');
assert.deepStrictEqual(await run([44.1, 26.0, 44.5, 26.9], false), [['select', [44.1, 26.0, 44.5, 26.9]]], 'does not start until the map shows the area');
assert.deepStrictEqual(out(), ['--threads=3'], 'settings are restored in every case');
