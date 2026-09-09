import { licenseText } from './license.js';
import { fetchLanguage, invalidJSON } from './language.js';
import { renderMarkdown, pickAssetForPlatform } from './update.js';
import {
  initSettingsStore,
  setDynamicDefault,
  refreshSettingsState,
  localizeSettingsStore,
  cancelSettingsResetConfirm,
  flushSettingsStore,
} from './settings-store.js';

let invoke;
if (window.__TAURI__) {
  invoke = window.__TAURI__.core.invoke;
} else {
  function dummyFunc() { }
  window.__TAURI__ = { event: { listen: dummyFunc } };
  invoke = dummyFunc;
}

const DEFAULT_LOCALE_PATH = `./locales/en.json`;

// Track current bbox selection info localization key for language changes
let currentBboxSelectionKey = "select_area_prompt";
let currentBboxSelectionColor = "#ffffff";

// Helper function to set bbox selection info text and track it for language changes
async function setBboxSelectionInfo(bboxSelectionElement, localizationKey, color) {
  currentBboxSelectionKey = localizationKey;
  currentBboxSelectionColor = color;
  
  // Ensure localization is available
  let localization = window.localization;
  if (!localization) {
    localization = await getLocalization();
  }
  
  localizeElement(localization, { element: bboxSelectionElement }, localizationKey);
  bboxSelectionElement.style.color = color;
}

// Initialize elements and start the demo progress
window.addEventListener("DOMContentLoaded", async () => {
  registerMessageEvent();
  window.startGeneration = startGeneration;
  setupProgressListener();
  await initSavePath();
  initSettings();
  initVoxyLightingCoupling();
  initHeightLimitNote();
  // After initSettings(), so the slider label and rotation handlers exist
  // before restored values are applied. Labels get localized a few lines below.
  initSettingsStore({ resetWorldFormat: () => setWorldFormat('java') });
  resolveDefaultSavePath();
  initTelemetryConsent();
  initClearCacheButton();
  initPrecomputeFacadesButton();
  initTooltips();
  handleBboxInput();
  const localization = await getLocalization();
  await applyLocalization(localization);
  updateFormatToggleUI(selectedWorldFormat);
  initFooter();
  initEasterEggs();
  checkForUpdates();
});

// Expose language functions to window for use by language-selector.js
window.fetchLanguage = fetchLanguage;
window.applyLocalization = applyLocalization;
window.initFooter = initFooter;

/**
 * Fetches and returns localization data based on user's language
 * Falls back to English if requested language is not available
 * @returns {Promise<Object>} The localization JSON object
 */
async function getLocalization() {
  // Check if user has a saved language preference
  const savedLanguage = localStorage.getItem('arnis-language');

  // If there's a saved preference, use it
  if (savedLanguage) {
    return await fetchLanguage(savedLanguage);
  }

  // Otherwise use the browser's language
  const lang = navigator.language;
  return await fetchLanguage(lang);
}

/**
 * Updates an HTML element with localized text
 * @param {Object} json - Localization data
 * @param {Object} elementObject - Object containing element or selector
 * @param {string} localizedStringKey - Key for the localized string
 */
async function localizeElement(json, elementObject, localizedStringKey) {
  const element =
    (!elementObject.element || elementObject.element === "")
      ? document.querySelector(elementObject.selector) : elementObject.element;
  const attribute = localizedStringKey.startsWith("placeholder_") ? "placeholder" : "textContent";

  if (element) {
    if (json && localizedStringKey in json) {
      element[attribute] = json[localizedStringKey];
    } else {
      // Fallback to default (English) string
      const defaultJson = await fetchLanguage('en');
      element[attribute] = defaultJson[localizedStringKey];
    }
  }
}

async function applyLocalization(localization) {
  const localizationElements = {
    "#start-button > span[data-localize='start_generation']": "start_generation",
    "#world-name-label[data-placeholder]": "no_world_generated_yet",
    "h2[data-localize='customization_settings']": "customization_settings",
    "span[data-localize='world_scale']": "world_scale",
    "span[data-localize='world_scale_objects_skipped']": "world_scale_objects_skipped",
    "span[data-localize='custom_bounding_box']": "custom_bounding_box",
    // DEPRECATED: Ground level localization removed
    // "label[data-localize='ground_level']": "ground_level",
    "span[data-localize='language']": "language",
    "span[data-localize='generation_mode']": "generation_mode",
    "option[data-localize='mode_geo_terrain']": "mode_geo_terrain",
    "option[data-localize='mode_geo_only']": "mode_geo_only",
    "option[data-localize='mode_terrain_only']": "mode_terrain_only",
    "span[data-localize='terrain']": "terrain",
    "span[data-localize='interior']": "interior",
    "span[data-localize='fillground']": "fillground",
    "span[data-localize='legacy_trees']": "legacy_trees",
    "span[data-localize='overture']": "overture",
    "span[data-localize='three_dmr']": "three_dmr",
    "span[data-localize='disable_height_limit']": "disable_height_limit",
    "span[data-localize='aws_only_elevation']": "aws_only_elevation",
    "span[data-localize='bake_lighting']": "bake_lighting",
    "span[data-localize='voxy_lod']": "voxy_lod",
    "span[data-localize='anonymous_crash_reports']": "anonymous_crash_reports",
    "span[data-localize='map_theme']": "map_theme",
    "span[data-localize='custom_map_source']": "custom_map_source",
    "span[data-localize='java_save_path']": "java_save_path",
    "span[data-localize='bedrock_save_path']": "bedrock_save_path",
    "span[data-localize='luanti_save_path']": "luanti_save_path",
    "span[data-localize='rotation_angle']": "rotation_angle",
    "span[data-localize='canopy_height']": "canopy_height",
    "span[data-localize='max_tree_size']": "max_tree_size",
    "button[data-localize='tree_size_small']": "tree_size_small",
    "button[data-localize='tree_size_medium']": "tree_size_medium",
    "button[data-localize='tree_size_big']": "tree_size_big",
    "button[data-localize='tree_size_tall']": "tree_size_tall",
    "button[data-localize='tree_size_giant']": "tree_size_giant",
    "span[data-localize='gamemode']": "gamemode",
    "button[data-localize='gamemode_survival']": "gamemode_survival",
    "button[data-localize='gamemode_creative']": "gamemode_creative",
    "button[data-localize='gamemode_spectator']": "gamemode_spectator",
    "span[data-localize='world_time']": "world_time",
    "span[data-localize='map_item']": "map_item",
    "span[data-localize='signage']": "signage",
    "span[data-localize='mapillary_token']": "mapillary_token",
    "span[data-localize='facade_precompute']": "facade_precompute",
    "span[data-localize='facade_mode']": "facade_mode",
    "button[data-localize='facade_mode_blocks']": "facade_mode_blocks",
    "button[data-localize='facade_mode_paintings']": "facade_mode_paintings",
    "button[data-localize='facade_mode_paintings_v2']": "facade_mode_paintings_v2",
    "div[data-localize='facade_mode_java_only']": "facade_mode_java_only",
    "button[data-localize='signage_none']": "signage_none",
    "button[data-localize='signage_basic']": "signage_basic",
    "button[data-localize='signage_full']": "signage_full",
    "div[data-localize='settings_section_generation']": "settings_section_generation",
    "div[data-localize='settings_section_facades']": "settings_section_facades",
    "span[data-localize='facade_source']": "facade_source",
    "span[data-localize='facade_detail']": "facade_detail",
    "button[data-localize='facade_detail_standard']": "facade_detail_standard",
    "button[data-localize='facade_detail_high']": "facade_detail_high",
    "button[data-localize='facade_source_off']": "facade_source_off",
    "button[data-localize='facade_source_preset']": "facade_source_preset",
    "button[data-localize='facade_source_mapillary']": "facade_source_mapillary",
    "span[data-localize='enable_luanti']": "enable_luanti",
    "div[data-localize='settings_section_world']": "settings_section_world",
    "div[data-localize='settings_section_map']": "settings_section_map",
    "div[data-localize='settings_section_application']": "settings_section_application",
    "button[data-localize='facade_precompute_button']": "facade_precompute_button",
    "span[data-localize='clear_tile_cache']": "clear_tile_cache",
    "button[data-localize='clear_tile_cache_button']": "clear_tile_cache_button",
    // Row label only; settings-store.js owns the button text.
    "span[data-localize='reset_all_settings']": "reset_all_settings",
    ".footer-link": "footer_text",
    "button[data-localize='license_and_credits']": "license_and_credits",
    "h2[data-localize='license_and_credits']": "license_and_credits",
    "button[data-localize='version_info']": "version_info",
    "h2[data-localize='update_modal_title']": "update_modal_title",
    "div[data-localize='update_modal_download_note']": "update_modal_download_note",
    "button[data-localize='update_view_on_github']": "update_view_on_github",
    "button[data-localize='update_download']": "update_download",

    // Placeholder strings
    "input[id='bbox-coords']": "placeholder_bbox",
    // DEPRECATED: Ground level placeholder removed
    // "input[id='ground-level']": "placeholder_ground"
  };

  for (const selector in localizationElements) {
    localizeElement(localization, { selector: selector }, localizationElements[selector]);
  }

  // settings-store.js creates these buttons and owns their text.
  localizeSettingsStore(localization);

  // Re-apply current bbox selection info text with new language
  const bboxSelectionInfo = document.getElementById("bbox-selection-info");
  if (bboxSelectionInfo && currentBboxSelectionKey) {
    localizeElement(localization, { element: bboxSelectionInfo }, currentBboxSelectionKey);
    bboxSelectionInfo.style.color = currentBboxSelectionColor;
  }

  // Update error messages
  window.localization = localization;

  // The line above has just written the idle label over a button that may be
  // saying Cancel, so put the running state back.
  refreshPrecomputeButton();
}

// Function to initialize the footer with the current year and version
async function initFooter() {
  const currentYear = new Date().getFullYear();
  let version = "x.x.x";

  try {
    version = await invoke('gui_get_version');
  } catch (error) {
    console.error("Failed to fetch version:", error);
  }

  const footerElement = document.querySelector(".footer-link");
  if (footerElement) {
    // Get the original text from localization if available, or use the current text
    let footerText = footerElement.textContent;

    // Check if the text is from localization and contains placeholders
    if (window.localization && window.localization.footer_text) {
      footerText = window.localization.footer_text;
    }

    // Replace placeholders with actual values
    footerElement.textContent = footerText
      .replace("{year}", currentYear)
      .replace("{version}", version);
  }
}

let latestReleaseInfo = null;
let currentPlatform = "unknown";

const SEEN_VERSION_KEY = "arnis-update-seen-version";

// Only forward http(s)/mailto URLs to the OS handler; reject javascript:/data:/file:/etc.
function isSafeExternalUrl(url) {
  return typeof url === "string" && /^(?:https?|mailto):/i.test(url);
}

async function openExternal(url) {
  if (!isSafeExternalUrl(url)) {
    console.warn("Refusing to open URL with disallowed scheme:", url);
    return;
  }
  try {
    if (window.__TAURI__ && window.__TAURI__.shell && window.__TAURI__.shell.open) {
      await window.__TAURI__.shell.open(url);
    } else {
      window.open(url, "_blank", "noopener,noreferrer");
    }
  } catch (err) {
    console.error("Failed to open URL:", url, err);
  }
}

async function checkForUpdates() {
  try {
    const [info, platform] = await Promise.all([
      invoke("gui_get_update_info"),
      invoke("gui_get_platform"),
    ]);
    latestReleaseInfo = info;
    currentPlatform = platform || "unknown";
    if (!info || !info.isNewer) return;

    const footer = document.querySelector(".footer");
    const updateMessage = document.createElement("span");
    updateMessage.setAttribute("role", "button");
    updateMessage.setAttribute("tabindex", "0");
    updateMessage.style.color = "#fecc44";
    updateMessage.style.marginTop = "-5px";
    updateMessage.style.fontSize = "0.95em";
    updateMessage.style.display = "block";
    updateMessage.style.cursor = "pointer";
    updateMessage.addEventListener("click", () => openUpdateModal());
    updateMessage.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        openUpdateModal();
      }
    });
    localizeElement(window.localization, { element: updateMessage }, "new_version_available");
    footer.style.marginTop = "10px";
    footer.appendChild(updateMessage);

    // Auto-open once per new remote version; "seen" key records the last auto-shown version.
    const seen = localStorage.getItem(SEEN_VERSION_KEY);
    if (seen !== info.remoteVersion) {
      openUpdateModal();
    }
  } catch (error) {
    console.error("Failed to check for updates: ", error);
  }
}

function openUpdateModal(opts = {}) {
  const showDownload = opts.showDownload !== false;
  const modal = document.getElementById("update-modal");
  if (!modal) return;
  const titleEl = document.getElementById("update-modal-title");
  const bodyEl = document.getElementById("update-modal-body");
  const downloadBtn = document.getElementById("update-download-button");
  const downloadNote = document.getElementById("update-modal-download-note");

  // Hide download button + the "opens in your browser" note in version-info mode.
  if (downloadBtn) downloadBtn.style.display = showDownload ? "" : "none";
  if (downloadNote) downloadNote.style.display = showDownload ? "" : "none";

  const info = latestReleaseInfo;
  if (!info || !info.release) {
    const fallbackMsg =
      (window.localization && window.localization.update_fetch_failed) ||
      "Could not fetch the latest release information. Please check your internet connection or visit GitHub directly.";
    bodyEl.innerHTML = `<p>${escapeHTMLLite(fallbackMsg)}</p>`;
    if (downloadBtn) downloadBtn.disabled = true;
    showModal(modal);
    return;
  }
  const rel = info.release;

  titleEl.textContent = (rel.name && rel.name.trim()) || rel.tag_name || "Latest release";
  bodyEl.innerHTML = renderMarkdown(rel.body || "");
  bodyEl.querySelectorAll("a[href]").forEach((a) => {
    const href = a.getAttribute("href");
    if (!href) return;
    a.addEventListener("click", (e) => {
      e.preventDefault();
      openExternal(href);
    });
  });

  if (downloadBtn && showDownload) {
    const asset = pickAssetForPlatform(rel.assets || [], currentPlatform);
    downloadBtn.disabled = false;
    downloadBtn.textContent =
      (window.localization && window.localization.update_download) || "Download";
    downloadBtn.dataset.downloadUrl = asset ? asset.browser_download_url : rel.html_url;
  }

  // "Seen" gate only applies to auto-open of a newer release, not manual version-info clicks.
  if (info.isNewer && showDownload) {
    try { localStorage.setItem(SEEN_VERSION_KEY, info.remoteVersion); } catch (_) {}
  }

  showModal(modal);
}

async function openVersionInfoModal() {
  if (!latestReleaseInfo) {
    try {
      latestReleaseInfo = await invoke("gui_get_update_info");
    } catch (e) {
      console.error("Failed to fetch release info:", e);
    }
  }
  openUpdateModal({ showDownload: false });
}

function closeUpdateModal() {
  const modal = document.getElementById("update-modal");
  if (modal) modal.style.display = "none";
}

function showModal(modal) {
  modal.style.display = "flex";
  modal.style.justifyContent = "center";
  modal.style.alignItems = "center";
}

function openUpdateInBrowser() {
  const url =
    (latestReleaseInfo && latestReleaseInfo.release && latestReleaseInfo.release.html_url) ||
    "https://github.com/louis-e/arnis/releases";
  openExternal(url);
}

function downloadLatestRelease() {
  const btn = document.getElementById("update-download-button");
  const url = btn && btn.dataset.downloadUrl;
  if (!url) return;
  openExternal(url);
}

function escapeHTMLLite(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

window.openUpdateModal = openUpdateModal;
window.openVersionInfoModal = openVersionInfoModal;
window.closeUpdateModal = closeUpdateModal;
window.openUpdateInBrowser = openUpdateInBrowser;
window.downloadLatestRelease = downloadLatestRelease;

// Earth on every start, so a Moon or Mars world stays a deliberate pick.
var selectedCelestialBody = 'earth';

// Earth-only options. Disabled rather than hidden, so it does not look like
// they were silently ignored.
const EARTH_ONLY_SETTINGS = [
  'generation-mode-select',
  'overture-toggle',
  'use-3d-toggle',
  'interior-toggle',
  'canopy-height-toggle',
  'legacy-trees-toggle',
  'scale-value-slider',
  'aws-only-elevation-toggle',
  'disable-height-limit-toggle',
  // Off Earth the body's own imagery is the only basemap, so neither the
  // Earth theme picker nor a custom Earth tile source has anything to act on.
  'tile-theme-select',
  'custom-tile-url'
];
const EARTH_ONLY_SEGMENTED = ['max-tree-size-group', 'signage-group'];

// A tile URL template Leaflet can actually fill in. Deliberately permissive
// about the host - the entire point is to reach something we do not know about -
// but the placeholders have to be there or every tile 404s.
function isValidTileTemplate(url) {
  if (!/^https?:\/\//i.test(url)) return false;
  return url.includes('{z}') && url.includes('{x}') && url.includes('{y}');
}

function getCustomTileUrl() {
  return (localStorage.getItem('customTileUrl') || '').trim();
}

function getMapillaryToken() {
  return (localStorage.getItem('mapillaryToken') || '').trim();
}

function getFacadeMode() {
  // Only the modes that exist; a value left behind by an older build falls
  // back to blocks, the same way the backend reads it.
  const stored = localStorage.getItem('facadeMode');
  return ['paintings', 'paintings-v2'].includes(stored) ? stored : 'blocks';
}

// Both panel modes hang Java entities carried by a resource pack, so no other
// world format can show them. The stored choice is left alone so that going
// back to Java restores it; only what the backend is asked for changes.
function getEffectiveFacadeMode() {
  const mode = getFacadeMode();
  if (mode !== 'blocks' && selectedWorldFormat !== 'java') return 'blocks';
  return mode;
}

// One control decides where facades come from, so the two old switches are
// gone: they were mutually exclusive anyway and could both be off in two
// different ways. Java only, and the stored choice survives a trip through
// Bedrock so coming back restores it.
function getFacadeSource() {
  const v = localStorage.getItem('facadeSource');
  return v === 'preset' || v === 'mapillary' ? v : 'off';
}

function getEffectiveFacadeSource() {
  return selectedWorldFormat === 'java' ? getFacadeSource() : 'off';
}

function getFacadesEnabled() {
  return getEffectiveFacadeSource() === 'mapillary';
}

function getBuildingFacadesEnabled() {
  return getEffectiveFacadeSource() === 'preset';
}

function getFacadeDetail() {
  return localStorage.getItem('facadeDetail') === 'high' ? 'high' : 'standard';
}

// Every row under the source control only means something for one of the
// sources, so each is greyed by the source rather than by a switch of its own.
function refreshFacadeSourceRows() {
  const group = document.getElementById('facade-source-group');
  if (!group) return;
  const java = selectedWorldFormat === 'java';
  const source = getEffectiveFacadeSource();

  group.querySelectorAll('.segment').forEach((btn) => {
    btn.disabled = !java && btn.dataset.facadeSource !== 'off';
    btn.classList.toggle('active', btn.dataset.facadeSource === source);
  });

  const grey = (id, live) => {
    const el = document.getElementById(id);
    const row = el && el.closest('.settings-row');
    if (row) row.classList.toggle('settings-row-unavailable', !live);
    if (el) {
      el.querySelectorAll('.segment, input, button').forEach((c) => {
        c.disabled = !live;
      });
      if (el.tagName === 'INPUT' || el.tagName === 'BUTTON') el.disabled = !live;
    }
  };
  grey('mapillary-token', source === 'mapillary');
  grey('facade-mode-group', source === 'mapillary' && !!getMapillaryToken());
  grey('facade-detail-group', source !== 'off');
  grey('precompute-facades-button', source === 'mapillary' && !!getMapillaryToken());

  const notice = document.getElementById('facade-java-only-notice');
  if (notice) notice.style.display = java ? 'none' : '';
}

// The facade rows react to the world format (panels are Java only), the on/off
// switch and the token. A row that cannot do anything is greyed out rather than
// left looking live, and the Precompute button reads the same facts.
function refreshFacadeRows() {
  const group = document.getElementById('facade-mode-group');
  const notice = document.getElementById('facade-java-only-notice');
  if (!group) return;

  const java = selectedWorldFormat === 'java';
  const effective = getEffectiveFacadeMode();
  group.querySelectorAll('.segment').forEach((btn) => {
    const panels = btn.dataset.facadeMode !== 'blocks';
    btn.disabled = panels && !java;
    btn.classList.toggle('active', btn.dataset.facadeMode === effective);
  });
  if (notice) notice.style.display = java ? 'none' : '';
  refreshFacadeSourceRows();
  refreshPrecomputeButton();
}

// The URL field is only meaningful for the Custom theme, so it is hidden
// rather than disabled for every other one.
function refreshCustomSourceRow() {
  const select = document.getElementById('tile-theme-select');
  const row = document.getElementById('custom-tile-row');
  if (!select || !row) return;
  row.style.display = select.value === 'custom' ? '' : 'none';
}

// The map iframe is addressed by class, never by its src attribute. Manual
// bbox entry used to rewrite src to "maps.html#lat,lng,lat,lng", after which
// every iframe[src="maps.html"] lookup silently matched nothing and the map
// theme and celestial body messages were dropped for the rest of the session.
function getMapFrame() {
  return document.querySelector('.map-container');
}

// The map owns the toggle, but an iframe reload restarts it on Earth, so the
// parent's value has to be pushed back.
function pushBodyToMap() {
  const mapIframe = getMapFrame();
  if (mapIframe && mapIframe.contentWindow) {
    mapIframe.contentWindow.postMessage(
      { type: 'changeBody', body: selectedCelestialBody },
      '*'
    );
  }
}

function setCelestialBody(body) {
  selectedCelestialBody = (body === 'moon' || body === 'mars') ? body : 'earth';
  const off = selectedCelestialBody !== 'earth';

  const markRow = (el) => {
    const row = el && el.closest('.settings-row');
    if (row) row.classList.toggle('settings-row-unavailable', off);
  };

  EARTH_ONLY_SETTINGS.forEach((id) => {
    const el = document.getElementById(id);
    if (!el) return;
    el.disabled = off;
    markRow(el);
  });

  EARTH_ONLY_SEGMENTED.forEach((id) => {
    const group = document.getElementById(id);
    if (!group) return;
    group.classList.toggle('segmented-disabled', off);
    markRow(group);
  });

  // Also format-gated, so it cannot simply follow the earth-only loop.
  refreshHeightLimitRow();

  const notice = document.getElementById('off-earth-notice');
  if (notice) {
    notice.style.display = off ? '' : 'none';
    document.getElementById('off-earth-notice-body').textContent =
      selectedCelestialBody === 'moon' ? 'Moon' : 'Mars';
  }

  // A default, not a lock. Slider units are clock minutes: 0 midnight, 720 noon.
  const timeSlider = document.getElementById('world-time-slider');
  if (timeSlider) {
    timeSlider.value = off ? 0 : 720;
    timeSlider.dispatchEvent(new Event('input', { bubbles: true }));
  }

  // Warnings are per body, so the current selection may read differently now.
  refreshBboxSelectionInfo();
}

// Function to register the event listener for bbox updates from iframe
function registerMessageEvent() {
  window.addEventListener('message', function (event) {
    const bboxText = event.data.bboxText;

    // Typed messages are handled below; only untyped bboxText messages are
    // selection updates (typed ones carrying coordinates must not be).
    if (bboxText && !event.data.type) {
      console.log("Updated BBOX Coordinates:", bboxText);
      displayBboxInfoText(bboxText);
    }

    // World toggled on the map toolbar
    if (event.data && event.data.type === 'bodyChanged') {
      setCelestialBody(event.data.body);
    }

    // Handle angle measurement from the map polyline tool
    if (event.data && event.data.type === 'angleMeasured') {
      var angle = event.data.angle;
      var rotationInput = document.getElementById("rotation-angle-input");
      if (rotationInput) {
        var clamped = Math.min(Math.max(angle, -90), 90);
        rotationInput.value = clamped.toFixed(2);
        // Also trigger the rotation preview update on the map
        var mapFrame = document.querySelector('.map-container');
        if (mapFrame && mapFrame.contentWindow) {
          mapFrame.contentWindow.postMessage({
            type: 'rotatePreview',
            angle: clamped
          }, '*');
        }
      }
    }
  });
}

// --- Self-calibrating, phase-aware ETA ------------------------------------
// The single 0-100% progress is non-linear in time: it has fixed phase
// breakpoints and the post-70% tail is ~instant under stream-to-disk but a real
// save otherwise. So we model three time-bands, extrapolate the CURRENT band to
// its own end from a least-squares rate, and budget the remaining bands via
// per-regime time weights calibrated to this run. The backend tells us the
// streaming regime via an optional `streaming` field (absent => non-streaming).
// Starts once generation begins (progress >= ETA_START, downloads done) and
// ticks down once a second so it reads like a live countdown.
const ETA_START = 20; // generation/terrain begins here, downloads done
const ETA_WINDOW_MS = 16000; // sliding window for the rate (wider = steadier)
const ETA_MIN_MS = 700; // min window span before trusting a rate
const ETA_MIN_SAMPLES = 4; // keep at least this many samples in the window
const ETA_STALL_MS = 1500; // progress flat this long => freeze belief, keep ticking
const ETA_MAX_S = 24 * 3600; // ignore absurd extrapolations
const ETA_A_DOWN = 0.28; // smoothing when the estimate falls (gentle)
const ETA_A_UP = 0.1; // smoothing when it rises (resist, stay calm)
const ETA_RISE_ABS = 2; // shown rises by at most max(2s, 10%) per update
const ETA_RISE_FRAC = 0.1; // => smooth, monotonic-feeling countdown

// Progress bands [lo, hi) and per-regime RELATIVE time weights (not % widths).
// Measured on Heidelberg 1/2.5/5/10 km runs (terrain + land cover + Overture):
// the finalize tail (map item, signage map tiles, world settings) is as long as
// the region write on small areas and several times longer on large ones, so it
// gets its own band rather than hiding behind the last save percent.
const ETA_PHASES = [
  { id: "terrain", lo: 20, hi: 70 },
  { id: "ground", lo: 70, hi: 90 },
  { id: "save", lo: 90, hi: 97 },
  { id: "finalize", lo: 97, hi: 100 },
];
const ETA_WPRIOR = {
  nonStreaming: [37, 2, 11, 20],
  streaming: [60, 0.3, 0.5, 3.0],
};
// Signage map tiles are what makes the finalize band long, and they are Java-only.
// Without them the tail is just the map item and level.dat settings.
const ETA_WFINALIZE_NO_SIGNAGE = 1.5;

let eta = null;
// Set from the generate handler; decides the finalize weight for the next run.
let etaSignageExpected = true;

function setEtaSignageExpected(expected) {
  etaSignageExpected = !!expected;
}

// Copy of the regime prior with the finalize weight adjusted for this run.
function etaWeightsFor(streaming) {
  const w = (streaming ? ETA_WPRIOR.streaming : ETA_WPRIOR.nonStreaming).slice();
  if (!etaSignageExpected) w[3] = ETA_WFINALIZE_NO_SIGNAGE;
  return w;
}

function etaPhaseIdx(p) {
  for (let i = 0; i < ETA_PHASES.length; i++) if (p < ETA_PHASES[i].hi) return i;
  return ETA_PHASES.length - 1;
}

// Least-squares slope of progress over the window -> %/sec (null if not rising).
function etaLsRate(s) {
  const n = s.length;
  if (n < 2) return null;
  let st = 0, sp = 0;
  for (const x of s) { st += x.t; sp += x.p; }
  const mt = st / n, mp = sp / n;
  let num = 0, den = 0;
  for (const x of s) { const d = x.t - mt; num += d * (x.p - mp); den += d * d; }
  if (den <= 0) return null;
  const slope = num / den;
  return slope > 0 ? slope * 1000 : null;
}

function resetEta() {
  if (eta && eta.tickHandle) clearInterval(eta.tickHandle);
  eta = null;
  const el = document.getElementById("progress-eta");
  if (el) {
    el.classList.remove("visible");
    el.textContent = "";
    el.removeAttribute("aria-label");
  }
}

function formatEtaDuration(sec) {
  sec = Math.max(0, Math.round(sec));
  if (sec < 60) return `${sec}s`;
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  if (m < 60) return s ? `${m}m ${s}s` : `${m}m`;
  const h = Math.floor(m / 60);
  return `${h}h ${m % 60}m`;
}

function renderEta() {
  const el = document.getElementById("progress-eta");
  if (!el) return;
  // Show nothing until we actually have an estimate (no "…" placeholder).
  if (!eta || eta.shown == null) {
    el.classList.remove("visible");
    el.textContent = "";
    el.removeAttribute("aria-label");
    return;
  }
  // Floor at 1s so it never reads "0s" before Done.
  const value = formatEtaDuration(Math.max(1, eta.shown));
  el.textContent = value; // duration only on the bar; no "~" prefix
  // Screen-reader context (the visible pill is just the duration). Hardcoded
  // English to match the rest of the progress messages, which aren't localized.
  el.setAttribute("aria-label", `Time remaining: ${value}`);
  el.classList.add("visible");
}

// Two-layer display: `est` is the belief, `shown` follows it but falls freely
// and rises slowly, so the countdown never jumps upward jarringly.
function etaReconcile() {
  if (!eta || eta.est == null) return;
  if (eta.shown == null || eta.est <= eta.shown) eta.shown = eta.est;
  else
    eta.shown = Math.min(
      eta.est,
      eta.shown + Math.max(ETA_RISE_ABS, eta.shown * ETA_RISE_FRAC)
    );
}

// Counts the value down between progress events (and through the 70% stall).
function etaTick() {
  if (!eta || eta.est == null) return;
  const now = performance.now();
  const dt = (now - eta.lastTickAt) / 1000;
  eta.lastTickAt = now;
  eta.est = Math.max(0, eta.est - dt);
  if (eta.shown != null) eta.shown = Math.max(0, eta.shown - dt);
  etaReconcile();
  renderEta();
}

function updateEta(progress, streaming) {
  // Reset before the generation phases and once finished / on a new run.
  if (progress < ETA_START || progress >= 100) {
    resetEta();
    return;
  }
  const now = performance.now();
  if (!eta) {
    eta = {
      streamingKnown: false, wprior: etaWeightsFor(false), phaseIdx: -1,
      phaseStartT: now, phaseStartProgress: null, movedInPhase: false,
      samples: [], doneSec: 0, doneWeight: 0, est: null, shown: null,
      lastTickAt: now, lastProgress: null, lastIncreaseAt: now, tickHandle: null,
    };
  }
  if (streaming != null && !eta.streamingKnown) {
    eta.streamingKnown = true;
    eta.wprior = etaWeightsFor(streaming);
  }

  const idx = etaPhaseIdx(progress), ph = ETA_PHASES[idx];
  if (idx !== eta.phaseIdx) {
    // Bank the real duration (incl. any stall) of the phase we just left.
    if (eta.phaseIdx >= 0) {
      eta.doneSec += (now - eta.phaseStartT) / 1000;
      eta.doneWeight += eta.wprior[eta.phaseIdx];
      for (let k = eta.phaseIdx + 1; k < idx; k++) eta.doneWeight += eta.wprior[k];
    }
    eta.phaseIdx = idx;
    eta.phaseStartT = now;
    eta.samples = [];
    eta.phaseStartProgress = progress;
    eta.movedInPhase = false;
  }

  if (eta.lastProgress == null || progress > eta.lastProgress) eta.lastIncreaseAt = now;
  eta.lastProgress = progress;
  const stalled = now - eta.lastIncreaseAt > ETA_STALL_MS;

  eta.samples.push({ t: now, p: progress });
  // Drop the flat phase-start hold (e.g. precompute sitting at 25%) the first
  // time progress actually moves, so the rate reflects real work, not setup.
  if (!eta.movedInPhase && eta.phaseStartProgress != null && progress > eta.phaseStartProgress) {
    eta.samples = [{ t: now, p: progress }];
    eta.movedInPhase = true;
  }
  const cut = now - ETA_WINDOW_MS;
  while (eta.samples.length > ETA_MIN_SAMPLES && eta.samples[0].t < cut) eta.samples.shift();
  const span = now - eta.samples[0].t;
  const rate = !stalled && span >= ETA_MIN_MS ? etaLsRate(eta.samples) : null;

  const phaseElapsed = (now - eta.phaseStartT) / 1000;
  let G = eta.doneWeight > 0 ? eta.doneSec / eta.doneWeight : null; // sec per prior-unit
  let remCur = null;
  if (rate) remCur = (ph.hi - progress) / rate;
  else if (G != null) remCur = ((ph.hi - progress) / (ph.hi - ph.lo)) * eta.wprior[idx] * G;
  if (G == null && remCur != null) G = (phaseElapsed + remCur) / eta.wprior[idx];
  if (G != null) G = Math.min(1000, Math.max(0.02, G));

  let raw = null;
  if (remCur != null) {
    // In the last band this is just the measured rate; earlier ones budget the rest.
    raw = Math.max(0, remCur);
    if (G != null) for (let j = idx + 1; j < ETA_PHASES.length; j++) raw += eta.wprior[j] * G;
  }

  if (!stalled && raw != null && isFinite(raw) && raw <= ETA_MAX_S) {
    const a = eta.est == null || raw <= eta.est ? ETA_A_DOWN : ETA_A_UP;
    eta.est = eta.est == null ? raw : a * raw + (1 - a) * eta.est;
    eta.lastTickAt = now;
  }
  if (eta.est != null && !eta.tickHandle) {
    eta.lastTickAt = now;
    eta.tickHandle = setInterval(etaTick, 1000);
  }
  etaReconcile();
  renderEta();
}

// Function to set up the progress bar listener
function setupProgressListener() {
  const progressBar = document.getElementById("progress-bar");
  const progressInfo = document.getElementById("progress-info");
  const progressDetail = document.getElementById("progress-detail");

  window.__TAURI__.event.listen("progress-update", (event) => {
    const { progress, message, streaming } = event.payload;

    if (progress != -1) {
      progressBar.style.width = `${progress}%`;
      progressDetail.textContent = `${Math.round(progress)}%`;
      updateEta(progress, streaming);
    }

    if (message != "") {
      progressInfo.textContent = message;

      if (message.startsWith("Error!")) {
        progressInfo.style.color = "#fa7878";
        generationButtonEnabled = true;
        window.arnisPreview3D?.setGenerationRunning(false);
        setWorldNameLabel("");
        resetEta();
        refreshPrecomputeButton();
      } else if (message.startsWith("Done!")) {
        progressInfo.style.color = "#7bd864";
        generationButtonEnabled = true;
        window.arnisPreview3D?.setGenerationRunning(false);
        resetEta();
        // A generation just built facades into the same cache, so the preview
        // and the Precompute button both have something new to say.
        window.arnisPreview3D?.refreshFacades();
        refreshPrecomputeButton();
      } else {
        progressInfo.style.color = "#ececec";
      }
      // The facade pipeline reports its stages here whichever job is driving it.
      notePrecomputeStage(message);
    }
  });

  // Listen for the finalized world name (Java adds the localized area suffix
  // during generation; Bedrock derives the name from the area up-front).
  window.__TAURI__.event.listen("world-name-update", (event) => {
    if (typeof event.payload === 'string') {
      setWorldNameLabel(event.payload);
    }
  });

  // Listen for map preview ready event from backend
  window.__TAURI__.event.listen("map-preview-ready", () => {
    console.log("Map preview ready event received");
    showWorldPreviewButton();
  });

  // Listen for show-in-folder event to reveal the generated world in the file explorer
  window.__TAURI__.event.listen("show-in-folder", async (event) => {
    const filePath = event.payload;
    try {
      await invoke("gui_show_in_folder", { path: filePath });
    } catch (error) {
      console.error("Failed to show file in folder:", error);
    }
  });
}

// Easter eggs
function showEasterEggAnimal() {
  const img = document.getElementById('secret-parrot');
  img.src = './images/parrot.gif';
  img.style.display = 'inline';
}

function initEasterEggs() {
  // 1 in 50 chance at startup
  if (Math.random() < 1 / 50) {
    showEasterEggAnimal();
  }

  // 5 rapid clicks on progress bar
  const progressBar = document.querySelector('.progress-bar-container');
  let clicks = [];
  progressBar.addEventListener('click', () => {
    const now = Date.now();
    clicks.push(now);
    clicks = clicks.filter(t => now - t < 1500);
    if (clicks.length >= 5) {
      showEasterEggAnimal();
      clicks = [];
    }
  });
}

// Language implied by the browser, ignoring any stored preference.
function detectBrowserLanguage(availableOptions) {
  const currentLang = navigator.language || 'en';
  if (availableOptions.includes(currentLang)) return currentLang;
  const base = currentLang.split('-')[0];
  if (availableOptions.includes(base)) return base;
  return 'en';
}

// Gives the settings store the save path defaults. Not awaited, since startup
// must not block on a filesystem probe; on failure the revert stays hidden.
function resolveDefaultSavePath() {
  const resolve = (command, name) => {
    Promise.resolve()
      .then(() => invoke(command))
      .then((detected) => {
        if (typeof detected === 'string' && detected) {
          setDynamicDefault(name, detected);
        }
      })
      .catch(() => {
        // No detectable default, so that row keeps no revert button.
      });
  };

  resolve('gui_get_default_save_path', 'savePath');
  resolve('gui_get_default_bedrock_save_path', 'bedrockSavePath');
  resolve('gui_get_default_luanti_save_path', 'luantiSavePath');
}

function initSettings() {
  // Settings
  const settingsModal = document.getElementById("settings-modal");
  const slider = document.getElementById("scale-value-slider");
  const sliderValue = document.getElementById("slider-value");

  // Open settings modal
  function openSettings() {
    settingsModal.style.display = "flex";
    settingsModal.style.justifyContent = "center";
    settingsModal.style.alignItems = "center";
    // The caches grow with every generation, so the number the panel shows
    // has to be read when the panel opens; measuring it once at startup left
    // it stale for the whole session.
    refreshCacheSize();
  }

  // Close settings modal
  function closeSettings() {
    settingsModal.style.display = "none";
    // Webview teardown events are not guaranteed, so commit here.
    flushSettingsStore();
    cancelSettingsResetConfirm();
  }

  // Close settings and license modals on escape key
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      if (settingsModal.style.display === "flex") {
        closeSettings();
      }
      
      const licenseModal = document.getElementById("license-modal");
      if (licenseModal && licenseModal.style.display === "flex") {
        closeLicense();
      }

      const updateModal = document.getElementById("update-modal");
      if (updateModal && updateModal.style.display === "flex") {
        closeUpdateModal();
      }
    }
  });

  window.openSettings = openSettings;
  window.closeSettings = closeSettings;

  // Mirrors OBJECT_SKIP_SCALE in src/args.rs
  const OBJECT_SKIP_SCALE = 0.3;
  const scaleObjectsNote = document.getElementById("scale-objects-note");

  function refreshScaleDisplay() {
    const value = parseFloat(slider.value);
    sliderValue.textContent = value.toFixed(2);
    if (scaleObjectsNote) {
      scaleObjectsNote.style.display = value < OBJECT_SKIP_SCALE ? "" : "none";
    }
  }

  slider.addEventListener("input", refreshScaleDisplay);
  // Double-click to reset world scale to default (1.00).
  // Assigning .value fires no event, so dispatch them for the label and store.
  slider.addEventListener("dblclick", () => {
    slider.value = 1;
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    slider.dispatchEvent(new Event("change", { bubbles: true }));
  });
  refreshScaleDisplay();

  // Game mode segmented control
  const gamemodeGroup = document.getElementById("gamemode-group");
  gamemodeGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      gamemodeGroup.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
    });
  });

  // Signage segmented control
  const signageGroup = document.getElementById("signage-group");
  signageGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      signageGroup.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
    });
  });

  // A reloaded map comes back on Earth; restore whatever was picked.
  const bodyMapFrame = getMapFrame();
  if (bodyMapFrame) bodyMapFrame.addEventListener('load', pushBodyToMap);

  // Max tree size segmented control
  const maxTreeSizeGroup = document.getElementById("max-tree-size-group");
  maxTreeSizeGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      maxTreeSizeGroup.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
      btn.classList.add("active");
    });
  });

  // World time slider (clock minutes 00:00-23:50; converted to ticks on submit)
  const timeSlider = document.getElementById("world-time-slider");
  const timeValue = document.getElementById("world-time-value");
  function formatClock(minutes) {
    if (minutes >= 1440) return "24:00";
    const h = String(Math.floor(minutes / 60)).padStart(2, "0");
    const m = String(minutes % 60).padStart(2, "0");
    return `${h}:${m}`;
  }
  timeSlider.addEventListener("input", () => {
    timeValue.textContent = formatClock(parseInt(timeSlider.value, 10));
  });
  timeSlider.addEventListener("dblclick", () => {
    timeSlider.value = 720;
    timeSlider.dispatchEvent(new Event("input", { bubbles: true }));
    timeSlider.dispatchEvent(new Event("change", { bubbles: true }));
  });

  // Rotation angle input
  const rotationInput = document.getElementById("rotation-angle-input");

  function updateRotation(val) {
    if (isNaN(val)) val = 0;
    val = Math.min(Math.max(val, -90), 90);
    rotationInput.value = val.toFixed(2);
    // The bbox handlers set this from code, which fires no input event.
    refreshSettingsState();
    // Tell the map iframe to update the rotation mask overlay
    const mapFrame = document.querySelector('.map-container');
    if (mapFrame && mapFrame.contentWindow) {
      mapFrame.contentWindow.postMessage({
        type: 'rotatePreview',
        angle: val
      }, '*');
    }
  }
  rotationInput.addEventListener("input", () => {
    updateRotation(parseFloat(rotationInput.value));
  });
  rotationInput.addEventListener("change", () => {
    updateRotation(parseFloat(rotationInput.value));
  });
  window.updateRotation = updateRotation;

  // World format toggle (Java/Bedrock/Luanti)
  initWorldFormatToggle();

  // Save path setting
  initSavePathSetting();

  // Language selector
  const languageSelect = document.getElementById("language-select");
  const availableOptions = Array.from(languageSelect.options).map(opt => opt.value);

  // The default here is the browser language, not an HTML attribute.
  setDynamicDefault('language', detectBrowserLanguage(availableOptions));

  // Check for saved language preference first
  const savedLanguage = localStorage.getItem('arnis-language');
  let languageToSet;

  if (savedLanguage && availableOptions.includes(savedLanguage)) {
    // Use saved language if it exists and is available
    languageToSet = savedLanguage;
  } else {
    // Otherwise use browser language
    languageToSet = detectBrowserLanguage(availableOptions);
  }

  languageSelect.value = languageToSet;

  // Handle language change
  languageSelect.addEventListener("change", async () => {
    const selectedLanguage = languageSelect.value;

    // Store the selected language in localStorage for persistence
    localStorage.setItem('arnis-language', selectedLanguage);

    // Reload localization with the new language
    const localization = await fetchLanguage(selectedLanguage);
    await applyLocalization(localization);

    // Restore correct format toggle state after localization
    updateFormatToggleUI(selectedWorldFormat);
  });

  // Tile theme selector
  const tileThemeSelect = document.getElementById("tile-theme-select");

  // Load saved tile theme preference
  const savedTileTheme = localStorage.getItem('selectedTileTheme') || 'osm';
  tileThemeSelect.value = savedTileTheme;

  // Handle tile theme change
  tileThemeSelect.addEventListener("change", () => {
    const selectedTheme = tileThemeSelect.value;

    // Store the selected theme in localStorage for persistence
    localStorage.setItem('selectedTileTheme', selectedTheme);
    refreshCustomSourceRow();

    // Send message to map iframe to change tile theme
    const mapIframe = getMapFrame();
    if (mapIframe && mapIframe.contentWindow) {
      mapIframe.contentWindow.postMessage({
        type: 'changeTileTheme',
        theme: selectedTheme
      }, '*');
    }
  });

  // Custom map source, the field behind the Custom theme. It exists because a
  // network that blocks every built-in provider turns the fallback chain into a
  // slower route to the same blank map (see issues #1222, #1298, #1299).
  const customTileInput = document.getElementById("custom-tile-url");
  customTileInput.value = getCustomTileUrl();

  // Mapillary token. Kept in localStorage like the save paths so it survives a
  // restart; it is a per-user API credential, so it is never written to a log
  // or sent anywhere except the backend that fetches with it.
  const mapillaryTokenInput = document.getElementById("mapillary-token");
  mapillaryTokenInput.value = getMapillaryToken();
  mapillaryTokenInput.addEventListener("change", () => {
    const raw = mapillaryTokenInput.value.trim();
    if (raw) {
      localStorage.setItem('mapillaryToken', raw);
    } else {
      localStorage.removeItem('mapillaryToken');
    }
    refreshFacadeRows();
  });

  // The source control, and the detail beside it. Both persist their own key
  // rather than going through settings-store.js, because the panel's Revert
  // reads that store and the facade choice is not part of a world's settings.
  const segmented = (id, storageKey, dataAttr, after) => {
    const group = document.getElementById(id);
    if (!group) return;
    group.querySelectorAll(".segment").forEach((btn) => {
      btn.addEventListener("click", () => {
        // A disabled segment is still clicked by settings-store.js when it
        // restores or reverts, and refusing that would lose a choice made on
        // Java the moment the format changed.
        localStorage.setItem(storageKey, btn.dataset[dataAttr]);
        group.querySelectorAll(".segment").forEach((b) => {
          b.classList.toggle("active", b === btn);
        });
        if (after) after();
      });
    });
  };
  segmented("facade-source-group", "facadeSource", "facadeSource", refreshFacadeRows);
  segmented("facade-detail-group", "facadeDetail", "facadeDetail", null);
  refreshFacadeRows();

  // An older build kept a facade export folder here. The field is gone, the
  // preview reads the cache and generation fetches into it, so the leftover
  // key means nothing and is dropped rather than left to look meaningful.
  localStorage.removeItem('facadeDir');

  const facadeModeGroup = document.getElementById("facade-mode-group");
  facadeModeGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      localStorage.setItem('facadeMode', btn.dataset.facadeMode);
      refreshFacadeRows();
    });
  });
  refreshFacadeRows();

  function applyCustomTileUrl() {
    const raw = customTileInput.value.trim();

    // An empty field is the normal way to go back to the themes. A non-empty
    // one that is not a usable template is left in the box so the user can see
    // and correct it, but is not handed to the map.
    if (raw && !isValidTileTemplate(raw)) {
      window.arnisLog('warn', 'Ignoring custom map source: expected an http(s) URL containing {z}, {x} and {y}.');
      localStorage.removeItem('customTileUrl');
    } else if (raw) {
      localStorage.setItem('customTileUrl', raw);
    } else {
      localStorage.removeItem('customTileUrl');
    }

    const mapIframe = getMapFrame();
    if (mapIframe && mapIframe.contentWindow) {
      mapIframe.contentWindow.postMessage({
        type: 'setCustomTileUrl',
        url: getCustomTileUrl()
      }, '*');
    }
  }

  // On change, not on input: a half-typed URL is not a source, and remounting
  // the basemap per keystroke would hammer whatever host they are aiming at.
  customTileInput.addEventListener("change", applyCustomTileUrl);
  refreshCustomSourceRow();

  // Telemetry consent toggle
  const telemetryToggle = document.getElementById("telemetry-toggle");
  const telemetryKey = 'telemetry-consent';

  // Load saved telemetry consent
  const savedConsent = localStorage.getItem(telemetryKey);
  telemetryToggle.checked = savedConsent === 'true';

  // Handle telemetry consent change
  telemetryToggle.addEventListener("change", () => {
    const isEnabled = telemetryToggle.checked;
    localStorage.setItem(telemetryKey, isEnabled ? 'true' : 'false');
  });


  /// License and Credits
  async function openLicense() {
    const licenseModal = document.getElementById("license-modal");
    const licenseContent = document.getElementById("license-content");

    licenseContent.innerHTML = licenseText;
    licenseModal.style.display = "flex";
    licenseModal.style.justifyContent = "center";
    licenseModal.style.alignItems = "center";

    const threeDmrBlock =
      `<p><b>3D Model Repository (3DMR):</b></p>` +
      `<p style="font-size: 0.9em;">Landmark models from <a href="https://3dmr.eu" style="color: inherit;" target="_blank" rel="noopener noreferrer">3dmr.eu</a> are fetched on demand and voxelized. Individual models retain the license declared by their uploader; specific per-model attribution is printed to the generation log. See the <a href="https://3dmr.eu" style="color: inherit;" target="_blank" rel="noopener noreferrer">3DMR website</a> for any model used.</p>`;
    licenseContent.insertAdjacentHTML("beforeend", threeDmrBlock);

    // The premade facade set. All CC0, so attribution is a courtesy rather than
    // a condition, but the sources are named because someone should be able to
    // find them and because it says plainly that the pixels are free to ship.
    const facadeTextureBlock =
      `<p><b>Preset Building Facade Textures:</b></p>` +
      `<p style="font-size: 0.9em;">The photographs hung on buildings by the Preset Facades setting, all released under ` +
      `<a href="https://creativecommons.org/publicdomain/zero/1.0/" style="color: inherit;" target="_blank" rel="noopener noreferrer">CC0</a>:</p>` +
      `<ul style="padding-left: 20px; font-size: 0.9em;">` +
      `<li>Urban building, apartment and shop front photographs by <b>Scouser</b>, from ` +
      `<a href="https://opengameart.org/content/free-urban-textures-buildings-apartments-shop-fronts" style="color: inherit;" target="_blank" rel="noopener noreferrer">OpenGameArt</a></li>` +
      `<li>Tiling facade materials from <b>TextureCan</b>: ` +
      `<a href="https://www.texturecan.com/details/315/" style="color: inherit;" target="_blank" rel="noopener noreferrer">315</a>, ` +
      `<a href="https://www.texturecan.com/details/316/" style="color: inherit;" target="_blank" rel="noopener noreferrer">316</a>, ` +
      `<a href="https://www.texturecan.com/details/357/" style="color: inherit;" target="_blank" rel="noopener noreferrer">357</a>, ` +
      `<a href="https://www.texturecan.com/details/360/" style="color: inherit;" target="_blank" rel="noopener noreferrer">360</a>, ` +
      `<a href="https://www.texturecan.com/details/563/" style="color: inherit;" target="_blank" rel="noopener noreferrer">563</a></li>` +
      `</ul>`;
    licenseContent.insertAdjacentHTML("beforeend", facadeTextureBlock);

    try {
      const rows = await invoke("gui_get_3d_model_attributions");
      if (Array.isArray(rows) && rows.length > 0) {
        const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));
        const items = rows.map(r => {
          const lic = r.license_url
            ? `<a href="${esc(r.license_url)}" style="color: inherit;" target="_blank" rel="noopener noreferrer">${esc(r.license)}</a>`
            : esc(r.license);
          return `<li><b>${esc(r.label)}</b> — ${esc(r.artist)}, ${lic} (<a href="${esc(r.source_url)}" style="color: inherit;" target="_blank" rel="noopener noreferrer">source</a>)</li>`;
        }).join("");
        const block =
          `<p><b>Bundled 3D Models (Wikimedia Commons via Wikidata P4896):</b></p>` +
          `<p style="font-size: 0.9em;">Permissive-licensed models used to render famous landmarks. Voxelized and rescaled by Arnis.</p>` +
          `<ul style="padding-left: 20px;">${items}</ul>`;
        licenseContent.insertAdjacentHTML("beforeend", block);
      }
    } catch (e) {
      console.warn("Failed to load 3D model attributions:", e);
    }

    // Mapillary imagery is CC BY-SA, and the licence is on the pixels: a world
    // built from street photographs has to name the photographers. One line per
    // image, in the shape Mapillary's own guidance asks for.
    try {
      const shots = await invoke("gui_get_mapillary_attributions");
      if (Array.isArray(shots) && shots.length > 0) {
        const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));
        const link = (url, text) =>
          `<a href="${esc(url)}" style="color: inherit;" target="_blank" rel="noopener noreferrer">${esc(text)}</a>`;
        // A record that carries the image id but not the uploader name has no
        // profile to link to, so the name is shown as plain text instead.
        const items = shots.map((s) => {
          const by = s.profile_url ? link(s.profile_url, s.username) : esc(s.username);
          return `<li>${link(s.image_url, s.title)} by ${by}, licensed under CC-BY-SA</li>`;
        }).join("");
        const unnamed = shots.filter((s) => !s.profile_url).length;
        const note = unnamed > 0
          ? ` ${unnamed} of these name only the photograph: their records carry the image id but not the uploader name. Each link opens the image, which names its uploader.`
          : "";
        const block =
          `<p><b>Building facades (Mapillary):</b></p>` +
          `<p style="font-size: 0.9em;">Facade textures and colours were measured from these street-level photographs, licensed <a href="https://creativecommons.org/licenses/by-sa/4.0/" style="color: inherit;" target="_blank" rel="noopener noreferrer">CC BY-SA 4.0</a>. Share-alike applies to anything you publish that carries them.${note}</p>` +
          `<ul style="padding-left: 20px; font-size: 0.9em;">${items}</ul>`;
        licenseContent.insertAdjacentHTML("beforeend", block);
      }
    } catch (e) {
      console.warn("Failed to load Mapillary attributions:", e);
    }
  }

  function closeLicense() {
    const licenseModal = document.getElementById("license-modal");
    licenseModal.style.display = "none";
  }

  window.openLicense = openLicense;
  window.closeLicense = closeLicense;
}

// World format selection (Java/Bedrock/Luanti)
let selectedWorldFormat = 'java'; // Default to Java

const VALID_FORMATS = ['java', 'bedrock', 'luanti'];

// Voxy renders distant terrain from the per-voxel light stored in its LOD
// cache, so pre-generating one without baked lighting would give a black
// horizon. Keep the two toggles consistent in the UI rather than quietly
// overriding the user's choice at generation time.
function initVoxyLightingCoupling() {
  const voxy = document.getElementById('voxy-lod-toggle');
  const bake = document.getElementById('bake-lighting-toggle');
  if (!voxy || !bake) return;

  // Dispatch so the settings store persists the knock-on change too.
  const set = (el, value) => {
    if (el.checked === value) return;
    el.checked = value;
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };

  voxy.addEventListener('change', () => {
    if (voxy.checked) set(bake, true);
  });
  bake.addEventListener('change', () => {
    if (!bake.checked) set(voxy, false);
  });
}

function initWorldFormatToggle() {
  initLuantiExperimentalToggle();

  const savedFormat = localStorage.getItem('arnis-world-format');
  if (savedFormat && VALID_FORMATS.includes(savedFormat)) {
    selectedWorldFormat = savedFormat;
  }
  if (selectedWorldFormat === 'luanti' && !isLuantiEnabled()) {
    selectedWorldFormat = 'java';
  }

  updateFormatToggleUI(selectedWorldFormat);
}

function isLuantiEnabled() {
  return localStorage.getItem('arnis-luanti-enabled') === 'true';
}

function initLuantiExperimentalToggle() {
  const toggle = document.getElementById('enable-luanti-toggle');
  const luantiBtn = document.getElementById('format-luanti');
  const bedrockBtn = document.getElementById('format-bedrock');
  if (!toggle || !luantiBtn) return;

  const applyRightmost = (enabled) => {
    luantiBtn.style.display = enabled ? '' : 'none';
    luantiBtn.classList.toggle('format-toggle-btn--rightmost', enabled);
    if (bedrockBtn) {
      bedrockBtn.classList.toggle('format-toggle-btn--rightmost', !enabled);
    }
  };

  const enabled = isLuantiEnabled();
  toggle.checked = enabled;
  applyRightmost(enabled);

  toggle.addEventListener('change', () => {
    const on = toggle.checked;
    localStorage.setItem('arnis-luanti-enabled', on ? 'true' : 'false');
    applyRightmost(on);
    if (!on && selectedWorldFormat === 'luanti') {
      setWorldFormat('java');
    }
  });
}

function setWorldFormat(format) {
  if (!VALID_FORMATS.includes(format)) return;
  if (format === 'luanti' && !isLuantiEnabled()) return;

  selectedWorldFormat = format;
  localStorage.setItem('arnis-world-format', format);
  updateFormatToggleUI(format);
}

function getEffectiveWorldFormat() {
  if (selectedWorldFormat === 'luanti') {
    return 'luanti_mineclonia';
  }
  return selectedWorldFormat;
}

// The extended dimension is declared by the Java datapack and the Bedrock
// behavior pack; Luanti ships neither, and off Earth the relief already fits
// vanilla height, so the backend forces the flag off there.
function heightLimitAvailable(format) {
  return selectedCelestialBody === 'earth' && format !== 'luanti';
}

function refreshHeightLimitRow(format) {
  const toggle = document.getElementById('disable-height-limit-toggle');
  if (!toggle) return;

  const available = heightLimitAvailable(format || selectedWorldFormat);
  toggle.disabled = !available;

  const row = toggle.closest('.settings-row');
  if (row) {
    // Cleared, not set to 1: an inline value would beat the class rule.
    row.style.opacity = '';
    row.classList.toggle('settings-row-unavailable', !available);
  }

  const note = document.getElementById('height-limit-note');
  if (note) note.hidden = !(available && toggle.checked);
}

// Consequences the CLI prints to stderr, which a GUI user never sees.
function initHeightLimitNote() {
  const toggle = document.getElementById('disable-height-limit-toggle');
  const row = toggle && toggle.closest('.settings-row');
  if (!row || document.getElementById('height-limit-note')) return;

  const note = document.createElement('div');
  note.id = 'height-limit-note';
  note.className = 'settings-row-note';
  note.hidden = true;
  note.textContent =
    'Needs Java 1.21.4+ or Bedrock 1.21.40+. First load asks to enable Experimental ' +
    'Features, the world cannot be uploaded to Realms, and generation is slower and ' +
    'the world bigger.';
  row.after(note);

  toggle.addEventListener('change', () => refreshHeightLimitRow());
  refreshHeightLimitRow();
}

function updateFormatToggleUI(format) {
  const javaBtn = document.getElementById('format-java');
  const bedrockBtn = document.getElementById('format-bedrock');
  const luantiBtn = document.getElementById('format-luanti');

  refreshHeightLimitRow(format);

  javaBtn.classList.remove('format-active');
  bedrockBtn.classList.remove('format-active');
  if (luantiBtn) luantiBtn.classList.remove('format-active');

  if (format === 'java') {
    javaBtn.classList.add('format-active');
  } else if (format === 'bedrock') {
    bedrockBtn.classList.add('format-active');
    // Clear world path for bedrock (auto-generated)
    worldPath = "";
  } else if (format === 'luanti') {
    if (luantiBtn) luantiBtn.classList.add('format-active');
    worldPath = "";
  }

  // The facade panels are Java entities, so the mode control changes with the
  // format. Called from here so a format picked before the settings modal is
  // ever opened still leaves it consistent.
  refreshFacadeRows();
}

// Expose to window for onclick handlers
window.setWorldFormat = setWorldFormat;

// Telemetry consent (first run only)
function initTelemetryConsent() {
  const key = 'telemetry-consent'; // values: 'true' | 'false'
  const existing = localStorage.getItem(key);

  const modal = document.getElementById('telemetry-modal');
  if (!modal) return;

  if (existing === null) {
    // First run: ask for consent
    modal.style.display = 'flex';
    modal.style.justifyContent = 'center';
    modal.style.alignItems = 'center';
  }

  // Expose handlers
  window.acceptTelemetry = () => {
    localStorage.setItem(key, 'true');
    modal.style.display = 'none';
    // Update settings toggle to reflect the consent
    const telemetryToggle = document.getElementById('telemetry-toggle');
    if (telemetryToggle) {
      telemetryToggle.checked = true;
    }
    // Set from code, so no change event fired.
    refreshSettingsState();
  };

  window.rejectTelemetry = () => {
    localStorage.setItem(key, 'false');
    modal.style.display = 'none';
    // Update settings toggle to reflect the consent
    const telemetryToggle = document.getElementById('telemetry-toggle');
    if (telemetryToggle) {
      telemetryToggle.checked = false;
    }
    refreshSettingsState();
  };

  // Utility for other scripts to read consent
  window.getTelemetryConsent = () => {
    const v = localStorage.getItem(key);
    return v === null ? null : v === 'true';
  };
}

// Wires the "Clear Tile Cache" button in the Application settings panel
// to the Rust-side `gui_clear_tile_caches` command. User feedback is a
// brief background flash (green on success, red on partial failure) —
// keeps the row visually consistent with the other checkbox/slider
// rows, no extra status label. The button stays disabled while the
// call is in flight so repeated clicks can't fire multiple concurrent
// wipes (Rust is idempotent, but the UI would look confused).
// How much disk the caches hold, shown next to the Clear button so the user
// can see whether clearing is worth doing. Asked for when the settings panel
// opens and again after anything that changes the caches, never at startup and
// never on a timer: the answer is a walk of every cached file, which is tenths
// of a second once a facade run has filled the tile cache.
async function refreshCacheSize() {
  const label = document.getElementById('cache-size');
  if (!label) {
    return;
  }
  try {
    label.textContent = await invoke('gui_get_cache_size');
  } catch (error) {
    console.warn('Cache size unavailable:', error);
    label.textContent = '';
  }
}
window.refreshCacheSize = refreshCacheSize;

function initClearCacheButton() {
  const button = document.getElementById('clear-cache-button');
  if (!button) {
    return;
  }
  // Deliberately not asking for the size here. This runs on DOMContentLoaded,
  // where the number cannot be seen by anyone: the label lives inside the
  // settings panel, and `openSettings` asks for it there. Reading it at startup
  // only bought a value that was stale by the time the panel opened, and paid
  // for it with a walk of every cached file while the window was going up.

  // How long the success/error flash stays applied before reverting to
  // the default outline. Long enough to register as confirmation, short
  // enough that a user can click again quickly if they want.
  const FLASH_MS = 1500;
  let flashTimer = null;

  const flash = (cls) => {
    button.classList.remove('is-success', 'is-error');
    button.classList.add(cls);
    if (flashTimer) {
      clearTimeout(flashTimer);
    }
    flashTimer = setTimeout(() => {
      button.classList.remove('is-success', 'is-error');
      flashTimer = null;
    }, FLASH_MS);
  };

  button.addEventListener('click', async () => {
    if (button.disabled) {
      return;
    }
    button.disabled = true;
    // Pre-emptively drop any lingering flash class from a previous run
    // so "clearing…" state isn't tinted green/red left over from before.
    button.classList.remove('is-success', 'is-error');
    try {
      await invoke('gui_clear_tile_caches');
      flash('is-success');
      refreshCacheSize();
    } catch (error) {
      // The Rust side returns Err(String) for partial failures (files
      // still locked). The user sees the red flash; the full text goes
      // to the browser console for debugging, not the UI.
      console.warn('Clear tile cache failed:', error);
      flash('is-error');
    } finally {
      button.disabled = false;
    }
  });
}

/* Precompute: fills the Mapillary facade cache for the selected area, so the
   generation that follows does no image work and the 3D preview can show the
   walls. The backend refuses a second precompute and one started beside a
   generation; the button state here is that same rule said early, so pressing
   it is never the way to find out it cannot run. */

let precomputeRunning = false;
let precomputeStartedAt = 0;
let precomputeTicker = null;
// The pipeline reports its stages on the shared progress channel. Nothing else
// is emitting while a precompute holds the process, so those lines are mirrored
// into the settings row instead of being left in a status bar the user is not
// looking at, under a progress bar that is not moving.
let precomputeStage = "";

function setPrecomputeStatus(text, kind, detail) {
  const el = document.getElementById('facade-precompute-status');
  if (!el) return;
  el.textContent = text || "";
  el.style.display = text ? "" : "none";
  el.classList.toggle('is-success', kind === 'success');
  el.classList.toggle('is-error', kind === 'error');
  if (detail) {
    el.title = detail;
  } else {
    el.removeAttribute('title');
  }
}

// Why the button cannot be pressed, or "" when it can. The wording is what the
// button's tooltip says, so a disabled button always explains itself.
function precomputeBlockedReason() {
  if (!getMapillaryToken()) return "Add a Mapillary token above first.";
  if (!selectedBBox || selectedBBox === "0.000000 0.000000 0.000000 0.000000") {
    return "Select an area on the map first.";
  }
  if (!generationButtonEnabled) return "A generation is running.";
  return "";
}

function refreshPrecomputeButton() {
  const button = document.getElementById('precompute-facades-button');
  if (!button) return;

  if (precomputeRunning) {
    // Never disabled while running: this is the only way to stop it.
    button.disabled = false;
    button.textContent = "Cancel";
    button.title = "Stops at the end of the stage it is in. Walls already built stay cached.";
    return;
  }

  const localized = window.localization || {};
  button.textContent = localized['facade_precompute_button'] || "Precompute";
  const blocked = precomputeBlockedReason();
  button.disabled = !!blocked;
  button.title = blocked;
}

// mm:ss since the run started, for a job whose stages are minutes long.
function precomputeElapsed() {
  const seconds = Math.max(0, Math.round((Date.now() - precomputeStartedAt) / 1000));
  return Math.floor(seconds / 60) + ":" + String(seconds % 60).padStart(2, '0');
}

function showPrecomputeProgress() {
  if (!precomputeRunning) return;
  setPrecomputeStatus((precomputeStage || "Working...") + " (" + precomputeElapsed() + ")");
}

// Called by the progress listener for every line the facade pipeline emits, so
// the row says which stage is running rather than only that something is.
function notePrecomputeStage(message) {
  if (!precomputeRunning || !message.startsWith("Mapillary facades:")) return;
  precomputeStage = message.slice("Mapillary facades:".length).trim();
  showPrecomputeProgress();
}

function initPrecomputeFacadesButton() {
  const button = document.getElementById('precompute-facades-button');
  if (!button) return;
  refreshPrecomputeButton();

  button.addEventListener('click', async () => {
    if (precomputeRunning) {
      // The pipeline checks between stages, so this is a request, not a stop.
      precomputeStage = "Cancelling after this stage";
      showPrecomputeProgress();
      try {
        await invoke('gui_cancel_precompute');
      } catch (error) {
        console.warn('Cancel precompute failed:', error);
      }
      return;
    }

    const blocked = precomputeBlockedReason();
    if (blocked) {
      setPrecomputeStatus(blocked, 'error');
      return;
    }

    precomputeRunning = true;
    precomputeStartedAt = Date.now();
    precomputeStage = "Starting";
    refreshPrecomputeButton();
    showPrecomputeProgress();
    // One second, so a run that spends twenty minutes in one stage still shows
    // something moving and cannot be mistaken for a hang.
    precomputeTicker = setInterval(showPrecomputeProgress, 1000);

    try {
      const outcome = await invoke('gui_precompute_facades', {
        bboxText: selectedBBox,
        mapillaryToken: getMapillaryToken(),
      });
      // Green only when there are facades here now. A cancelled run and an
      // area with nothing to find both come back plain: neither is a failure
      // and neither left a wall behind.
      setPrecomputeStatus(outcome.summary, outcome.built ? 'success' : '', outcome.detail);
      if (outcome.built) window.arnisPreview3D?.refreshFacades();
      refreshCacheSize();
    } catch (error) {
      // Every refusal from the backend is a sentence meant to be read.
      setPrecomputeStatus(String(error), 'error');
      refreshCacheSize();
    } finally {
      clearInterval(precomputeTicker);
      precomputeTicker = null;
      precomputeRunning = false;
      refreshPrecomputeButton();
    }
  });
}

// Single shared tooltip element appended to <body>, so it escapes the
// `.settings-scrollable` container's `overflow: hidden` clip and can
// extend past the top / sides of the panel. Previously the tooltip
// lived as a `::after` pseudo-element on each `.tooltip-icon`, which
// meant long text or icons near an edge got cut off by the scroll
// container. This global element is positioned via
// `getBoundingClientRect` on hover and auto-flips above ↔ below when
// close to the viewport edge.
function initTooltips() {
  const tooltip = document.createElement('div');
  tooltip.className = 'global-tooltip';
  tooltip.setAttribute('role', 'tooltip');
  tooltip.setAttribute('aria-hidden', 'true');
  const arrow = document.createElement('div');
  arrow.className = 'global-tooltip-arrow';
  tooltip.appendChild(arrow);
  const body = document.createElement('div');
  body.className = 'global-tooltip-body';
  tooltip.appendChild(body);
  document.body.appendChild(tooltip);

  const VIEWPORT_MARGIN = 8; // px gap between tooltip and viewport edge
  const ICON_GAP = 8; // px gap between tooltip and icon

  let currentIcon = null;

  const position = () => {
    if (!currentIcon) return;
    const iconRect = currentIcon.getBoundingClientRect();
    // Measure after text is set; reset to allow natural width.
    const ttRect = tooltip.getBoundingClientRect();

    // Default: centered above the icon. Flip below when there isn't
    // enough room above (e.g. icon near the top of the settings panel,
    // which is the "cut off at the top" case the user reported).
    const spaceAbove = iconRect.top;
    const flipBelow = spaceAbove < ttRect.height + ICON_GAP + VIEWPORT_MARGIN;
    const top = flipBelow
      ? iconRect.bottom + ICON_GAP
      : iconRect.top - ttRect.height - ICON_GAP;

    // Horizontal: center on the icon, then clamp into the viewport so
    // tooltips near the right edge don't overflow into hidden space.
    const desiredLeft = iconRect.left + iconRect.width / 2 - ttRect.width / 2;
    const maxLeft = window.innerWidth - ttRect.width - VIEWPORT_MARGIN;
    const left = Math.max(VIEWPORT_MARGIN, Math.min(desiredLeft, maxLeft));

    tooltip.style.top = top + 'px';
    tooltip.style.left = left + 'px';

    // Point the arrow back at the icon's center, regardless of the
    // horizontal clamp above, and flip it to the opposite edge when the
    // tooltip opens below the icon.
    const iconCenter = iconRect.left + iconRect.width / 2;
    const arrowLeft = Math.max(8, Math.min(ttRect.width - 8, iconCenter - left));
    arrow.style.left = arrowLeft + 'px';
    tooltip.classList.toggle('flipped', flipBelow);
  };

  const show = (icon) => {
    const text = icon.getAttribute('data-tooltip');
    if (!text) return;
    currentIcon = icon;
    body.textContent = text;
    // Position BEFORE making visible. The tooltip stays `visibility:
    // hidden` (layout-active, paint-inactive) so `getBoundingClientRect`
    // returns the real dimensions, but the user never sees a 0,0 flash
    // between insertion and the first position-frame.
    position();
    tooltip.classList.add('is-visible');
    tooltip.setAttribute('aria-hidden', 'false');
  };

  const hide = () => {
    currentIcon = null;
    tooltip.classList.remove('is-visible', 'flipped');
    tooltip.setAttribute('aria-hidden', 'true');
  };

  const bind = (icon) => {
    // Make `<span class="tooltip-icon">` focusable via Tab so keyboard
    // users can reveal the tooltip. Spans are not focusable by default,
    // so the focus/blur listeners below are dead without this. Done in
    // JS rather than HTML so every icon picks it up automatically and
    // we don't have to keep the 14 call sites in sync. `role="button"`
    // is a reasonable hint for screen readers that this thing is
    // interactive even though it doesn't do anything on click.
    if (icon.tabIndex < 0) {
      icon.tabIndex = 0;
    }
    if (!icon.hasAttribute('role')) {
      icon.setAttribute('role', 'button');
    }
    if (!icon.hasAttribute('aria-label')) {
      const text = icon.getAttribute('data-tooltip');
      if (text) {
        icon.setAttribute('aria-label', text);
      }
    }
    icon.addEventListener('mouseenter', () => show(icon));
    icon.addEventListener('mouseleave', hide);
    icon.addEventListener('focus', () => show(icon));
    icon.addEventListener('blur', hide);
    // Escape closes the tooltip while it's focused.
    icon.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        hide();
      }
    });
  };

  document.querySelectorAll('.tooltip-icon').forEach(bind);

  // Reposition on viewport resize / scroll (including inside the
  // settings-scrollable container). Also hide on scroll inside the
  // settings panel, because the icon may have scrolled off-screen
  // and a stale tooltip hovering over the wrong row is worse than
  // hiding eagerly.
  window.addEventListener('resize', () => {
    if (currentIcon) position();
  });
  const scrollable = document.querySelector('.settings-scrollable');
  if (scrollable) {
    scrollable.addEventListener('scroll', hide, { passive: true });
  }
}

/// Save path management, one path per world format
let savePath = "";
let bedrockSavePath = "";
let luantiSavePath = "";

const SAVE_PATHS = {
  java: {
    storageKey: 'arnis-save-path',
    defaultCommand: 'gui_get_default_save_path',
    inputId: 'save-path-input',
    browseId: 'save-path-browse',
    get: () => savePath,
    set: (value) => { savePath = value; },
  },
  bedrock: {
    storageKey: 'arnis-bedrock-save-path',
    defaultCommand: 'gui_get_default_bedrock_save_path',
    inputId: 'bedrock-save-path-input',
    browseId: 'bedrock-save-path-browse',
    get: () => bedrockSavePath,
    set: (value) => { bedrockSavePath = value; },
  },
  luanti: {
    storageKey: 'arnis-luanti-save-path',
    defaultCommand: 'gui_get_default_luanti_save_path',
    inputId: 'luanti-save-path-input',
    browseId: 'luanti-save-path-browse',
    get: () => luantiSavePath,
    set: (value) => { luantiSavePath = value; },
  },
};

async function initSavePath() {
  for (const config of Object.values(SAVE_PATHS)) {
    config.set(await resolveStoredSavePath(config));
    const input = document.getElementById(config.inputId);
    if (input) {
      input.value = config.get();
    }
  }
}

async function resolveStoredSavePath({ storageKey, defaultCommand }) {
  const saved = localStorage.getItem(storageKey);
  if (saved) {
    // Validate the saved path still exists (handles upgrades / moved directories)
    try {
      const normalized = await invoke('gui_set_save_path', { path: saved });
      localStorage.setItem(storageKey, normalized);
      return normalized;
    } catch (_) {
      console.warn(`Stored path ${storageKey} no longer valid, re-detecting...`);
      localStorage.removeItem(storageKey);
    }
  }

  try {
    const detected = await invoke(defaultCommand);
    localStorage.setItem(storageKey, detected);
    return detected;
  } catch (error) {
    console.error(`Failed to detect path for ${storageKey}:`, error);
    return "";
  }
}

function initSavePathSetting() {
  for (const config of Object.values(SAVE_PATHS)) {
    initSavePathRow(config);
  }
}

function initSavePathRow({ storageKey, inputId, browseId, get, set }) {
  const input = document.getElementById(inputId);
  if (!input) return;

  input.value = get();

  // Manual text input – validate on change, revert if invalid
  input.addEventListener('change', async () => {
    const newPath = input.value.trim();
    if (!newPath) {
      input.value = get();
      return;
    }

    try {
      const validated = await invoke('gui_set_save_path', { path: newPath });
      set(validated);
      input.value = validated;
      localStorage.setItem(storageKey, validated);
    } catch (_) {
      // Invalid path – silently revert to previous value
      input.value = get();
    }
  });

  // Folder picker button
  const browseBtn = document.getElementById(browseId);
  if (browseBtn) {
    browseBtn.addEventListener('click', async () => {
      try {
        const picked = await invoke('gui_pick_save_directory', { startPath: get() });
        if (picked) {
          set(picked);
          input.value = picked;
          localStorage.setItem(storageKey, picked);
        }
      } catch (error) {
        console.error("Folder picker failed:", error);
      }
    });
  }
}

/**
 * Validates and processes bounding box coordinates input
 * Supports both comma and space-separated formats
 * Updates the map display when valid coordinates are entered
 */
function handleBboxInput() {
  const inputBox = document.getElementById("bbox-coords");
  const bboxSelectionInfo = document.getElementById("bbox-selection-info");

  inputBox.addEventListener("input", function () {
    const input = inputBox.value.trim();

    if (input === "") {
      // Empty input - revert to map selection if available
      customBBoxValid = false;
      selectedBBox = mapSelectedBBox;
      
      // Clear the info text only if no map selection exists
      if (!mapSelectedBBox) {
        setBboxSelectionInfo(bboxSelectionInfo, "select_area_prompt", "#ffffff");
      } else {
        // Restore map selection info display but don't update input field
        const [lat1, lng1, lat2, lng2] = mapSelectedBBox.split(" ").map(Number);
        const selectedSize = calculateBBoxSize(lat1, lng1, lat2, lng2);
        displayBboxSizeStatus(bboxSelectionInfo, selectedSize);
      }
      return;
    }

    // Regular expression to validate bbox input (supports both comma and space-separated formats)
    const bboxPattern = /^(-?\d+(\.\d+)?)[,\s](-?\d+(\.\d+)?)[,\s](-?\d+(\.\d+)?)[,\s](-?\d+(\.\d+)?)$/;

    if (bboxPattern.test(input)) {
      const matches = input.match(bboxPattern);

      // Extract coordinates (Lat / Lng order expected)
      const lat1 = parseFloat(matches[1]);
      const lng1 = parseFloat(matches[3]);
      const lat2 = parseFloat(matches[5]);
      const lng2 = parseFloat(matches[7]);

      // Validate latitude and longitude ranges in the expected Lat / Lng order
      if (
        lat1 >= -90 && lat1 <= 90 &&
        lng1 >= -180 && lng1 <= 180 &&
        lat2 >= -90 && lat2 <= 90 &&
        lng2 >= -180 && lng2 <= 180
      ) {
        // Input is valid; trigger the event with consistent comma-separated format
        const bboxText = `${lat1},${lng1},${lat2},${lng2}`;
        window.dispatchEvent(new MessageEvent('message', { data: { bboxText } }));

        // Show the typed bbox on the map. Handed over by message rather than
        // by reloading the frame: setting src and then calling reload() on the
        // same frame raced - reload() runs against the pre-hash URL, so the
        // selection could be dropped and the map came back empty. A reload also
        // throws away every tile already fetched, which is the last thing a slow
        // or filtered connection can afford.
        const mapFrame = getMapFrame();
        if (mapFrame && mapFrame.contentWindow) {
          mapFrame.contentWindow.postMessage({
            type: 'setBbox',
            bounds: [lat1, lng1, lat2, lng2]
          }, '*');
        }

        // Update the info text and mark custom input as valid
        customBBoxValid = true;
        selectedBBox = bboxText.replace(/,/g, ' '); // Convert to space format for consistency
        setBboxSelectionInfo(bboxSelectionInfo, "custom_selection_confirmed", "#7bd864");

        // Reset rotation when bbox changes via manual input
        if (typeof window.updateRotation === 'function') {
          window.updateRotation(0);
        }
      } else {
        // Valid numbers but invalid order or range
        customBBoxValid = false;
        // Don't clear selectedBBox - keep map selection if available
        if (!mapSelectedBBox) {
          selectedBBox = "";
        } else {
          selectedBBox = mapSelectedBBox;
        }
        setBboxSelectionInfo(bboxSelectionInfo, "error_coordinates_out_of_range", "#fecc44");
      }
    } else {
      // Input doesn't match the required format
      customBBoxValid = false;
      // Don't clear selectedBBox - keep map selection if available
      if (!mapSelectedBBox) {
        selectedBBox = "";
      } else {
        selectedBBox = mapSelectedBBox;
      }
      setBboxSelectionInfo(bboxSelectionInfo, "invalid_format", "#fecc44");
    }
    // The Precompute button next to this field turns on the selection, and the
    // field is inside the same panel, so it has to follow every keystroke.
    refreshPrecomputeButton();
  });
}

/**
 * Calculates the approximate area of a bounding box in square meters
 * Uses the Haversine formula for geodesic calculations
 * @param {number} lat1 - South latitude
 * @param {number} lng1 - West longitude
 * @param {number} lat2 - North latitude
 * @param {number} lng2 - East longitude
 * @returns {number} Area in square meters
 */
// Radii used to turn a bbox into true ground area for the selected body.
const BODY_RADIUS_M = { earth: 6371000, moon: 1737400, mars: 3396000 };

function calculateBBoxSize(lat1, lng1, lat2, lng2) {
  // Approximate distance calculation using Haversine formula or geodesic formula
  const toRad = (angle) => (angle * Math.PI) / 180;
  // Real ground, not an Earth-sized overestimate: a lunar box reads 13x too large.
  const R = BODY_RADIUS_M[selectedCelestialBody] || BODY_RADIUS_M.earth;

  const latDistance = toRad(lat2 - lat1);
  const lngDistance = toRad(lng2 - lng1);

  const a = Math.sin(latDistance / 2) * Math.sin(latDistance / 2) +
    Math.cos(toRad(lat1)) * Math.cos(toRad(lat2)) *
    Math.sin(lngDistance / 2) * Math.sin(lngDistance / 2);
  const c = 2 * Math.atan2(Math.sqrt(a), Math.sqrt(1 - a));

  // Width and height of the box
  const height = R * latDistance;
  const width = R * lngDistance;

  return Math.abs(width * height);
}

/**
 * Normalizes a longitude value to the range [-180, 180]
 * @param {number} lon - Longitude value to normalize
 * @returns {number} Normalized longitude value
 */
function normalizeLongitude(lon) {
  return ((lon + 180) % 360 + 360) % 360 - 180;
}

// Selection-size warnings, in true square metres of ground. Measured timings and
// world sizes, square selections:
//   Earth 1km2 18s/19MB | 4km2 25s/70MB | 9km2 39s/154MB | 25km2 47s/415MB
//   Moon  2deg 5s/4MB | 5deg 9s/16MB | 10deg 21s/36MB | 20deg 68s/144MB
//   Mars  2deg 5s/4MB | 5deg 9s/16MB | 10deg 20s/36MB | 20deg 51s/100MB
// Earth keeps its long-standing tiers, which guard memory more than the clock.
// The Moon and Mars tiers land near one, three and nine minutes.
const AREA_THRESHOLDS = {
  earth: { extensive: 44e6, large: 85e6, extreme: 500e6 },
  moon: { extensive: 3e11, large: 1e12, extreme: 3e12 },
  mars: { extensive: 1.5e12, large: 5e12, extreme: 1.5e13 }
};

let selectedBBox = "";
let mapSelectedBBox = "";  // Tracks bbox from map selection
let customBBoxValid = false;  // Tracks if custom input is valid

/**
 * Displays the appropriate bbox size status message based on area thresholds
 * @param {HTMLElement} bboxSelectionElement - The element to display the message in
 * @param {number} selectedSize - The calculated bbox area in square meters
 */
function displayBboxSizeStatus(bboxSelectionElement, selectedSize) {
  const t = AREA_THRESHOLDS[selectedCelestialBody] || AREA_THRESHOLDS.earth;
  if (selectedSize > t.extreme) {
    setBboxSelectionInfo(bboxSelectionElement, "area_extreme", "#ff4444");
  } else if (selectedSize > t.large) {
    setBboxSelectionInfo(bboxSelectionElement, "area_too_large", "#fa7878");
  } else if (selectedSize > t.extensive) {
    setBboxSelectionInfo(bboxSelectionElement, "area_extensive", "#fecc44");
  } else {
    setBboxSelectionInfo(bboxSelectionElement, "selection_confirmed", "#7bd864");
  }
}

// Re-runs the size status, e.g. after a body switch changes which tiers apply.
function refreshBboxSelectionInfo() {
  if (!mapSelectedBBox) return;
  const [lat1, lng1, lat2, lng2] = mapSelectedBBox.split(" ").map(Number);
  displayBboxSizeStatus(
    document.getElementById("bbox-selection-info"),
    calculateBBoxSize(lat1, lng1, lat2, lng2)
  );
}

// Function to handle incoming bbox data
function displayBboxInfoText(bboxText) {
  // Two producers, two separators: the map posts formatBounds output, which is
  // space separated, while manual coordinate entry synthesizes a comma
  // separated string. Splitting on " " alone turned the manual one into a
  // single NaN, which then got written back over the user's half-typed
  // coordinates as "NaN,NaN,undefined,NaN" - the next keystroke failed the
  // format check and the selection was gone.
  // lat,lng,lat,lng throughout - what formatBounds emits, what the manual
  // input accepts, and what LLBBox::from_str parses on the Rust side. Do not
  // "fix" this to lng-first; the backend has a comment saying the same.
  let [lat1, lng1, lat2, lng2] = bboxText.trim().split(/[,\s]+/).map(Number);

  // Normalize longitudes
  lng1 = parseFloat(normalizeLongitude(lng1).toFixed(6));
  lng2 = parseFloat(normalizeLongitude(lng2).toFixed(6));
  mapSelectedBBox = `${lat1} ${lng1} ${lat2} ${lng2}`;

  // Map selection always takes priority - clear custom input and update selectedBBox
  selectedBBox = mapSelectedBBox;
  customBBoxValid = false;

  // Reset rotation when bbox changes
  if (typeof window.updateRotation === 'function') {
    window.updateRotation(0);
  }

  const bboxSelectionInfo = document.getElementById("bbox-selection-info");
  const bboxCoordsInput = document.getElementById("bbox-coords");

  // Reset the info text if the bbox is 0,0,0,0
  if (lat1 === 0 && lng1 === 0 && lat2 === 0 && lng2 === 0) {
    setBboxSelectionInfo(bboxSelectionInfo, "select_area_prompt", "#ffffff");
    bboxCoordsInput.value = "";
    mapSelectedBBox = "";
    if (!customBBoxValid) {
      selectedBBox = "";
    }
    window.arnisPreview3D?.onBboxCleared();
    refreshPrecomputeButton();
    return;
  }

  // Update the custom bbox input with the map selection (comma-separated
  // format) - but never type over the user. This also runs as the echo of a
  // bbox they are entering into this very field, and rewriting it mid-edit
  // moves the caret so the next keystroke lands in the wrong place.
  //
  // The echo is caught by value, not by focus: focus can legitimately be
  // elsewhere (the map takes it on interaction, and activeElement is
  // unreliable while the window itself is unfocused) even though the field
  // still holds what the user typed. The focus check then covers the other
  // direction - a genuinely different, map-driven selection arriving while
  // the caret is in the field.
  const current = bboxCoordsInput.value.trim().split(/[,\s]+/).map(Number);
  const echoesField = current.length === 4 && current[0] === lat1 &&
    current[1] === lng1 && current[2] === lat2 && current[3] === lng2;
  if (!echoesField && document.activeElement !== bboxCoordsInput) {
    bboxCoordsInput.value = `${lat1},${lng1},${lat2},${lng2}`;
  }

  // Calculate the size of the selected bbox
  const selectedSize = calculateBBoxSize(lat1, lng1, lat2, lng2);

  displayBboxSizeStatus(bboxSelectionInfo, selectedSize);

  // Hide any rendered mini 3D preview if the selection actually changed
  window.arnisPreview3D?.onBboxChanged(selectedBBox);
  refreshPrecomputeButton();
}

let worldPath = "";

function setWorldNameLabel(text) {
  const label = document.getElementById('world-name-label');
  if (!label) return;
  if (text) {
    label.removeAttribute('data-placeholder');
    label.textContent = text;
  } else {
    label.setAttribute('data-placeholder', 'true');
    localizeElement(window.localization, { element: label }, 'no_world_generated_yet');
  }
}

function basenameFromPath(p) {
  if (!p) return "";
  return p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "";
}

/**
 * Handles world selection errors and displays appropriate messages
 * @param {number} errorCode - Error code from the backend
 */
function handleWorldSelectionError(errorCode) {
  const errorKeys = {
    1: "minecraft_directory_not_found",
    2: "world_in_use",
    3: "failed_to_create_world",
    4: "no_world_selected_error"
  };

  const errorKey = errorKeys[errorCode] || "unknown_error";
  const progressInfo = document.getElementById('progress-info');
  localizeElement(window.localization, { element: progressInfo }, errorKey);
  progressInfo.style.color = "#fa7878";
  worldPath = "";
  setWorldNameLabel("");
  console.error(errorCode);
}

let generationButtonEnabled = true;

/**
 * Initiates the world generation process
 * Validates required inputs and sends generation parameters to the backend
 * @returns {Promise<void>}
 */
async function startGeneration() {
  if (generationButtonEnabled === false) {
    return;
  }
  // The backend refuses this too, but only after gui_create_world has already
  // made an empty world for a run that is not going to happen. Said here, the
  // world is never created and the user is told where the machine has gone.
  if (precomputeRunning) {
    const info = document.getElementById('progress-info');
    if (info) {
      info.textContent = "Waiting for the Mapillary precompute. Cancel it in Settings, or let it finish.";
      info.style.color = "#fecc44";
    }
    return;
  }
  // Claim the guard before the first await. gui_create_world and gui_start_generation are
  // both awaited round-trips, so leaving the claim until after them lets a second click
  // through and starts a parallel run against the same process-global world floor.
  generationButtonEnabled = false;
  // The two jobs exclude each other in the backend, so grey the other one out.
  refreshPrecomputeButton();
  let started = false;

  try {
    if (!selectedBBox || selectedBBox == "0.000000 0.000000 0.000000 0.000000") {
      const bboxSelectionInfo = document.getElementById('bbox-selection-info');
      setBboxSelectionInfo(bboxSelectionInfo, "select_location_first", "#fa7878");
      return;
    }

    // Auto-create world for Java format
    if (selectedWorldFormat === 'java') {
      if (!savePath) {
        console.warn("Cannot create world: save path not set");
        return;
      }
      try {
        const worldName = await invoke('gui_create_world', { savePath: savePath });
        if (worldName) {
          worldPath = worldName;
          setWorldNameLabel(basenameFromPath(worldName));
        }
      } catch (error) {
        handleWorldSelectionError(error);
        return;
      }
    }

    // Clear any existing world preview since we're generating a new one
    notifyWorldChanged();

    // Get the map iframe reference
    const mapFrame = document.querySelector('.map-container');
    // Get spawn point coordinates if marker exists
    let spawnPoint = null;
    if (mapFrame && mapFrame.contentWindow && mapFrame.contentWindow.getSpawnPointCoords) {
      const coords = mapFrame.contentWindow.getSpawnPointCoords();
      // Convert object format to tuple format if coordinates exist
      if (coords) {
        spawnPoint = [coords.lat, coords.lng];
      }
    }

    // Get generation mode from dropdown
    var generationMode = document.getElementById("generation-mode-select").value;
    var terrain = (generationMode === "geo-terrain" || generationMode === "terrain-only");
    var skipOsmObjects = (generationMode === "terrain-only");

    var interior = document.getElementById("interior-toggle").checked;
    var fill_ground = document.getElementById("fillground-toggle").checked;
    var legacy_trees = document.getElementById("legacy-trees-toggle").checked;
    var canopy_height = document.getElementById("canopy-height-toggle").checked;
    var maxTreeSizeBtn = document.querySelector("#max-tree-size-group .segment.active");
    var maxTreeSize = maxTreeSizeBtn ? maxTreeSizeBtn.dataset.maxTreeSize : "giant";
    var overture = document.getElementById("overture-toggle").checked;
    var use_3d = document.getElementById("use-3d-toggle").checked;
    var heightLimitToggle = document.getElementById("disable-height-limit-toggle");
    // Disabled means unsupported for this body or format, so never send a stale tick.
    var disable_height_limit = !heightLimitToggle.disabled && heightLimitToggle.checked;
    var aws_only_elevation = document.getElementById("aws-only-elevation-toggle").checked;
    var bake_lighting = document.getElementById("bake-lighting-toggle").checked;
    var voxy_lod = document.getElementById("voxy-lod-toggle").checked;
    var scale = parseFloat(document.getElementById("scale-value-slider").value);
    // var ground_level = parseInt(document.getElementById("ground-level").value, 10);
    // DEPRECATED: Ground level input removed from UI
    var ground_level = -62;

    // Validate ground_level
    ground_level = isNaN(ground_level) || ground_level < -62 ? -62 : ground_level;

    // Get telemetry consent (defaults to false if not set)
    const telemetryConsent = window.getTelemetryConsent ? window.getTelemetryConsent() : false;

    // Get rotation angle
    var rotationAngle = parseFloat(document.getElementById("rotation-angle-input").value) || 0;

    var gamemodeBtn = document.querySelector("#gamemode-group .segment.active");
    var gamemode = gamemodeBtn ? gamemodeBtn.dataset.gamemode : "creative";
    var mapItem = document.getElementById("map-item-toggle").checked;
    var signageBtn = document.querySelector("#signage-group .segment.active");
    var signage = signageBtn ? signageBtn.dataset.signage : "basic";
    // Clock minutes -> Minecraft ticks (tick 0 = 06:00; 24:00 wraps to 00:00)
    var clockMinutes = (parseInt(document.getElementById("world-time-slider").value, 10) || 0) % 1440;
    var worldTime = Math.round(((clockMinutes + 1440 - 360) % 1440) * (24000 / 1440));

    // Pass the selected options to the Rust backend
    await invoke("gui_start_generation", {
        bboxText: selectedBBox,
        selectedWorld: worldPath,
        bedrockSavePath: bedrockSavePath,
        luantiSavePath: luantiSavePath,
        worldScale: scale,
        groundLevel: ground_level,
        terrainEnabled: terrain,
        skipOsmObjects: skipOsmObjects,
        interiorEnabled: interior,
        fillgroundEnabled: fill_ground,
        legacyTreesEnabled: legacy_trees,
        maxTreeSize: maxTreeSize,
        canopyHeightEnabled: canopy_height,
        overtureEnabled: overture,
        use3dEnabled: use_3d,
        disableHeightLimit: disable_height_limit,
        awsOnlyElevation: aws_only_elevation,
        bakeLightingEnabled: bake_lighting,
        voxyLodEnabled: voxy_lod,
        isNewWorld: true,
        spawnPoint: spawnPoint,
        telemetryConsent: telemetryConsent || false,
        worldFormat: getEffectiveWorldFormat(),
        rotationAngle: rotationAngle,
        gamemode: gamemode,
        worldTime: worldTime,
        mapItem: mapItem,
        signage: signage,
        mapillaryToken: getMapillaryToken(),
        facadesEnabled: getFacadesEnabled(),
        facadeMode: getEffectiveFacadeMode(),
        buildingFacadesEnabled: getBuildingFacadesEnabled(),
        facadeDetail: getFacadeDetail(),
        celestialBodyName: selectedCelestialBody
    });

    console.log("Generation process started.");
    setEtaSignageExpected(signage !== "none" && getEffectiveWorldFormat() === "java");
    resetEta();
    started = true;
    window.arnisPreview3D?.setGenerationRunning(true);
  } catch (error) {
    console.error("Error starting generation:", error);
  } finally {
    // Hand the guard back unless a run actually started; once it has, the Done!/Error!
    // progress message releases it instead.
    if (!started) {
      generationButtonEnabled = true;
      window.arnisPreview3D?.setGenerationRunning(false);
    }
    refreshPrecomputeButton();
  }
}

// World preview overlay state
let worldPreviewEnabled = false;
let currentWorldMapData = null;

/**
 * Notifies the map iframe that world preview data is ready
 * Called when the backend emits the map-preview-ready event
 */
async function showWorldPreviewButton() {
  // Try to load the world map data
  await loadWorldMapData();

  if (currentWorldMapData) {
    // Send data to the map iframe
    const mapFrame = document.querySelector('.map-container');
    if (mapFrame && mapFrame.contentWindow) {
      mapFrame.contentWindow.postMessage({
        type: 'worldPreviewReady',
        data: currentWorldMapData
      }, '*');
      console.log("World preview data sent to map iframe");
    }
  } else {
    console.warn("Map data not available yet");
  }
}

/**
 * Notifies the map iframe that the world has changed (reset preview)
 */
function notifyWorldChanged() {
  currentWorldMapData = null;
  const mapFrame = document.querySelector('.map-container');
  if (mapFrame && mapFrame.contentWindow) {
    mapFrame.contentWindow.postMessage({
      type: 'worldChanged'
    }, '*');
  }
}

/**
 * Loads the world map data from the backend
 */
async function loadWorldMapData() {
  try {
    const mapData = await invoke('gui_get_world_map_data', { worldPath: worldPath });
    if (mapData) {
      currentWorldMapData = mapData;
      console.log("World map data loaded successfully");
    }
  } catch (error) {
    console.error("Failed to load world map data:", error);
  }
}
