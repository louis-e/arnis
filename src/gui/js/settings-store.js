// Persistence, per-setting revert and global reset for the Settings modal.
// Values live in one localStorage blob. The preferences that already had their
// own keys (language, map theme, world format, Luanti, the two save paths,
// telemetry) keep them and are only listed here for revert and reset.
// Defaults come from the HTML attributes, which JS assignments never change,
// so there is no ordering hazard with initSettings().
// Restore and revert always write through the control and dispatch real
// events, so the existing side-effect handlers run unchanged.

const STORAGE_KEY = 'arnis-settings';

// Bump only when stored values need migrating, not when settings are added.
const SCHEMA_VERSION = 1;

const OWN = 'own'; // stored by this module
const EXTERNAL = 'external'; // stored elsewhere, revert/reset only
const NONE = 'none'; // never stored, revert only

// Every user-changeable control in the Settings modal, in DOM order.
// kind picks how the value is read, compared and written.
const SETTINGS = [
  // Generation
  { id: 'generation-mode-select', kind: 'select', store: OWN },
  { id: 'overture-toggle', kind: 'checkbox', store: OWN },
  { id: 'use-3d-toggle', kind: 'checkbox', store: OWN },
  { id: 'interior-toggle', kind: 'checkbox', store: OWN },
  { id: 'fillground-toggle', kind: 'checkbox', store: OWN },
  { id: 'canopy-height-toggle', kind: 'checkbox', store: OWN },
  { id: 'max-tree-size-group', kind: 'segmented', store: OWN, valueAttr: 'data-max-tree-size' },
  { id: 'legacy-trees-toggle', kind: 'checkbox', store: OWN },

  // World
  { id: 'gamemode-group', kind: 'segmented', store: OWN, valueAttr: 'data-gamemode' },
  { id: 'world-time-slider', kind: 'number', store: OWN },
  { id: 'map-item-toggle', kind: 'checkbox', store: OWN },
  { id: 'signage-group', kind: 'segmented', store: OWN, valueAttr: 'data-signage' },
  { id: 'disable-height-limit-toggle', kind: 'checkbox', store: OWN },
  { id: 'aws-only-elevation-toggle', kind: 'checkbox', store: OWN },
  { id: 'bake-lighting-toggle', kind: 'checkbox', store: OWN },
  { id: 'scale-value-slider', kind: 'number', store: OWN },

  // Not persisted: picking a bbox force-resets the angle to 0 anyway.
  { id: 'rotation-angle-input', kind: 'number', store: NONE },

  { id: 'enable-luanti-toggle', kind: 'checkbox', store: EXTERNAL },

  // Map & Input. #bbox-coords is the area selection, not a preference, so it
  // is intentionally absent.
  { id: 'tile-theme-select', kind: 'select', store: EXTERNAL },

  // Application
  { id: 'save-path-input', kind: 'text', store: EXTERNAL, dynamicDefault: 'savePath' },
  { id: 'bedrock-save-path-input', kind: 'text', store: EXTERNAL, dynamicDefault: 'bedrockSavePath' },
  { id: 'language-select', kind: 'select', store: EXTERNAL, dynamicDefault: 'language' },

  // Consent record, not a preference. The user answered it on first run and the
  // HTML default is off, so a revert or reset could only withdraw consent, which
  // is not their initial choice. No revert button, and reset leaves it alone.
  { id: 'telemetry-toggle', kind: 'checkbox', store: EXTERNAL, revertable: false },
];

// Defaults the host resolves at runtime rather than from a DOM attribute.
const dynamicDefaults = Object.create(null);

let hooks = {};

// Set while restore/revert/reset writes, to suppress re-entrant saves.
let applying = false;

// Set when writing back would destroy a newer build's blob, or after storage
// has already refused a write. Persistence then lasts only for this run.
let writesSuppressed = false;

let saveTimer = null;

/* localStorage access */

// Any failure falls back to defaults rather than throwing during startup.
function readStored() {
  let raw;
  try {
    raw = localStorage.getItem(STORAGE_KEY);
  } catch (_) {
    return {};
  }
  if (!raw) return {};

  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (_) {
    console.warn('Stored Arnis settings are not valid JSON, ignoring them.');
    clearStored();
    return {};
  }

  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {};
  if (parsed.version !== SCHEMA_VERSION) {
    if (typeof parsed.version === 'number' && parsed.version > SCHEMA_VERSION) {
      // A newer build wrote this. Leave it alone so upgrading again restores it.
      console.warn('Arnis settings were written by a newer version, using defaults.');
      writesSuppressed = true;
      return {};
    }
    // No older schema exists yet, so anything else here is unusable.
    clearStored();
    return {};
  }

  const values = parsed.values;
  if (!values || typeof values !== 'object' || Array.isArray(values)) return {};
  return values;
}

function writeStored(values) {
  if (writesSuppressed) return;
  try {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({ version: SCHEMA_VERSION, values })
    );
  } catch (error) {
    // Quota or blocked storage. Stop retrying so a slider drag does not throw
    // once per pixel.
    writesSuppressed = true;
    console.warn('Could not persist Arnis settings:', error);
  }
}

function clearStored() {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch (_) {
    // Nothing sensible to do.
  }
}

/* Reading and writing a control */

function elementFor(entry) {
  return document.getElementById(entry.id);
}

function currentValue(entry) {
  const el = elementFor(entry);
  if (!el) return undefined;

  switch (entry.kind) {
    case 'checkbox':
      return el.checked;
    case 'number': {
      const n = parseFloat(el.value);
      return Number.isFinite(n) ? n : undefined;
    }
    case 'segmented': {
      const active = el.querySelector('.segment.active');
      return active ? active.getAttribute(entry.valueAttr) : undefined;
    }
    default:
      return el.value;
  }
}

function defaultValue(entry) {
  const el = elementFor(entry);
  if (!el) return undefined;

  if (entry.dynamicDefault) {
    // Unresolved means unknown, so the revert button stays hidden rather than
    // offering a wrong target.
    const resolved = dynamicDefaults[entry.dynamicDefault];
    return resolved === undefined ? undefined : resolved;
  }

  switch (entry.kind) {
    case 'checkbox':
      return el.defaultChecked;
    case 'number': {
      const n = parseFloat(el.defaultValue);
      return Number.isFinite(n) ? n : undefined;
    }
    case 'segmented': {
      const marked = el.querySelector('.segment[data-default]');
      const fallback = el.querySelector('.segment');
      const seg = marked || fallback;
      return seg ? seg.getAttribute(entry.valueAttr) : undefined;
    }
    case 'select': {
      const selected = Array.from(el.options).find((o) => o.defaultSelected);
      const opt = selected || el.options[0];
      return opt ? opt.value : undefined;
    }
    default:
      return el.defaultValue;
  }
}

// Returns undefined when the stored value is unusable, so the caller keeps the
// default. This is what covers a range or option list changing between versions.
function sanitize(entry, value) {
  const el = elementFor(entry);
  if (!el || value === undefined || value === null) return undefined;

  switch (entry.kind) {
    case 'checkbox':
      return typeof value === 'boolean' ? value : undefined;

    case 'number': {
      const n = typeof value === 'number' ? value : parseFloat(value);
      if (!Number.isFinite(n)) return undefined;
      const min = parseFloat(el.min);
      const max = parseFloat(el.max);
      if (Number.isFinite(min) && n < min) return undefined;
      if (Number.isFinite(max) && n > max) return undefined;
      return n;
    }

    case 'select': {
      if (typeof value !== 'string') return undefined;
      const known = Array.from(el.options).some((o) => o.value === value);
      return known ? value : undefined;
    }

    case 'segmented': {
      if (typeof value !== 'string') return undefined;
      const known = el.querySelector(
        `.segment[data-gamemode="${CSS.escape(value)}"]`
      );
      return known ? value : undefined;
    }

    default:
      return typeof value === 'string' ? value : undefined;
  }
}

function writeValue(entry, value) {
  const el = elementFor(entry);
  if (!el || value === undefined) return;

  if (entry.kind === 'segmented') {
    // The group's click handler owns the active class, so click it.
    const seg = el.querySelector(
      `.segment[${entry.valueAttr}="${CSS.escape(String(value))}"]`
    );
    if (seg && !seg.classList.contains('active')) seg.click();
    return;
  }

  if (entry.kind === 'checkbox') {
    if (el.checked === value) return;
    el.checked = value;
  } else {
    const next = String(value);
    if (el.value === next) return;
    el.value = next;
  }

  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
}

function isModified(entry) {
  const current = currentValue(entry);
  const def = defaultValue(entry);
  if (current === undefined || def === undefined) return false;
  if (entry.kind === 'number') return Math.abs(current - def) > 1e-9;
  return current !== def;
}

/* Persistence */

// Only stores what differs from the default, so a future change to a default
// still reaches users who never touched that setting.
function collectOwn() {
  const values = {};
  for (const entry of SETTINGS) {
    if (entry.store !== OWN) continue;
    const value = currentValue(entry);
    if (value === undefined) continue;
    const def = defaultValue(entry);
    if (def !== undefined) {
      if (entry.kind === 'number') {
        if (Math.abs(value - def) <= 1e-9) continue;
      } else if (value === def) {
        continue;
      }
    }
    values[entry.id] = value;
  }
  return values;
}

// Debounced, since dragging a slider fires input on every pixel.
function scheduleSave() {
  if (applying) return;
  if (saveTimer !== null) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    saveTimer = null;
    writeStored(collectOwn());
  }, 200);
}

function flushSave() {
  if (saveTimer === null) return;
  clearTimeout(saveTimer);
  saveTimer = null;
  writeStored(collectOwn());
}

function restore() {
  const stored = readStored();
  applying = true;
  try {
    for (const entry of SETTINGS) {
      if (entry.store !== OWN) continue;
      if (!Object.prototype.hasOwnProperty.call(stored, entry.id)) continue;
      const value = sanitize(entry, stored[entry.id]);
      if (value === undefined) continue;
      writeValue(entry, value);
    }
  } finally {
    applying = false;
  }
}

/* Revert and reset */

function revert(entry) {
  const def = defaultValue(entry);
  if (def === undefined) return;
  writeValue(entry, def);
  scheduleSave();
  refresh();
}

function resetAll() {
  // Drop a pending write, or it would fire after clearStored and re-create it.
  if (saveTimer !== null) {
    clearTimeout(saveTimer);
    saveTimer = null;
  }

  applying = true;
  try {
    for (const entry of SETTINGS) {
      if (entry.revertable === false) continue;
      const def = defaultValue(entry);
      if (def === undefined) continue;
      writeValue(entry, def);
    }
  } finally {
    applying = false;
  }

  // Runs even when writes are suppressed: discarding a newer build's blob is
  // what the user just asked for.
  clearStored();
  writesSuppressed = false;

  // The world format toggle is on the main screen, so it has no registry entry.
  if (typeof hooks.resetWorldFormat === 'function') {
    try {
      hooks.resetWorldFormat();
    } catch (error) {
      console.error('Failed to reset world format:', error);
    }
  }

  refresh();
}

/* UI */

// Counter-clockwise circular arrow. The viewBox is cropped to the path's ink
// box so the glyph fills the button and matches the 16px tooltip icon.
const REVERT_ICON =
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="4 4 16 16" aria-hidden="true">' +
  '<path transform="scale(-1,1) translate(-24,0)" d="M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-8 8s3.57 8 8 8c3.73 0 6.84-2.55 7.73-6h-2.08c-.82 2.33-3.04 4-5.65 4-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z"/>' +
  '</svg>';

// Appends a revert button to each row's label, right after the tooltip icon.
// Nesting it in the label is safe: the HTML spec gives a label no activation
// behaviour for interactive descendants, so this does not toggle the checkbox.
function injectRevertButtons(localization) {
  for (const entry of SETTINGS) {
    if (entry.revertable === false) continue;
    const el = elementFor(entry);
    if (!el) continue;

    const row = el.closest('.settings-row');
    if (!row) continue;

    const label = row.querySelector(':scope > label');
    if (!label || label.querySelector(':scope > .setting-revert')) continue;

    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'setting-revert';
    button.dataset.settingId = entry.id;
    button.innerHTML = REVERT_ICON;
    // Hidden buttons must not be keyboard reachable. Toggled in refresh().
    button.tabIndex = -1;
    button.setAttribute('aria-hidden', 'true');
    applyRevertLabel(button, localization);

    button.addEventListener('click', (event) => {
      event.preventDefault();
      revert(entry);
    });

    label.appendChild(button);
  }
}

function localizedString(localization, key, fallback) {
  if (localization && typeof localization[key] === 'string' && localization[key]) {
    return localization[key];
  }
  return fallback;
}

function applyRevertLabel(button, localization) {
  const text = localizedString(localization, 'reset_to_default', 'Reset to default');
  button.title = text;
  button.setAttribute('aria-label', text);
}

// Updates the modified markers and the reset button. A couple of dozen DOM
// reads, so it is fine to run on every input event.
function refresh() {
  let anyModified = false;

  for (const entry of SETTINGS) {
    if (entry.revertable === false) continue;
    const el = elementFor(entry);
    if (!el) continue;
    const row = el.closest('.settings-row');
    if (!row) continue;

    // A greyed out control must not advertise an action. Its stored value is
    // still restored and the button returns once it is enabled again.
    const modified = !el.disabled && isModified(entry);
    if (modified) anyModified = true;

    row.classList.toggle('is-modified', modified);

    const button = row.querySelector('.setting-revert');
    if (button) {
      button.tabIndex = modified ? 0 : -1;
      button.setAttribute('aria-hidden', modified ? 'false' : 'true');
    }
  }

  const resetButton = document.getElementById('reset-settings-button');
  if (resetButton) {
    resetButton.disabled = !anyModified;
    if (!anyModified) cancelResetConfirm();
  }
}

/* Global reset button, with an inline two-step confirmation */

let confirmTimer = null;

function cancelResetConfirm() {
  const button = document.getElementById('reset-settings-button');
  if (confirmTimer !== null) {
    clearTimeout(confirmTimer);
    confirmTimer = null;
  }
  if (!button) return;
  button.classList.remove('is-confirming');
  button.textContent = button.dataset.idleLabel || 'Reset';
}

function initResetButton(localization) {
  const button = document.getElementById('reset-settings-button');
  if (!button) return;

  applyResetLabels(button, localization);
  button.textContent = button.dataset.idleLabel;

  button.addEventListener('click', () => {
    if (button.classList.contains('is-confirming')) {
      cancelResetConfirm();
      resetAll();
      return;
    }
    // Ask once, so a stray click cannot wipe every setting. Self-cancels.
    button.classList.add('is-confirming');
    button.textContent = button.dataset.confirmLabel;
    confirmTimer = setTimeout(cancelResetConfirm, 4000);
  });

  // Focus stays on the button after the first click, so this runs before the
  // document Escape handler. Backing out must not also close the panel.
  button.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && button.classList.contains('is-confirming')) {
      cancelResetConfirm();
      event.stopPropagation();
    }
  });
}

// The row label says what this resets, so the button stays short in every locale.
function applyResetLabels(button, localization) {
  button.dataset.idleLabel = localizedString(
    localization,
    'reset_all_settings_button',
    'Reset'
  );
  button.dataset.confirmLabel = localizedString(
    localization,
    'reset_all_settings_confirm',
    'Confirm?'
  );
  if (!button.classList.contains('is-confirming')) {
    button.textContent = button.dataset.idleLabel;
  } else {
    button.textContent = button.dataset.confirmLabel;
  }
}

/* Public API */

// Must run after initSettings(), so the slider label and rotation preview
// handlers are attached before restore() dispatches its events.
export function initSettingsStore(options = {}) {
  hooks = options || {};

  injectRevertButtons(options.localization);
  initResetButton(options.localization);
  restore();

  const scrollable = document.querySelector('.settings-scrollable');
  if (scrollable) {
    const onEdit = () => {
      scheduleSave();
      refresh();
    };
    scrollable.addEventListener('input', onEdit);
    scrollable.addEventListener('change', onEdit);
    // The game mode group is buttons, so it never fires input or change.
    scrollable.addEventListener('click', (event) => {
      if (event.target.closest('.segment')) onEdit();
    });
  }

  window.addEventListener('beforeunload', flushSave);
  window.addEventListener('pagehide', flushSave);

  refresh();
}

// Supplies a default the host had to detect at runtime.
export function setDynamicDefault(name, value) {
  dynamicDefaults[name] = value;
  refresh();
}

// Call after changing a control from code without dispatching events.
export function refreshSettingsState() {
  refresh();
}

export function localizeSettingsStore(localization) {
  document.querySelectorAll('.setting-revert').forEach((button) => {
    applyRevertLabel(button, localization);
  });
  const resetButton = document.getElementById('reset-settings-button');
  if (resetButton) applyResetLabels(resetButton, localization);
}

export function cancelSettingsResetConfirm() {
  cancelResetConfirm();
}

// Called when the panel closes, which is more reliable than webview teardown.
export function flushSettingsStore() {
  flushSave();
}
