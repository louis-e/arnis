// Run: node tests/gui/resume.mjs
// A job's flags go back into the controls and come out of meldFlags unchanged.
import assert from 'node:assert';

const defaults = { 'meld-margin-input': '0', 'meld-piece-order-select': 'centre', 'meld-tree-realm-select': 'auto', 'meld-cave-style-select': 'all-mix', 'meld-rock-density-input': '0.02', 'meld-bush-density-input': '0.05', 'meld-blinear-level-input': '6' };
const groups = { 'meld-snap-group': 'off', 'meld-piece-size-group': '8', 'meld-road-detail-group': 'max', 'meld-props-group': 'auto', 'meld-tree-pack-mode-group': 'add', 'meld-cave-ores-group': 'normal', 'meld-river-bed-group': 'off', 'meld-water-detail-group': 'default', 'meld-climate-mode-group': 'origin', 'meld-osm-source-group': 'default', 'meld-region-format-group': 'mca' };
const controls = new Map();
const reset = () => {
  controls.clear();
  Object.assign(groups, { 'meld-snap-group': 'off', 'meld-piece-size-group': '8', 'meld-road-detail-group': 'max', 'meld-props-group': 'auto', 'meld-cave-ores-group': 'normal', 'meld-river-bed-group': 'off', 'meld-water-detail-group': 'default', 'meld-climate-mode-group': 'origin', 'meld-osm-source-group': 'default', 'meld-region-format-group': 'mca' });
};
const control = (id) => {
  if (!controls.has(id)) controls.set(id, { value: defaults[id] ?? '', checked: id === 'meld-buildings-toggle' || id === 'advanced-meld-features-toggle', dispatchEvent() {} });
  return controls.get(id);
};
// The choices each button group offers: a job's value is only taken when its group has it.
const offered = { 'meld-snap-group': ['off', 'cover', 'fit', 'merge'], 'meld-piece-size-group': ['auto', '2', '4', '8'], 'meld-road-detail-group': ['max', 'clean', 'compact'], 'meld-props-group': ['auto', 'all', 'none'], 'meld-tree-pack-mode-group': ['add', 'replace'], 'meld-cave-ores-group': ['normal', 'more'], 'meld-river-bed-group': ['off', 'v1'], 'meld-water-detail-group': ['default', 'scaled'], 'meld-climate-mode-group': ['origin', 'per-position'], 'meld-region-format-group': ['mca', 'blinear'], 'meld-osm-source-group': ['default', 'archive', 'region'] };
let clicks = 0;
// Groups greyed out: their buttons are disabled, and a disabled button ignores click().
const greyed = new Set();
globalThis.document = {
  getElementById: control,
  querySelectorAll(sel) {
    const m = /^#(\S+) \.segment$/.exec(sel);
    return m ? (offered[m[1]] || []).map((v) => ({ dataset: { v }, disabled: greyed.has(m[1]), click() { if (this.disabled) return; clicks++; groups[m[1]] = v; } })) : [];
  },
  querySelector(sel) {
    let m = /^#(\S+) \.segment\.active$/.exec(sel);
    if (m) return { dataset: { v: groups[m[1]] } };
    m = /^#(\S+) \.segment\[data-v="(.*)"\]$/.exec(sel);
    if (m) return { click() { groups[m[1]] = m[2]; } };
    return null;
  },
};
globalThis.window = {};
const { applyFlags, meldFlags, jobFits } = await import('../../src/gui/js/meld-features.js');
const out = () => meldFlags({ oneWorld: true, java: true, caves: true, extendHeight: true, lods: false }).sort();

const job = ['--one-world-workers=4', '--snap-regions=cover', '--margin-regions=1', '--unit-regions=4', '--piece-order=spiral', '--skip-built', '--stitch-map', '--threads=5', '--ram-budget-mb=8000', '--seed=7', '--min-y=-100', '--max-y=500', '--road-detail=clean', '--surface-patches', '--no-buildings', '--props=none', '--rock-density=0.05', '--bushes=false', '--tree-realm=forest', '--cave-style=lush', '--cave-ores=more', '--river-bed=v1', '--water-detail=scaled', '--climate-mode=per-position', '--origin=44.4', '--world-border', '--region-format=blinear', '--blinear-level=7'];
// meldFlags builds a Region Format flag only for a world that is not One World: check it apart.
reset();
applyFlags(job);
const flags = meldFlags({ oneWorld: false, java: true, caves: true, extendHeight: true, lods: false });
const oneWorld = meldFlags({ oneWorld: true, java: true, caves: true, extendHeight: true, lods: false });
const got = new Set([...flags, ...oneWorld]);
for (const f of job) assert.ok(got.has(f), `${f} survives the round trip`);
assert.strictEqual([...got].length, job.length, 'and nothing else appears');

// Where the data comes from is never taken from a job (its world may be someone else's), and a value
// its control does not offer is ignored instead of becoming part of a selector.
reset();
clicks = 0;
applyFlags(['--overpass-url=http://x', '--osm-pbf=geofabrik', '--osm-pbf-url=http://y', '--offline', '--snap-regions=x"],#telemetry-toggle,[data-v="y']);
assert.deepStrictEqual(out(), [], 'no server, source or offline switch comes back');
assert.strictEqual(clicks, 0, 'and nothing was clicked');

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

// Cell Size and Snap are greyed while Parallel cells is Off: the job's values still land.
greyed.add('meld-piece-size-group').add('meld-snap-group');
applyFlags(['--one-world-workers=4', '--unit-regions=4', '--snap-regions=cover']);
assert.deepStrictEqual(out(), ['--one-world-workers=4', '--snap-regions=cover', '--unit-regions=4'], 'a greyed group takes the value');
greyed.clear();
applyFlags([]);

// A job the controls cannot hold is not offered for Resume: it would run as another job.
const options = (id, values) => { control(id).options = values.map((value) => ({ value })); };
options('meld-workers-select', ['', 'auto', '2', '3', '4', '6', '8']);
options('meld-piece-order-select', ['centre', 'west', 'east', 'north', 'south']);
assert.ok(jobFits(['--one-world-workers=auto', '--unit-regions=auto', '--piece-order=west', '--threads=5', '--skip-built']));
assert.ok(!jobFits(['--one-world-workers=5']), 'a worker count the select does not have');
assert.ok(!jobFits(['--one-world-workers=4', '--unit-regions=16']), 'a cell size the group does not have');
assert.ok(!jobFits(['--one-world-workers=4', '--piece-order=3,1,2']), 'a cell order given as a list');
assert.ok(!jobFits(['--threads=2']) && !jobFits([]) && !jobFits(undefined), 'no Parallel cells: not a job this window built');
console.log('resume ok');

// Resume / Rebuild select the job's own area, [south, west, north, east], then start.
const { resumeJob } = await import('../../src/gui/js/meld-features.js');
const run = async (bbox, matches = true, mask) => {
  const log = [];
  let sel = '';
  await resumeJob({ settings: ['--threads=3'], bbox, mask }, false, {
    select: (b) => { log.push(['select', b]); if (matches) sel = b.join(' '); },
    current: () => sel,
    start: () => log.push(['start']),
    noArea: () => log.push(['noArea']),
    mask: (path, area) => log.push(['mask', path, area]),
    wait: async () => {},
  });
  return log;
};
assert.deepStrictEqual(await run([44.1, 26.0, 44.5, 26.9]), [['select', [44.1, 26.0, 44.5, 26.9]], ['start']], 'non-square area, S/W/N/E order, then start');
assert.deepStrictEqual(await run(null), [['noArea']], 'no area: settings restored, said once, nothing starts');
assert.deepStrictEqual(await run([1, 2, 3]), [['noArea']], 'a malformed area counts as unknown');
assert.deepStrictEqual(await run([44.1, 26.0, 44.5, 26.9], false), [['select', [44.1, 26.0, 44.5, 26.9]], ['noArea']], 'never starts before the map shows the area, and says so when it does not');
assert.deepStrictEqual(await run([44.1, 26.0, 44.5, 26.9], true, '/w/m.geojson'), [['select', [44.1, 26.0, 44.5, 26.9]], ['mask', '/w/m.geojson', '44.1 26 44.5 26.9'], ['start']], 'a job cut to an outline gets it back, for that area, before it starts');
assert.deepStrictEqual(out(), ['--threads=3'], 'settings are restored in every case');
