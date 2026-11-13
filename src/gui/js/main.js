import { licenseText } from './license.js';
import { fetchLanguage, interfaceLocale, invalidJSON, matchLanguage } from './language.js';
import { renderMarkdown, pickAssetForPlatform } from './update.js';
import {
  initSettingsStore,
  setDynamicDefault,
  refreshSettingsState,
  localizeSettingsStore,
  cancelSettingsResetConfirm,
  flushSettingsStore,
} from './settings-store.js';
import { initPreviews } from './preview-popover.js';
import { initSettingsLayout, syncSettingsLayout } from './settings-layout.js';
import { initNewBadges } from './new-badges.js';
import {
  t as oneWorldText,
  initMeldFeatures, refreshLayout, meldFlags, cellsKey, generationTarget, generationShapes, progressText, holdsNotice,
  initRunControls, initCellMenu, notePiece, clearPieces, dropStalePieces, resetPieces, showRunControls,
  initJobPlan, schedulePlan, planPieces, planPending, isPiecesJob, selectWithRing,
  setPlanMask, hasPlanMask, refreshStretch,
  initDataPanel, initProjects, resumeJob, placeForRun,
} from './meld-features.js';

let invoke;
if (window.__TAURI__) {
  invoke = window.__TAURI__.core.invoke;
} else {
  function dummyFunc() { }
  window.__TAURI__ = { event: { listen: dummyFunc } };
  // A plain browser preview: callers chain .then on invoke.
  invoke = async () => undefined;
}

const DEFAULT_LOCALE_PATH = `./locales/en.json`;

// Track current bbox selection info localization key for language changes
let currentBboxSelectionKey = "select_area_prompt";
let currentBboxSelectionColor = "#ffffff";
let currentBboxSelectionVars = null;

// "{n}"-style values of the current bbox line (area_pieces_info), kept for language changes.
function fillBboxSelectionVars(element) {
  for (const k in (currentBboxSelectionVars || {})) {
    element.textContent = element.textContent.split('{' + k + '}').join(currentBboxSelectionVars[k]);
  }
}

// Helper function to set bbox selection info text and track it for language changes
async function setBboxSelectionInfo(bboxSelectionElement, localizationKey, color, vars) {
  // Only a size warning brings the Advanced link back (displayBboxSizeStatus).
  const cta = document.getElementById('bbox-features-cta');
  if (cta) cta.hidden = true;
  currentBboxSelectionKey = localizationKey;
  currentBboxSelectionColor = color;
  currentBboxSelectionVars = vars || null;
  
  // Ensure localization is available
  let localization = window.localization;
  if (!localization) {
    localization = await getLocalization();
  }
  
  localizeElement(localization, { element: bboxSelectionElement }, localizationKey);
  fillBboxSelectionVars(bboxSelectionElement);
  bboxSelectionElement.style.color = color;
  // The map-first card says the pieces in its plan block; this line would repeat them.
  bboxSelectionElement.classList.toggle('is-plan', localizationKey === 'area_pieces_info' || localizationKey === 'area_single_info');
}

// Initialize elements and start the demo progress
window.addEventListener("DOMContentLoaded", async () => {
  applyStoredLayout();
  registerMessageEvent();
  window.startGeneration = startGeneration;
  setupProgressListener();
  await initSavePath();
  initSettings();
  // Before the store restores values, so the cards follow the restored ones.
  initSettingsLayout();
  initDialogs();
  initLodLightingCoupling();
  initCavesFillCoupling();
  initDisastersCoupling();
  initMobSpawningLockout();
  initAdvancedRows();
  refreshHeightLimitRow();
  refreshGameplayRows();
  // After initSettings(), so the slider label and rotation handlers exist
  // before restored values are applied. Labels get localized a few lines below.
  initSettingsStore({ resetWorldFormat: () => setWorldFormat('java') });
  // The restore may have switched Advanced features back on under a saved Hytale format.
  refreshAdvancedFeaturesForFormat(selectedWorldFormat);
  initNewBadges();
  syncDiscordStatus();
  initOfflineModeSync();
  // The store's restore fired the toggle's change event before the save path
  // and the world name were known; read the world once more with both in hand.
  refreshOneWorldState();
  resolveDefaultSavePath();
  initTelemetryConsent();
  initClearCacheButton();
  initPrecomputeFacadesButton();
  initTooltips();
  initPreviews();
  handleBboxInput();
  const localization = await getLocalization();
  await applyLocalization(localization);
  updateFormatToggleUI(selectedWorldFormat);
  initEasterEggs();
  checkForUpdates();
});

// The stored Main Page Layout, before the first await: the store restores it only later, and a
// Map first page would show as Classic until then (meld-features.js keeps it from there).
function applyStoredLayout() {
  try {
    const values = JSON.parse(localStorage.getItem('arnis-settings') || '{}').values || {};
    // Auto is the default, so it is not stored; it shows the map first for a build in cells.
    const mode = values['layout-select'] || 'auto';
    const cells = values['advanced-meld-features-toggle'] === true && values['one-world-group'] === 'on' && !!values['meld-workers-select'];
    document.body.classList.toggle('map-first', mode === 'map' || (mode === 'auto' && cells));
  } catch (_) { /* the store's own restore sets it */ }
}

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

  // Otherwise the offered language closest to the browser's
  const select = document.getElementById("language-select");
  const offered = select ? Array.from(select.options).map(opt => opt.value) : [];
  return await fetchLanguage(detectBrowserLanguage(offered));
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
    // Advanced features: the size warning's way out and the One World's Expand.
    "#bbox-features-cta": "meld_use_pieces",
    "#plan-expand-button": "plan_expand",
    "#world-name-label[data-placeholder]": "no_world_generated_yet",
    "input[id='world-name-input']": "placeholder_world_name",
    // DEPRECATED: Ground level localization removed
    // "label[data-localize='ground_level']": "ground_level",
    ".footer-link": "footer_text",

    // Placeholder strings
    "input[id='bbox-coords']": "placeholder_bbox",
    // DEPRECATED: Ground level placeholder removed
    // "input[id='ground-level']": "placeholder_ground"
  };

  for (const selector in localizationElements) {
    localizeElement(localization, { selector: selector }, localizationElements[selector]);
  }

  // Every text on the settings page and in the dialogs carries its key, and
  // several appear more than once (a section name is in the sidebar and on the
  // section itself), so they are localized in one pass rather than selector by
  // selector.
  document.querySelectorAll("#settings-modal [data-localize], .dialog [data-localize], #run-controls [data-localize], #run-confirm [data-localize], #cells-confirm [data-localize]").forEach((element) => {
    localizeElement(localization, { element }, element.dataset.localize);
  });

  // Attributes: tooltips (English stays in data-tooltip as the fallback) and
  // title / aria-label, anywhere on the page.
  const loc = (key, fallback) => localization[key] ?? fallback;
  document.querySelectorAll("[data-tooltip-key]").forEach((el) => {
    const text = loc(el.dataset.tooltipKey, el.dataset.tooltip);
    el.dataset.tooltip = text;
    el.setAttribute("aria-label", text);
  });
  document.querySelectorAll("[data-localize-title]").forEach((el) => {
    el.title = loc(el.dataset.localizeTitle, el.title);
  });
  document.querySelectorAll("[data-localize-aria-label]").forEach((el) => {
    el.setAttribute("aria-label", loc(el.dataset.localizeAriaLabel, el.getAttribute("aria-label")));
  });
  document.querySelectorAll("[data-localize-alt]").forEach((el) => {
    el.alt = loc(el.dataset.localizeAlt, el.alt);
  });

  // settings-store.js creates these buttons and owns their text.
  localizeSettingsStore(localization);

  // Re-apply current bbox selection info text with new language
  const bboxSelectionInfo = document.getElementById("bbox-selection-info");
  if (bboxSelectionInfo && currentBboxSelectionKey) {
    localizeElement(localization, { element: bboxSelectionInfo }, currentBboxSelectionKey);
    fillBboxSelectionVars(bboxSelectionInfo);
    bboxSelectionInfo.style.color = currentBboxSelectionColor;
  }

  // Update error messages
  window.localization = localization;
  renderOneWorldStatus();
  refreshStartButton();
  // An open Projects list is built in the language it was drawn in.
  redrawProjects();
  window.dispatchEvent(new Event('arnis-localized'));
  // The plan block is built in the language it was drawn in.
  schedulePlan(true);
  labelLiveViewDistances();
  fillLiveVersions();
  // liveStatus stays "stopped" after a session, which must not take over a normal run's line.
  if (liveActive()) renderLiveProgress();
  // The map hint lives in the map iframe, which cannot see this assignment.
  document.querySelectorAll('iframe').forEach((frame) => {
    try {
      const w = frame.contentWindow;
      if (w && typeof w.renderBboxHint === 'function') w.renderBboxHint();
      // The toolbar's labels and the search placeholder, same reason.
      if (w && typeof w.refreshMapToolLabels === 'function') w.refreshMapToolLabels();
    } catch (_) {
      // A frame that is not ours or not loaded yet; the hint renders itself
      // once it is.
    }
  });

  // The line above has just written the idle label over a button that may be
  // saying Cancel, so put the running state back.
  refreshPrecomputeButton();
  // The pass wrote the footer's raw template ("{year}", "{version}") back; every
  // pass fills it in, whoever started it.
  initFooter();
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

    // The map-first page shows the version under the logo on hover.
    document.getElementById("arnis-logo")?.closest("a")?.setAttribute("data-version", `v${version}`);
    // The About section of the settings page repeats the same line.
    const aboutVersion = document.getElementById("about-version");
    if (aboutVersion) aboutVersion.textContent = footerElement.textContent;
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
  const info = latestReleaseInfo;
  // Version info opened from Settings only reads, unless the release it shows
  // is newer than this build: then the download is offered there as well.
  const showDownload =
    opts.showDownload !== false || !!(info && info.release && info.isNewer);
  const modal = document.getElementById("update-modal");
  if (!modal) return;
  const titleEl = document.getElementById("update-modal-title");
  const bodyEl = document.getElementById("update-modal-body");
  const downloadBtn = document.getElementById("update-download-button");
  const downloadNote = document.getElementById("update-modal-download-note");

  // Hide download button + the "opens in your browser" note in version-info mode.
  if (downloadBtn) downloadBtn.style.display = showDownload ? "" : "none";
  if (downloadNote) downloadNote.style.display = showDownload ? "" : "none";

  const metaEl = document.getElementById("update-modal-meta");

  if (!info || !info.release) {
    titleEl.textContent =
      (window.localization && window.localization.update_modal_title) || "What's new in Arnis";
    if (metaEl) metaEl.replaceChildren();
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
  renderReleaseMeta(metaEl, info, rel);
  bodyEl.innerHTML = renderMarkdown(rel.body || "");
  bodyEl.scrollTop = 0;
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
    // Only the label: the button also holds its icon.
    const label = downloadBtn.querySelector(".dialog-button-label") || downloadBtn;
    label.textContent =
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

// The line under the release title: its date, then the version, or
// "3.2.0 → 3.3.0" when it is newer than this build. A localized date and
// numbers only, so nothing here needs a translation.
function renderReleaseMeta(metaEl, info, rel) {
  if (!metaEl) return;
  metaEl.replaceChildren();
  const parts = [];
  const date = formatReleaseDate(rel.published_at);
  if (date) parts.push(document.createTextNode(date));

  const plain = (v) => String(v || "").replace(/^v/i, "");
  const remote = plain(info.remoteVersion || rel.tag_name);
  if (info.isNewer && info.localVersion && remote) {
    const version = document.createElement("span");
    const newer = document.createElement("span");
    newer.className = "is-new";
    newer.textContent = remote;
    version.append(`${plain(info.localVersion)} → `, newer);
    parts.push(version);
  } else if (remote) {
    parts.push(document.createTextNode(remote));
  }

  parts.forEach((part, i) => {
    if (i > 0) metaEl.append(" · ");
    metaEl.append(part);
  });
}

function formatReleaseDate(iso) {
  if (!iso) return "";
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  try {
    return new Intl.DateTimeFormat(interfaceLocale(), { dateStyle: "long" }).format(date);
  } catch (_) {
    return date.toDateString();
  }
}

function closeUpdateModal() {
  const modal = document.getElementById("update-modal");
  if (modal) hideModal(modal);
}

// Where focus returns to once a dialog closes.
const modalReturnFocus = new WeakMap();

// Shows a dialog's backdrop and moves focus onto the dialog, so the keyboard
// acts on it rather than on whatever is behind.
function showModal(modal) {
  if (modal.style.display !== "flex") {
    modalReturnFocus.set(modal, document.activeElement);
  }
  modal.style.display = "flex";
  modal.style.justifyContent = "center";
  modal.style.alignItems = "center";
  const dialog = modal.querySelector(".dialog");
  if (dialog) dialog.focus({ preventScroll: true });
}

function hideModal(modal) {
  modal.style.display = "none";
  const back = modalReturnFocus.get(modal);
  modalReturnFocus.delete(modal);
  if (back && document.contains(back) && typeof back.focus === "function") {
    back.focus({ preventScroll: true });
  }
}

// A click on the backdrop closes the dialog, but only when the press started
// there too: selecting text inside and letting go outside must not close it.
function closeOnBackdrop(modal, close) {
  let pressedOnBackdrop = false;
  modal.addEventListener("pointerdown", (event) => {
    pressedOnBackdrop = event.target === modal;
  });
  modal.addEventListener("click", (event) => {
    if (pressedOnBackdrop && event.target === modal) close();
    pressedOnBackdrop = false;
  });
}

// License, version info, the 3D preview and the Discord invite ask nothing, so
// a click beside them may close them. The consent dialog is left alone: it
// wants an answer.
function initDialogs() {
  const license = document.getElementById("license-modal");
  const update = document.getElementById("update-modal");
  const preview3d = document.getElementById("preview3d-modal");
  const discord = document.getElementById("discord-modal");
  if (license) closeOnBackdrop(license, () => window.closeLicense());
  if (update) closeOnBackdrop(update, closeUpdateModal);
  if (preview3d) closeOnBackdrop(preview3d, () => window.closePreview3D && window.closePreview3D());
  if (discord) closeOnBackdrop(discord, closeDiscordModal);
}

const DISCORD_INVITE_URL = "https://discord.gg/Tba5NKzKjT";
const DISCORD_PROMPT_KEY = "discord-prompt-shown";
let sessionGenerations = 0;

// Invites once ever, after the second finished world of a session. If another
// dialog is up at that moment, it waits for the next one.
function noteGenerationDone() {
  sessionGenerations++;
  if (sessionGenerations < 2) return;
  // A beat later, so the green "Done!" registers before the dialog covers it.
  setTimeout(() => {
    if (localStorage.getItem(DISCORD_PROMPT_KEY)) return;
    const modal = document.getElementById("discord-modal");
    const busy = [...document.querySelectorAll(".modal, #settings-modal")]
      .some((m) => m.style.display === "flex");
    if (!modal || busy) return;
    // Marked on show, so closing Arnis without answering also counts.
    localStorage.setItem(DISCORD_PROMPT_KEY, "true");
    showModal(modal);
  }, 1000);
}

function closeDiscordModal() {
  const modal = document.getElementById("discord-modal");
  if (modal) hideModal(modal);
}

function joinDiscord() {
  openExternal(DISCORD_INVITE_URL);
  closeDiscordModal();
}

window.closeDiscordModal = closeDiscordModal;
window.joinDiscord = joinDiscord;

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
  'winter-toggle',
  'post-apocalyptic-toggle',
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
const EARTH_ONLY_SEGMENTED = ['max-tree-size-group', 'field-preset-group', 'signage-group'];

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
  const stored = localStorage.getItem('facadeMode');
  if (stored === 'blocks') return 'blocks';
  // Anything else means the photographs: 'photos' itself, the 'paintings' and
  // 'paintings-v2' an earlier build saved for what are now the photo panels,
  // and nothing at all, which takes the default the same way the backend does.
  return 'photos';
}

// The photo panels are Java entities carried by a resource pack, so no other
// world format can show them. The stored choice is left alone so that going
// back to Java restores it; only what the backend is asked for changes.
function getEffectiveFacadeMode() {
  const mode = getFacadeMode();
  // One World builds Mapillary facades as blocks.
  if (mode !== 'blocks' && (selectedWorldFormat !== 'java' || isOneWorldEnabled())) return 'blocks';
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

// Only the presets are Java only. Mapillary falls back to block facades on
// Bedrock, Luanti and Hytale, which is what the notice under the mode control says.
// The Moon and Mars are terrain only, so there is no wall to hang anything on
// and every source resolves to off there. As with the format gate, the stored
// choice is left alone so coming back to Earth restores it.
function getEffectiveFacadeSource() {
  const source = getFacadeSource();
  if (selectedCelestialBody !== 'earth') return 'off';
  // The presets are off in One World too.
  if (source === 'preset' && (selectedWorldFormat !== 'java' || isOneWorldEnabled())) return 'off';
  return source;
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
  const earth = selectedCelestialBody === 'earth';
  const source = getEffectiveFacadeSource();

  // Off Earth the whole section is dead, the source picker included, so it is
  // greyed like the row it sits in rather than left looking live.
  group.classList.toggle('segmented-disabled', !earth);
  const sourceRow = group.closest('.settings-row');
  if (sourceRow) {
    sourceRow.classList.toggle('settings-row-unavailable', !earth);
    setRowReason(sourceRow, earth ? null : 'reason_earth');
  }
  group.querySelectorAll('.segment').forEach((btn) => {
    btn.disabled = !earth || ((!java || isOneWorldEnabled()) && btn.dataset.facadeSource === 'preset');
    btn.classList.toggle('active', btn.dataset.facadeSource === source);
  });

  // A row that belongs to another source is hidden: it has nothing to say
  // until that source is picked, and the whole Mapillary block greyed out
  // under Off only buried the rest of the page. A row that belongs to this
  // source but cannot act yet (no token, wrong format) is greyed instead, so
  // it is still there to explain itself.
  const grey = (id, shown, live, reason = null) => {
    const el = document.getElementById(id);
    const row = el && el.closest('.settings-row');
    if (row) {
      row.style.display = shown ? '' : 'none';
      row.classList.toggle('settings-row-unavailable', !live);
      setRowReason(row, live ? null : reason);
    }
    if (el) {
      el.querySelectorAll('.segment, input, button').forEach((c) => {
        c.disabled = !live;
      });
      if (el.tagName === 'INPUT' || el.tagName === 'BUTTON') el.disabled = !live;
    }
  };
  const mapillary = source === 'mapillary';
  const token = !!getMapillaryToken();
  grey('mapillary-token', mapillary, mapillary);
  grey('facade-mode-group', mapillary, mapillary && token, 'reason_token');
  // Panel resolution, so it means nothing where no panels are hung.
  grey('facade-detail-group', source !== 'off', source !== 'off' && java, 'reason_java');
  grey('precompute-facades-button', mapillary, mapillary && token, 'reason_token');
}

// The facade rows react to the world format (panels are Java only), the on/off
// switch and the token. A row that cannot do anything is greyed out rather than
// left looking live, and the Precompute button reads the same facts.
function refreshFacadeRows() {
  const group = document.getElementById('facade-mode-group');
  if (!group) return;

  const java = selectedWorldFormat === 'java';
  const effective = getEffectiveFacadeMode();
  // The source rows enable the whole group first; photo panels stay off where they cannot hang.
  refreshFacadeSourceRows();
  group.querySelectorAll('.segment').forEach((btn) => {
    const panels = btn.dataset.facadeMode !== 'blocks';
    if (panels && (!java || isOneWorldEnabled())) btn.disabled = true;
    btn.classList.toggle('active', btn.dataset.facadeMode === effective);
  });
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
    if (row) {
      row.classList.toggle('settings-row-unavailable', off);
      setRowReason(row, off ? 'reason_earth' : null);
    }
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

  // Gated on more than the body, so they cannot simply follow the earth-only
  // loop: the height-limit pack is also format-gated and the facade rows read
  // the world format and the stored source too.
  refreshHeightLimitRow();
  refreshGameplayRows();
  refreshWorldTypeRow();
  refreshJavaOnlyRows();
  refreshFacadeRows();
  refreshOneWorldState();

  // A default, not a lock. Slider units are clock minutes: 0 midnight, 720 noon.
  const timeSlider = document.getElementById('world-time-slider');
  if (timeSlider) {
    timeSlider.value = off ? 0 : 720;
    timeSlider.dispatchEvent(new Event('input', { bubbles: true }));
  }

  // The cached preview describes a world on the body we just left. The map
  // clears its own overlay in changeBody; this drops the parent's copy so a
  // later ready event cannot put a stale one back.
  currentWorldMapData = null;

  // Warnings are per body, so the current selection may read differently now.
  refreshBboxSelectionInfo();
}

// The last search result used for a selection, and { name, path } of the run that builds a One
// World's first land, taken when it starts.
let searchedPlace = null;
let runPlace = null;

// That world is named after the search result once the run is done.
async function rememberRunPlace() {
  const run = runPlace;
  runPlace = null;
  if (run) await invoke('gui_set_place_name', { worldPath: run.path, placeName: run.name }).catch(() => {});
}

// Function to register the event listener for bbox updates from iframe
function registerMessageEvent() {
  window.addEventListener('message', function (event) {
    const bboxText = event.data.bboxText;

    // Typed messages are handled below; only untyped bboxText messages are
    // selection updates (typed ones carrying coordinates must not be).
    if (bboxText && !event.data.type) {
      console.log("Updated BBOX Coordinates:", bboxText);
      displayBboxInfoText(bboxText, event.data.shapes);
    }

    // "Select this area" on a search result: the area and, in a One World, a ring of regions around it.
    if (event.data && event.data.type === 'selectArea' && event.source === getMapFrame()?.contentWindow) {
      selectWithRing(event.data.bounds, isOneWorldEnabled());
    }

    // The search result the selection came from: its name goes to the world a build makes.
    if (event.data && event.data.type === 'searchPlace' && event.source === getMapFrame()?.contentWindow) {
      searchedPlace = { name: String(event.data.name), lat: Number(event.data.lat), lon: Number(event.data.lon) };
    }

    // The outline the selection was made from (shape tool or "Use outline"), or null.
    if (event.data && event.data.type === 'planMask' && event.source === getMapFrame()?.contentWindow) {
      noteOutline(event.data.geojson);
    }

    // World toggled on the map toolbar
    if (event.data && event.data.type === 'bodyChanged') {
      setCelestialBody(event.data.body);
    }

    // Handle angle measurement from the map polyline tool
    if (event.data && event.data.type === 'angleMeasured') {
      // A One World is aligned to real coordinates; the tool is disabled on
      // the map too, this only covers a measurement already in flight.
      if (isOneWorldEnabled()) return;
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
    setEtaText(el, "");
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

// The estimate's label is two stacked copies (white over the fill, dark over the track; styles.css).
function setEtaText(el, text) {
  for (const span of el.children) span.textContent = text;
}

function renderEta() {
  const el = document.getElementById("progress-eta");
  if (!el) return;
  // Show nothing until we actually have an estimate (no "…" placeholder).
  if (!eta || eta.shown == null) {
    el.classList.remove("visible");
    setEtaText(el, "");
    el.removeAttribute("aria-label");
    return;
  }
  // Floor at 1s so it never reads "0s" before Done.
  const value = formatEtaDuration(Math.max(1, eta.shown));
  setEtaText(el, value); // duration only on the bar; no "~" prefix
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

// What the status line says between the click and the backend's first real
// progress event. Kept in a constant because the abort paths have to be able to
// recognize it as still-unclaimed and take it back down.
const STARTING_MESSAGE = "Starting...";

// The bar and the status line only ever move on a `progress-update` event, and
// the first one is a long way from the click: a world folder gets created, a
// spawn point written into level.dat, a datapack possibly installed, disk space
// probed and a session lock taken, all before the download that emits 1%. Until
// then the previous run's green "Done!" and full bar sit there, which reads as
// the button having done nothing. So claim both here, at the moment the click
// is accepted, and let the first real update take over from 0%.
function resetProgressUi(message) {
  const bar = document.getElementById("progress-bar");
  const info = document.getElementById("progress-info");
  const detail = document.getElementById("progress-detail");
  if (bar) bar.style.width = "0%";
  if (detail) detail.textContent = "0%";
  if (info) {
    info.textContent = message;
    info.style.color = "#ececec";
    info.classList.remove("is-notice");
  }
  noticeSince = 0;
  prefetchDone = false;
  resetEta();
}

// A finished Download now leaves "Done!" and a full bar, which say nothing about a new selection
// or plan; they go once the plan underneath changes. An error stays until the next run.
let prefetchDone = false;
function clearPrefetchDone() {
  if (prefetchDone && generationButtonEnabled) resetProgressUi("");
}

// When the status line last showed a fallback notice (0: none).
let noticeSince = 0;

// Function to set up the progress bar listener
function setupProgressListener() {
  const progressBar = document.getElementById("progress-bar");
  const progressInfo = document.getElementById("progress-info");
  const progressDetail = document.getElementById("progress-detail");

  window.__TAURI__.event.listen("progress-update", (event) => {
    if (event.payload.piece) {
      notePiece(event.payload.piece);
      showRunControls(!generationButtonEnabled);
      return;
    }
    const { progress, message, streaming } = event.payload;

    if (progress != -1) {
      progressBar.style.width = `${progress}%`;
      progressDetail.textContent = `${Math.round(progress)}%`;
      updateEta(progress, streaming);
    }

    if (message != "") {
      const shown = progressText(event.payload);
      // A fallback notice outlasts the stage lines that follow it.
      if (!holdsNotice(shown, message, noticeSince, Date.now())) {
        progressInfo.textContent = shown.text;
        progressInfo.title = shown.title;
        progressInfo.classList.toggle("is-notice", shown.notice);
        noticeSince = shown.notice ? Date.now() : 0;
      }
      if (shown.log) window.arnisLog?.(shown.notice ? "warn" : "error", shown.log);

      if (bakeRunning && generationButtonEnabled && /^(Error!|Done!)/.test(message)) {
        // A bake's end is not a run's: no world name, facades or Discord prompt follow it.
        progressInfo.style.color = message.startsWith("Done!") ? "#7bd864" : "#fa7878";
        resetEta();
      } else if (message.startsWith("Error!")) {
        progressInfo.style.color = "#fa7878";
        setGenerationButtonEnabled(true);
        window.arnisPreview3D?.setGenerationRunning(false);
        if (lastRunOneWorld) {
          // The world keeps its name; the status line carries the reason.
          setWorldNameLabel(oneWorldDisplayName());
          if (isOneWorldEnabled()) setOneWorldStatus(message.replace(/^Error!\s*/, ''), 'error');
        } else if (!lastRunPrewarm) {
          setWorldNameLabel("");
        }
        resetEta();
      } else if (message.startsWith("Done!")) {
        progressInfo.style.color = "#7bd864";
        setGenerationButtonEnabled(true);
        window.arnisPreview3D?.setGenerationRunning(false);
        resetEta();
        // The world just grew by one area: reload its status and overlays.
        if (lastRunOneWorld) {
          oneWorldOverlayKey = null;
          refreshOneWorldState().then(rememberRunPlace);
        }
        // A generation just built facades into the same cache, so the preview
        // has something new to say.
        if (lastRunPrewarm) {
          prefetchDone = true;
        } else {
          window.arnisPreview3D?.refreshFacades();
          noteGenerationDone();
        }
      } else if (message.startsWith("Stopped.") || message.startsWith("The retried cells are built")) {
        // A job in pieces ends here when stopped or when only some pieces were asked for.
        progressInfo.style.color = "#ececec";
        setGenerationButtonEnabled(true);
        window.arnisPreview3D?.setGenerationRunning(false);
        resetEta();
        if (lastRunOneWorld) {
          oneWorldOverlayKey = null;
          refreshOneWorldState();
        }
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
  // The estimate's two copies are clipped at the fill's edge: follow the fill's width (also while it animates).
  const fill = document.getElementById('progress-bar');
  if (fill && window.ResizeObserver) {
    new ResizeObserver(() => progressBar.style.setProperty('--p', `${fill.offsetWidth}px`)).observe(fill);
  }
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
  return matchLanguage(navigator.language, availableOptions);
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
  resolve('gui_get_default_hytale_save_path', 'hytaleSavePath');
}

function initSettings() {
  // Settings
  const settingsModal = document.getElementById("settings-modal");
  const slider = document.getElementById("scale-value-slider");
  const sliderValue = document.getElementById("slider-value");

  // Where focus goes back to once the page closes.
  let focusBeforeSettings = null;

  let refreshProjects = null;
  // Open and Expand follow the format, the body, Live and a run while the page is open; with
  // Advanced features off the page is hidden and its list is never read.
  const projectsShown = () => refreshProjects && document.getElementById('advanced-meld-features-toggle').checked;
  redrawProjects = () => { if (projectsShown() && settingsModal.style.display === "flex") refreshProjects(); };

  // Open the settings page
  function openSettings() {
    if (projectsShown()) refreshProjects();
    focusBeforeSettings = document.activeElement;
    settingsModal.style.display = "flex";
    syncSettingsLayout();
    // Focus moves onto the page, so Tab and the arrow keys act on it rather
    // than on the map behind it.
    settingsModal.focus({ preventScroll: true });
    // The caches grow with every generation, so the number the panel shows
    // has to be read when the panel opens; measuring it once at startup left
    // it stale for the whole session.
    refreshCacheSize();
  }

  // Close the settings page
  function closeSettings() {
    settingsModal.style.display = "none";
    // Webview teardown events are not guaranteed, so commit here.
    flushSettingsStore();
    cancelSettingsResetConfirm();
    if (focusBeforeSettings && typeof focusBeforeSettings.focus === "function") {
      focusBeforeSettings.focus({ preventScroll: true });
    }
    focusBeforeSettings = null;
  }

  // Escape closes the topmost dialog only. License and version info open on
  // top of the settings page, so closing them returns to it.
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;

    const licenseModal = document.getElementById("license-modal");
    const updateModal = document.getElementById("update-modal");
    const discordModal = document.getElementById("discord-modal");
    const licenseOpen = licenseModal && licenseModal.style.display === "flex";
    const updateOpen = updateModal && updateModal.style.display === "flex";
    const discordOpen = discordModal && discordModal.style.display === "flex";

    if (licenseOpen) closeLicense();
    if (updateOpen) closeUpdateModal();
    if (discordOpen) closeDiscordModal();
    if (!licenseOpen && !updateOpen && !discordOpen && settingsModal.style.display === "flex") {
      closeSettings();
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
  slider.addEventListener("input", refreshBboxSelectionInfo);
  // Double-click to reset world scale to default (1.00).
  // Assigning .value fires no event, so dispatch them for the label and store.
  slider.addEventListener("dblclick", () => {
    slider.value = 1;
    slider.dispatchEvent(new Event("input", { bubbles: true }));
    slider.dispatchEvent(new Event("change", { bubbles: true }));
  });
  refreshScaleDisplay();

  const heightSlider = document.getElementById("height-multiplier-slider");
  const heightValue = document.getElementById("height-multiplier-value");
  const refreshHeightDisplay = () => {
    heightValue.textContent = parseFloat(heightSlider.value).toFixed(2) + "\u00d7";
  };
  heightSlider.addEventListener("input", refreshHeightDisplay);
  heightSlider.addEventListener("dblclick", () => {
    heightSlider.value = 1;
    heightSlider.dispatchEvent(new Event("input", { bubbles: true }));
    heightSlider.dispatchEvent(new Event("change", { bubbles: true }));
  });
  refreshHeightDisplay();

  // Segmented controls whose click only moves the active segment
  document.querySelectorAll('#gamemode-group, #world-type-group, #mob-spawning-group, #signage-group, .segmented-control[id^="meld-"]').forEach((group) => {
    group.querySelectorAll(".segment").forEach((btn) => {
      btn.addEventListener("click", () => {
        group.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
        btn.classList.add("active");
      });
    });
  });

  initMeldFeatures({
    buildsInCells: () => isOneWorldEnabled() && !isLiveMode() && !!document.getElementById('meld-workers-select')?.value,
  });
  initRunControls({
    invoke,
    post: (message) => getMapFrame()?.contentWindow?.postMessage(message, '*'),
    // A retry was asked for on the map or with Retry: it starts without the hold.
    start: () => startGeneration(true),
    localize: (element, key) => localizeElement(window.localization, { element }, key),
    // The flags that cut the job into pieces are part of it: --only-pieces numbers its pieces.
    key: () => cellsKey(selectedBBox, isOneWorldEnabled() ? oneWorldFolderName() : ''),
    map: () => getMapFrame()?.contentWindow,
    // Remove and Rebuild in the cell menu work on the open One World, then read it again.
    world: () => oneWorldInfo?.world_path || '',
    refreshWorld: () => { oneWorldOverlayKey = null; refreshOneWorldState(); },
    // Start refuses during a bake or a Mapillary precompute: a Rebuild must not remove cells then.
    ready: () => !bakeRunning && !precomputeRunning,
  });
  initCellMenu();
  document.getElementById("bbox-features-cta").addEventListener("click", useMeldPieces);
  initJobPlan({
    // A new plan, or none: the line under the buttons says how the selection is built, and
    // the hold to start waits for the plan it starts.
    changed: () => {
      clearPrefetchDone();
      refreshBboxSelectionInfo();
      refreshStartButton();
      const plan = planPieces();
      document.getElementById('run-confirm-hold').disabled = !plan;
      // The hold is the one question: it says what the replace dialog would have asked.
      const overlap = document.getElementById('run-confirm-overlap');
      overlap.hidden = !plan?.overlap;
      overlap.textContent = plan?.overlap ? oneWorldText('plan_confirm_overlap', '{n} cells overlap land already built and will be rebuilt. Anything built there in Minecraft is replaced.', { n: plan.overlap }) : '';
      if (!isPiecesJob()) document.getElementById('run-confirm').hidden = true;
    },
    // A job in pieces of a One World with a selection, as the run decides it: Live, Bedrock,
    // the Moon and Mars build no pieces.
    request: () => {
      const bbox = selectedBBox && selectedBBox !== "0.000000 0.000000 0.000000 0.000000" ? selectedBBox : "";
      if (!bbox || !isOneWorldEnabled() || isLiveMode() || !savePath || !document.getElementById('meld-workers-select').value) return null;
      return { savePath, worldName: oneWorldFolderName(), bboxText: bbox, worldScale: readGenerationSettings(true).worldScale };
    },
    // A One World's selection against the latitude its blocks per metre were fixed at: the
    // world's origin, else --origin, else the selection's centre (what the first area sets).
    stretch: () => {
      const [a, b, c, d] = (selectedBBox || '').split(/[\s,]+/).map(Number);
      if (!isOneWorldEnabled() || !(a || c)) return null;
      const world = oneWorldInfo && oneWorldInfo.exists ? oneWorldInfo : null;
      const typed = parseFloat(document.getElementById('meld-origin-input').value);
      const origin = world ? world.origin_lat : Number.isFinite(typed) ? typed : (a + c) / 2;
      return { origin, knee: world ? world.knee_m : null, south: Math.min(a, c), north: Math.max(a, c), lon: (b + d) / 2 };
    },
  });
  // The map offers "Select this area" on a country or region, the edge handles and the size
  // readout only with Advanced features on; the readout counts regions at the world scale.
  const meldToggle = document.getElementById('advanced-meld-features-toggle');
  const pushMeldToMap = () => postToMap({ type: 'meldFeatures', on: meldToggle.checked, scale: readGenerationSettings(true).worldScale, snap: Boolean(document.querySelector('#meld-snap-group .segment.active:not([data-v="off"])')), mapFirst: document.body.classList.contains('map-first') });
  document.addEventListener('change', pushMeldToMap);
  // Advanced off: a finished One World's name is no longer the world under Start.
  meldToggle.addEventListener('change', () => {
    if (!meldToggle.checked && (lastRunOneWorld || oneWorldPicked)) setWorldNameLabel("");
    // A world picked in Projects is the target only with the switch on: read the target again.
    oneWorldOverlayKey = null;
    refreshOneWorldState();
  });
  document.getElementById('meld-snap-group')?.addEventListener('click', () => setTimeout(pushMeldToMap));
  getMapFrame()?.addEventListener('load', pushMeldToMap);
  pushMeldToMap();
  // A One World's built land, and a ring of regions around it to build next.
  document.getElementById('plan-expand-button').addEventListener('click', () => {
    if (!builtAreas.length) return;
    const pick = (f, key) => f(...builtAreas.map((a) => a[key]));
    selectWithRing([pick(Math.min, 'min_lat'), pick(Math.min, 'min_lon'), pick(Math.max, 'max_lat'), pick(Math.max, 'max_lon')], isOneWorldEnabled(), true);
    askRunConfirm();
  });
  // Cancel (or Escape) closes the panel and gives the focus back to Start.
  const cancelConfirm = () => {
    document.getElementById('run-confirm').hidden = true;
    document.getElementById('start-button').focus();
  };
  document.getElementById('run-confirm-cancel').addEventListener('click', cancelConfirm);
  document.getElementById('run-confirm').addEventListener('keydown', (e) => {
    if (e.key === 'Escape') cancelConfirm();
  });
  document.getElementById('run-confirm-docs').addEventListener('click', (e) => {
    e.preventDefault();
    openExternal(e.currentTarget.href);
  });
  holdToConfirm(document.getElementById('run-confirm-hold'), () => {
    document.getElementById('run-confirm').hidden = true;
    startGeneration(true);
  });
  refreshProjects = initProjects({
    invoke,
    savePath: () => savePath,
    scale: () => readGenerationSettings(true).worldScale,
    available: () => isOneWorldAvailable() && !liveActive(),
    busy: () => !generationButtonEnabled || bakeRunning,
    canRun: () => !isLiveMode(),
    renamed: (from, to) => worldRenamed(from, to),
    trashed: (name) => worldTrashed(name),
    // A new One World is picked by its name alone: the first Start makes it.
    created: async (name) => {
      oneWorldName = name;
      oneWorldPicked = true;
      oneWorldNew = true;
      clearPrefetchDone();
      if (oneWorldMode() === 'off') document.querySelector('#one-world-group .segment[data-one-world="on"]')?.click();
      oneWorldOverlayKey = null;
      await refreshOneWorldState();
      closeSettings();
    },
    open: async (name, expand) => {
      oneWorldName = name;
      oneWorldPicked = true;
      oneWorldNew = false;
      clearPrefetchDone();
      if (oneWorldMode() === 'off') document.querySelector('#one-world-group .segment[data-one-world="on"]')?.click();
      oneWorldOverlayKey = null;
      oneWorldRefit = true; // Open on the world already shown refits the map too
      await refreshOneWorldState();
      oneWorldRefit = false;
      closeSettings();
      if (expand) document.getElementById('plan-expand-button').click();
    },
    // Resume and Rebuild: the job's settings and area go back in, and the same run starts. Resume
    // asks with Start's hold; Rebuild was held already.
    resume: (world, job, rebuild) => resumeJob(job, rebuild, {
      current: () => selectedBBox,
      start: () => startGeneration(rebuild),
      noArea: () => setBboxSelectionInfo(document.getElementById('bbox-selection-info'), 'select_area_prompt', '#ffffff'),
    }),
  });
  initDataPanel({
    invoke,
    baking: (on) => { bakeRunning = on; redrawProjects(); },
    prefetch: () => startGeneration(true, true),
    area: () => (selectedBBox && selectedBBox !== "0.000000 0.000000 0.000000 0.000000" ? selectedBBox : ""),
    plan: () => {
      const mode = document.getElementById("generation-mode-select").value;
      return {
        worldScale: parseFloat(document.getElementById("scale-value-slider").value),
        terrainEnabled: mode === "geo-terrain" || mode === "terrain-only",
        skipOsmObjects: mode === "terrain-only",
        canopyHeightEnabled: document.getElementById("canopy-height-toggle").checked,
        overtureEnabled: document.getElementById("overture-toggle").checked,
        awsOnlyElevation: document.getElementById("aws-only-elevation-toggle").checked,
        // A plan's flags: the next run's pending retry and skipped cells stay.
        flags: meldFlags({ oneWorld: false, java: true, caves: false, extendHeight: false, lods: false, plan: true }),
      };
    },
  });

  // Blueprint greys out the traffic lights.
  document.getElementById("generation-mode-select").addEventListener("change", () => refreshGameplayRows());

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

  const fieldPresetGroup = document.getElementById("field-preset-group");
  fieldPresetGroup.querySelectorAll(".segment").forEach((btn) => {
    btn.addEventListener("click", () => {
      fieldPresetGroup.querySelectorAll(".segment").forEach((b) => b.classList.remove("active"));
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

  // Custom world name editor (Java only), gated by its Settings toggle
  initCustomWorldNameToggle();

  // One World: a persistent world every area extends (Java only)
  initOneWorld();
  initLive();

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

  // The source control, and the detail beside it. Each keeps its own key so
  // the rest of main.js can read it without the store, and is registered in
  // settings-store.js as well so the panel's Revert and Reset reach it.
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
    syncTelemetryConsent();
  });

  // Discord status: sent on change and after restore; hidden in builds without a Discord app.
  const discordToggle = document.getElementById("discord-status-toggle");
  if (discordToggle) {
    discordToggle.addEventListener("change", syncDiscordStatus);
    Promise.resolve(invoke('gui_discord_available')).then((available) => {
      if (available) return;
      const row = discordToggle.closest('.settings-row');
      if (row) row.style.display = 'none';
    }).catch(() => {});
  }


  /// License and Credits
  async function openLicense() {
    const licenseModal = document.getElementById("license-modal");
    const licenseContent = document.getElementById("license-content");

    licenseContent.innerHTML = licenseText;
    licenseContent.scrollTop = 0;
    showModal(licenseModal);

    // The credits only known at runtime go into their own spot in the
    // "Models, Textures and Fonts" list. Held by reference, so a slow answer
    // from an earlier opening lands in that opening's detached copy rather
    // than doubling up in this one.
    const runtime = licenseContent.querySelector("#license-runtime-credits") || licenseContent;
    const credit = (title, body) =>
      `<div class="credit"><h4>${title}</h4>${body}</div>`;

    runtime.insertAdjacentHTML("beforeend", credit(
      "3D Model Repository (3DMR)",
      `<p>Landmark models from <a href="https://3dmr.eu" target="_blank" rel="noopener noreferrer">3dmr.eu</a> are fetched on demand and voxelized. Individual models retain the license declared by their uploader; specific per-model attribution is printed to the generation log. See the <a href="https://3dmr.eu" target="_blank" rel="noopener noreferrer">3DMR website</a> for any model used.</p>`
    ));

    // The premade facade set. All CC0, so attribution is a courtesy rather than
    // a condition, but the sources are named because someone should be able to
    // find them and because it says plainly that the pixels are free to ship.
    runtime.insertAdjacentHTML("beforeend", credit(
      "Preset Building Facade Textures",
      `<p>The photographs hung on buildings by the Preset Facades setting, all released under ` +
      `<a href="https://creativecommons.org/publicdomain/zero/1.0/" target="_blank" rel="noopener noreferrer">CC0</a>:</p>` +
      `<ul>` +
      `<li>Urban building, apartment and shop front photographs by <b>Scouser</b>, from ` +
      `<a href="https://opengameart.org/content/free-urban-textures-buildings-apartments-shop-fronts" target="_blank" rel="noopener noreferrer">OpenGameArt</a></li>` +
      `<li>Tiling facade materials from <b>TextureCan</b>: ` +
      `<a href="https://www.texturecan.com/details/315/" target="_blank" rel="noopener noreferrer">315</a>, ` +
      `<a href="https://www.texturecan.com/details/316/" target="_blank" rel="noopener noreferrer">316</a>, ` +
      `<a href="https://www.texturecan.com/details/357/" target="_blank" rel="noopener noreferrer">357</a>, ` +
      `<a href="https://www.texturecan.com/details/360/" target="_blank" rel="noopener noreferrer">360</a>, ` +
      `<a href="https://www.texturecan.com/details/563/" target="_blank" rel="noopener noreferrer">563</a></li>` +
      `</ul>`
    ));

    const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({"&":"&amp;","<":"&lt;",">":"&gt;","\"":"&quot;","'":"&#39;"}[c]));
    const link = (url, text) =>
      `<a href="${esc(url)}" target="_blank" rel="noopener noreferrer">${esc(text)}</a>`;

    // Mapillary imagery is CC BY-SA, and the licence is on the pixels: a world
    // built from street photographs has to name the photographers. The source
    // is named whether or not a run has used it yet, so the credit can be
    // found before the first generation; the per-image list underneath, one
    // line per photograph in the shape Mapillary's own guidance asks for,
    // fills in after one.
    let shots = [];
    try {
      const used = await invoke("gui_get_mapillary_attributions");
      if (Array.isArray(used)) shots = used;
    } catch (e) {
      console.warn("Failed to load Mapillary attributions:", e);
    }
    let mapillaryList;
    if (shots.length > 0) {
      // A record that carries the image id but not the uploader name has no
      // profile to link to, so the name is shown as plain text instead.
      const lines = shots.map((s) => {
        const by = s.profile_url ? link(s.profile_url, s.username) : esc(s.username);
        return `<li>${link(s.image_url, s.title)} by ${by}, licensed under CC-BY-SA</li>`;
      }).join("");
      const unnamed = shots.filter((s) => !s.profile_url).length;
      const note = unnamed > 0
        ? ` ${unnamed} of these name only the photograph: their records carry the image id but not the uploader name. Each link opens the image, which names its uploader.`
        : "";
      mapillaryList =
        `<p>Photographs used by the last generation:${note}</p>` +
        `<ul>${lines}</ul>`;
    } else {
      mapillaryList =
        `<p>The photographs a generation used are listed here once it has run.</p>`;
    }
    runtime.insertAdjacentHTML("beforeend", credit(
      "Building Facades (Mapillary)",
      `<p>The Mapillary facade source measures wall textures and colours from street-level photographs on ` +
      `${link("https://www.mapillary.com", "Mapillary")}, licensed ` +
      `${link("https://creativecommons.org/licenses/by-sa/4.0/", "CC BY-SA 4.0")}. ` +
      `Share-alike applies to anything you publish that carries them.</p>` +
      mapillaryList
    ));

    try {
      const rows = await invoke("gui_get_3d_model_attributions");
      if (Array.isArray(rows) && rows.length > 0) {
        const items = rows.map(r => {
          const lic = r.license_url
            ? `<a href="${esc(r.license_url)}" target="_blank" rel="noopener noreferrer">${esc(r.license)}</a>`
            : esc(r.license);
          return `<li><b>${esc(r.label)}</b> — ${esc(r.artist)}, ${lic} (<a href="${esc(r.source_url)}" target="_blank" rel="noopener noreferrer">source</a>)</li>`;
        }).join("");
        runtime.insertAdjacentHTML("beforeend", credit(
          "Bundled 3D Models (Wikimedia Commons via Wikidata P4896)",
          `<p>Permissive-licensed models used to render famous landmarks. Voxelized and rescaled by Arnis.</p>` +
          `<ul>${items}</ul>`
        ));
      }
    } catch (e) {
      console.warn("Failed to load 3D model attributions:", e);
    }
  }

  function closeLicense() {
    const licenseModal = document.getElementById("license-modal");
    hideModal(licenseModal);
  }

  window.openLicense = openLicense;
  window.closeLicense = closeLicense;
}

// World format selection (Java/Bedrock/Luanti/Hytale)
let selectedWorldFormat = 'java'; // Default to Java

const VALID_FORMATS = ['java', 'bedrock', 'luanti', 'hytale'];
// Experimental: each shows its button and save path once enabled under Advanced.
const EXPERIMENTAL_FORMATS = ['luanti', 'hytale'];

// Sets a toggle another one depends on. Dispatches, so the settings store persists the
// knock-on change too.
function setChecked(el, value) {
  if (el.checked === value) return;
  el.checked = value;
  el.dispatchEvent(new Event('change', { bubbles: true }));
}

// Voxy and Distant Horizons render distant terrain from the light stored in
// their LOD caches, so pre-generating one without baked lighting would give a
// black horizon. Keep the toggles consistent in the UI rather than quietly
// overriding the user's choice at generation time.
function initLodLightingCoupling() {
  const bake = document.getElementById('bake-lighting-toggle');
  const lods = ['voxy-lod-toggle', 'dh-lod-toggle']
    .map((id) => document.getElementById(id))
    .filter(Boolean);
  if (!bake) return;

  for (const lod of lods) {
    lod.addEventListener('change', () => {
      if (lod.checked) setChecked(bake, true);
    });
  }
  bake.addEventListener('change', () => {
    if (!bake.checked) lods.forEach((lod) => setChecked(lod, false));
  });
}

// Caves are carved into the filled ground, so turning them on turns Fill Ground
// on, and turning Fill Ground off takes the caves with it.
function initCavesFillCoupling() {
  const caves = document.getElementById('caves-toggle');
  const fill = document.getElementById('fillground-toggle');
  if (!caves || !fill) return;

  caves.addEventListener('change', () => {
    if (caves.checked) setChecked(fill, true);
  });
  fill.addEventListener('change', () => {
    if (!fill.checked) setChecked(caves, false);
  });
}

// Disasters open up the ground and send people indoors, so turning them on turns on Fill
// Ground and interiors. Turning Fill Ground off takes the disasters with it, as with caves.
function initDisastersCoupling() {
  const disasters = document.getElementById('disasters-toggle');
  const fill = document.getElementById('fillground-toggle');
  const interior = document.getElementById('interior-toggle');
  if (!disasters || !fill || !interior) return;

  // Only on a click: restoring saved settings fires change events too, and must not undo a
  // choice to leave interiors off.
  disasters.addEventListener('change', (event) => {
    if (!disasters.checked || !event.isTrusted) return;
    setChecked(fill, true);
    if (!interior.disabled) setChecked(interior, true);
  });
  fill.addEventListener('change', () => {
    if (!fill.checked) setChecked(disasters, false);
  });
}

function initWorldFormatToggle() {
  EXPERIMENTAL_FORMATS.forEach(initExperimentalFormatToggle);

  const savedFormat = localStorage.getItem('arnis-world-format');
  if (savedFormat && VALID_FORMATS.includes(savedFormat)) {
    selectedWorldFormat = savedFormat;
  }
  if (EXPERIMENTAL_FORMATS.includes(selectedWorldFormat) && !isFormatEnabled(selectedWorldFormat)) {
    selectedWorldFormat = 'java';
  }

  updateFormatToggleUI(selectedWorldFormat);
}

function isFormatEnabled(format) {
  return localStorage.getItem(`arnis-${format}-enabled`) === 'true';
}

// The last visible button closes the segmented control.
function refreshFormatButtonEdges() {
  const buttons = ['bedrock', ...EXPERIMENTAL_FORMATS]
    .map((f) => document.getElementById(`format-${f}`))
    .filter(Boolean);
  const visible = buttons.filter((b) => b.style.display !== 'none');
  const last = visible[visible.length - 1];
  buttons.forEach((b) => b.classList.toggle('format-toggle-btn--rightmost', b === last));
}

function initExperimentalFormatToggle(format) {
  const toggle = document.getElementById(`enable-${format}-toggle`);
  const btn = document.getElementById(`format-${format}`);
  if (!toggle || !btn) return;

  const pathRow = document.getElementById(`${format}-save-path-row`);
  const apply = (enabled) => {
    btn.style.display = enabled ? '' : 'none';
    // A save path for a format nobody can pick is only noise.
    if (pathRow) pathRow.style.display = enabled ? '' : 'none';
    refreshFormatButtonEdges();
  };

  const enabled = isFormatEnabled(format);
  toggle.checked = enabled;
  apply(enabled);

  toggle.addEventListener('change', () => {
    const on = toggle.checked;
    localStorage.setItem(`arnis-${format}-enabled`, on ? 'true' : 'false');
    apply(on);
    if (!on && selectedWorldFormat === format) {
      setWorldFormat('java');
    }
  });
}

function setWorldFormat(format) {
  if (!VALID_FORMATS.includes(format) || liveActive()) return;
  if (EXPERIMENTAL_FORMATS.includes(format) && (!isFormatEnabled(format) || isOneWorldToggleOn())) return;

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
// behavior pack; Luanti and Hytale ship neither, and off Earth the relief already fits
// vanilla height, so the backend forces the flag off there.
function heightLimitAvailable(format) {
  return selectedCelestialBody === 'earth' && !EXPERIMENTAL_FORMATS.includes(format);
}

function refreshHeightLimitRow(format) {
  const toggle = document.getElementById('disable-height-limit-toggle');
  if (!toggle) return;

  const current = format || selectedWorldFormat;
  const reason = why([selectedCelestialBody === 'earth', 'reason_earth'], [!EXPERIMENTAL_FORMATS.includes(current), 'reason_java_bedrock'],
    [!oneWorldPinned, 'reason_not_one_world']);
  const available = !reason;
  toggle.disabled = !available;

  const row = toggle.closest('.settings-row');
  if (row) {
    // Cleared, not set to 1: an inline value would beat the class rule.
    row.style.opacity = '';
    row.classList.toggle('settings-row-unavailable', !available);
    setRowReason(row, reason);
  }
}

// Bedrock and Luanti keep their own flat generators, so the choice only means
// something for Java. Greyed rather than hidden, like the other format gates.
// A One World's areas sit at their real heights and a superflat off Earth is
// grass, so the backend makes both void.
function refreshWorldTypeRow(format) {
  const reason = why([(format || selectedWorldFormat) === 'java', 'reason_java'],
    [selectedCelestialBody === 'earth', 'reason_earth'], [!isOneWorldEnabled(), 'reason_not_one_world']);
  setSegmentedRowAvailable('world-type-group', !reason, reason);
}

// Greyed the way the settings store reads it: every segment disabled, so the row is out of
// the tab order and never shows as modified, and inert like a greyed toggle's row (all but
// its reason, when it has one). The active segment stays, so the choice comes back with the row.
function setSegmentedRowAvailable(groupId, available, reason = null) {
  const group = document.getElementById(groupId);
  if (!group) return;
  group.classList.toggle('segmented-disabled', !available);
  group.querySelectorAll('.segment').forEach((btn) => { btn.disabled = !available; });
  const row = group.closest('.settings-row');
  if (row) {
    row.classList.toggle('settings-row-unavailable', !available);
    row.toggleAttribute('inert', !available && !reason);
    setRowReason(row, available ? null : reason);
  }
}

// The extras are a Java data pack, and each One World area would write its own over
// the last one's. Vehicles also run on Bedrock, from an add-on of their own. Off Earth
// there are no roads or places, and gravity only differs there. Blueprint builds no
// signal heads, so there are no lights to run.
function refreshGameplayRows(format) {
  setTimeout(refreshAdvancedRows, 0);
  // The effective state: a ticked One World is off on the Moon and Mars, and greyed out there.
  const current = format || selectedWorldFormat;
  const pack = current === 'java' && !isOneWorldEnabled();
  const vehicles = pack || current === 'bedrock';
  const earth = selectedCelestialBody === 'earth';
  const mode = document.getElementById('generation-mode-select');
  const blueprint = !!mode && mode.value === 'blueprint';
  const packWhy = [[current === 'java', 'reason_java'], [!isOneWorldEnabled(), 'reason_not_one_world']];
  const gate = (id, ...more) => {
    const reason = why(...packWhy, ...more);
    setSettingsRowAvailable(id, !reason, reason);
  };
  gate('pause-menu-toggle');
  gate('zombie-outbreak-toggle');
  gate('disasters-toggle');
  gate('discoveries-toggle', [earth, 'reason_earth']);
  gate('traffic-lights-toggle', [earth, 'reason_earth'], [!blueprint, 'reason_blueprint']);
  const carsWhy = why([current === 'java' || current === 'bedrock', 'reason_java_bedrock'],
    [vehicles, 'reason_not_one_world'], [earth, 'reason_earth']);
  setSettingsRowAvailable('moving-cars-toggle', !carsWhy, carsWhy);
  gate('pedestrians-toggle', [earth, 'reason_earth']);
  const gravityWhy = why([!earth, 'reason_moon_mars'], ...packWhy);
  setSettingsRowAvailable('body-gravity-toggle', !gravityWhy, gravityWhy);
  // Last, as it reads whether the outbreak and the disasters are still available.
  refreshMobSpawningRow(current);
}

// Mineclonia spawns by settings outside the world, nothing lives off Earth, and the Zombie
// Outbreak and Disasters bring their own mobs on their own difficulty. The backend turns it
// off in all of these too.
function mobSpawningReason(format) {
  const current = format || selectedWorldFormat;
  return why([current === 'java' || current === 'bedrock', 'reason_java_bedrock'],
    [selectedCelestialBody === 'earth', 'reason_earth'],
    [!gameplayToggle('zombie-outbreak-toggle'), 'reason_outbreak'], [!gameplayToggle('disasters-toggle'), 'reason_disasters'],
    [!(isOneWorldEnabled() && !!(oneWorldInfo && oneWorldInfo.exists)), 'reason_world_set']);
}

function mobSpawningAvailable(format) {
  return !mobSpawningReason(format);
}

function refreshMobSpawningRow(format) {
  const reason = mobSpawningReason(format);
  setSegmentedRowAvailable('mob-spawning-group', !reason, reason);
}

// Off wherever the row is greyed, so a stale choice is never sent.
function getMobSpawning() {
  if (!mobSpawningAvailable()) return 'off';
  const btn = document.querySelector('#mob-spawning-group .segment.active');
  return btn ? btn.dataset.mobSpawning : 'off';
}

function initMobSpawningLockout() {
  ['zombie-outbreak-toggle', 'disasters-toggle'].forEach((id) => {
    const toggle = document.getElementById(id);
    if (toggle) toggle.addEventListener('change', () => refreshMobSpawningRow());
  });
}

// Disabled means unavailable for this format or body, so a stale tick is never sent.
function gameplayToggle(id) {
  const el = document.getElementById(id);
  return !!(el && el.checked && !el.disabled);
}

function updateFormatToggleUI(format) {
  const javaBtn = document.getElementById('format-java');
  const bedrockBtn = document.getElementById('format-bedrock');
  const experimentalBtns = EXPERIMENTAL_FORMATS.map((f) => document.getElementById(`format-${f}`));

  refreshHeightLimitRow(format);
  refreshGameplayRows(format);
  refreshWorldTypeRow(format);
  refreshJavaOnlyRows(format);
  refreshWorldRulesRows();
  refreshAdvancedFeaturesForFormat(format);

  javaBtn.classList.remove('format-active');
  bedrockBtn.classList.remove('format-active');
  experimentalBtns.forEach((b) => b && b.classList.remove('format-active'));

  if (format === 'java') {
    javaBtn.classList.add('format-active');
  } else if (format === 'bedrock') {
    bedrockBtn.classList.add('format-active');
    // Clear world path for bedrock (auto-generated)
    worldPath = "";
  } else if (EXPERIMENTAL_FORMATS.includes(format)) {
    const btn = document.getElementById(`format-${format}`);
    if (btn) btn.classList.add('format-active');
    worldPath = "";
  }
  // The label names the last Java world only while that is still the target.
  if (!worldPath) worldLabelName = "";

  // The facade panels are Java entities, so the mode control changes with the
  // format. Called from here so a format picked before the settings modal is
  // ever opened still leaves it consistent.
  refreshFacadeRows();
  // Custom names are Java-only; hide/show the pencil and re-derive the
  // label preview whenever the active format changes.
  refreshWorldNameEditUI();
  // One World is Java-only too: its status and pins follow the format.
  refreshOneWorldState();
  // The size warning's Advanced features link is Java-only.
  refreshBboxSelectionInfo();
}

// Advanced features are built for Java: Hytale turns them off, another format back on.
const ADVANCED_HELD_KEY = 'arnis-advanced-before-hytale';
const advancedHeldForHytale = () => localStorage.getItem(ADVANCED_HELD_KEY) === 'true';

function holdAdvancedForHytale(held) {
  if (held) localStorage.setItem(ADVANCED_HELD_KEY, 'true');
  else localStorage.removeItem(ADVANCED_HELD_KEY);
}

function refreshAdvancedFeaturesForFormat(format) {
  const toggle = document.getElementById('advanced-meld-features-toggle');
  if (!toggle) return;
  const hytale = format === 'hytale';
  if (hytale && toggle.checked) {
    holdAdvancedForHytale(true);
    setChecked(toggle, false);
  }
  setSettingsRowAvailable('advanced-meld-features-toggle', !hytale, 'reason_not_hytale');
  if (!hytale && advancedHeldForHytale()) {
    holdAdvancedForHytale(false);
    setChecked(toggle, true);
  }
}

// Expose to window for onclick handlers
window.setWorldFormat = setWorldFormat;

// The backend shows nothing on Discord until it is told the toggle is on.
function syncDiscordStatus() {
  const toggle = document.getElementById("discord-status-toggle");
  const enabled = !!(toggle && toggle.checked);
  Promise.resolve(invoke('gui_set_discord_status', { enabled })).catch(() => {});
}

// The backend is told Offline Mode once the store has restored it, and on every change: while
// it is on it leaves old cached tiles alone and sends no telemetry or update check.
function initOfflineModeSync() {
  ['advanced-meld-features-toggle', 'meld-offline-toggle'].forEach((id) =>
    document.getElementById(id)?.addEventListener('change', syncOfflineMode));
  syncOfflineMode();
}

function syncOfflineMode() {
  Promise.resolve(invoke('gui_set_offline_mode', { offline: offlineModeOn() })).catch(() => {});
}

// Off with the Advanced switch, as the flag is, but not while Hytale holds that switch off.
function offlineModeOn() {
  const on = (id) => !!document.getElementById(id)?.checked;
  return on('meld-offline-toggle') && (on('advanced-meld-features-toggle') || advancedHeldForHytale());
}

// The backend only reports crashes and errors while it holds a consent of true,
// so push the stored answer to it at startup and on every change.
function syncTelemetryConsent() {
  const consent = localStorage.getItem('telemetry-consent') === 'true';
  Promise.resolve(invoke('gui_set_telemetry_consent', { consent })).catch(() => {});
}

// Telemetry consent (first run only)
function initTelemetryConsent() {
  const key = 'telemetry-consent'; // values: 'true' | 'false'
  const existing = localStorage.getItem(key);

  const modal = document.getElementById('telemetry-modal');
  if (!modal) return;

  if (existing === null) {
    // First run: ask for consent
    showModal(modal);
  }
  syncTelemetryConsent();

  // Expose handlers
  window.acceptTelemetry = () => {
    localStorage.setItem(key, 'true');
    hideModal(modal);
    syncTelemetryConsent();
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
    hideModal(modal);
    syncTelemetryConsent();
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

// What the pipeline prefixes its stage lines with. Kept short because the same
// lines go to the progress bar's status line, which is one line of a 320px
// panel.
const FACADE_STAGE_PREFIX = "Facades:";

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
  if (selectedCelestialBody !== 'earth') {
    return "Facades are Earth only. Switch the world back in the map toolbar.";
  }
  // The source control decides this, and it is the reason the row above greys
  // the button out; without it here the next refresh switches it back on.
  if (getEffectiveFacadeSource() !== 'mapillary') {
    return "Set Facade Source to Mapillary first.";
  }
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
  if (!precomputeRunning || !message.startsWith(FACADE_STAGE_PREFIX)) return;
  precomputeStage = message.slice(FACADE_STAGE_PREFIX.length).trim();
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
      // Every refusal from the backend is a sentence meant to be read. The
      // longer ones are written as that sentence, a blank line, and the reason
      // behind it: the row holds one line, so the reason becomes its tooltip.
      const text = String(error);
      const split = text.indexOf("\n\n");
      const head = split < 0 ? text : text.slice(0, split).trim();
      const rest = split < 0 ? "" : text.slice(split + 2).trim();
      setPrecomputeStatus(head, 'error', rest || undefined);
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
    // The icon sits inside the setting's <label>, whose click would otherwise
    // flip the switch it belongs to.
    icon.addEventListener('click', (e) => e.preventDefault());
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
let hytaleSavePath = "";

const SAVE_PATHS = {
  java: {
    storageKey: 'arnis-save-path',
    defaultCommand: 'gui_get_default_save_path',
    inputId: 'save-path-input',
    browseId: 'save-path-browse',
    get: () => savePath,
    set: (value) => {
      const changed = savePath !== value;
      savePath = value;
      // The One World lives under the saves path, so a new path is a new world.
      if (changed && typeof refreshOneWorldState === 'function') refreshOneWorldState();
    },
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
  hytale: {
    storageKey: 'arnis-hytale-save-path',
    defaultCommand: 'gui_get_default_hytale_save_path',
    inputId: 'hytale-save-path-input',
    browseId: 'hytale-save-path-browse',
    get: () => hytaleSavePath,
    set: (value) => { hytaleSavePath = value; },
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
      bboxInputError = false;
      selectedBBox = mapSelectedBBox;
      dropStalePieces(selectedBBox);
      
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
        bboxInputError = false;
        selectedBBox = bboxText.replace(/,/g, ' '); // Convert to space format for consistency
        dropStalePieces(selectedBBox);
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
        bboxInputError = true;
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
      bboxInputError = true;
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
  const toRad = (angle) => (angle * Math.PI) / 180;
  // Real ground, not an Earth-sized overestimate: a lunar box reads 13x too large.
  const R = BODY_RADIUS_M[selectedCelestialBody] || BODY_RADIUS_M.earth;
  // Width at the middle latitude, as the world is built and as the CLI's area_km2 measures.
  const height = R * toRad(lat2 - lat1);
  const width = R * toRad(lng2 - lng1) * Math.cos(toRad((lat1 + lat2) / 2));
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

// Selection-size warnings, in square metres of ground at world scale 1. Measured
// timings, peak memory and world sizes, square selections over central Munich:
//   Earth 9km2 8s/1.1GB/146MB | 25km2 20s/1.7GB/399MB | 85km2 66s/3.6GB/1.3GB
//         150km2 111s/5.1GB/2.3GB, so about 1.6GB plus 0.023GB per km2
//   Moon  2deg 5s/4MB | 5deg 9s/16MB | 10deg 21s/36MB | 20deg 68s/144MB
//   Mars  2deg 5s/4MB | 5deg 9s/16MB | 10deg 20s/36MB | 20deg 51s/100MB
// The Earth tiers guard memory more than the clock: about 4GB, 6GB (fine on an
// 8GB machine) and 13GB. The Moon and Mars tiers land near one, three and nine
// minutes.
const AREA_THRESHOLDS = {
  earth: { extensive: 100e6, large: 200e6, extreme: 500e6 },
  moon: { extensive: 3e11, large: 1e12, extreme: 3e12 },
  mars: { extensive: 1.5e12, large: 5e12, extreme: 1.5e13 }
};

let selectedBBox = "";
let mapSelectedBBox = "";  // Tracks bbox from map selection
// Drawn areas as [lat, lng] rings, or null for a single rectangle (the plain bbox).
let selectedShapes = null;
let customBBoxValid = false;  // Tracks if custom input is valid
let bboxInputError = false;  // The coordinate field holds input that did not validate

/**
 * Displays the appropriate bbox size status message based on area thresholds
 * @param {HTMLElement} bboxSelectionElement - The element to display the message in
 * @param {number} selectedSize - The calculated bbox area in square meters
 */
function displayBboxSizeStatus(bboxSelectionElement, selectedSize) {
  const t = AREA_THRESHOLDS[selectedCelestialBody] || AREA_THRESHOLDS.earth;
  selectedSize *= earthScaleFactor();
  // Advanced features: built in pieces, the size is no longer a worry; say how instead.
  const pieces = planPieces();
  let warned = false;
  if (planPending()) {
    // The plan is being recomputed: neither warn nor offer pieces until it answers.
    setBboxSelectionInfo(bboxSelectionElement, "selection_confirmed", "#7bd864");
  } else if (pieces) {
    // The plan under the button says how it is built; this line stays what it always was.
    setBboxSelectionInfo(bboxSelectionElement, "selection_confirmed", "#7bd864");
  } else if ((hasPlanMask() || outline) && !selectedShapes) {
    // An outline only leaves pieces out: a build in one go (One World off too) builds the whole box.
    setBboxSelectionInfo(bboxSelectionElement, "area_outline_needs_pieces", "#fecc44");
    warned = true;
  } else if (selectedSize > t.extreme) {
    setBboxSelectionInfo(bboxSelectionElement, "area_extreme", "#ff4444");
    warned = true;
  } else if (selectedSize > t.large) {
    setBboxSelectionInfo(bboxSelectionElement, "area_too_large", "#fa7878");
    warned = true;
  } else if (selectedSize > t.extensive) {
    setBboxSelectionInfo(bboxSelectionElement, "area_extensive", "#fecc44");
    warned = true;
  } else if (selectedShapes && isOneWorldEnabled() && meldOn()) {
    setBboxSelectionInfo(bboxSelectionElement, "shapes_one_world", "#fecc44");
  } else {
    setBboxSelectionInfo(bboxSelectionElement, "selection_confirmed", "#7bd864");
  }
  // The warning's way out (Advanced features off or no pieces yet): build it in cells, a One World
  // so Java on Earth only.
  document.getElementById("bbox-features-cta").hidden = !(warned && isOneWorldAvailable());
}

// Turns on Advanced features, One World and parallel pieces, and shows the Scale card.
function useMeldPieces() {
  window.openSettings();
  const meld = document.getElementById("advanced-meld-features-toggle");
  if (!meld.checked) {
    meld.checked = true;
    meld.dispatchEvent(new Event("change", { bubbles: true }));
  }
  if (oneWorldMode() !== "on") document.querySelector('#one-world-group .segment[data-one-world="on"]')?.click();
  const workers = document.getElementById("meld-workers-select");
  if (!workers.value) {
    workers.value = "auto";
    workers.dispatchEvent(new Event("change", { bubbles: true }));
  }
  // Once the page is up and the section is shown: the sidebar's own jump, so the entry lights up too.
  setTimeout(() => document.querySelector('.settings-nav-item[data-target="settings-section-scale"]')?.click(), 80);
}

// Blocks, and so memory, grow with the square of the scale; Moon and Mars use a fixed one.
function earthScaleFactor() {
  if (selectedCelestialBody !== 'earth') return 1;
  const s = parseFloat(document.getElementById("scale-value-slider")?.value);
  return isFinite(s) && s > 0 ? s * s : 1;
}

// Re-runs the size status, e.g. after a body switch or scale change moves the tier.
function refreshBboxSelectionInfo() {
  // An error about the typed coordinates stays until the field is fixed.
  if (!mapSelectedBBox || bboxInputError) return;
  const [lat1, lng1, lat2, lng2] = mapSelectedBBox.split(" ").map(Number);
  displayBboxSizeStatus(
    document.getElementById("bbox-selection-info"),
    calculateBBoxSize(lat1, lng1, lat2, lng2)
  );
}

// The outline the map reports (Use outline, a drawn shape). Only a One World job in pieces reads
// it, so the file is written for that alone; when the backend cannot write it the map drops it.
let outline = null;
let outlineWritten = false;
let outlineFailed = false;
async function noteOutline(geojson) {
  outline = geojson;
  // The map answers a dropped outline with null: that must not wipe the reason.
  if (!geojson && outlineFailed) { outlineFailed = false; return; }
  outlineWritten = Boolean(geojson) && isOneWorldEnabled();
  const error = outlineWritten ? await setPlanMask(geojson) : (await setPlanMask(null), '');
  if (error) {
    outlineFailed = true;
    postToMap({ type: 'planMask', geojson: null });
    const info = document.getElementById('bbox-selection-info');
    info.textContent = error;
    info.style.color = "#fa7878";
    return;
  }
  refreshBboxSelectionInfo();
}

function meldOn() {
  return Boolean(document.getElementById('advanced-meld-features-toggle')?.checked);
}

// Drawn shapes are the outline a job in pieces keeps its pieces in (--plan-mask); written when
// One World and Advanced features are on, cleared otherwise. Called on a new selection and when
// either changes.
let shapesWritten = false;
const wantsShapesMask = () => isOneWorldEnabled() && meldOn();
function writeShapesMask() {
  shapesWritten = Boolean(selectedShapes) && wantsShapesMask();
  if (shapesWritten) {
    setPlanMask({
      type: 'MultiPolygon',
      coordinates: selectedShapes.map((ring) => [ring.concat([ring[0]]).map(([lat, lng]) => [lng, lat])])
    });
  } else {
    setPlanMask(null);
  }
}

// Function to handle incoming bbox data
function displayBboxInfoText(bboxText, shapes) {
  clearPrefetchDone();
  const hadShapes = Boolean(selectedShapes);
  selectedShapes = Array.isArray(shapes) && shapes.length ? shapes : null;
  if (selectedShapes || hadShapes) writeShapesMask();
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
  dropStalePieces(selectedBBox);
  // Advanced features: a single run's preview belongs to that run's area; a new selection drops it
  // (a One World keeps showing its own areas).
  if (document.getElementById('advanced-meld-features-toggle').checked && currentWorldMapData && !lastRunOneWorld) {
    notifyWorldChanged();
  }
  customBBoxValid = false;
  bboxInputError = false;

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

// Name of the world the last run made (or is making), as the label shows it.
// A pending custom name is drawn over it without replacing it.
let worldLabelName = "";

function setWorldNameLabel(text) {
  worldLabelName = text || "";
  renderWorldNameLabel();
}

function renderWorldNameLabel() {
  const label = document.getElementById('world-name-label');
  if (!label) return;
  const pending = pendingWorldName();
  const text = pending || worldLabelName;
  // The world under Start can be renamed by clicking its name.
  const renamable = !pending && !!text && !!renamableWorld();
  label.toggleAttribute('data-pending', !!pending);
  label.toggleAttribute('data-renamable', renamable);
  label.title = pending
    ? (window.localization && window.localization.placeholder_world_name) || 'Name of the next world'
    : renamable ? oneWorldText('world_rename_tip', 'Click to rename this world') : '';
  if (text) {
    label.removeAttribute('data-placeholder');
    label.textContent = text;
  } else {
    label.setAttribute('data-placeholder', 'true');
    // Before the first localization pass there is nothing to look the text up
    // in, and that pass fills the placeholder itself. Looking it up here
    // instead fetched English, which arrived after the translation and
    // overwrote it.
    if (window.localization) {
      localizeElement(window.localization, { element: label }, 'no_world_generated_yet');
    }
  }
}

function basenameFromPath(p) {
  if (!p) return "";
  return p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "";
}

/* Renaming the world on disk: click its name under Start, or the pencil in Projects */

// Folder name of the world a click on the name would rename, or "": the One World shown, or the
// Java world the last run made, once nothing is running and Advanced features are on.
function renamableWorld() {
  if (!savePath || document.body.classList.contains('meld-off') || selectedWorldFormat !== 'java' || !generationButtonEnabled || liveActive()) return "";
  if (isOneWorldEnabled()) return oneWorldInfo && oneWorldInfo.exists && !oneWorldInfo.locked ? basenameFromPath(oneWorldInfo.world_path) : "";
  // The rename goes by name in the saves folder: a world made in another one is not it.
  return inSavesFolder(worldPath) ? basenameFromPath(worldPath) : "";
}

function inSavesFolder(path) {
  const norm = (p) => p.replace(/\\/g, '/').replace(/\/+$/, '');
  return Boolean(path) && norm(path).replace(/\/[^/]*$/, '') === norm(savePath);
}

// What the window remembers of a world's name once its folder has been renamed.
function worldRenamed(from, to) {
  if (inSavesFolder(worldPath) && basenameFromPath(worldPath) === from) worldPath = worldPath.replace(/[^\\/]+([\\/]*)$/, (_, tail) => to + tail);
  // A One World keeps being the one under Start, now picked by its new name.
  if (oneWorldName === from || (oneWorldInfo && basenameFromPath(oneWorldInfo.world_path) === from)) {
    oneWorldName = to;
    oneWorldPicked = true;
    oneWorldNew = false;
    localStorage.setItem(ONE_WORLD_NAME_KEY, to);
    localStorage.setItem('arnis-one-world-picked', '1');
  }
  if (worldLabelName === from) setWorldNameLabel(to);
  oneWorldOverlayKey = null;
  refreshOneWorldState();
}

// A One World moved to the trash from Projects: nothing in the window points at it any more.
function worldTrashed(name) {
  if (inSavesFolder(worldPath) && basenameFromPath(worldPath) === name) worldPath = "";
  if (oneWorldName === name) setOneWorldName('');
  if (worldLabelName === name) setWorldNameLabel("");
  oneWorldOverlayKey = null;
  refreshOneWorldState();
}

// The name under Start becomes the editor; Enter renames, Escape leaves it.
let renamingWorld = "";
function startWorldRename(event) {
  const from = renamableWorld();
  if (!from || !worldLabelName) return;
  event.preventDefault();
  event.stopPropagation();
  const label = document.getElementById('world-name-label');
  const input = document.getElementById('world-name-input');
  if (!label || !input) return;
  renamingWorld = from;
  input.value = from;
  label.style.display = 'none';
  const editButton = document.getElementById('world-name-edit-button');
  if (editButton) editButton.style.display = 'none';
  input.style.display = '';
  updateWorldNameLimitState();
  input.focus();
  input.select();
}

async function commitWorldRename(input) {
  const from = renamingWorld;
  const to = input.value.trim();
  endWorldNameEdit();
  if (!to || to === from) return;
  try {
    const folder = (await invoke('gui_rename_world', { savePath: savePath, oldName: from, newName: to })) || to;
    worldRenamed(from, folder);
  } catch (error) {
    // The engine says why (a clash, a path character, a world in use).
    const info = document.getElementById('progress-info');
    info.textContent = String((error && error.message) || error);
    info.style.color = "#fa7878";
  }
}

/* Custom world name (Java only, opt-in via Settings > Custom World Name) */

// Name for the next Java world, "" for the default "Arnis World N". The pencil
// only ever names the world the next Start creates, never one already on disk,
// and that world uses the name up. Only sent while the setting is on; the
// backend sanitizes it and appends " (2)" if the folder is taken.
let customWorldName = "";

function isCustomWorldNameFeatureEnabled() {
  const toggle = document.getElementById('custom-world-name-toggle');
  // Custom names are Java-only: creating a Bedrock/Luanti world never calls
  // gui_create_world, so offering the feature there would silently do
  // nothing.
  return !!(toggle && toggle.checked) && selectedWorldFormat === 'java';
}

// The name the next Start will ask for, if any.
function pendingWorldName() {
  if (isOneWorldEnabled() || !isCustomWorldNameFeatureEnabled()) return "";
  return customWorldName;
}

function canEditCustomWorldName() {
  // While a generation is in flight the next world's name is already taken
  // from here, so an edit would only look like it applied to the running one.
  // A live session plays the world under this name.
  return isCustomWorldNameFeatureEnabled() && generationButtonEnabled && !liveActive();
}

function updateWorldNamePreviewLabel() {
  if (isOneWorldEnabled()) {
    setWorldNameLabel(oneWorldDisplayName());
    return;
  }
  renderWorldNameLabel();
}

// Cancels any in-progress edit and shows/hides the pencil to match the
// current setting + world format. Safe to call anytime state that affects
// availability changes (toggle flipped, format switched, language changed).
function refreshWorldNameEditUI() {
  endWorldNameEdit();
  const editButton = document.getElementById('world-name-edit-button');
  if (editButton) {
    editButton.style.display = canEditCustomWorldName() ? '' : 'none';
    const title = isOneWorldEnabled()
      ? oneWorldText('world_name_edit_one_world', 'Choose the One World to extend or create')
      : oneWorldText('world_name_edit_title', 'Name the next world');
    editButton.title = title;
    editButton.setAttribute('aria-label', title);
  }
  updateWorldNamePreviewLabel();
}

// Marks the editor red once it is full. The input's own maxlength is what
// actually refuses further characters; this only makes that visible. The
// limit is read off that same attribute rather than duplicating the number
// here, so the colour can never disagree with what the field accepts.
function updateWorldNameLimitState() {
  const input = document.getElementById('world-name-input');
  if (!input) return;
  const limit = input.maxLength;
  if (limit > 0 && input.value.length >= limit) {
    input.setAttribute('data-at-limit', 'true');
  } else {
    input.removeAttribute('data-at-limit');
  }
}

function startWorldNameEdit(event) {
  if (event) {
    event.preventDefault();
    event.stopPropagation();
  }
  if (!canEditCustomWorldName()) return;

  const label = document.getElementById('world-name-label');
  const input = document.getElementById('world-name-input');
  const editButton = document.getElementById('world-name-edit-button');
  if (!label || !input) return;

  // Starts from the pending name, never from the last world's: that world
  // keeps its name, and asking for it again would only get "Name (2)".
  input.value = isOneWorldEnabled() ? oneWorldFolderName() : customWorldName;
  label.style.display = 'none';
  if (editButton) editButton.style.display = 'none';
  input.style.display = '';
  updateWorldNameLimitState();
  input.focus();
  input.select();
}

// Commits the pencil editor: remembers the name for the next generation, or
// clears it when left blank. Nothing on disk changes.
function commitWorldNameEdit() {
  const input = document.getElementById('world-name-input');
  if (!input) {
    endWorldNameEdit();
    return;
  }
  if (renamingWorld) {
    commitWorldRename(input);
    return;
  }
  const newName = input.value.trim();

  // One World: the name picks the world to extend or create. Nothing is renamed.
  if (isOneWorldEnabled()) {
    setOneWorldName(newName === ONE_WORLD_DEFAULT_NAME ? '' : newName);
    endWorldNameEdit();
    refreshOneWorldState();
    return;
  }

  customWorldName = newName;
  endWorldNameEdit();
}

// Hiding the still-focused input fires a native blur (asynchronously, after
// the handler that hid it has returned), which would otherwise re-enter the
// blur handler below and re-run the edit we are already finishing: Escape
// would re-commit what it just discarded.
// endWorldNameEdit() sets this whenever it hides a focused input.
let suppressNextWorldNameBlur = false;

// Shared teardown for both commit and cancel: hides the input, restores the
// label (and the pencil, if the feature is still enabled), and refreshes
// what the label shows.
function endWorldNameEdit() {
  const label = document.getElementById('world-name-label');
  const input = document.getElementById('world-name-input');
  const editButton = document.getElementById('world-name-edit-button');
  renamingWorld = "";
  if (!input || input.style.display === 'none') return;
  // Only when the input still holds focus: a teardown reached *from* the blur
  // handler must not arm this, or it would swallow the next real blur.
  if (document.activeElement === input) suppressNextWorldNameBlur = true;
  input.style.display = 'none';
  if (label) label.style.display = '';
  if (editButton && canEditCustomWorldName()) editButton.style.display = '';
  updateWorldNamePreviewLabel();
}

function initCustomWorldNameToggle() {
  const toggle = document.getElementById('custom-world-name-toggle');
  const editButton = document.getElementById('world-name-edit-button');
  const input = document.getElementById('world-name-input');
  if (!toggle) return;

  // Covers manual clicks and settings-store restoring/reverting the value
  // (both dispatch a real "change" event), as long as this listener is
  // attached before initSettingsStore() runs its restore().
  toggle.addEventListener('change', refreshWorldNameEditUI);

  document.getElementById('world-name-label')?.addEventListener('click', (event) => {
    if (document.getElementById('world-name-label').hasAttribute('data-renamable')) startWorldRename(event);
  });

  if (editButton) {
    editButton.addEventListener('click', startWorldNameEdit);
    editButton.addEventListener('mousedown', (event) => event.stopPropagation());
  }

  if (input) {
    // The input lives inside .world-name-row, a sibling of #start-button
    // (not nested inside it) that has its own onclick="startGeneration()"
    // for clicks on the label/background; without this, placing the caret
    // would bubble up and trigger that too.
    input.addEventListener('click', (event) => event.stopPropagation());
    input.addEventListener('mousedown', (event) => event.stopPropagation());
    // "input" rather than "keydown": it also covers pasting, cutting and
    // undo, and fires after the value has actually changed.
    input.addEventListener('input', updateWorldNameLimitState);
    input.addEventListener('keydown', (event) => {
      event.stopPropagation();
      if (event.key === 'Enter') {
        event.preventDefault();
        commitWorldNameEdit();
      } else if (event.key === 'Escape') {
        // Discard: tear the editor down without committing.
        event.preventDefault();
        endWorldNameEdit();
      }
    });
    input.addEventListener('blur', () => {
      if (suppressNextWorldNameBlur) {
        suppressNextWorldNameBlur = false;
        return;
      }
      commitWorldNameEdit();
    });
  }

  refreshWorldNameEditUI();
}

/* One World (Java only, opt-in via Settings > One World) */

const ONE_WORLD_DEFAULT_NAME = "Arnis One World";
const ONE_WORLD_NAME_KEY = 'arnis-one-world-name';
// The user's own values of the settings a One World pins, put back when the
// mode goes off.
const ONE_WORLD_RESTORE_KEY = 'arnis-one-world-restore';
// Last answer from gui_one_world_info, null while the mode is off.
let oneWorldInfo = null;
let oneWorldPinned = false;
// Key the map overlays were last sent for.
let oneWorldOverlayKey = null;
// Whether the running (or last) generation was a One World run.
let lastRunOneWorld = false;
// Whether it only filled the caches (Download now).
let lastRunPrewarm = false;
let oneWorldName = localStorage.getItem(ONE_WORLD_NAME_KEY) || '';
// Set by Settings > Projects > Open: the name above is then used with or without Custom World Name.
let oneWorldPicked = ['1', 'new'].includes(localStorage.getItem('arnis-one-world-picked'));
// Picked as a new One World in Projects: its folder is not there until the first Start.
let oneWorldNew = localStorage.getItem('arnis-one-world-picked') === 'new';

function refreshOneWorldRow() {
  const reason = why([selectedWorldFormat === 'java', 'reason_java'], [selectedCelestialBody === 'earth', 'reason_earth'],
    [!liveActive(), 'reason_live_running']);
  setSegmentedRowAvailable('one-world-group', !reason, reason);
}

function isOneWorldAvailable() {
  return selectedWorldFormat === 'java' && selectedCelestialBody === 'earth';
}

// Off, On (areas join one world) or Live (that world played on a local server).
function oneWorldMode() {
  const btn = document.querySelector('#one-world-group .segment.active');
  return btn ? btn.dataset.oneWorld : 'off';
}

function isOneWorldToggleOn() {
  return oneWorldMode() !== 'off';
}

// Luanti and Hytale cannot hold a One World, so they are off while the toggle is.
function refreshExperimentalFormatAvailability() {
  const on = isOneWorldToggleOn();
  for (const format of EXPERIMENTAL_FORMATS) {
    const btn = document.getElementById(`format-${format}`);
    if (btn) {
      btn.disabled = on;
      btn.title = on ? 'Not available in One World mode' : '';
    }
    setSettingsRowAvailable(`enable-${format}-toggle`, !on, 'reason_not_one_world');
    if (on && selectedWorldFormat === format) setWorldFormat('java');
  }
}

function isOneWorldEnabled() {
  return isOneWorldToggleOn() && isOneWorldAvailable();
}

function isLiveMode() {
  return oneWorldMode() === 'live' && isOneWorldAvailable();
}

// Says what the picked mode does.
function refreshOneWorldDesc() {
  const desc = document.getElementById('one-world-desc');
  if (!desc) return;
  const mode = oneWorldMode();
  const texts = {
    off: ['one_world_desc_off', 'Every area becomes a world of its own.'],
    on: ['one_world_desc_on', 'Each area joins one growing world at its real position.'],
    live: ['one_world_desc_live', 'One growing world that you play in Minecraft while Arnis generates the land around you.']
  };
  const [key, fallback] = texts[mode] || texts.off;
  desc.dataset.localize = key;
  desc.textContent = oneWorldText(key, fallback);
}

// A world keeps the game mode, time, mob spawning and map item of its first area, and a
// live world gets no map item at all.
function refreshWorldRulesRows() {
  const fixed = isOneWorldEnabled() && !!(oneWorldInfo && oneWorldInfo.exists);
  setSegmentedRowAvailable('gamemode-group', !fixed, 'reason_world_set');
  setSettingsRowAvailable('world-time-slider', !fixed, 'reason_world_set');
  const mapFormat = selectedWorldFormat === 'java' || selectedWorldFormat === 'bedrock';
  const mapWhy = why([mapFormat, 'reason_java_bedrock'], [!fixed, 'reason_world_set'], [!isLiveMode(), 'reason_live']);
  setSettingsRowAvailable('map-item-toggle', !mapWhy, mapWhy);
  refreshMobSpawningRow();
}

// The name picks the world; it never renames one on disk.
function oneWorldFolderName() {
  if (oneWorldName && (isCustomWorldNameFeatureEnabled() || (oneWorldPicked && !document.body.classList.contains('meld-off')))) return oneWorldName;
  return ONE_WORLD_DEFAULT_NAME;
}

function oneWorldDisplayName() {
  if (oneWorldInfo && oneWorldInfo.world_path) return basenameFromPath(oneWorldInfo.world_path);
  return oneWorldFolderName();
}

function setOneWorldName(name) {
  oneWorldName = name;
  oneWorldPicked = false;
  oneWorldNew = false;
  localStorage.removeItem('arnis-one-world-picked');
  if (name) localStorage.setItem(ONE_WORLD_NAME_KEY, name);
  else localStorage.removeItem(ONE_WORLD_NAME_KEY);
}

function setOneWorldStatus(text, tone) {
  // Every read of the world ends here: the stretch view follows its origin.
  refreshStretch();
  const el = document.getElementById('one-world-status');
  if (!el) return;
  el.style.display = text ? '' : 'none';
  el.textContent = text || '';
  el.classList.toggle('is-warn', tone === 'warn');
  el.classList.toggle('is-error', tone === 'error');
}

function renderOneWorldStatus() {
  const info = oneWorldInfo;
  if (!isOneWorldEnabled() || !info) return;
  const name = oneWorldDisplayName();
  if (!savePath) {
    setOneWorldStatus(oneWorldText('one_world_no_save_path', 'Set the Minecraft saves folder in Settings first'), 'error');
    return;
  }
  if (info.foreign) {
    setOneWorldStatus(oneWorldText('one_world_foreign', 'Folder "{name}" exists but is not a One World', { name }), 'error');
    return;
  }
  if (!info.exists && !liveActive()) {
    setOneWorldStatus(oneWorldText('one_world_will_create', 'One World "{name}" will be created', { name }));
    return;
  }
  let text = oneWorldText('one_world_areas', 'One World "{name}" \u00b7 Areas: {count}', { name, count: info.area_count || 0 });
  let tone = '';
  if (liveActive()) {
    const players = (liveStatus.players || []).length;
    text += ' \u00b7 ' + oneWorldText('live_players', 'Players: {count}', { count: players });
    if (liveStatus.tiles_failed > 0) {
      text += ' \u00b7 ' + oneWorldText('live_some_failed', 'some land could not be generated');
      tone = 'warn';
    }
    setOneWorldStatus(text, tone);
    return;
  }
  if (info.locked) {
    text += ' \u00b7 ' + oneWorldText('one_world_locked', 'open in Minecraft');
    tone = 'warn';
  }
  const mode = document.getElementById('generation-mode-select');
  const wantsTerrain = mode ? mode.value !== 'geo-only' : true;
  if (typeof info.terrain === 'boolean' && info.terrain !== wantsTerrain) {
    text += ' \u00b7 ' + oneWorldText(info.terrain ? 'one_world_needs_terrain' : 'one_world_needs_flat',
      info.terrain ? 'this world uses terrain' : 'this world is flat');
    tone = 'warn';
  }
  setOneWorldStatus(text, tone);
}

// Why a greyed row is greyed, said as what to do, in place of its description. Where that
// is another setting on this page, the text is a link that shows it.
const ROW_REASONS = {
  reason_earth: ['Switch the map to Earth.'],
  reason_moon_mars: ['Switch the map to the Moon or Mars.'],
  reason_java: ['Switch the world format to Java.'],
  reason_java_bedrock: ['Switch the world format to Java or Bedrock.'],
  reason_not_hytale: ['Switch the world format to Java, Bedrock or Luanti.'],
  reason_one_world: ['Switch One World to On.', 'one-world-group'],
  reason_not_one_world: ['Switch One World to Off.', 'one-world-group'],
  reason_live: ['Switch One World from Live to On.', 'one-world-group'],
  reason_live_running: ['Stop Live Mode first.'],
  reason_world_set: ['Locked by this One World. Start a new one to change it.'],
  reason_cells: ['Set Parallel cells to Auto.', 'meld-workers-select'],
  reason_height: ['Turn on Extend build height.', 'disable-height-limit-toggle'],
  reason_voxy: ['Turn off Pre-generate Voxy LOD.', 'voxy-lod-toggle'],
  reason_dh: ['Turn off Pre-generate Distant Horizons LOD.', 'dh-lod-toggle'],
  reason_blinear: ['Pick B_Linear as the Region Format.', 'meld-region-format-group'],
  reason_rocks: ['Turn on Rocks.', 'meld-rocks-toggle'],
  reason_bushes: ['Turn on Bushes.', 'meld-bushes-toggle'],
  reason_caves: ['Turn on Caves.', 'caves-toggle'],
  reason_tree_folder: ['Enter a Custom Tree Folder.', 'meld-tree-pack-dir-input'],
  reason_archive: ['Pick Local Archive as the OSM Data Source.', 'meld-osm-source-group'],
  reason_region_download: ['Pick Region Download as the OSM Data Source.', 'meld-osm-source-group'],
  reason_blueprint: ['Pick a Generation Mode other than Blueprint.', 'generation-mode-select'],
  reason_token: ['Enter a Mapillary Token.', 'mapillary-token'],
  reason_outbreak: ['Turn off Zombie Outbreak.', 'zombie-outbreak-toggle'],
  reason_disasters: ['Turn off Disasters.', 'disasters-toggle'],
};

// The key of the first check that fails, as [ok, key] pairs; null when all pass.
const why = (...checks) => checks.find(([ok]) => !ok)?.[1] || null;

// Scrolls to the setting a reason names, lights it up for a moment and moves focus there.
function showSetting(id) {
  const row = document.getElementById(id)?.closest('.settings-row');
  if (!row) return;
  const search = document.getElementById('settings-search');
  if (search?.value && row.classList.contains('search-miss')) {
    search.value = '';
    search.dispatchEvent(new Event('input'));
  }
  if (row.offsetParent === null) return;
  const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  row.scrollIntoView({ block: 'center', behavior: reduced ? 'auto' : 'smooth' });
  clearTimeout(row.pointedTimer);
  row.classList.remove('is-pointed');
  void row.offsetWidth;
  row.classList.add('is-pointed');
  row.pointedTimer = setTimeout(() => row.classList.remove('is-pointed'), 1600);
  const control = row.querySelector('input:not([type=hidden]):not(:disabled), select:not(.visually-hidden):not(:disabled), .segment.active:not(:disabled), button:not(.tooltip-icon):not(.setting-revert):not(.setting-reason):not(:disabled)');
  control?.focus({ preventScroll: true });
}

// A greyed row with a reason keeps its name readable and only its controls locked, so the
// reason can be a link. The callers leave such a row out of `inert`.
function setRowReason(row, key) {
  if (!row) return;
  // While a Live session runs, One World is locked: the step before any change to it.
  if (key && liveActive() && ROW_REASONS[key]?.[1] === 'one-world-group') key = 'reason_live_running';
  if (key && !ROW_REASONS[key]) key = null;
  let note = row.querySelector('.setting-reason');
  row.classList.toggle('has-reason', !!key);
  const locked = row.querySelectorAll(':scope > :not(.setting-label), .setting-label > :not(.setting-text):not(.setting-heading):not(.setting-reason), .setting-heading > .tooltip-icon, .setting-heading > .setting-revert');
  locked.forEach((el) => { el.inert = !!key; });
  if (!key) {
    if (note) note.remove();
    return;
  }
  row.removeAttribute('inert');
  const [fallback, target] = ROW_REASONS[key];
  if (!note || note.dataset.reason !== key) {
    note?.remove();
    note = document.createElement(target ? 'button' : 'span');
    note.className = 'setting-reason';
    note.dataset.reason = key;
    if (target) {
      note.type = 'button';
      note.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        showSetting(target);
      });
    }
    (row.querySelector('.setting-text') || row.querySelector('.setting-label') || row).appendChild(note);
  }
  note.dataset.localize = key;
  const text = oneWorldText(key, fallback);
  if (note.textContent !== text) note.textContent = text;
}

function setSettingsRowAvailable(inputId, available, reason = null) {
  const input = document.getElementById(inputId);
  if (!input) return;
  input.disabled = !available;
  const row = input.closest('.settings-row');
  if (row) {
    row.classList.toggle('settings-row-unavailable', !available);
    row.toggleAttribute('inert', !available && !reason);
    setRowReason(row, available ? null : reason);
  }
}

// Advanced settings that only work with another one grey out while it is off, as the gameplay rows do,
// so no two settings contradict each other and nothing is set that a run would ignore.
function refreshAdvancedRows() {
  const val = (id) => document.getElementById(id)?.value || '';
  const on = (id) => { const el = document.getElementById(id); return !!(el && el.checked && !el.disabled); };
  const seg = (id) => document.querySelector(`#${id} .segment.active`)?.dataset.v || '';
  // Live builds with standard settings, so nothing here applies while it is the mode.
  const live = isLiveMode();
  // In Live, a row that needs One World off says that, not the step to On.
  const liveWhy = (ok, reason) => (live && (ok || reason !== 'reason_not_one_world') ? 'reason_live' : reason);
  const row = (id, ok, reason = null) => setSettingsRowAvailable(id, ok && !live, liveWhy(ok, reason));
  const group = (id, ok, reason = null) => {
    row(id, ok, reason);
    setSegmentedRowAvailable(id, ok && !live, liveWhy(ok, reason));
  };
  const java = selectedWorldFormat === 'java';
  const earth = selectedCelestialBody === 'earth';
  const oneWorld = isOneWorldEnabled();
  const cells = oneWorld && !!val('meld-workers-select');
  row('meld-rocks-toggle', earth, 'reason_earth');
  row('meld-bushes-toggle', earth, 'reason_earth');
  const owChecks = [[java, 'reason_java'], [earth, 'reason_earth'], [oneWorld, 'reason_one_world']];
  row('meld-workers-select', oneWorld, why(...owChecks));
  const cellsWhy = why(...owChecks, [cells, 'reason_cells']);
  group('meld-piece-size-group', cells, cellsWhy);
  group('meld-snap-group', cells, cellsWhy);
  for (const id of ['meld-margin-input', 'meld-piece-order-select', 'meld-skip-built-toggle', 'meld-stitch-map-toggle']) row(id, cells, cellsWhy);
  row('meld-origin-input', oneWorld, why(...owChecks));
  row('meld-stretch-toggle', oneWorld, why(...owChecks));
  const heightWhy = why([java, 'reason_java'], [!oneWorld, 'reason_not_one_world'], [earth, 'reason_earth'],
    [on('disable-height-limit-toggle'), 'reason_height']);
  row('meld-min-y-input', !heightWhy, heightWhy);
  row('meld-max-y-input', !heightWhy, heightWhy);
  row('meld-world-border-toggle', java, 'reason_java');
  const formatChecks = [[java, 'reason_java'], [!oneWorld, 'reason_not_one_world'], [!on('voxy-lod-toggle'), 'reason_voxy'],
    [!on('dh-lod-toggle'), 'reason_dh']];
  const formatWhy = why(...formatChecks);
  group('meld-region-format-group', !formatWhy, formatWhy);
  const levelWhy = why(...formatChecks, [seg('meld-region-format-group') === 'blinear', 'reason_blinear']);
  row('meld-blinear-level-input', !levelWhy, levelWhy);
  const rockWhy = why([earth, 'reason_earth'], [on('meld-rocks-toggle'), 'reason_rocks']);
  row('meld-rock-density-input', !rockWhy, rockWhy);
  const bushWhy = why([earth, 'reason_earth'], [on('meld-bushes-toggle'), 'reason_bushes']);
  row('meld-bush-density-input', !bushWhy, bushWhy);
  group('meld-tree-pack-mode-group', !!val('meld-tree-pack-dir-input').trim(), 'reason_tree_folder');
  row('meld-cave-style-select', on('caves-toggle'), 'reason_caves');
  group('meld-cave-ores-group', on('caves-toggle'), 'reason_caves');
  row('meld-archive-folder-input', seg('meld-osm-source-group') === 'archive', 'reason_archive');
  row('meld-pbf-url-input', seg('meld-osm-source-group') === 'region', 'reason_region_download');
  for (const id of ['meld-seed-input', 'meld-surface-patches-toggle', 'meld-buildings-toggle', 'meld-tree-realm-select',
    'meld-tree-pack-dir-input', 'meld-overpass-input', 'meld-offline-toggle', 'meld-ram-budget-input', 'meld-threads-input']) row(id, true);
  for (const id of ['meld-road-detail-group', 'meld-props-group', 'meld-river-bed-group', 'meld-water-detail-group',
    'meld-climate-mode-group', 'meld-osm-source-group']) group(id, true);
  // Auto's page follows One World and Parallel cells, which these same changes move.
  refreshLayout();
}

function initAdvancedRows() {
  const modal = document.getElementById('settings-modal');
  if (!modal) return;
  // Any change in Settings can be the one a row depends on; the check is cheap.
  for (const ev of ['change', 'input', 'click']) modal.addEventListener(ev, () => setTimeout(refreshAdvancedRows, 0));
  refreshAdvancedRows();
}

function readOneWorldRestore() {
  try {
    return JSON.parse(localStorage.getItem(ONE_WORLD_RESTORE_KEY) || '{}') || {};
  } catch (_) {
    return {};
  }
}

function writeOneWorldRestore(record) {
  if (Object.keys(record).length) localStorage.setItem(ONE_WORLD_RESTORE_KEY, JSON.stringify(record));
  else localStorage.removeItem(ONE_WORLD_RESTORE_KEY);
}

function controlValue(el) {
  return el.type === 'checkbox' ? el.checked : parseFloat(el.value);
}

function writeControl(el, value) {
  if (controlValue(el) === value) return;
  if (el.type === 'checkbox') el.checked = !!value;
  else el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  el.dispatchEvent(new Event('change', { bubbles: true }));
}

// Pins a control to the world's value, remembering the user's own once.
// `null` puts the user's value back.
function pinControl(id, value) {
  const el = document.getElementById(id);
  if (!el) return;
  const record = readOneWorldRestore();
  if (value === null) {
    if (id in record) {
      writeControl(el, record[id]);
      delete record[id];
      writeOneWorldRestore(record);
    }
    return;
  }
  if (!(id in record)) {
    record[id] = controlValue(el);
    writeOneWorldRestore(record);
  }
  writeControl(el, value);
}

// Settings the backend builds into Java worlds only.
function refreshJavaOnlyRows(format) {
  const java = (format || selectedWorldFormat) === 'java';
  if (!oneWorldPinned) {
    setSettingsRowAvailable('voxy-lod-toggle', java, 'reason_java');
    setSettingsRowAvailable('dh-lod-toggle', java, 'reason_java');
  }
  setSettingsRowAvailable('bake-lighting-toggle', java, 'reason_java');
  const signWhy = why([java, 'reason_java'], [selectedCelestialBody === 'earth', 'reason_earth']);
  setSegmentedRowAvailable('signage-group', !signWhy, signWhy);
  refreshWorldNameRow(format);
}

function refreshWorldNameRow(format) {
  const reason = why([(format || selectedWorldFormat) === 'java', 'reason_java'], [!liveActive(), 'reason_live_running']);
  setSettingsRowAvailable('custom-world-name-toggle', !reason, reason);
}

function restoreNaturalRows() {
  const java = selectedWorldFormat === 'java';
  setSettingsRowAvailable('scale-value-slider', selectedCelestialBody === 'earth', 'reason_earth');
  setSettingsRowAvailable('height-multiplier-slider', true);
  setSettingsRowAvailable('aws-only-elevation-toggle', selectedCelestialBody === 'earth', 'reason_earth');
  setSettingsRowAvailable('voxy-lod-toggle', java, 'reason_java');
  setSettingsRowAvailable('dh-lod-toggle', java, 'reason_java');
  setSettingsRowAvailable('disable-height-limit-toggle', true);
  refreshHeightLimitRow();
}

// Rotation stays 0, the LOD caches off and the build height extended while
// the mode is on; scale, build height and elevation source follow the world
// once it exists.
function applyOneWorldPins(info) {
  oneWorldPinned = true;
  const rotationInput = document.getElementById('rotation-angle-input');
  if (rotationInput && parseFloat(rotationInput.value) !== 0 && window.updateRotation) window.updateRotation(0);
  setSettingsRowAvailable('rotation-angle-input', false, 'reason_not_one_world');
  setSettingsRowAvailable('voxy-lod-toggle', false, 'reason_not_one_world');
  setSettingsRowAvailable('dh-lod-toggle', false, 'reason_not_one_world');
  postToMap({ type: 'setRotationLocked', locked: true });
  syncShapesRect();

  const exists = !!(info && info.exists);
  pinControl('scale-value-slider', exists && typeof info.scale === 'number' ? info.scale : null);
  pinControl('height-multiplier-slider',
    exists && typeof info.height_multiplier === 'number' ? info.height_multiplier : null);
  pinControl('disable-height-limit-toggle',
    exists && typeof info.disable_height_limit === 'boolean' ? info.disable_height_limit : true);
  restoreNaturalRows();
  setSettingsRowAvailable('voxy-lod-toggle', false, 'reason_not_one_world');
  setSettingsRowAvailable('dh-lod-toggle', false, 'reason_not_one_world');
  setSettingsRowAvailable('disable-height-limit-toggle', false, 'reason_not_one_world');
  if (exists) {
    setSettingsRowAvailable('scale-value-slider', false, 'reason_world_set');
    setSettingsRowAvailable('height-multiplier-slider', false, 'reason_world_set');
    setSettingsRowAvailable('aws-only-elevation-toggle', false, 'reason_world_set');
  }
}

function releaseOneWorldPins() {
  if (!oneWorldPinned) return;
  oneWorldPinned = false;
  pinControl('scale-value-slider', null);
  pinControl('height-multiplier-slider', null);
  pinControl('disable-height-limit-toggle', null);
  setSettingsRowAvailable('rotation-angle-input', true);
  restoreNaturalRows();
  postToMap({ type: 'setRotationLocked', locked: false });
  postToMap({ type: 'setShapesMode', mode: '' });
}

// A One World takes rectangles only. With Advanced features it builds the rectangle around the
// drawn shapes (the map outlines it), and in pieces the shapes are the plan mask.
function syncShapesRect() {
  const meld = meldOn();
  const pieces = meld && document.getElementById('meld-workers-select').value;
  if (oneWorldPinned) postToMap({ type: 'setShapesMode', mode: !meld ? 'locked' : pieces ? 'pieces' : 'rect' });
  if (!outline && selectedShapes && wantsShapesMask() !== shapesWritten) writeShapesMask();
  refreshBboxSelectionInfo();
}
document.addEventListener('change', syncShapesRect);

// The areas of the One World on the map, which "Expand" rings.
let builtAreas = [];
function setBuiltAreas(areas) {
  builtAreas = areas;
  document.getElementById('plan-expand-button').hidden = !areas.length;
}

function clearOneWorldOverlays() {
  setBuiltAreas([]);
  if (oneWorldOverlayKey === null) return;
  oneWorldOverlayKey = null;
  postToMap({ type: 'oneWorldOverlays', areas: [] });
}

function isWorldNameEditing() {
  const input = document.getElementById('world-name-input');
  return !!(input && input.style.display !== 'none');
}

async function fetchOneWorldInfo() {
  return invoke('gui_one_world_info', { savePath: savePath, worldName: oneWorldFolderName() });
}

// Re-reads the world the next area joins. Call whenever something that picks
// the world changes: toggle, name, save path, format, body, window focus.
let oneWorldRefreshSeq = 0;
let oneWorldRefit = false;
let redrawProjects = () => {};
async function refreshOneWorldState() {
  const seq = ++oneWorldRefreshSeq;
  refreshLayout();
  // An outline picked while One World was off is written once it is on.
  if (outline && isOneWorldEnabled() !== outlineWritten) noteOutline(outline);
  if (!outline && selectedShapes && wantsShapesMask() !== shapesWritten) writeShapesMask();
  refreshExperimentalFormatAvailability();
  refreshOneWorldRow();
  redrawProjects();
  refreshLiveRows();
  refreshStartButton();
  if (!isOneWorldEnabled()) {
    const wasOn = oneWorldInfo !== null;
    oneWorldInfo = null;
    releaseOneWorldPins();
    refreshWorldRulesRows();
    setOneWorldStatus('');
    clearOneWorldOverlays();
    if (wasOn) setWorldNameLabel(basenameFromPath(worldPath));
    if (!isWorldNameEditing()) refreshWorldNameEditUI();
    return;
  }
  if (!savePath) {
    oneWorldInfo = { exists: false, foreign: false };
    applyOneWorldPins(oneWorldInfo);
    renderOneWorldStatus();
    return;
  }
  let info;
  try {
    info = await fetchOneWorldInfo();
  } catch (error) {
    if (seq !== oneWorldRefreshSeq) return;
    console.error('Failed to read the One World:', error);
    oneWorldInfo = null;
    setOneWorldStatus(String(error), 'error');
    clearOneWorldOverlays();
    return;
  }
  if (seq !== oneWorldRefreshSeq) return;
  if (oneWorldPicked && oneWorldFolderName() === oneWorldName && info) {
    if (oneWorldNew && info.exists) {
      // Built: from now on it is a world like any other picked in Projects.
      oneWorldNew = false;
      localStorage.setItem('arnis-one-world-picked', '1');
    } else if (!oneWorldNew && !info.exists) {
      // The world picked in Projects is gone (deleted outside Arnis): back to the default name.
      setOneWorldName('');
      return refreshOneWorldState();
    }
  }
  oneWorldInfo = info;
  applyOneWorldPins(info);
  refreshWorldRulesRows();
  if (!isWorldNameEditing()) refreshWorldNameEditUI();
  renderOneWorldStatus();

  if (!info.exists || info.foreign) {
    clearOneWorldOverlays();
    return;
  }
  const key = info.world_path + '|' + info.revision + '|' + info.area_count;
  if (key === oneWorldOverlayKey) return;
  try {
    const overlays = await invoke('gui_get_one_world_overlays', { worldPath: info.world_path });
    if (seq !== oneWorldRefreshSeq) return;
    oneWorldOverlayKey = key;
    setBuiltAreas((overlays && overlays.areas) || []);
    postToMap({
      type: 'oneWorldOverlays',
      world_path: info.world_path,
      refit: oneWorldRefit,
      areas: (overlays && overlays.areas) || [],
      origin_lat: overlays ? overlays.origin_lat : 0,
      origin_lon: overlays ? overlays.origin_lon : 0,
      scale: overlays ? overlays.scale : 1
    });
  } catch (error) {
    console.error('Failed to load the One World overlays:', error);
  }
}

// Resolves when the user answers the overlap question.
function confirmOneWorldOverlap(count) {
  return new Promise((resolve) => {
    const modal = document.getElementById('one-world-confirm-modal');
    const text = document.getElementById('one-world-confirm-text');
    const ok = document.getElementById('one-world-confirm-ok');
    const cancel = document.getElementById('one-world-confirm-cancel');
    const close = document.getElementById('one-world-confirm-close');
    if (!modal || !text || !ok || !cancel) {
      resolve(true);
      return;
    }
    text.textContent = oneWorldText('one_world_confirm_text',
      '{count} chunks of this area already exist in the world. They are generated again, and anything built there in Minecraft is replaced.',
      { count });
    const finish = (answer) => {
      hideModal(modal);
      ok.removeEventListener('click', onOk);
      cancel.removeEventListener('click', onCancel);
      if (close) close.removeEventListener('click', onCancel);
      document.removeEventListener('keydown', onKey);
      resolve(answer);
    };
    const onOk = () => finish(true);
    const onCancel = () => finish(false);
    const onKey = (event) => { if (event.key === 'Escape') finish(false); };
    ok.addEventListener('click', onOk);
    cancel.addEventListener('click', onCancel);
    if (close) close.addEventListener('click', onCancel);
    document.addEventListener('keydown', onKey);
    showModal(modal);
    ok.focus();
  });
}

// Checks the world right before a run. Returns false when the run must not start.
// `held`: the hold to start already said what gets rebuilt, so no second question.
async function prepareOneWorldRun(held = false) {
  if (!savePath) {
    renderOneWorldStatus();
    return false;
  }
  let info;
  try {
    info = await fetchOneWorldInfo();
  } catch (error) {
    setOneWorldStatus(String(error), 'error');
    return false;
  }
  oneWorldInfo = info;
  applyOneWorldPins(info);
  renderOneWorldStatus();
  if (info.foreign) return false;
  if (info.locked) {
    setOneWorldStatus(oneWorldText('one_world_areas', 'One World "{name}" \u00b7 Areas: {count}',
      { name: oneWorldDisplayName(), count: info.area_count }) + ' \u00b7 ' +
      oneWorldText('one_world_locked', 'open in Minecraft'), 'error');
    return false;
  }
  if (!info.exists || held) return true;
  let overlap = 0;
  try {
    overlap = await invoke('gui_one_world_overlap', {
      savePath: savePath, worldName: oneWorldFolderName(), bboxText: selectedBBox
    });
  } catch (error) {
    setOneWorldStatus(String(error), 'error');
    return false;
  }
  return overlap > 0 ? confirmOneWorldOverlap(overlap) : true;
}

function postToMap(message) {
  const mapFrame = document.querySelector('.map-container');
  if (mapFrame && mapFrame.contentWindow) {
    mapFrame.contentWindow.postMessage(message, '*');
  }
}

// ---- Live mode: the One World on a local Minecraft server, generated around the players ----

const LIVE_VERSION_KEY = 'arnis-live-version';
const LIVE_VIEW_DISTANCES = [8, 12, 16, 24, 32];
// Last state the backend reported, null before the first session.
let liveStatus = null;
// Versions to offer and whether the EULA is accepted, read once Live is picked.
let liveInfo = null;
let liveInfoLoading = null;
// The saves folder and world liveInfo was read for.
let liveInfoKey = '';
// The selection and spawn marker the last session started from, per world.
const LIVE_START_KEY = 'arnis-live-start';

function liveActive() {
  return !!liveStatus && ['starting', 'running', 'stopping'].includes(liveStatus.phase);
}

function liveHasSelection() {
  return !!selectedBBox && selectedBBox !== "0.000000 0.000000 0.000000 0.000000";
}

function loadLiveInfo() {
  if (liveInfoLoading) return liveInfoLoading;
  const key = (savePath || '') + '|' + oneWorldFolderName();
  liveInfoLoading = invoke('gui_live_info', { savePath: savePath || '', worldName: oneWorldFolderName() })
    .then((info) => {
      liveInfo = info;
      liveInfoKey = key;
      fillLiveVersions();
      return info;
    })
    .catch((error) => {
      console.error('Failed to read live mode info:', error);
      return null;
    })
    .finally(() => { liveInfoLoading = null; });
  return liveInfoLoading;
}

function fillLiveVersions() {
  const select = document.getElementById('live-version-select');
  if (!select || !liveInfo) return;
  Array.from(select.options).filter((o) => o.value).forEach((o) => o.remove());
  const auto = select.options[0];
  if (auto) {
    auto.textContent = oneWorldText('live_version_auto', 'Automatic') +
      (liveInfo.default_version ? ` (${liveInfo.default_version})` : '');
  }
  const versions = liveInfo.versions || [];
  for (const v of versions) {
    const opt = document.createElement('option');
    opt.value = v;
    opt.textContent = v === liveInfo.last_played
      ? `${v} (${oneWorldText('live_last_played', 'last played')})`
      : v;
    select.appendChild(opt);
  }
  const saved = localStorage.getItem(LIVE_VERSION_KEY) || '';
  select.value = versions.includes(saved) ? saved : '';
}

function labelLiveViewDistances() {
  const select = document.getElementById('live-view-distance-select');
  if (!select) return;
  for (const opt of select.options) {
    opt.textContent = oneWorldText('live_chunks', '{count} chunks', { count: opt.value });
  }
}

// The version and view distance only mean something in Live, and stay put while it runs.
function refreshLiveRows() {
  const live = isLiveMode();
  for (const id of ['live-version-row', 'live-view-distance-row']) {
    const row = document.getElementById(id);
    if (row) row.style.display = live ? '' : 'none';
  }
  document.getElementById('one-world-stack')?.classList.toggle('is-open', live);
  setSettingsRowAvailable('live-version-select', !liveActive(), 'reason_live_running');
  setSettingsRowAvailable('live-view-distance-select', !liveActive(), 'reason_live_running');
  // The world the session plays is fixed until it ends.
  setSettingsRowAvailable('save-path-input', !liveActive(), 'reason_live_running');
  refreshWorldNameRow();
  if (liveInfo && liveInfoKey !== (savePath || '') + '|' + oneWorldFolderName()) liveInfo = null;
  if (live && !liveInfo) loadLiveInfo();
}

// In Live the main button starts and stops the session.
function refreshStartButton() {
  const button = document.getElementById('start-button');
  const label = button && button.querySelector('span');
  if (!label) return;
  let key = 'start_generation';
  let fallback = 'Start Generation';
  let vars;
  // A job in pieces an earlier run built part of says how much.
  const plan = planPieces();
  const stopping = liveActive() && liveStatus.phase === 'stopping';
  if (stopping) {
    key = 'live_stopping_button';
    fallback = 'Stopping\u2026';
  } else if (liveActive()) {
    key = 'live_stop_button';
    fallback = 'Stop Live Mode';
  } else if (isLiveMode()) {
    key = 'live_play';
    fallback = 'Play Live';
  } else if (plan && plan.done && plan.done < plan.n) {
    key = 'plan_resume';
    fallback = 'Resume ({n} of {m})';
    vars = { n: plan.done, m: plan.n };
  }
  label.dataset.localize = key;
  label.textContent = oneWorldText(key, fallback, vars);
  const wrap = button.closest('.start-button-wrap');
  if (wrap) {
    wrap.classList.toggle('is-live', liveActive() && !stopping);
    wrap.classList.toggle('is-stopping', stopping);
  }
}

// The backend names each step; the words are the GUI's, in the user's language.
function liveStageText(status) {
  // The server stopped from the game is still saving.
  if ((status.phase === 'stopping' && !['saving', 'stopping'].includes(status.stage)) ||
    status.stage === 'stopped_in_game') {
    return oneWorldText('live_stage_stopping', 'Stopping the server and saving the world');
  }
  switch (status.stage) {
    case 'checking':
      return oneWorldText('live_stage_checking', 'Checking the world');
    case 'start_area':
      return status.step
        ? oneWorldText('live_stage_start_area', 'Generating the land around the start: {done} of {total}',
          { done: status.step[0], total: status.step[1] })
        : oneWorldText('live_stage_starting', 'Starting');
    case 'server_starting':
      return oneWorldText('live_stage_server', 'Starting the Minecraft {version} server', { version: status.version || '' });
    case 'running':
      if (status.teleport_pending) return oneWorldText('live_stage_teleport', 'Generating the place you picked');
      if (status.teleport_failed) return oneWorldText('live_teleport_failed', 'The place you picked could not be generated');
      return oneWorldText('live_ready', 'Ready. Join "Arnis Live" under Multiplayer.');
    case 'saving':
      return oneWorldText('live_stage_saving', 'Saving the world');
    case 'stopping':
      return oneWorldText('live_stage_stopping', 'Stopping the server and saving the world');
    case 'stopped':
      return oneWorldText('live_stage_stopped', 'Stopped. The world is saved.');
    case 'failed':
      return status.error || oneWorldText('live_stage_failed', 'Live mode stopped because of an error.');
    default:
      return oneWorldText('live_stage_starting', 'Starting');
  }
}

function liveDownloadText(download) {
  const percent = typeof download.progress === 'number' ? Math.round(download.progress * 100) : 0;
  if (download.what === 'java') {
    return oneWorldText('live_download_java', 'Downloading Java for the server: {percent}%', { percent });
  }
  return oneWorldText('live_download_server', 'Downloading the Minecraft {version} server: {percent}%',
    { version: (liveStatus && liveStatus.version) || '', percent });
}

// A session reports in the progress line and bar, where a generation does.
function renderLiveProgress() {
  const status = liveStatus;
  const info = document.getElementById('progress-info');
  const bar = document.getElementById('progress-bar');
  const detail = document.getElementById('progress-detail');
  if (!status || !info || !bar || !detail) return;
  let text = liveStageText(status);
  let fraction = typeof status.progress === 'number' ? status.progress : null;
  if (status.download && status.phase === 'starting') {
    text = liveDownloadText(status.download);
    fraction = typeof status.download.progress === 'number' ? status.download.progress : fraction;
  }
  const ready = status.stage === 'running' && !status.teleport_pending && !status.teleport_failed;
  if (['running', 'stopped'].includes(status.phase) || status.stage === 'stopped') fraction = 1;
  if (status.phase === 'failed') fraction = 0;
  info.textContent = text;
  info.style.color = status.phase === 'failed' ? '#fa7878'
    : status.stage === 'running' && status.teleport_failed ? '#fecc44'
      : ready ? '#7bd864' : '#ececec';
  // The percentage stays beside the bar, as in a generation, so the bar keeps its width.
  if (fraction !== null) {
    bar.style.width = `${Math.round(fraction * 100)}%`;
    detail.textContent = `${Math.round(fraction * 100)}%`;
  }
  resetEta();
}

// Resolves to true once the user accepted Mojang's EULA.
function confirmLiveEula() {
  return new Promise((resolve) => {
    const modal = document.getElementById('live-modal');
    const accept = document.getElementById('live-accept');
    const cancel = document.getElementById('live-cancel');
    const close = document.getElementById('live-close');
    if (!modal || !accept || !cancel) {
      resolve(false);
      return;
    }
    const finish = (answer) => {
      hideModal(modal);
      accept.removeEventListener('click', onAccept);
      cancel.removeEventListener('click', onCancel);
      if (close) close.removeEventListener('click', onCancel);
      document.removeEventListener('keydown', onKey);
      resolve(answer);
    };
    const onAccept = () => finish(true);
    const onCancel = () => finish(false);
    const onKey = (event) => { if (event.key === 'Escape') finish(false); };
    accept.addEventListener('click', onAccept);
    cancel.addEventListener('click', onCancel);
    if (close) close.addEventListener('click', onCancel);
    document.addEventListener('keydown', onKey);
    showModal(modal);
    accept.focus();
  });
}

// What players read in the game, in the language set here.
function liveGameMessages() {
  return {
    welcome: oneWorldText('live_game_welcome', 'Arnis keeps generating the land as you explore.'),
    building: oneWorldText('live_game_building', 'Arnis is generating the land ahead'),
    wall: oneWorldText('live_game_wall', 'Arnis is still generating this way'),
    moved_back: oneWorldText('live_game_moved_back', 'That land is not ready yet'),
    teleport_wait: oneWorldText('live_game_teleport_wait', 'Arnis is generating the place you picked. You will be moved there when it is ready.'),
    teleport_failed: oneWorldText('live_game_teleport_failed', 'Arnis could not generate the place you picked.')
  };
}

function showLiveRefusal(text) {
  const info = document.getElementById('progress-info');
  if (!info) return;
  info.textContent = text;
  info.style.color = '#fa7878';
}

async function startLive() {
  if (liveActive()) return;
  if (!savePath) {
    showLiveRefusal(oneWorldText('one_world_no_save_path', 'Set the Minecraft saves folder in Settings first'));
    return;
  }
  // A lock is left to the backend: it may be a server an earlier Arnis left running.
  const world = oneWorldInfo;
  if (world && world.foreign) {
    renderOneWorldStatus();
    return;
  }
  if (!liveHasSelection() && !(world && world.exists)) {
    setBboxSelectionInfo(document.getElementById('bbox-selection-info'), "select_location_first", "#fa7878");
    return;
  }
  let acceptEula = false;
  let accepted = false;
  try {
    accepted = await invoke('gui_live_eula_accepted');
  } catch (_) { /* asked below */ }
  if (!accepted) {
    if (!(await confirmLiveEula())) return;
    acceptEula = true;
  }
  if (liveActive() || !isLiveMode()) return;

  const settings = readGenerationSettings(true);
  let spawnPoint = null;
  const mapFrame = document.querySelector('.map-container');
  if (liveHasSelection() && mapFrame && mapFrame.contentWindow && mapFrame.contentWindow.getSpawnPointCoords) {
    const coords = mapFrame.contentWindow.getSpawnPointCoords();
    if (coords) spawnPoint = [coords.lat, normalizeLongitude(coords.lng)];
  }
  // A selection starts play once; played again unchanged, players go on where they left.
  const startKey = [oneWorldFolderName(), savePath, selectedBBox, JSON.stringify(spawnPoint)].join('|');
  const useSelection = liveHasSelection() &&
    !(world && world.exists && localStorage.getItem(LIVE_START_KEY) === startKey);
  const versionSelect = document.getElementById('live-version-select');
  const viewSelect = document.getElementById('live-view-distance-select');
  const options = {
    savePath: savePath,
    worldName: oneWorldFolderName(),
    version: (versionSelect && versionSelect.value) || null,
    viewDistance: parseInt(viewSelect && viewSelect.value, 10) || 12,
    acceptEula: acceptEula,
    bboxText: useSelection ? selectedBBox : null,
    spawnPoint: useSelection ? spawnPoint : null,
    worldScale: settings.worldScale,
    heightMultiplier: settings.heightMultiplier,
    generationMode: settings.generationMode,
    winterEnabled: settings.winterEnabled,
    postApocalypticEnabled: settings.postApocalypticEnabled,
    interiorEnabled: settings.interiorEnabled,
    fillgroundEnabled: settings.fillgroundEnabled,
    cavesEnabled: settings.cavesEnabled,
    legacyTreesEnabled: settings.legacyTreesEnabled,
    maxTreeSize: settings.maxTreeSize,
    fieldPreset: settings.fieldPreset,
    canopyHeightEnabled: settings.canopyHeightEnabled,
    overtureEnabled: settings.overtureEnabled,
    use3dEnabled: settings.use3dEnabled,
    awsOnlyElevation: settings.awsOnlyElevation,
    bakeLightingEnabled: settings.bakeLightingEnabled,
    signage: settings.signage,
    mapillaryToken: settings.mapillaryToken,
    facadesEnabled: settings.facadesEnabled,
    facadeMode: settings.facadeMode,
    gamemode: settings.gamemode,
    worldTime: settings.worldTime,
    mobSpawning: settings.mobSpawning,
    messages: liveGameMessages()
  };
  liveStatus = { phase: 'starting', stage: 'starting' };
  const bar = document.getElementById('progress-bar');
  if (bar) bar.style.width = '0%';
  const detail = document.getElementById('progress-detail');
  if (detail) detail.textContent = '0%';
  onLiveStatus();
  setWorldNameLabel(oneWorldFolderName());
  try {
    await invoke('gui_live_start', { options: options });
    if (liveInfo) liveInfo.eula_accepted = true;
    if (useSelection) localStorage.setItem(LIVE_START_KEY, startKey);
  } catch (error) {
    liveStatus = { phase: 'failed', stage: 'failed', error: String(error) };
    onLiveStatus();
  }
}

async function stopLive() {
  if (!liveActive() || liveStatus.phase === 'stopping') return;
  liveStatus.phase = 'stopping';
  onLiveStatus();
  try {
    await invoke('gui_live_stop');
  } catch (error) {
    console.error('Failed to stop live mode:', error);
  }
}

// Everything that shows the session, after each report.
function onLiveStatus() {
  renderLiveProgress();
  refreshStartButton();
  if (!isWorldNameEditing()) refreshWorldNameEditUI();
  refreshLiveRows();
  refreshOneWorldRow();
  redrawProjects();
  const bedrockBtn = document.getElementById('format-bedrock');
  if (bedrockBtn) bedrockBtn.disabled = liveActive();
  renderOneWorldStatus();
  postToMap({
    type: 'livePlayers',
    active: liveActive(),
    canTeleport: !!liveStatus && liveStatus.stage === 'running',
    players: (liveStatus && liveStatus.players) || []
  });
}

function initLive() {
  document.getElementById('live-eula-link')?.addEventListener('click', (event) => {
    event.preventDefault();
    openExternal((liveInfo && liveInfo.eula_url) || 'https://aka.ms/MinecraftEULA');
  });
  const modal = document.getElementById('live-modal');
  if (modal) closeOnBackdrop(modal, () => document.getElementById('live-cancel')?.click());
  document.getElementById('live-version-select')?.addEventListener('change', (event) => {
    if (event.target.value) localStorage.setItem(LIVE_VERSION_KEY, event.target.value);
    else localStorage.removeItem(LIVE_VERSION_KEY);
  });
  labelLiveViewDistances();
  // "Teleport here" in the map's right-click menu.
  window.addEventListener('message', (event) => {
    const mapFrame = document.querySelector('.map-container');
    if (!mapFrame || event.source !== mapFrame.contentWindow) return;
    if (!event.data || event.data.type !== 'liveTeleport' || !liveActive()) return;
    invoke('gui_live_teleport', { lat: event.data.lat, lng: normalizeLongitude(event.data.lng) })
      .catch((error) => console.error('Teleport failed:', error));
  });
  // The map shows new areas as they come in, a batch at a time.
  let overlayTiles = 0;
  let overlayAt = 0;
  window.__TAURI__.event.listen('live-status', (event) => {
    const wasActive = liveActive();
    const wasPhase = liveStatus && liveStatus.phase;
    liveStatus = event.payload;
    // A session may have saved the world with a newer version.
    if (wasActive && !liveActive()) liveInfo = null;
    onLiveStatus();
    const done = liveStatus.tiles_done || 0;
    const now = Date.now();
    // The world unlocks when a session ends and exists once it runs, so it is read again.
    if (wasActive !== liveActive() || (liveStatus.phase === 'running' && wasPhase !== 'running')) {
      overlayTiles = 0;
      overlayAt = now;
      oneWorldOverlayKey = null;
      refreshOneWorldState();
    } else if (done - overlayTiles >= 8 && now - overlayAt > 15000) {
      overlayTiles = done;
      overlayAt = now;
      oneWorldOverlayKey = null;
      refreshOneWorldState();
    }
  });
  invoke('gui_live_status').then((status) => {
    if (status) {
      liveStatus = status;
      onLiveStatus();
    }
  }).catch(() => {});
  refreshLiveRows();
  refreshStartButton();
}

function initOneWorld() {
  const group = document.getElementById('one-world-group');
  if (!group) return;
  // The settings store restores the value by clicking too.
  group.querySelectorAll('.segment').forEach((btn) => {
    btn.addEventListener('click', () => {
      if (btn.disabled || btn.classList.contains('active') || liveActive()) return;
      group.querySelectorAll('.segment').forEach((b) => b.classList.remove('active'));
      btn.classList.add('active');
      refreshOneWorldDesc();
      refreshOneWorldState();
      refreshGameplayRows();
      refreshWorldTypeRow();
      refreshWorldRulesRows();
      refreshFacadeRows();
    });
  });
  refreshOneWorldDesc();
  const mode = document.getElementById('generation-mode-select');
  if (mode) mode.addEventListener('change', renderOneWorldStatus);
  const nameToggle = document.getElementById('custom-world-name-toggle');
  if (nameToggle) nameToggle.addEventListener('change', () => { if (isOneWorldEnabled()) refreshOneWorldState(); });
  // Minecraft may have opened or closed the world meanwhile.
  window.addEventListener('focus', () => {
    if (isOneWorldEnabled() && generationButtonEnabled) refreshOneWorldState();
  });
  // A reloaded map iframe comes back without overlays.
  const mapFrame = getMapFrame();
  if (mapFrame) {
    mapFrame.addEventListener('load', () => {
      oneWorldOverlayKey = null;
      if (isOneWorldEnabled()) refreshOneWorldState();
    });
  }
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
// A Download & Bake is under way (Data Sources): Start waits for it.
let bakeRunning = false;

// Central setter so every place that toggles generation state also
// refreshes the world-name pencil (hidden/blocked while a generation, or a
// rename, is in flight - see canEditCustomWorldName()).
function setGenerationButtonEnabled(enabled) {
  generationButtonEnabled = enabled;
  if (!enabled) document.getElementById('run-confirm').hidden = true;
  showRunControls(!enabled);
  redrawProjects();
  refreshWorldNameEditUI();
  // The two jobs exclude each other in the backend, so the other one greys out.
  refreshPrecomputeButton();
}

/**
 * Initiates the world generation process
 * Validates required inputs and sends generation parameters to the backend
 * @returns {Promise<void>}
 */
// The generation settings as the backend commands name them. A One World that exists
// fixes its scale, height and ground level, and sends those instead.
function readGenerationSettings(oneWorld) {
  const segment = (group, key, fallback) => {
    const btn = document.querySelector(`#${group} .segment.active`);
    return btn ? btn.dataset[key] : fallback;
  };
  const checked = (id) => document.getElementById(id).checked;
  // Disabled means unsupported for this format or mode, so never send a stale tick.
  const live = (id) => checked(id) && !document.getElementById(id).disabled;
  const heightLimitToggle = document.getElementById("disable-height-limit-toggle");
  let disableHeightLimit = !heightLimitToggle.disabled && heightLimitToggle.checked;
  let scale = parseFloat(document.getElementById("scale-value-slider").value);
  let heightMultiplier = parseFloat(document.getElementById("height-multiplier-slider").value) || 1;
  if (oneWorld && oneWorldInfo && oneWorldInfo.exists) {
    if (typeof oneWorldInfo.scale === 'number') scale = oneWorldInfo.scale;
    if (typeof oneWorldInfo.height_multiplier === 'number') heightMultiplier = oneWorldInfo.height_multiplier;
    if (typeof oneWorldInfo.disable_height_limit === 'boolean') disableHeightLimit = oneWorldInfo.disable_height_limit;
  } else if (oneWorld) {
    disableHeightLimit = true;
  }
  // The ground level input is gone from the UI; a world the CLI made may sit on another level.
  let groundLevel = -62;
  if (oneWorld && oneWorldInfo && oneWorldInfo.exists && typeof oneWorldInfo.ground_level === 'number') {
    groundLevel = oneWorldInfo.ground_level;
  }
  // Clock minutes -> Minecraft ticks (tick 0 = 06:00; 24:00 wraps to 00:00)
  const clockMinutes = (parseInt(document.getElementById("world-time-slider").value, 10) || 0) % 1440;
  const worldTime = Math.round(((clockMinutes + 1440 - 360) % 1440) * (24000 / 1440));
  return {
    worldScale: scale,
    heightMultiplier: heightMultiplier,
    groundLevel: groundLevel,
    generationMode: document.getElementById("generation-mode-select").value,
    winterEnabled: checked("winter-toggle"),
    postApocalypticEnabled: checked("post-apocalyptic-toggle"),
    interiorEnabled: checked("interior-toggle"),
    fillgroundEnabled: checked("fillground-toggle"),
    cavesEnabled: checked("caves-toggle"),
    legacyTreesEnabled: checked("legacy-trees-toggle"),
    maxTreeSize: segment("max-tree-size-group", "maxTreeSize", "giant"),
    fieldPreset: segment("field-preset-group", "fieldPreset", "patchwork"),
    canopyHeightEnabled: checked("canopy-height-toggle"),
    overtureEnabled: checked("overture-toggle"),
    use3dEnabled: checked("use-3d-toggle"),
    disableHeightLimit: disableHeightLimit,
    awsOnlyElevation: checked("aws-only-elevation-toggle"),
    bakeLightingEnabled: live("bake-lighting-toggle"),
    voxyLodEnabled: live("voxy-lod-toggle"),
    dhLodEnabled: live("dh-lod-toggle"),
    gamemode: segment("gamemode-group", "gamemode", "creative"),
    worldTime: worldTime,
    worldType: segment("world-type-group", "worldType", "void"),
    mobSpawning: getMobSpawning(),
    mapItem: checked("map-item-toggle"),
    signage: segment("signage-group", "signage", "basic"),
    mapillaryToken: getMapillaryToken(),
    facadesEnabled: getFacadesEnabled(),
    facadeMode: getEffectiveFacadeMode(),
    buildingFacadesEnabled: getBuildingFacadesEnabled(),
    facadeDetail: getFacadeDetail()
  };
}

// A job in pieces runs for long and takes most of the machine: Start (and Expand) first show
// its plan and the warnings, and only a held button starts it.
function askRunConfirm() {
  const box = document.getElementById('run-confirm');
  box.hidden = false;
  box.scrollIntoView({ block: 'nearest' });
  document.getElementById('run-confirm-hold').focus();
}

// Calls `done` once the button has been held for HOLD_MS (pointer, or Space/Enter); letting go
// earlier cancels (arnisHold, piece-style.js). The fill is CSS (.run-hold.is-holding), timed to the same HOLD_MS.
const HOLD_MS = 1500;
function holdToConfirm(button, done) {
  globalThis.arnisHold(button, done, HOLD_MS);
}

// `confirmed`: the hold (or a retry) already said yes to a job in pieces.
// `prewarm`: Download Plan's "Download now", a run that only fills the caches.
// Returns whether a run started.
async function startGeneration(confirmed = false, prewarm = false) {
  // In Live the button starts and stops the session instead; a prewarm is never a Live session.
  if (!prewarm && liveActive()) {
    stopLive();
    return;
  }
  if (generationButtonEnabled === false) {
    return;
  }
  // The backend refuses a run during a bake only after a new world was made for it.
  if (bakeRunning) {
    const info = document.getElementById('progress-info');
    if (info) {
      info.textContent = oneWorldText('meld_baking', 'Baking: the progress is on the main bar.');
      info.style.color = "#fecc44";
    }
    return;
  }
  // One World and Live keep their areas as rectangles of whole chunks; Advanced features build
  // the rectangle around the shapes.
  if (selectedShapes && (isLiveMode() || isOneWorldEnabled()) && !meldOn()) {
    setBboxSelectionInfo(document.getElementById('bbox-selection-info'), "shapes_rectangles_only", "#fa7878");
    return;
  }
  if (!prewarm && isLiveMode()) {
    startLive();
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
  const cells = isPiecesJob();
  if (confirmed !== true && cells) {
    askRunConfirm();
    return;
  }
  // The hold names the cells built again; a start that is no job in cells gets main's question.
  const held = confirmed === true && cells && !planPieces()?.single;
  // Claim the guard before the first await. gui_create_world and gui_start_generation are
  // both awaited round-trips, so leaving the claim until after them lets a second click
  // through and starts a parallel run against the same process-global world floor.
  setGenerationButtonEnabled(false);
  let started = false;

  try {
    if (!selectedBBox || selectedBBox == "0.000000 0.000000 0.000000 0.000000") {
      const bboxSelectionInfo = document.getElementById('bbox-selection-info');
      setBboxSelectionInfo(bboxSelectionInfo, "select_location_first", "#fa7878");
      return;
    }

    // Past every synchronous refusal, so from here the click is a real start.
    resetProgressUi(STARTING_MESSAGE);
    if (!prewarm) clearPieces();

    const oneWorld = isOneWorldEnabled();
    const oneWorldBBox = selectedBBox;
    // The overlap dialog leaves the page live, so the body, format or area may have changed under it.
    if (oneWorld && (!(await prepareOneWorldRun(held || prewarm)) || !isOneWorldEnabled() || selectedBBox !== oneWorldBBox)) {
      const info = document.getElementById('progress-info');
      if (info && info.textContent === STARTING_MESSAGE) info.textContent = "";
      return;
    }
    lastRunOneWorld = oneWorld;
    lastRunPrewarm = prewarm;
    const place = oneWorld && !prewarm && document.getElementById('advanced-meld-features-toggle').checked
      ? placeForRun(searchedPlace, selectedBBox) : '';
    runPlace = place && oneWorldInfo && !oneWorldInfo.area_count ? { name: place, path: oneWorldInfo.world_path } : null;

    // Auto-create world for Java format (a One World is resolved by the backend)
    if (selectedWorldFormat === 'java' && !oneWorld && !prewarm) {
      if (!savePath) {
        console.warn("Cannot create world: save path not set");
        return;
      }
      try {
        const requestedName = pendingWorldName() || null;
        const worldName = await invoke('gui_create_world', { savePath: savePath, worldName: requestedName });
        if (worldName) {
          worldPath = worldName;
          // Used up: the world after this one gets the default name again
          // unless the pencil names it too.
          if (requestedName) customWorldName = "";
          setWorldNameLabel(basenameFromPath(worldName));
        }
      } catch (error) {
        handleWorldSelectionError(error);
        return;
      }
    }

    // Clear any existing world preview since we're generating a new one.
    // A One World keeps its areas on the map; the new one joins them at the end.
    if (!oneWorld && !prewarm) notifyWorldChanged();
    if (oneWorld) setWorldNameLabel(oneWorldFolderName());

    // Get the map iframe reference
    const mapFrame = document.querySelector('.map-container');
    // Get spawn point coordinates if marker exists
    let spawnPoint = null;
    if (mapFrame && mapFrame.contentWindow && mapFrame.contentWindow.getSpawnPointCoords) {
      const coords = mapFrame.contentWindow.getSpawnPointCoords();
      // Convert object format to tuple format if coordinates exist
      if (coords) {
        // The map repeats sideways, and the selection is brought back into range too.
        spawnPoint = [coords.lat, normalizeLongitude(coords.lng)];
      }
    }

    const settings = readGenerationSettings(oneWorld);

    // Get telemetry consent (defaults to false if not set)
    const telemetryConsent = window.getTelemetryConsent ? window.getTelemetryConsent() : false;

    // Get rotation angle
    var rotationAngle = oneWorld ? 0 : (parseFloat(document.getElementById("rotation-angle-input").value) || 0);

    const flags = meldFlags({
      oneWorld: oneWorld,
      java: getEffectiveWorldFormat() === "java",
      caves: settings.cavesEnabled,
      extendHeight: settings.disableHeightLimit && !oneWorld && getEffectiveWorldFormat() === "java",
      lods: settings.voxyLodEnabled || settings.dhLodEnabled,
      prewarm
    });
    // meldFlags skips the Advanced settings Hytale holds off, but Offline Mode still applies.
    if (!document.getElementById('advanced-meld-features-toggle').checked && offlineModeOn()) flags.push('--offline');
    // Held or retried as a job in cells: it runs as one, or not at all.
    if (cells && !prewarm && !flags.some((f) => f.startsWith('--one-world-workers='))) return;

    // Pass the selected options to the Rust backend
    await invoke("gui_start_generation", {
        ...settings,
        bboxText: selectedBBox,
        ...generationTarget({ prewarm, oneWorld, savePath, worldPath }),
        bedrockSavePath: bedrockSavePath,
        luantiSavePath: luantiSavePath,
        hytaleSavePath: hytaleSavePath,
        spawnPoint: spawnPoint,
        telemetryConsent: telemetryConsent || false,
        worldFormat: getEffectiveWorldFormat(),
        rotationAngle: rotationAngle,
        celestialBodyName: selectedCelestialBody,
        pauseMenu: gameplayToggle('pause-menu-toggle'),
        discoveries: gameplayToggle('discoveries-toggle'),
        trafficLights: gameplayToggle('traffic-lights-toggle'),
        movingVehicles: gameplayToggle('moving-cars-toggle'),
        pedestrians: gameplayToggle('pedestrians-toggle'),
        zombieOutbreak: gameplayToggle('zombie-outbreak-toggle'),
        disasters: gameplayToggle('disasters-toggle'),
        bodyGravity: gameplayToggle('body-gravity-toggle'),
        oneWorld: oneWorld,
        oneWorldName: oneWorld ? oneWorldFolderName() : "",
        flags: [...flags, ...(prewarm ? ["--prewarm"] : [])],
        selectionShapes: generationShapes({ oneWorld, shapes: selectedShapes })
    });

    console.log("Generation process started.");
    setEtaSignageExpected(settings.signage !== "none" && getEffectiveWorldFormat() === "java");
    resetEta();
    started = true;
    window.arnisPreview3D?.setGenerationRunning(true);
  } catch (error) {
    console.error("Error starting generation:", error);
  } finally {
    // Hand the guard back unless a run actually started; once it has, the Done!/Error!
    // progress message releases it instead.
    if (!started) {
      // Some abort paths say why (handleWorldSelectionError writes here); the
      // rest would leave our placeholder claiming a run that never began, so
      // clear it only if nothing else has taken the line since.
      const info = document.getElementById('progress-info');
      if (info && info.textContent === STARTING_MESSAGE) info.textContent = "";
      setGenerationButtonEnabled(true);
      window.arnisPreview3D?.setGenerationRunning(false);
    }
    refreshPrecomputeButton();
  }
  return started;
}

// World preview overlay state
let worldPreviewEnabled = false;
let currentWorldMapData = null;

/**
 * Notifies the map iframe that world preview data is ready
 * Called when the backend emits the map-preview-ready event
 */
async function showWorldPreviewButton() {
  // A One World shows every area at once, reloaded when the run reports Done.
  if (lastRunOneWorld) return;
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
  resetPieces();
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
