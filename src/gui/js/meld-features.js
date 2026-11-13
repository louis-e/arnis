// The "Advanced features" switch (Settings > Advanced) and the flags it sends. Off, every
// control marked meld-only is hidden and generation gets no flags, so it runs as stock.

const el = (id) => document.getElementById(id);
const value = (id) => (el(id) ? el(id).value.trim() : '');
const choice = (id) => {
  const active = document.querySelector(`#${id} .segment.active`);
  return active ? active.dataset.v : '';
};
let defaultArchive = '';
const on = (id) => Boolean(el(id) && el(id).checked);

// A string from the loaded language, `{name}` placeholders filled in; the English text is the
// fallback while no language is loaded.
export const t = (key, fallback, vars = {}) => {
  let s = (window.localization && window.localization[key]) || fallback;
  for (const [k, v] of Object.entries(vars)) s = s.split(`{${k}}`).join(v);
  return s;
};

// The same, singular when n is 1: the language's `<key>_one` string, or English's own while no
// language is loaded; a language with a plain string only keeps it.
export const tn = (key, fallback, one, n) =>
  n === 1 && (window.localization?.[`${key}_one`] || !window.localization?.[key]) ? t(`${key}_one`, one, { n }) : t(key, fallback, { n });

// A count in the language's plural category (one / few / other ...): `<key>_<category>`, else
// `<key>_other`, else the plain `<key>`, else English's. The category comes from the language
// the app is showing (`arnis-language`, as language-selector.js); `lang` overrides it.
export const tp = (key, fallback, n, lang) => {
  let cat = n === 1 ? 'one' : 'other';
  try {
    lang ||= globalThis.localStorage?.getItem('arnis-language') || globalThis.navigator?.language || 'en';
    cat = new Intl.PluralRules(lang).select(n);
  } catch {}
  const L = window.localization || {};
  const k = [`${key}_${cat}`, `${key}_other`, key].find((x) => L[x]);
  return t(k || key, fallback, { n });
};

// What the status line shows for a progress payload. An error that carries `detail` keeps the
// short `message` on the line and the full text as its tooltip and in the log; a Region Download
// fallback or a resume with changed settings is a notice in the user's language rather than the raw log line.
// A fallback notice stays on the status line for 8 s: a following ordinary line then moves only
// the bar. The end of a run (Error!, Done!, Stopped, a retry's "The retried cells are built") always shows.
export const holdsNotice = (shown, message, since, now) =>
  !shown.notice && since > 0 && now - since < 8000 && !/^(Error!|Done!|Stopped|The retried cells are built)/.test(message);

// Which world a run writes to. A prewarm only fills the caches: it has no world, so the backend
// must never be handed the last built one (a new-world run deletes its world when it ends).
// A One World is resolved from the save folder, so a prewarm with One World on still sends it.
export const generationTarget = ({ prewarm, oneWorld, savePath, worldPath }) =>
  prewarm ? { selectedWorld: oneWorld ? savePath : '', isNewWorld: false } : { selectedWorld: oneWorld ? savePath : worldPath, isNewWorld: true };

// What a run is sent as `selectionShapes`: One World builds the rectangle around the shapes (the
// bbox the map reports), in pieces they travel as the plan mask; only another run clips to them.
export const generationShapes = ({ oneWorld, shapes }) => (oneWorld ? null : shapes);

// Sent by the engine when a job is resumed with other settings (scale::SETTINGS_CHANGED).
const SETTINGS_CHANGED = 'Settings changed since this job started:';

export function progressText({ message = '', detail = '' }) {
  if (message.startsWith('region_download_fallback')) {
    return { text: t('region_download_fallback', 'Region Download is not available for this selection, so Arnis used the default source.'), title: '', log: message, notice: true };
  }
  if (message.startsWith(SETTINGS_CHANGED)) {
    const names = message.slice(SETTINGS_CHANGED.length).trim();
    return { text: t('job_settings_changed', 'Settings changed since this job started: {names}. The cells still to build use the new ones.', { names }), title: '', log: message, notice: true };
  }
  // The backend counts "pieces"; the window says cells.
  const built = /^Building pieces\.\.\. (\d+)\/(\d+) done(\. Paused\.)?$/.exec(message);
  if (built) {
    const text = t('run_building', 'Building cells… {done}/{total}', { done: built[1], total: built[2] });
    return { text: built[3] ? `${text} ${t('run_paused', 'Paused.')}` : text, title: detail, log: detail || null, notice: false };
  }
  return { text: message, title: detail, log: detail || null, notice: false };
}

// The docked 3D preview's width after its top-left grip moved `dx` px (left grows it): 200 px up to
// 60 % of the window's width, and of its height at 16:10.
export const dockWidth = (start, dx, vw, vh) => Math.round(Math.max(200, Math.min(start - dx, vw * 0.6, vh * 0.6 * 1.6)));

// The grip: dragging or the arrow keys (20 px) resize `dock`; its right and bottom stay where they are.
// A click on the grip is not a click on the card, so it never opens the full preview.
export function initDockGrip(dock, grip) {
  let from = null; // { x, w } while dragging
  const size = (w) => { dock.style.width = `${w}px`; };
  const stop = (e) => e.stopPropagation();
  grip.addEventListener('click', stop);
  grip.addEventListener('pointerdown', (e) => {
    e.stopPropagation();
    e.preventDefault();
    from = { x: e.clientX, w: dock.offsetWidth };
    grip.setPointerCapture?.(e.pointerId);
  });
  grip.addEventListener('pointermove', (e) => {
    if (from) size(dockWidth(from.w, e.clientX - from.x, window.innerWidth, window.innerHeight));
  });
  const end = (e) => { if (from) { from = null; grip.releasePointerCapture?.(e.pointerId); } };
  grip.addEventListener('pointerup', end);
  grip.addEventListener('pointercancel', end);
  grip.addEventListener('keydown', (e) => {
    const step = { ArrowLeft: 20, ArrowUp: 20, ArrowRight: -20, ArrowDown: -20 }[e.key];
    if (!step) return;
    e.preventDefault();
    e.stopPropagation();
    size(dockWidth(dock.offsetWidth, -step, window.innerWidth, window.innerHeight));
  });
}

export function initMeldFeatures() {
  const toggle = el('advanced-meld-features-toggle');
  if (!toggle) return;
  // The map-first page (styles.css, body.map-first): Map first, or Auto with Advanced features on.
  const layout = el('layout-select');
  const apply = () => {
    const mode = layout ? layout.value : 'classic';
    document.body.classList.toggle('meld-off', !toggle.checked);
    const mapFirst = mode === 'map' || (mode === 'auto' && toggle.checked);
    document.body.classList.toggle('map-first', mapFirst);
    const dock = el('preview3d-mini');
    if (dock) {
      let w = 0;
      try { w = Number(localStorage.getItem('arnis-preview-dock-width')); } catch (_) { /* default size */ }
      dock.style.width = mapFirst && w >= 200 ? `${w}px` : '';
    }
    // Off, a finished job's pieces leave the map with the rest of Advanced features.
    if (!toggle.checked && !running && pieces.size) resetPieces();
    // So does the outline a Resume or Rebuild put back: the map never drew it, so cannot clear it.
    if (!toggle.checked && maskArea) {
      maskArea = maskPath = '';
      maskSeq++;
    }
  };
  toggle.addEventListener('change', apply);
  if (layout) layout.addEventListener('change', apply);
  apply();

  // The docked 3D preview keeps the width it was dragged to (map-first page only: the classic
  // page sizes it itself, so apply() takes the dragged width off there).
  const dock = el('preview3d-mini');
  if (dock && el('preview3d-grip')) initDockGrip(dock, el('preview3d-grip'));
  if (dock && window.ResizeObserver) {
    new ResizeObserver(() => {
      if (!document.body.classList.contains('map-first') || !dock.offsetWidth) return;
      try { localStorage.setItem('arnis-preview-dock-width', String(dock.offsetWidth)); } catch (_) { /* not kept */ }
    }).observe(dock);
  }
}

// The cells the menu skipped or included since the last run (a Set of plan indexes), and the job
// they are for; null until the menu changes them, so the engine's saved set stands.
let skipCells = null;
let skipKey = '';

// The flags for one run, as the CLI takes them (`--name=value`). What a field holds is passed
// as typed: the backend parses it with the CLI's own parser and says what is wrong.
// `plan` is for the plan drawn before a run: it leaves a pending retry for the run itself.
// `prewarm` is Download now, which is the download Offline Mode would refuse.
export function meldFlags({ oneWorld, java, caves, extendHeight, lods, plan = false, forKey = false, prewarm = false }) {
  // Rocks and Bushes are regular settings; everything else needs the Advanced switch.
  const f = [];
  if (!on('meld-rocks-toggle')) f.push('--rocks=false');
  if (!on('meld-bushes-toggle')) f.push('--bushes=false');
  if (!on('advanced-meld-features-toggle')) return f;
  // Never for another selection or world than the pieces came from.
  if (!plan) {
    if (retryNext.length && keyOf() === jobKey) f.push(`--only-pieces=${retryNext.join(',')}`);
    retryNext = [];
  }
  const add = (flag, v) => v !== '' && f.push(`${flag}=${v}`);

  // Scale
  if (oneWorld) add('--one-world-workers', value('meld-workers-select'));
  // How the job is cut and built: only a job in pieces has them.
  if (oneWorld && value('meld-workers-select')) {
    if (choice('meld-snap-group') !== 'off') add('--snap-regions', choice('meld-snap-group'));
    if (value('meld-margin-input') !== '0') add('--margin-regions', value('meld-margin-input'));
    if (choice('meld-piece-size-group') !== '8') add('--unit-regions', choice('meld-piece-size-group'));
    if (value('meld-piece-order-select') !== 'centre') add('--piece-order', value('meld-piece-order-select'));
    if (on('meld-skip-built-toggle')) f.push('--skip-built');
    // Cells the menu skipped or included before this run; the run takes them (then the engine keeps them).
    if (!forKey && skipCells && skipKey === keyOf()) f.push(globalThis.arnisCells.skipFlag([...skipCells]));
    if (on('meld-stitch-map-toggle')) f.push('--stitch-map');
    add('--plan-mask', maskPath);
  }
  add('--threads', value('meld-threads-input'));
  add('--ram-budget-mb', value('meld-ram-budget-input'));

  // Generation
  add('--seed', value('meld-seed-input'));
  if (extendHeight) {
    add('--min-y', value('meld-min-y-input'));
    add('--max-y', value('meld-max-y-input'));
  }
  if (choice('meld-road-detail-group') !== 'max') add('--road-detail', choice('meld-road-detail-group'));
  if (on('meld-surface-patches-toggle')) f.push('--surface-patches');
  if (!on('meld-buildings-toggle')) f.push('--no-buildings');
  if (choice('meld-props-group') !== 'auto') add('--props', choice('meld-props-group'));

  // Nature
  if (on('meld-rocks-toggle') && value('meld-rock-density-input') !== '0.02') add('--rock-density', value('meld-rock-density-input'));
  if (on('meld-bushes-toggle') && value('meld-bush-density-input') !== '0.05') add('--bush-density', value('meld-bush-density-input'));
  if (value('meld-tree-realm-select') !== 'auto') add('--tree-realm', value('meld-tree-realm-select'));
  if (value('meld-tree-pack-dir-input')) {
    add('--tree-pack-dir', value('meld-tree-pack-dir-input'));
    if (choice('meld-tree-pack-mode-group') !== 'add') add('--tree-pack-mode', choice('meld-tree-pack-mode-group'));
  }
  if (caves) {
    if (value('meld-cave-style-select') !== 'all-mix') add('--cave-style', value('meld-cave-style-select'));
    if (choice('meld-cave-ores-group') !== 'normal') add('--cave-ores', choice('meld-cave-ores-group'));
  }
  if (choice('meld-river-bed-group') !== 'off') add('--river-bed', choice('meld-river-bed-group'));
  if (choice('meld-water-detail-group') !== 'default') add('--water-detail', choice('meld-water-detail-group'));
  if (choice('meld-climate-mode-group') !== 'origin') add('--climate-mode', choice('meld-climate-mode-group'));

  // Data
  const source = choice('meld-osm-source-group');
  if (source === 'archive') add('--osm-tiles-url', value('meld-archive-folder-input') || defaultArchive);
  if (source === 'region') {
    f.push('--osm-pbf=geofabrik');
    add('--osm-pbf-url', value('meld-pbf-url-input'));
  }
  add('--overpass-url', value('meld-overpass-input'));
  if (on('meld-offline-toggle') && !prewarm) f.push('--offline');

  // Map and World
  if (oneWorld) add('--origin', value('meld-origin-input'));
  if (java && on('meld-world-border-toggle')) f.push('--world-border');
  if (java && !oneWorld && !lods && choice('meld-region-format-group') !== 'mca') {
    add('--region-format', choice('meld-region-format-group'));
    if (value('meld-blinear-level-input') !== '6') add('--blinear-level', value('meld-blinear-level-input'));
  }
  // The run took the cells the menu skipped; the engine keeps them from here.
  if (!plan) skipCells = null;
  return f;
}

// What a job's cells belong to: the selection, the One World, whether a run builds in cells at all
// (Advanced on, Parallel cells not Off) and the flags that cut it (--only-pieces and --skip-cells
// number its cells). Leaves out the skip list it guards.
export const cellsKey = (bbox, world) => {
  const flags = meldFlags({ oneWorld: true, java: true, plan: true, forKey: true });
  const cells = flags.some((f) => f.startsWith('--one-world-workers='));
  return `${bbox}|${world}|${cells}|${flags.filter((f) => /^--(unit-regions|margin-regions|snap-regions|plan-mask|origin)=/.test(f))}`;
};

// meldFlags in reverse: each flag a job kept, and the control it came from, with the control's
// value when the flag is absent. Flags that carry a path (an archive, a tree pack, the plan
// mask) are not kept by a job, so they are not here.
const FLAG_CONTROLS = [
  ['--one-world-workers', 'meld-workers-select', 'value', ''], ['--snap-regions', 'meld-snap-group', 'group', 'off'],
  ['--margin-regions', 'meld-margin-input', 'value', '0'], ['--unit-regions', 'meld-piece-size-group', 'group', '8'],
  ['--piece-order', 'meld-piece-order-select', 'value', 'centre'], ['--skip-built', 'meld-skip-built-toggle', 'toggle'],
  ['--stitch-map', 'meld-stitch-map-toggle', 'toggle'], ['--threads', 'meld-threads-input', 'value', ''],
  ['--ram-budget-mb', 'meld-ram-budget-input', 'value', ''], ['--seed', 'meld-seed-input', 'value', ''],
  ['--min-y', 'meld-min-y-input', 'value', ''], ['--max-y', 'meld-max-y-input', 'value', ''],
  ['--road-detail', 'meld-road-detail-group', 'group', 'max'], ['--surface-patches', 'meld-surface-patches-toggle', 'toggle'],
  ['--no-buildings', 'meld-buildings-toggle', 'untoggle'], ['--props', 'meld-props-group', 'group', 'auto'],
  ['--rocks', 'meld-rocks-toggle', 'on'], ['--rock-density', 'meld-rock-density-input', 'value', '0.02'],
  ['--bushes', 'meld-bushes-toggle', 'on'], ['--bush-density', 'meld-bush-density-input', 'value', '0.05'],
  ['--tree-realm', 'meld-tree-realm-select', 'value', 'auto'], ['--cave-style', 'meld-cave-style-select', 'value', 'all-mix'],
  ['--cave-ores', 'meld-cave-ores-group', 'group', 'normal'], ['--river-bed', 'meld-river-bed-group', 'group', 'off'],
  ['--water-detail', 'meld-water-detail-group', 'group', 'default'], ['--climate-mode', 'meld-climate-mode-group', 'group', 'origin'],
  ['--osm-pbf-url', 'meld-pbf-url-input', 'value', ''], ['--overpass-url', 'meld-overpass-input', 'value', ''],
  ['--offline', 'meld-offline-toggle', 'toggle'], ['--origin', 'meld-origin-input', 'value', ''],
  ['--world-border', 'meld-world-border-toggle', 'toggle'], ['--region-format', 'meld-region-format-group', 'group', 'mca'],
  ['--blinear-level', 'meld-blinear-level-input', 'value', '6'],
];

const SOURCE_FLAGS = new Set(['--osm-pbf-url', '--overpass-url', '--offline']);

// flag -> value (true for a switch) of a job's kept flags.
function keptFlags(flags) {
  const kept = new Map();
  for (const f of flags || []) {
    const i = f.indexOf('=');
    kept.set(i < 0 ? f : f.slice(0, i), i < 0 ? true : f.slice(i + 1));
  }
  return kept;
}

// Puts a job's flags back into the controls (the Start path in reverse); a flag the job did not
// send puts its control back to the default. `rebuild` leaves out what would skip built cells.
export function applyFlags(flags, rebuild = false) {
  const kept = keptFlags(flags);
  if (rebuild) kept.delete('--skip-built');
  // The flags come from a world's manifest, which may be someone else's: a value is only taken when
  // the control offers it, and it never becomes part of a selector.
  const set = (id, kind, v) => {
    if (kind === 'group') {
      const seg = Array.from(document.querySelectorAll(`#${id} .segment`)).find((s) => s.dataset.v === v);
      if (seg && choice(id) !== v) {
        // A greyed group's buttons ignore click(): lifted for the one call, as settings-store.js does.
        const greyed = seg.disabled;
        seg.disabled = false;
        seg.click();
        seg.disabled = greyed;
      }
      return;
    }
    const box = el(id);
    if (!box) return;
    if (kind === 'value' && box.options && !Array.from(box.options).some((o) => o.value === v)) return;
    const prop = kind === 'value' ? 'value' : 'checked';
    if (box[prop] !== v) {
      box[prop] = v;
      box.dispatchEvent(new Event('change', { bubbles: true }));
    }
  };
  for (const [flag, id, kind, fallback] of FLAG_CONTROLS) {
    // Where the data comes from stays the user's own choice: a job never brings its servers along.
    if (SOURCE_FLAGS.has(flag)) continue;
    const v = kept.get(flag);
    if (kind === 'value' || kind === 'group') set(id, kind, v === undefined ? fallback : String(v));
    else if (kind === 'on') set(id, 'toggle', v !== 'false');
    else set(id, 'toggle', kind === 'untoggle' ? v === undefined : v !== undefined);
  }
}

// Whether applyFlags can put all of a job's settings back, so a Resume runs that job and not
// another: it was built in cells, and every value it kept is one its control offers.
export function jobFits(flags) {
  const kept = keptFlags(flags);
  if (!kept.has('--one-world-workers')) return false;
  return FLAG_CONTROLS.every(([flag, id, kind]) => {
    const v = kept.get(flag);
    if (v === undefined || SOURCE_FLAGS.has(flag)) return true;
    if (kind === 'group') return Array.from(document.querySelectorAll(`#${id} .segment`)).some((s) => s.dataset.v === v);
    const options = kind === 'value' && el(id)?.options;
    return !options || Array.from(options).some((o) => o.value === v);
  });
}

// Selects `bounds` ([south, west, north, east]) exactly, with no ring around it.
export const selectBounds = (bounds) => postToMap({ type: 'setBbox', bounds });

// The outline drawn or picked on the map, as the file `--plan-mask` reads ('' for none): a
// job in pieces then builds only the pieces it touches.
let maskPath = '';
let maskSeq = 0;
// The selection a job's kept outline was put back on (Resume, Rebuild); it goes with that selection.
let maskArea = '';
export const hasPlanMask = () => Boolean(maskPath);
export function useJobMask(path, area) {
  maskSeq++;
  maskPath = path;
  maskArea = area;
  schedulePlan();
}
export async function setPlanMask(geojson) {
  const seq = ++maskSeq;
  maskArea = '';
  let path = '';
  let error = '';
  if (geojson) {
    try {
      path = await call('gui_write_plan_mask', { geojson: JSON.stringify(geojson) });
    } catch (e) {
      error = String(e);
      window.arnisLog?.('warn', `Outline not used: ${e}`);
    }
  }
  if (seq !== maskSeq) return '';
  maskPath = path || '';
  schedulePlan();
  return error;
}

// ---- A job built in pieces: outlines on the map, Pause, Stop and Retry ----

// piece index -> { state, bounds, halo, worker, pct, since, end, error }, from the job's
// `piece` records. `since` is when it began running, `end` when it stopped.
const pieces = new Map();
let paused = false;
let running = false;
// Whether the overlap border is drawn (the Show overlap button).
let halo = false;
// What the pieces belong to (selection and world): a different one makes them, and Retry, stale.
let jobKey = '';
let keyOf = () => '';
let retryNext = [];
let call = () => {};
let postToMap = () => {};
let restart = () => {};
let localizer = () => {};

const RUNNING = new Set(['start', 'retry']);
// States of a cell that waits: it has no clock and no percent.
const WAITING = new Set(['queued', 'held', 'omitted']);
const STATE_TEXT = {
  queued: ['run_state_queued', 'Waiting'], start: ['run_state_running', 'Running'],
  retry: ['run_state_running', 'Running'], done: ['run_state_done', 'Done'],
  skipped: ['run_state_skipped', 'Already built'], failed: ['run_state_failed', 'Failed'],
  stopped: ['run_state_stopped', 'Stopped'], held: ['run_state_held', 'Held'],
  omitted: ['run_state_omitted', 'Skipped'], planned: ['plan_state', 'Planned'],
};

// What the map draws; the words are localized here because the map has no language of its own.
const outlines = () =>
  [...pieces].map(([i, p]) => {
    const [key, fallback] = STATE_TEXT[p.state] || STATE_TEXT.queued;
    return {
      i, n: i + 1, state: p.state, bounds: p.bounds, halo: p.halo, pct: p.pct, since: p.since, end: p.end,
      head: i === headCell && RUNNING.has(p.state),
      text: {
        piece: t('run_piece', 'Cell {n}', { n: i + 1 }),
        state: t(key, fallback),
        size: sizeText(i),
        worker: p.worker ? t('run_worker', 'Worker {n}', { n: p.worker }) : '',
        // The first line is the failure; the child's output after it stays in the log.
        error: (p.error || '').split('\n')[0],
        hint: canRetry(p) ? t('run_retry_hint', 'Click to retry this cell') : '',
        openLog: p.state === 'failed' && p.log ? t('run_open_log', 'Open log') : '',
      },
      log: p.state === 'failed' ? p.log : undefined,
    };
  });

// A finished job's failed or stopped piece can be rebuilt alone.
const canRetry = (p) => !running && (p.state === 'failed' || p.state === 'stopped');

// Under the bar: "2 failed", only above 0 (the title and the bar say the rest).
export function failedLine() {
  const n = [...pieces.values()].filter((p) => p.state === 'failed').length;
  return n ? tp('run_failed', '{n} failed', n) : '';
}

// When this run's first piece record came, so after any dialog: the clock and the ETA count
// only what it built. 0 until then.
let runStart = 0;
// The cell building alone at the start of a new world (it cannot be paused or cancelled); -1: none.
let headCell = -1;

// The bar's tooltip: "3:12 elapsed · 4 running" (the estimate is on the bar itself).
function barTip() {
  if (!running || !runStart) return '';
  const s = Math.floor((Date.now() - runStart) / 1000);
  const clock = `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  return t('run_tip', '{elapsed} elapsed · {running} running', { elapsed: clock, running: [...pieces.values()].filter((p) => RUNNING.has(p.state)).length });
}

function showRunLine() {
  el('run-failed').textContent = pieces.size ? failedLine() : '';
  const bar = el('progress-bar')?.parentElement;
  if (bar) {
    const tip = barTip();
    bar.title = tip;
    bar.tabIndex = tip ? 0 : -1; // focus shows it to a screen reader too
    if (tip) bar.setAttribute?.('aria-label', tip); else bar.removeAttribute?.('aria-label');
  }
}

// One run of the job over the given pieces (1-based, as --only-pieces takes them). The run's
// flags (meldFlags) take them; one a refused start leaves behind is dropped here.
function retry(list) {
  // --only-pieces names pieces of the job the outlines came from, not of another area.
  if (keyOf() !== jobKey) return dropStalePieces();
  // No job in cells would start (Live, say): the run would build something else.
  if (!isPiecesJob()) return say(t('cell_ignored', '{n} could not be changed.', { n: list.length }));
  retryNext = list;
  Promise.resolve(restart()).finally(() => { retryNext = []; });
}

let mapWindow = () => null;
let world = () => '';
let refreshWorld = () => {};
// Whether Start would start a run now (main.js: not during a bake or a precompute).
let startReady = () => true;

export function initRunControls({ invoke, post, start, localize, key = () => '', map = () => null, world: worldOf = () => '', refreshWorld: refresh = () => {}, ready = () => true }) {
  world = worldOf;
  refreshWorld = refresh;
  startReady = ready;
  keyOf = key;
  mapWindow = map;

  call = invoke;
  localizer = localize;
  // The legend shows what the map was last sent.
  postToMap = (m) => {
    if (m.type === 'pieceOutlines') {
      drawn = m.pieces;
      if (legendOpen) showPlan(...shown); // an open card lists the lines now on the map
    }
    post(m.type === 'pieceOutlines' ? { ...m, running } : m);
  };
  restart = start;
  // The bar's tooltip ticks once a second while a job runs.
  setInterval(() => running && showRunLine(), 1000);
  // A job's outlines carry their words to the map: a new language sends them again.
  window.addEventListener('arnis-localized', () => pieces.size && postToMap({ type: 'pieceOutlines', pieces: outlines() }));
  el('run-pause-button').addEventListener('click', async () => {
    setPaused(!paused);
    await call('gui_pause_generation', { paused });
  });
  el('run-stop-button').addEventListener('click', async () => {
    // A stopped queue must not stay held: the next run starts unpaused.
    setPaused(false);
    await call('gui_stop_generation');
  });
  el('run-retry-button').addEventListener('click', () => {
    retry([...pieces].filter(([, p]) => RETRYABLE.has(p.state)).map(([i]) => i + 1));
  });
  // The map: a click on a failed or stopped piece's outline retries just that piece.
  window.addEventListener('message', (event) => {
    const m = event.data;
    if (!m || event.source !== mapWindow()) return;
    if (m.type === 'cellsPicked') {
      if (legendOpen) showPlan(...shown); // the card's selected-cells row follows the selection
      return;
    }
    if (m.type === 'cellsAction') return void cellAction(m.action, m.cells, { world, refreshWorld });
    const p = pieces.get(m.piece);
    if (m.type === 'openLog') {
      if (p?.state === 'failed' && p.log) call('gui_show_in_folder', { path: p.log }).catch(() => {});
      return;
    }
    if (m.type === 'retryPiece' && p && canRetry(p)) retry([m.piece + 1]);
  });
  // The faint border around each piece showing what it builds beyond its own area.
  el('run-halo-toggle').addEventListener('click', (e) => {
    halo = e.currentTarget.getAttribute('aria-pressed') !== 'true';
    e.currentTarget.setAttribute('aria-pressed', String(halo));
    postToMap({ type: 'pieceHalo', on: halo });
    if (legendOpen) showPlan(...shown); // the card's overlap row follows the toggle
  });
}

// ---- The cell menu: Skip, Pause, Cancel, Rebuild, Remove ... on the cells picked on the map ----

// A line under the bar (red for an engine error).
function say(text, bad = true) {
  const info = el('progress-info');
  if (!info) return;
  info.textContent = text;
  info.style.color = bad ? '#fa7878' : '#ececec';
}

// The question before a Rebuild or Remove, with the window's hold button; true once held.
let askCells = async () => true;
export function initCellMenu() {
  // The map writes the count of picked cells in the language shown.
  window.arnisCellCount = (n) => tn('plan_pieces', '{n} cells', '{n} cell', n);
  const box = el('cells-confirm');
  let answer = null;
  const close = (yes) => {
    box.hidden = true;
    answer?.(yes);
    answer = null;
  };
  askCells = (action, list) => new Promise((resolve) => {
    answer?.(false);
    answer = resolve;
    el('cells-confirm-text').textContent = t(`cell_${action}_confirm`, '', { n: list.length });
    box.hidden = false;
    box.scrollIntoView({ block: 'nearest' });
    el('cells-confirm-hold').focus();
  });
  el('cells-confirm-cancel').addEventListener('click', () => close(false));
  box.addEventListener('keydown', (e) => e.key === 'Escape' && close(false));
  globalThis.arnisHold(el('cells-confirm-hold'), () => close(true), 1500);
  // Esc clears the cells picked on the map (the map clears them itself when it has the focus).
  document.addEventListener('keydown', (e) => e.key === 'Escape' && postToMap({ type: 'cellsClear' }));
}

// Skip or Include before a run, or on the outlines a finished job left: the next run takes them.
function skipLocal(list, include) {
  const set = new Set(drawn.filter((p) => p.state === 'omitted').map((p) => p.i));
  list.forEach((i) => (include ? set.delete(i) : set.add(i)));
  skipCells = set;
  skipKey = keyOf();
  if (pieces.size) {
    list.forEach((i) => pieces.has(i) && (pieces.get(i).state = include ? 'queued' : 'omitted'));
    postToMap({ type: 'pieceOutlines', pieces: outlines() });
  }
  schedulePlan();
}

// The job the cells on the map belong to: the run's, else the plan's.
let pieceJob = '';
const cellsJob = () => (pieces.size && pieceJob) || lastPlan?.job || '';

// Whether a start now would build cells of the job on the map.
const runsCells = () => startReady() && isPiecesJob() && (!pieces.size || keyOf() === jobKey);

// A menu action on `cells` (plan indexes). A running job takes it through the engine; between
// runs Skip, Include and Retry are the next run's, and Rebuild and Remove delete built land
// first, after the hold. `deps` are for tests.
export async function cellAction(action, cells, deps = {}) {
  const list = [...new Set(cells)].sort((a, b) => a - b);
  const { call: invoke = call, ask = askCells, live = running, world: worldPath = world, job = cellsJob, after = afterRemove, runs = runsCells } = deps;
  if (!list.length) return;
  try {
    if (globalThis.arnisCells.confirm.has(action) && !(await ask(action, list))) return;
    if (live) {
      let result;
      for (const [command, args] of globalThis.arnisCells.calls(action, list)) { const r = await invoke(command, args); result ??= r; }
      const missed = (result?.ignored || []).filter((i) => list.includes(i)).length;
      if (missed) say(t('cell_ignored', '{n} could not be changed.', { n: missed }));
    } else if (action === 'skip' || action === 'include') {
      skipLocal(list, action === 'include');
    } else if (action === 'retry') {
      retry(list.map((i) => i + 1));
    } else {
      // Rebuild removes the land first: only when the run after it builds these cells again.
      if (action === 'rebuild' && !runs()) return say(t('cell_ignored', '{n} could not be changed.', { n: list.length }));
      const [command, args] = globalThis.arnisCells.calls('remove', list, worldPath(), job())[0];
      const report = await invoke(command, args);
      say(removedLine(report), false);
      after(list, action === 'rebuild');
    }
  } catch (e) {
    say(String(e));
  }
}

// "Cells removed: 3. Not built: 1." from the engine's report.
export const removedLine = (r) => [
  t('cell_removed', 'Cells removed: {n}.', { n: (r?.removed || []).length }),
  ...((r?.not_built || []).length ? [t('cell_not_built', 'Not built, so not removed: {n}.', { n: r.not_built.length })] : []),
].join(' ');

// The cells' land is gone: the plan is read again; a Rebuild builds just those cells now.
function afterRemove(list, rebuild) {
  if (rebuild) {
    // A plan not yet run becomes the job's outlines, so the cells around stay on the map.
    if (!pieces.size) {
      drawn.filter(globalThis.arnisCells.pickable).forEach((p) => pieces.set(p.i, { state: p.state, bounds: p.bounds }));
      jobKey = keyOf();
      pieceJob = lastPlan?.job || '';
    }
    list.forEach((i) => pieces.has(i) && (pieces.get(i).state = 'planned'));
    postToMap({ type: 'pieceOutlines', pieces: outlines() });
    retry(list.map((i) => i + 1));
  } else {
    resetPieces();
  }
  schedulePlan(true);
  refreshWorld();
}


// Whether the queue is held, and what the button says: the two are one variable.
function setPaused(next) {
  paused = next;
  const button = el('run-pause-button');
  button.dataset.localize = paused ? 'run_resume' : 'run_pause';
  localizer(button, button.dataset.localize);
}

// What Retry runs: pieces that failed, were stopped, or never got to start.
const RETRYABLE = new Set(['failed', 'stopped', 'queued']);

// A piece record from the backend. A retry keeps the pieces it does not rebuild.
export function notePiece(piece) {
  if (!runStart) runStart = Date.now();
  const was = pieces.get(piece.piece);
  // A cell left out by --skip-cells is not one that was already built.
  if (piece.state === 'skipped' && piece.reason === 'skip_cells') piece = { ...piece, state: 'omitted' };
  // The head cell builds alone first, in a job with nothing built yet: the first to start then.
  if (piece.state === 'start' && headCell < 0 && ![...pieces.values()].some((p) => RUNNING.has(p.state) || p.state === 'done' || p.state === 'skipped')) headCell = piece.piece;
  // Progress records repeat the running state: the clock starts with the first.
  const since = RUNNING.has(piece.state) ? (was && RUNNING.has(was.state) ? was.since : Date.now()) : was?.since;
  pieces.set(piece.piece, {
    state: piece.state, bounds: piece.bounds, halo: piece.halo || was?.halo,
    worker: piece.worker ?? was?.worker, since,
    end: RUNNING.has(piece.state) || WAITING.has(piece.state) ? undefined : (was && !RUNNING.has(was.state) && was.end) || Date.now(),
    // A piece that starts again begins at its first percent, and a failure's error is its own.
    pct: piece.pct ?? (WAITING.has(piece.state) ? undefined : was?.pct),
    error: piece.state === 'failed' ? piece.error : '',
    log: piece.log || was?.log,
  });
  sendOutlines();
}

// A large job sends many records a second: the map gets them at most once a frame (at once
// where nothing draws frames).
let outlinesQueued = false;
function sendOutlines() {
  const frame = globalThis.requestAnimationFrame;
  if (!frame) return postToMap({ type: 'pieceOutlines', pieces: outlines() });
  if (outlinesQueued) return;
  outlinesQueued = true;
  frame(() => {
    outlinesQueued = false;
    postToMap({ type: 'pieceOutlines', pieces: outlines() });
  });
}

// A new run starts clean; a retry keeps the pieces it does not rebuild.
export function clearPieces() {
  runStart = 0;
  headCell = -1;
  if (retryNext.length) return;
  jobKey = keyOf();
  pieceJob = lastPlan?.job || '';
  forgetPieces();
}

// The selection changed under a finished job: its pieces (and Retry) no longer fit it, nor
// does an outline put back for another `area`.
export function dropStalePieces(area) {
  if (maskArea && area !== undefined && area !== maskArea) {
    maskArea = maskPath = '';
    maskSeq++;
  }
  schedulePlan();
  if (running || !pieces.size || keyOf() === jobKey) return;
  resetPieces();
}

// A new world: no piece of the old one is left on the map.
export function resetPieces() {
  if (running) return;
  forgetPieces();
  showRunControls(false);
}

function forgetPieces() {
  pieces.clear();
  headCell = -1;
  postToMap({ type: 'pieceOutlines', pieces: [] });
}

// `running` is whether a generation is under way. Pause and Stop belong to a running job,
// Retry to a finished one that left pieces behind.
export function showRunControls(isRunning) {
  const wasRunning = running;
  running = isRunning;
  // Cells whose job the window would not run again (Advanced or Parallel cells went off) end with the run.
  if (wasRunning && !running && pieces.size && keyOf() !== jobKey) forgetPieces();
  // The hint to click a failed piece comes and goes with the run.
  if (wasRunning !== running && pieces.size) postToMap({ type: 'pieceOutlines', pieces: outlines() });
  showRunLine();
  // A run that starts puts its own line in place of the plan; one that ended built pieces,
  // so the plan's states are read again.
  if (wasRunning !== running) showPlan(...shown); // the stretch line goes with the run
  if (!wasRunning && running) schedulePlan();
  if (wasRunning && !running) schedulePlan(true);
  // Expand is for between runs.
  el('plan-expand-button').classList.toggle('is-running', running);
  const unfinished = [...pieces.values()].some((p) => RETRYABLE.has(p.state));
  el('run-controls').hidden = pieces.size === 0 || !(running || unfinished);
  el('run-pause-button').hidden = el('run-stop-button').hidden = !running;
  el('run-retry-button').hidden = running || !unfinished;
  // A run that ends while held must not leave the backend held for the next one.
  if (!running && paused) {
    setPaused(false);
    call('gui_pause_generation', { paused: false });
  }
}

// ---- The plan of a job in pieces, drawn before it runs ----

// `request()` is what the plan is for ({ savePath, worldName, bboxText, worldScale }), or null
// when no job in pieces would run.
let planRequest = () => null;
let planTimer = 0;
let planKey = '';
// Only the newest answer is drawn.
let planSeq = 0;
// The last plan drawn, { n pieces, w workers, done }, while it is the current one; null otherwise.
let lastPlan = null;
export const planPieces = () => lastPlan;
// The key of the request the plan last answered (or failed); a wanted plan with another key is
// being recomputed, and the size warning waits for it.
let planAnswered = '';
export const planPending = () => { const { key } = planWanted(); return key !== '' && key !== planAnswered; };
// Whether Start would run a job in pieces (it asks first: main.js, the hold to confirm).
export const isPiecesJob = () => Boolean(planWanted().req);
let onPlan = () => {};

export function initJobPlan({ request, changed = () => {}, stretch = () => null }) {
  planRequest = request;
  onPlan = changed;
  stretchSource = stretch;
  // Any setting may change the plan; an unchanged request is not asked again.
  ['change', 'click'].forEach((type) => document.addEventListener(type, () => schedulePlan()));
  schedulePlan();
}

// What the plan would be asked for now, and its key; null when no job in pieces would run.
function planWanted() {
  const req = on('advanced-meld-features-toggle') && !running ? planRequest() : null;
  const flags = req ? meldFlags({ oneWorld: true, java: true, plan: true }) : [];
  return { req, flags, key: req ? JSON.stringify([req, flags]) : '' };
}

// `fresh` asks again even for an unchanged request: the world under it changed. A changed
// request clears the old plan from the map at once and drops any answer still on its way.
export function schedulePlan(fresh = false) {
  refreshStretch();
  if (fresh) planKey = planAnswered = '';
  if (planWanted().key !== planKey) {
    planSeq++;
    // A running job keeps its "Builds in n pieces" line.
    if ((lastPlan || planPending()) && !running) {
      lastPlan = null;
      onPlan();
    }
    if (!pieces.size) postToMap({ type: 'pieceOutlines', pieces: [] });
    // A finished job's pieces (and Retry) belong to its own plan, not to this one.
    if (!running && pieces.size && keyOf() !== jobKey) resetPieces();
  }
  clearTimeout(planTimer);
  planTimer = setTimeout(updatePlan, 250);
}

// Selects `bounds` ([south, west, north, east]); in a One World with a ring of regions around
// it: a job in cells with a margin of at least one region. `skipBuilt` leaves out what the world
// has. The settings it changed stay so: their names are returned and said under the bar.
export function selectWithRing(bounds, oneWorld, skipBuilt = false) {
  const changed = [];
  const set = (id, prop, v, key, fallback) => {
    if (!el(id) || el(id)[prop] === v) return;
    el(id)[prop] = v;
    el(id).dispatchEvent(new Event('change', { bubbles: true }));
    changed.push(t(key, fallback));
  };
  // Without One World they would only wait, saved, for the day it is turned on.
  if (oneWorld) {
    if (!value('meld-workers-select')) set('meld-workers-select', 'value', 'auto', 'meld_workers', 'Parallel cells');
    if (value('meld-margin-input') === '0') set('meld-margin-input', 'value', '1', 'meld_margin', 'Margin Regions');
    if (skipBuilt) set('meld-skip-built-toggle', 'checked', true, 'meld_skip_built', 'Skip Built Cells');
  }
  postToMap({ type: 'setBbox', bounds });
  if (changed.length) say(t('settings_changed', 'Settings changed: {list}', { list: changed.join(', ') }), false);
  return changed;
}

const gb = (mbs) => (mbs >= 1048576 ? `${(mbs / 1048576).toFixed(1)} TB` : mbs >= 1024 ? `${(mbs / 1024).toFixed(1)} GB` : `${Math.round(mbs)} MB`);
// "327–388 MB": a unit both ends share is said once.
const range = ([lo, hi], unit) => {
  const [a, b] = [unit(lo), unit(hi)];
  if (a === b) return a;
  const u = a.match(/^[\d.,]+( \S+)$/);
  return u && b.endsWith(u[1]) ? `${a.slice(0, -u[1].length)}–${b}` : `${a}–${b}`;
};
// Rounded so a time never claims more than it knows: 5 s
// steps, then minutes, then 10 minutes.
const minutes = (m) => {
  if (m < 1.5) return t('plan_seconds', '{s} s', { s: Math.max(5, Math.round((m * 60) / 5) * 5) });
  if (m < 60) return t('plan_minutes', '{m} min', { m: Math.round(m) });
  const r = Math.round(m / 10) * 10;
  return t('plan_hours', '{h} h {m} min', { h: Math.floor(r / 60), m: r % 60 });
};

// A long job in whole hours ("48–139 h"); a shorter one as `minutes`.
const longTime = (hours) => (m) => (hours ? t('plan_hours_only', '{h} h', { h: Math.max(1, Math.round(m / 60)) }) : minutes(m));

// The plan's headline (time and size, one small row, then the disk line), and a "?" with everything else.
// `more` is the lines behind the "?". `disk` is null while the free space is unknown.
export function planHead(e, more) {
  const head = div('plan-head', '');
  // One small row, one value each: the midpoint of the estimate, never a range (the "?" keeps the ranges).
  const mid = ([lo, hi]) => (lo + hi) / 2;
  const ram = mid(e.ram_mb) * (e.workers || 1);
  for (const [kind, key, tip, text] of [
    ['time', 'plan_time_tip', 'Time (estimate, warm caches)', `≈ ${longTime(e.time_min[1] >= 600)(mid(e.time_min))}`],
    ['ram', 'plan_ram_tip', 'Memory per worker', `≈ ${gb(ram)} RAM`],
    ['disk', 'plan_disk_tip', 'Disk space', `≈ ${gb(mid(e.disk_mb))}`],
  ]) head.append(Object.assign(div(`plan-stat plan-stat-${kind}`, text), { title: t(key, tip) }));
  const tip = Object.assign(div('plan-tip', ''), { id: 'plan-tip', role: 'tooltip' });
  tip.append(...[`${range(e.time_min, longTime(e.time_min[1] >= 600))} · ${range(e.disk_mb, gb)}`, ...more].map((x) => div('', x)));
  // The one "?": hover or focus shows the details; a click holds them open, with the legend card (showPlan).
  const help = Object.assign(document.createElement('button'), { type: 'button', className: 'tooltip-icon plan-help', id: 'legend-help', textContent: '?', title: t('plan_more', 'Details') });
  help.setAttribute('aria-describedby', 'plan-tip');
  const toggle = (open) => {
    legendOpen = open;
    showPlan(...shown);
    el('legend-help')?.focus();
  };
  help.addEventListener('click', () => toggle(!legendOpen));
  help.addEventListener('keydown', (ev) => ev.key === 'Escape' && legendOpen && toggle(false));
  head.append(help, tip);
  head.tip = tip;
  head.help = help;
  const fits = e.free_disk_mb == null ? null : e.disk_mb[1] <= e.free_disk_mb;
  const disk = div(`plan-disk${fits === false ? ' is-low' : ''}`, fits == null ? '' : fits
    ? t('plan_disk_fits', 'Fits: {free} free', { free: gb(e.free_disk_mb) })
    : t('plan_disk_short', 'Needs {need}, {free} free on this disk', { need: range(e.disk_mb, gb), free: gb(e.free_disk_mb) }));
  disk.hidden = fits == null;
  return [head, disk];
}

// "4 × 4 regions" for the piece numbered `i` in the last plan.
const planSizes = new Map();
const sizeText = (i) => (planSizes.has(i) ? t('plan_piece_size', '{n} × {m} regions', { n: planSizes.get(i), m: planSizes.get(i) }) : '');

const div = (className, textContent) => Object.assign(document.createElement('div'), { className, textContent });

// The plan's blocks and the legend of what the map shows: the selection, the
// pieces to build and, with a margin, the ring around it.
function showPlan(rows, ring = false, single = false) {
  shown = [rows, ring, single];
  const box = el('job-plan');
  if (!box) return;
  // With a plan the stretch lines sit behind its "?" (amber or red there too); alone they show as they are.
  const head = rows[0]?.tip ? rows[0] : null;
  const stretch = running ? [] : stretchRows; // the stretch readout is for planning, not for a run
  if (head) {
    head.tip.querySelectorAll?.('.plan-stretch, .legend-card').forEach((x) => x.remove());
    head.tip.append(...stretch);
    head.help.classList.toggle('is-open', legendOpen);
    head.help.ariaExpanded = String(legendOpen);
    // The "?" opens the card of every line the map can show right now, inside the details.
    if (legendOpen && rows.length > 1) head.tip.append(legendCard());
    head.help.classList.toggle('is-warn', stretch.some((x) => x.classList.contains('is-warn')));
    head.help.classList.toggle('is-error', stretch.some((x) => x.classList.contains('is-error')));
  }
  box.replaceChildren(...rows, ...(head ? [] : stretch));
  if (rows.length > 1) {
    const legend = div('job-plan-legend', '');
    const keys = [['selection', 'plan_legend_selection', 'Selection'], ...(single ? [] : [['pieces', 'plan_legend_pieces', 'Cells to build']])];
    if (ring) keys.push(['ring', 'plan_legend_ring', 'Ring (margin)']);
    for (const [kind, key, fallback] of keys) {
      const item = document.createElement('span');
      item.append(Object.assign(document.createElement('i'), { className: `swatch swatch-${kind}` }), t(key, fallback));
      legend.append(item);
    }
    box.append(legend);
  }
  box.hidden = !rows.length && !stretch.length;
}

// ---- What the lines on the map mean ----

// Every row is drawn from the map's own tables (piece-style.js), only when it is on the map.
// Words: the plan's and the run's own keys where they say the same, a few of its own.
const LEGEND_TEXT = {
  legend_title: 'What the lines on the map mean',
  legend_drawing: 'Your selection while you draw it',
  plan_legend_selection: 'Selection',
  legend_shape_rect: 'The rectangle One World builds around your shapes',
  legend_shape: 'The shape you drew; a cell it touches is built in full',
  plan_state: 'Planned',
  run_state_queued: 'Waiting',
  run_state_running: 'Running',
  run_state_done: 'Done',
  run_state_failed: 'Failed',
  run_state_stopped: 'Stopped',
  run_state_held: 'Held',
  run_state_omitted: 'Skipped',
  legend_picked: 'Selected cells',
  plan_state_excluded: 'Outside the outline: not built',
  plan_state_room: 'Room to grow: you can build here later',
  plan_legend_ring: 'Ring (margin)',
  legend_overlap: 'Overlap two neighbouring cells share',
  legend_stretch_5: 'Past this line the map is stretched 5% against real distances',
  legend_stretch_10: 'Past this line the stretch is 10%; start a new world instead',
  legend_held: 'Land this world already holds',
};

// state: { drawing, selection, shape, cells: [{ state, ring }], halo, stretch: [0.05, 0.1], held }
// -> [{ swatch: { color, weight, fill, dash?, line? }, key }]
export function legendRows(state) {
  const { colors, dash, selection, selectionDash, mask } = globalThis.arnisPieceStyle;
  const rows = [];
  const add = (show, key, color, swatch = {}) => show && rows.push({ key, swatch: { color, weight: 2, fill: 0, ...swatch } });
  const has = (...states) => state.cells.some((c) => states.includes(c.state));
  add(state.drawing, 'legend_drawing', selection, { weight: 3, fill: 0.1, dash: selectionDash });
  add(state.selection, 'plan_legend_selection', selection, { weight: 3, fill: 0.08 });
  add(state.shape && !state.rect, 'legend_shape', mask, { fill: 0.05 });
  add(state.rect, 'legend_shape_rect', selection, { dash: selectionDash });
  add(state.cells.some((c) => c.state === 'planned' && !c.ring), 'plan_state', colors.planned, { weight: 1, fill: 0.08 });
  add(has('queued'), 'run_state_queued', colors.queued, { dash: dash.queued });
  add(has('start', 'retry'), 'run_state_running', colors.start, { dash: dash.start });
  add(has('done', 'skipped'), 'run_state_done', colors.done);
  add(has('failed'), 'run_state_failed', colors.failed, { weight: 3 });
  add(has('stopped'), 'run_state_stopped', colors.stopped, { dash: dash.stopped });
  add(has('held'), 'run_state_held', colors.held, { dash: dash.held });
  add(has('omitted'), 'run_state_omitted', colors.omitted, { fill: 0.3, dash: dash.omitted });
  add(state.picked, 'legend_picked', colors.picked, { weight: 4, fill: 0.3 });
  add(has('excluded'), 'plan_state_excluded', colors.excluded, { weight: 1, fill: 0.3 });
  add(has('room'), 'plan_state_room', colors.room, { weight: 1.5, dash: dash.room });
  add(state.cells.some((c) => c.state === 'planned' && c.ring), 'plan_legend_ring', colors.planned, { weight: 1, fill: 0.03, dash: dash.ring });
  add(state.halo, 'legend_overlap', colors.queued, { weight: 1, dash: dash.halo });
  for (const [level, color] of STRETCH_LEVELS) {
    add(state.stretch.includes(level), `legend_stretch_${Math.round(level * 100)}`, color, { line: true, dash: dash.stretch });
  }
  add(state.held, 'legend_held', selection, { weight: 1 });
  return rows;
}

// The last outlines sent to the map, and whether the legend card is open.
let drawn = [];
let legendOpen = false;

function legendCard() {
  const map = mapWindow()?.arnisLegendState?.() || {};
  const rows = legendRows({
    drawing: map.drawing, selection: map.selection, held: map.held, picked: map.picked, rect: map.rect, shape: hasPlanMask(),
    cells: drawn, halo, stretch: stretchShown,
  });
  const card = div('legend-card', '');
  Object.assign(card, { role: 'group', ariaLabel: t('legend_title', LEGEND_TEXT.legend_title) });
  const svg = (name, attrs) => Object.entries(attrs).reduce((n, [k, v]) => (n.setAttribute(k, v), n), document.createElementNS('http://www.w3.org/2000/svg', name));
  for (const { swatch: w, key } of rows) {
    const line = { stroke: w.color, 'stroke-width': w.weight, 'stroke-dasharray': w.dash || 'none' };
    const mark = svg('svg', { viewBox: '0 0 24 12', width: 24, height: 12, 'aria-hidden': 'true' });
    mark.append(w.line
      ? svg('line', { x1: 0, x2: 24, y1: 6, y2: 6, ...line })
      : svg('rect', { x: 2, y: 2, width: 20, height: 8, fill: w.color, 'fill-opacity': w.fill, ...line }));
    const row = document.createElement('div');
    const swatch = Object.assign(document.createElement('i'), { className: 'swatch' });
    swatch.append(mark);
    row.append(swatch, t(key, LEGEND_TEXT[key]));
    card.append(row);
  }
  return card;
}

// Cells the run generates again over land the world already has: not this job's finished cells,
// nor the ones left out, nor the ones Skip Built skips (every chunk is there).
export const overlapCells = (units, skipBuilt) =>
  units.filter((u) => u.existing_chunks > 0 && !u.done && !u.skip && !(skipBuilt && u.existing_chunks >= u.chunks)).length;

async function updatePlan() {
  const { req, flags, key } = planWanted();
  if (key === planKey) return;
  planKey = key;
  const seq = ++planSeq;
  if (!req) {
    if (!running) {
      lastPlan = null;
      onPlan();
    }
    showPlan([]);
    if (!pieces.size) postToMap({ type: 'pieceOutlines', pieces: [] });
    return;
  }
  let plan;
  try {
    plan = await call('gui_plan_job', { ...req, flags });
  } catch (e) {
    if (seq !== planSeq) return;
    planAnswered = key;
    showPlan([div('', t('plan_error', 'Could not plan this selection: {error}', { error: e }))]);
    onPlan();
    return;
  }
  if (seq !== planSeq) return;
  const units = plan.units || [];
  const e = plan.estimate || {};
  const done = units.filter((u) => u.done).length;
  // `done`: how much of this job an earlier run built (main.js says "Resume (n of m)").
  // `overlap`: what the hold warns of; one plain run has no cells, so main's question asks instead.
  const overlap = plan.single === true ? 0 : overlapCells(units, on('meld-skip-built-toggle'));
  planAnswered = key;
  lastPlan = { n: units.length, w: e.workers, done, overlap, single: plan.single === true, job: plan.job };
  onPlan();
  planSizes.clear();
  units.forEach((u) => planSizes.set(u.index, u.size));
  const regions = (list) => list.reduce((n, u) => n + u.regions, 0);
  const ring = regions(units.filter((u) => u.ring));
  const totals = { pieces: units.length, regions: regions(units), ring };
  // Time and size large, the disk line, the rest behind the "?".
  // A small area is built in one run: no cells to count or draw.
  const single = plan.single === true;
  showPlan(planHead(e, [
    single ? t('plan_single', 'Small area: built in one run.') : [
      tn('plan_pieces', '{n} cells', '{n} cell', totals.pieces),
      tn('plan_regions', '{n} regions', '{n} region', totals.regions),
      ...(ring ? [t('plan_ring', 'ring {n}', { n: ring })] : []),
      tn('plan_workers', '{n} workers', '{n} worker', e.workers),
    ].join(' · '),
    `${t('plan_ram_tip', 'Memory per worker')}: ${range(e.ram_mb, gb)}`,
  ]), units.some((u) => u.ring), single);
  if (single && !pieces.size) {
    postToMap({ type: 'pieceOutlines', pieces: [] });
    return;
  }
  // A job's own outlines (running, or left to retry) stay until they go stale.
  if (pieces.size) return;
  const planned = units.map((u) => {
    const state = u.done ? 'done' : u.skip ? 'omitted' : on('meld-skip-built-toggle') && u.existing_chunks >= u.chunks ? 'skipped' : 'planned';
    const [key, fallback] = STATE_TEXT[state];
    return {
      i: u.index, n: u.index + 1, state, ring: u.ring, bounds: u.bbox.split(',').map(Number),
      text: { piece: t('run_piece', 'Cell {n}', { n: u.index + 1 }), state: t(key, fallback), size: sizeText(u.index), worker: '', error: '', hint: '' },
    };
  });
  // Outside the outline: grey, unnumbered, not built (numbered after the plan's own, as ids).
  // Around the job, within 5% stretch: faint, dotted, room to grow later.
  const extra = [
    ...(plan.excluded || []).map((bbox) => ['excluded', bbox, t('plan_state_excluded', 'Outside the outline: not built')]),
    ...(plan.room || []).map((bbox) => ['room', bbox, t('plan_state_room', 'Room to grow: you can build here later')]),
  ];
  if (planned.length) {
    extra.forEach(([state, bbox, text], k) => planned.push({
      i: units.length + k, n: '', state, ring: false, bounds: bbox.split(',').map(Number),
      text: { piece: '', state: text, size: '', worker: '', error: '', hint: '' },
    }));
  }
  postToMap({ type: 'pieceOutlines', pieces: planned });
}

// ---- How far a One World can grow before it looks stretched ----

// Blocks per metre are fixed at the origin's latitude and heights are not, so at latitude `lat`
// the ground is this much wider (+) or narrower (-) than it is tall. Only latitude counts.
const rad = Math.PI / 180;
export const stretchAt = (origin, lat) => Math.cos(origin * rad) / Math.cos(lat * rad) - 1;

// The nearest latitudes south and north of `origin` where the stretch reaches `s` either way
// (cos lat = cos origin / (1 ± s)); null on a side that has none.
export function stretchLines(origin, s) {
  const lats = [];
  for (const k of [1 + s, 1 - s]) {
    const c = Math.cos(origin * rad) / k;
    if (c <= 1) lats.push(Math.acos(c) / rad, -Math.acos(c) / rad);
  }
  const south = lats.filter((l) => l < origin);
  const north = lats.filter((l) => l > origin);
  return [south.length ? Math.max(...south) : null, north.length ? Math.min(...north) : null];
}

// The stretch furthest from none over [south, north] (it grows away from the equator).
export function worstStretch(origin, south, north) {
  const lats = south < 0 && north > 0 ? [south, north, 0] : [south, north];
  return lats.map((l) => stretchAt(origin, l)).reduce((a, b) => (Math.abs(b) > Math.abs(a) ? b : a));
}

// `stretchSource()`: { origin, knee, south, north, lon } for a One World selection, or null.
let stretchSource = () => null;
let stretchRows = [];
let stretchKey = 'null';
let shown = [[], false, false];
// The stretch levels drawn on the map now (STRETCH_LEVELS), for the legend.
let stretchShown = [];
const STRETCH_LEVELS = [[0.05, '#fecc44'], [0.1, '#fa7878']];

export function refreshStretch() {
  // Off by default: no row in the plan, no lines or labels on the map, no legend rows.
  const s = on('advanced-meld-features-toggle') && on('meld-stretch-toggle') ? stretchSource() : null;
  const key = JSON.stringify(s);
  if (key === stretchKey) return;
  stretchKey = key;
  stretchRows = [];
  const lines = [];
  if (s) {
    for (const [level, color] of STRETCH_LEVELS) {
      for (const lat of stretchLines(s.origin, level)) {
        if (lat !== null) lines.push({ lat, color, label: `${level * 100}%`, lon: s.lon });
      }
    }
    const worst = worstStretch(s.origin, s.south, s.north);
    const pct = Math.round(worst * 100);
    const row = div('plan-stretch', t('plan_stretch', 'Stretch here: {pct} · up to 5% recommended', {
      pct: `${pct > 0 ? '+' : pct < 0 ? '−' : ''}${Math.abs(pct)}%`,
    }));
    row.classList.toggle('is-warn', Math.abs(worst) > 0.05);
    row.classList.toggle('is-error', Math.abs(worst) > 0.1);
    stretchRows.push(row);
    // The selection's heights are not known here: said as information.
    if (s.knee != null) {
      stretchRows.push(div('plan-stretch', t('plan_soft_top', 'Land above {m} m is squashed', {
        m: (Math.round(s.knee / 100) * 100).toLocaleString(),
      })));
    }
  }
  stretchShown = STRETCH_LEVELS.filter(([, color]) => lines.some((l) => l.color === color)).map(([level]) => level);
  postToMap({ type: 'stretchLines', lines });
  showPlan(...shown);
}

// ---- Data panel: Prepare Countries and the Download Plan ----

// Sizes for the Download Plan; nothing missing prints nothing, anything missing at least 1 MB.
export const mb = (b) => (b <= 0 ? '0 MB' : b >= 1e12 ? `${(b / 1e12).toFixed(1)} TB` : b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.max(1, Math.round(b / 1e6))} MB`);

function lines(box, rows) {
  box.replaceChildren(
    ...rows.map(([a, b]) => {
      const line = document.createElement('div');
      line.className = 'meld-data-line';
      const l = document.createElement('span');
      l.textContent = a;
      const r = document.createElement('span');
      r.textContent = b;
      line.append(l, r);
      return line;
    })
  );
  box.hidden = false;
}

const SOURCE_NAMES = { osm: 'Map data', elevation: 'Elevation', land_cover: 'Land cover', overture: 'Building footprints' };

// `area()` is the selected bbox; `plan()` the settings a run's plan depends on; `baking(on)` hears
// a Download & Bake start and end.
export function initDataPanel({ invoke, area, plan, prefetch, baking = () => {} }) {
  const needArea = () => t('meld_select_area', 'Select an area on the map first.');
  const list = el('meld-prepare-list');
  const out = el('meld-plan-out');
  const folder = () => value('meld-archive-folder-input');
  const say = (box, text) => lines(box, [[text, '']]);
  invoke('gui_default_archive_folder')
    .then((dir) => { defaultArchive = dir || ''; })
    .catch(() => {});

  el('meld-prepare-check').addEventListener('click', async () => {
    if (!area()) return say(list, needArea());
    say(list, t('meld_asking_geofabrik', 'Asking Geofabrik...'));
    try {
      const p = await invoke('gui_prepare_plan', { bboxText: area(), folder: folder() });
      const rows = p.extracts.map((e) => [e.baked ? t('meld_baked', '{name} (baked)', { name: e.name }) : e.name, t('meld_extract_sizes', '{download} download, {archive} archive', { download: mb(e.bytes), archive: mb(e.archive_bytes) })]);
      rows.push([t('meld_archive_folder_line', 'Archive folder'), p.folder]);
      if (p.free_bytes != null) rows.push([t('meld_free_peak', 'Free space / peak during the bake'), `${mb(p.free_bytes)} / ${mb(p.peak_bytes)}`]);
      if (p.uncovered_points) rows.push([t('meld_uncovered', 'Not covered by any country (open sea)'), t('meld_points', '{n} points', { n: p.uncovered_points })]);
      lines(list, rows);
    } catch (e) {
      say(list, t('plan_error', 'Could not plan this selection: {error}', { error: e }));
    }
  });

  const stop = el('meld-prepare-stop');
  const bake = el('meld-prepare-bake');
  bake.addEventListener('click', async () => {
    if (!area()) return say(list, needArea());
    // One bake at a time: a second click would be refused and take Stop away from the first.
    bake.disabled = true;
    stop.hidden = false;
    baking(true);
    say(list, t('meld_baking', 'Baking: the progress is on the main bar.'));
    try {
      const done = await invoke('gui_bake_archive', {
        bboxText: area(), folder: folder(), flags: plan().flags,
      });
      say(list, done ? t('meld_bake_done', 'The local archive is ready. Pick Local Archive as the OSM Data Source to use it.') : t('meld_bake_stopped', 'Stopped. Finished countries are kept.'));
    } catch (e) {
      say(list, t('meld_bake_error', 'Could not bake: {error}', { error: e }));
    }
    stop.hidden = true;
    bake.disabled = false;
    baking(false);
  });
  stop.addEventListener('click', () => invoke('gui_cancel_bake'));

  const where = el('meld-plan-where');
  let cacheRoot = '';
  const showPath = () => {
    el('meld-plan-path').textContent = cacheRoot ? t('meld_cache_path', 'Files are kept in {path}', { path: cacheRoot }) : '';
    where.hidden = !cacheRoot;
  };
  // The path line is built from a stored path, so a language switch only redraws it.
  window.addEventListener('arnis-localized', showPath);
  const check = async (ready = false) => {
    if (!area()) return say(out, needArea());
    try {
      const p = await invoke('gui_data_plan', { bboxText: area(), ...plan() });
      const rows = p.items.map((i) => {
        const name = t(`data_source_${i.source}`, SOURCE_NAMES[i.source] || i.source);
        if (!i.total) return [name, t('meld_size_unknown', 'size unknown')];
        return [t('meld_cached', '{source}: {cached} of {total} cached', { source: name, cached: i.cached, total: i.total }), i.missing_bytes != null ? (i.missing_bytes > 0 ? t('meld_to_download', '{size} to download', { size: mb(i.missing_bytes) }) : '') : t('meld_size_unknown', 'size unknown')];
      });
      if (p.free_bytes != null) rows.push([t('meld_free_space', 'Free space'), mb(p.free_bytes)]);
      if (ready) rows.unshift([t('meld_ready_offline', 'Ready to build offline.'), '']);
      lines(out, rows.length ? rows : [[t('meld_nothing_to_download', 'Nothing to download.'), '']]);
      cacheRoot = p.cache_root || '';
      showPath();
    } catch (e) {
      say(out, t('plan_error', 'Could not plan this selection: {error}', { error: e }));
    }
  };
  el('meld-plan-show').addEventListener('click', () => check());
  el('meld-plan-open').addEventListener('click', () => cacheRoot && invoke('gui_show_in_folder', { path: cacheRoot }).catch(() => {}));

  // Download now: a run with --prewarm fills the caches and writes no world. The run's own
  // progress lines come through the window's progress-update event.
  let prefetching = false;
  const listen = window.__TAURI__?.event?.listen;
  if (listen) {
    listen('progress-update', (e) => {
      const m = e.payload && e.payload.message;
      if (!prefetching || !m || e.payload.piece) return;
      // Same text as the main status line (a fallback notice is translated).
      if (m.startsWith('Done!')) {
        prefetching = false;
        check(true);
      } else {
        if (/^(Error!|Stopped)/.test(m)) prefetching = false;
        say(out, progressText(e.payload).text);
      }
    });
  }
  el('meld-plan-download').addEventListener('click', async () => {
    if (!area()) return say(out, needArea());
    // Set before the run starts so its first lines are caught; cleared when Start refused.
    prefetching = true;
    say(out, t('meld_downloading', 'Downloading. Progress shows on the main bar.'));
    if (!(await prefetch())) {
      prefetching = false;
      check();
    }
  });
}

// Settings > Projects: the One Worlds in the save folder, as a list or a grid of cards.
// `open(name, expand)` makes one the target; the name is stored here so the choice outlives the
// session. Every field the engine adds later (place name, thumbnail, jobs) may be missing.
// Returns a refresh function.
const VIEW_KEY = 'arnis-projects-view';
const readView = () => {
  try { return localStorage.getItem(VIEW_KEY) === 'grid' ? 'grid' : 'list'; } catch (_) { return 'list'; }
};

// Nominatim's zoom for a reverse lookup, by the longer side of the area in km: a country for a
// huge area, a city or town for a mid one, the suburb or main place for a small one.
export function placeZoom([south, west, north, east]) {
  const km = Math.max((north - south) * 111, (east - west) * 111 * Math.cos(((north + south) / 2) * Math.PI / 180));
  return km >= 1500 ? 3 : km >= 300 ? 5 : km >= 60 ? 8 : km >= 15 ? 10 : km >= 3 ? 14 : 16;
}

// The name of a search result the selection came from, if its point is inside the area built.
export function placeForRun(found, bboxText) {
  const [s, w, n, e] = (bboxText || '').split(/[\s,]+/).map(Number);
  return found && found.name && found.lat >= s && found.lat <= n && found.lon >= w && found.lon <= e ? found.name : '';
}

// The same geocoder, language and browser identity as the search box; only the coordinates leave,
// rounded to about a kilometre.
const nominatim = async (lat, lon, zoom) => {
  const r = await fetch(`https://nominatim.openstreetmap.org/reverse?format=jsonv2&lat=${lat.toFixed(2)}&lon=${lon.toFixed(2)}&zoom=${zoom}&accept-language=en`);
  if (!r.ok) return '';
  const j = await r.json();
  return String(j.name || String(j.display_name || '').split(',')[0]).trim();
};
const MISS_KEY = 'arnis-place-misses';
const MISS_MS = 7 * 864e5;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// The job a card offers: the unfinished one that ran last, else the last one that finished.
export function projectJob(w) {
  const jobs = [...(Array.isArray(w.jobs) ? w.jobs : [])].sort((a, b) => (b.last_run_ms || 0) - (a.last_run_ms || 0));
  return jobs.find((j) => !j.finished) || jobs[0] || null;
}

// Resume / Rebuild after the world is open: the job's settings go back in, its area ([south,
// west, north, east]) is selected, its kept outline put back, and the run starts once the map
// reports the same area. A job without an area (an old one), or a map that never shows it, only
// restores the settings and tells the user to pick one.
export async function resumeJob(job, rebuild, { select = selectBounds, current, start, noArea, mask = useJobMask, wait = (ms) => new Promise((r) => setTimeout(r, ms)) }) {
  applyFlags(job.settings, rebuild);
  const b = Array.isArray(job.bbox) ? job.bbox.map(Number) : [];
  if (b.length !== 4 || !b.every(Number.isFinite)) return noArea();
  select(b);
  for (let i = 0; i < 80; i++) {
    const now = (current() || '').split(/[\s,]+/).map(Number);
    if (now.length === 4 && now.every((v, k) => Math.abs(v - b[k]) < 1e-4)) {
      if (job.mask) mask(job.mask, current());
      return start();
    }
    await wait(50);
  }
  noArea();
}

// `resume(world, job, rebuild)` starts a job again with its settings; without it (or without
// job data from the engine) only Open and Expand are offered.
// `busy()`: a run is under way, so no world is picked, renamed or run. `canRun()`: Resume and
// Rebuild can start a run here (not in Live).
export function initProjects({ invoke, savePath, scale, open, available = () => true, busy = () => false, canRun = () => true, geocode = nominatim, pause = sleep, resume = null, renamed = null }) {
  const list = el('projects-list');
  let view = readView();
  const ratio = (s) => (s >= 1 ? `${+s.toFixed(2)}:1` : `1:${+(1 / s).toFixed(2)}`);
  const button = (key, fallback, disabled, onClick) => {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'settings-action-button';
    b.disabled = disabled;
    b.textContent = t(key, fallback);
    b.addEventListener('click', onClick);
    return b;
  };
  const text = (cls, s) => {
    const span = document.createElement('span');
    span.className = cls;
    span.textContent = s;
    return span;
  };
  const pick = async (name, expand) => {
    localStorage.setItem('arnis-one-world-name', name);
    localStorage.setItem('arnis-one-world-picked', '1');
    await open(name, expand);
  };

  // A thumbnail is asked for when its card scrolls into view; no image leaves the placeholder.
  const thumbs = new Map();
  const loaders = new Map();
  const seen = typeof IntersectionObserver === 'undefined' ? null : new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      seen.unobserve(e.target);
      loaders.get(e.target)?.();
    }
  }, { rootMargin: '200px' });
  const thumb = (w) => {
    const box = document.createElement('div');
    box.className = 'projects-thumb is-empty';
    const path = w.world_path || `${savePath()}/${w.name}`;
    const key = `${path}|${w.modified_ms || 0}`;
    const show = (url) => {
      if (!url) return;
      const img = document.createElement('img');
      img.alt = '';
      img.src = url;
      box.className = 'projects-thumb';
      box.replaceChildren(img);
    };
    const load = async () => {
      if (!thumbs.has(key)) {
        let url = null;
        try { url = w.preview === false ? null : await invoke('gui_world_thumbnail', { worldPath: path }); } catch (_) { /* a world without an image */ }
        thumbs.set(key, url || null);
      }
      show(thumbs.get(key));
    };
    if (thumbs.has(key)) show(thumbs.get(key));
    else if (seen) { loaders.set(box, load); seen.observe(box); } else load();
    return box;
  };

  const setView = (v) => {
    view = v;
    try { localStorage.setItem(VIEW_KEY, v); } catch (_) { /* not kept */ }
    paint();
    refresh();
  };
  const paint = () => {
    list.className = view === 'grid' ? 'projects-list is-grid' : 'projects-list';
    for (const v of ['list', 'grid']) el(`projects-view-${v}`)?.setAttribute('aria-pressed', String(v === view));
  };
  el('projects-view-list')?.addEventListener('click', () => setView('list'));
  el('projects-view-grid')?.addEventListener('click', () => setView('grid'));
  paint();

  // A world with a null place name (the engine knows the field) is looked up once, at its
  // centre, one world at a time and a second apart (the geocoder's limit). A miss is kept for a week.
  const titles = new Map();
  const misses = () => { try { return JSON.parse(localStorage.getItem(MISS_KEY) || '{}'); } catch (_) { return {}; } };
  let filling = false;
  async function fillPlaces(worlds) {
    // Offline Mode means no request leaves, this one included.
    if (filling || on('meld-offline-toggle')) return;
    filling = true;
    let first = true;
    try {
      for (const w of worlds) {
        if (w.place_name !== null || w.locked) continue;
        const path = w.world_path || `${savePath()}/${w.name}`;
        if (Date.now() - (misses()[path] || 0) < MISS_MS) continue;
        let name = '';
        try {
          const b = Array.isArray(w.bbox) && w.bbox.length === 4 && w.bbox.every(Number.isFinite) ? w.bbox : null;
          if (b) {
            if (!first) await pause(1100);
            first = false;
            name = await geocode((b[0] + b[2]) / 2, (b[1] + b[3]) / 2, placeZoom(b));
          }
          if (name) await invoke('gui_set_place_name', { worldPath: path, placeName: name });
        } catch (_) { name = ''; }
        if (name) {
          w.place_name = name;
          titles.get(path)?.append(text('projects-place', name));
        } else {
          try { localStorage.setItem(MISS_KEY, JSON.stringify({ ...misses(), [path]: Date.now() })); } catch (_) { /* tried again next time */ }
        }
      }
    } finally {
      filling = false;
    }
  }

  // The folder and the name inside level.dat change together in the engine; a refusal comes back
  // as text and is shown beside the field.
  const PENCIL = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M3 17.25V21h3.75L17.81 9.94l-3.75-3.75zm17.71-10.04a1 1 0 0 0 0-1.42l-2.5-2.5a1 1 0 0 0-1.42 0l-1.83 1.83 3.75 3.75z"/></svg>';
  function renameField(w, title) {
    const before = [...title.children];
    const from = w.name;
    const input = document.createElement('input');
    input.type = 'text';
    input.className = 'projects-rename';
    input.maxLength = 48;
    input.value = w.name;
    input.setAttribute('aria-label', t('projects_rename', 'Rename'));
    const err = text('projects-error', '');
    let ended = false;
    const cancel = () => { if (!ended) { ended = true; title.replaceChildren(...before); } };
    const commit = async () => {
      const name = input.value.trim();
      if (!name || name === from) return cancel();
      try {
        const folder = (await invoke('gui_rename_world', { savePath: savePath(), oldName: from, newName: name })) || name;
        ended = true;
        try { if (localStorage.getItem('arnis-one-world-name') === from) localStorage.setItem('arnis-one-world-name', folder); } catch (_) { /* not kept */ }
        if (renamed) renamed(from, folder);
        await refresh();
      } catch (e) {
        err.textContent = String((e && e.message) || e);
      }
    };
    input.addEventListener('keydown', (e) => {
      e.stopPropagation?.();
      if (e.key === 'Enter') { e.preventDefault?.(); commit(); } else if (e.key === 'Escape') { e.preventDefault?.(); cancel(); }
    });
    input.addEventListener('blur', cancel);
    title.replaceChildren(input, err);
    input.focus?.();
    input.select?.();
    return input;
  }

  async function refresh() {
    let worlds = [];
    try { worlds = savePath() ? (await invoke('gui_list_one_worlds', { savePath: savePath() })) || [] : []; } catch (e) { console.error(e); }
    // The cards drawn before go: their pending thumbnails with them.
    seen?.disconnect();
    loaders.clear();
    const rows = worlds.map((w) => {
      const row = document.createElement('div');
      row.className = 'projects-row';
      const odd = w.scale != null && scale() != null && Math.abs(w.scale - scale()) > 1e-6;
      const title = document.createElement('div');
      title.className = 'projects-title';
      const name = text('projects-name', w.name);
      const pencil = document.createElement('button');
      pencil.type = 'button';
      pencil.className = 'projects-pencil';
      pencil.innerHTML = PENCIL;
      pencil.title = t('projects_rename', 'Rename');
      pencil.setAttribute('aria-label', t('projects_rename', 'Rename'));
      pencil.disabled = Boolean(w.locked || busy());
      const edit = () => { if (!w.locked && !busy()) renameField(w, title); };
      name.addEventListener('click', edit);
      pencil.addEventListener('click', edit);
      title.append(name, pencil);
      if (w.place_name) title.append(text('projects-place', w.place_name));
      titles.set(w.world_path || `${savePath()}/${w.name}`, title);
      const kids = [
        thumb(w),
        title,
        text('projects-meta', t('projects_areas', 'Areas: {n}', { n: w.area_count })),
        text(odd ? 'projects-meta is-warn' : 'projects-meta', t('projects_scale', 'Scale {scale}', { scale: w.scale ? ratio(w.scale) : '-' })),
      ];
      if (w.modified_ms) kids.push(text('projects-meta', new Date(w.modified_ms).toLocaleDateString()));
      if (w.locked) kids.push(text('projects-meta is-warn', t('projects_in_use', 'In use')));
      // One World needs Java on Earth; a run holds every world.
      const off = !available();
      const held = Boolean(w.locked || off || busy());
      const job = resume ? projectJob(w) : null;
      const acts = [];
      if (job && job.total > 0) {
        const lang = (() => { try { return localStorage.getItem('arnis-language') || undefined; } catch (_) { return undefined; } })();
        const n = (x) => { try { return Number(x).toLocaleString(lang); } catch (_) { return String(x); } };
        if (!job.finished) kids.push(text('projects-meta', t('projects_cells', '{done} / {total} cells', { done: n(job.done), total: n(job.total) })));
        const go = async (rebuild) => { await pick(w.name, false); await resume(w, job, rebuild); };
        // Without its outline the job would run again as a new, larger one.
        const lost = Boolean(job.masked && !job.mask);
        // Settings the controls cannot take would run as another job.
        const foreign = !jobFits(job.settings);
        const why = lost ? t('projects_mask_lost', 'This job was cut to an outline that was not kept with it. Select the area and its outline again to build it.')
          : foreign ? t('projects_settings_unknown', 'This job uses settings this window does not offer, so it cannot be run from here.')
          : !canRun() ? t('projects_live_off', 'Not while One World is set to Live.') : '';
        const runOff = held || lost || foreign || !canRun();
        let b;
        if (!job.finished) b = button('projects_resume', 'Resume', runOff, () => go(false));
        else {
          // Rebuilding replaces what is built: it runs only when held, as the cell menu's Rebuild.
          b = button('projects_rebuild', 'Rebuild', runOff, () => {});
          b.classList.add('projects-hold');
          b.title = t('cell_hold', 'Hold to confirm');
          globalThis.arnisHold(b, () => go(true), 1500);
        }
        if (why) b.title = why;
        acts.push(b);
      }
      acts.push(button('projects_open', 'Open', held, () => pick(w.name, false)));
      acts.push(button('projects_expand', 'Expand', held, () => pick(w.name, true)));
      if (off) acts.slice(-2).forEach((b) => { b.title = t('one_world_desc_on', 'Each area joins one growing world at its real position. Java only.'); });
      const actions = document.createElement('div');
      actions.className = 'projects-actions';
      actions.append(...acts);
      row.append(...kids, actions);
      return row;
    });
    if (!rows.length) rows.push(text('projects-empty', t('projects_empty', 'No worlds yet. Build one with One World on.')));
    list.replaceChildren(...rows);
    fillPlaces(worlds);
  }
  return refresh;
}

// Lowest / Highest Y: what is typed is snapped to a height the world can have (sections are 16 tall).
function snapY(id, lo, hi, off) {
  const el = typeof document !== 'undefined' && document.getElementById && document.getElementById(id);
  if (!el || !el.addEventListener) return;
  el.addEventListener('change', () => {
    const v = parseInt(el.value, 10);
    const s = Number.isFinite(v) ? String(Math.min(hi, Math.max(lo, Math.round((v - off) / 16) * 16 + off))) : '';
    if (s === el.value) return;
    el.value = s;
    el.dispatchEvent(new Event('change', { bubbles: true }));
  });
}
snapY('meld-min-y-input', -2032, -64, 0);
snapY('meld-max-y-input', 319, 2031, -1);
