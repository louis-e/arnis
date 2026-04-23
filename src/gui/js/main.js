import { licenseText } from './license.js';
import { fetchLanguage, invalidJSON } from './language.js';

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
  initTelemetryConsent();
  initClearCacheButton();
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
    "span[data-localize='roof']": "roof",
    "span[data-localize='fillground']": "fillground",
    "span[data-localize='land_cover']": "land_cover",
    "span[data-localize='disable_height_limit']": "disable_height_limit",
    "span[data-localize='anonymous_crash_reports']": "anonymous_crash_reports",
    "span[data-localize='map_theme']": "map_theme",
    "span[data-localize='save_path']": "save_path",
    "span[data-localize='rotation_angle']": "rotation_angle",
    "div[data-localize='settings_section_generation']": "settings_section_generation",
    "div[data-localize='settings_section_world']": "settings_section_world",
    "div[data-localize='settings_section_map']": "settings_section_map",
    "div[data-localize='settings_section_application']": "settings_section_application",
    "span[data-localize='clear_tile_cache']": "clear_tile_cache",
    "button[data-localize='clear_tile_cache_button']": "clear_tile_cache_button",
    ".footer-link": "footer_text",
    "button[data-localize='license_and_credits']": "license_and_credits",
    "h2[data-localize='license_and_credits']": "license_and_credits",

    // Placeholder strings
    "input[id='bbox-coords']": "placeholder_bbox",
    // DEPRECATED: Ground level placeholder removed
    // "input[id='ground-level']": "placeholder_ground"
  };

  for (const selector in localizationElements) {
    localizeElement(localization, { selector: selector }, localizationElements[selector]);
  }

  // Re-apply current bbox selection info text with new language
  const bboxSelectionInfo = document.getElementById("bbox-selection-info");
  if (bboxSelectionInfo && currentBboxSelectionKey) {
    localizeElement(localization, { element: bboxSelectionInfo }, currentBboxSelectionKey);
    bboxSelectionInfo.style.color = currentBboxSelectionColor;
  }

  // Update error messages
  window.localization = localization;
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

// Function to check for updates and display a notification if available
async function checkForUpdates() {
  try {
    const isUpdateAvailable = await invoke('gui_check_for_updates');
    if (isUpdateAvailable) {
      const footer = document.querySelector(".footer");
      const updateMessage = document.createElement("a");
      updateMessage.href = "https://github.com/louis-e/arnis/releases";
      updateMessage.target = "_blank";
      updateMessage.style.color = "#fecc44";
      updateMessage.style.marginTop = "-5px";
      updateMessage.style.fontSize = "0.95em";
      updateMessage.style.display = "block";
      updateMessage.style.textDecoration = "none";

      localizeElement(window.localization, { element: updateMessage }, "new_version_available");
      footer.style.marginTop = "10px";
      footer.appendChild(updateMessage);
    }
  } catch (error) {
    console.error("Failed to check for updates: ", error);
  }
}

// Function to register the event listener for bbox updates from iframe
function registerMessageEvent() {
  window.addEventListener('message', function (event) {
    const bboxText = event.data.bboxText;

    if (bboxText) {
      console.log("Updated BBOX Coordinates:", bboxText);
      displayBboxInfoText(bboxText);
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

// Function to set up the progress bar listener
function setupProgressListener() {
  const progressBar = document.getElementById("progress-bar");
  const progressInfo = document.getElementById("progress-info");
  const progressDetail = document.getElementById("progress-detail");

  window.__TAURI__.event.listen("progress-update", (event) => {
    const { progress, message } = event.payload;

    if (progress != -1) {
      progressBar.style.width = `${progress}%`;
      progressDetail.textContent = `${Math.round(progress)}%`;
    }

    if (message != "") {
      progressInfo.textContent = message;

      if (message.startsWith("Error!")) {
        progressInfo.style.color = "#fa7878";
        generationButtonEnabled = true;
        setWorldNameLabel("");
      } else if (message.startsWith("Done!")) {
        progressInfo.style.color = "#7bd864";
        generationButtonEnabled = true;
      } else {
        progressInfo.style.color = "#ececec";
      }
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
  }

  // Close settings modal
  function closeSettings() {
    settingsModal.style.display = "none";
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
    }
  });

  window.openSettings = openSettings;
  window.closeSettings = closeSettings;

  // Update slider value display
  slider.addEventListener("input", () => {
    sliderValue.textContent = parseFloat(slider.value).toFixed(2);
  });
  // Double-click to reset world scale to default (1.00)
  slider.addEventListener("dblclick", () => {
    slider.value = 1;
    sliderValue.textContent = "1.00";
  });

  // Rotation angle input
  const rotationInput = document.getElementById("rotation-angle-input");

  function updateRotation(val) {
    if (isNaN(val)) val = 0;
    val = Math.min(Math.max(val, -90), 90);
    rotationInput.value = val.toFixed(2);
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

  // World format toggle (Java/Bedrock)
  initWorldFormatToggle();

  // Save path setting
  initSavePathSetting();

  // Language selector
  const languageSelect = document.getElementById("language-select");
  const availableOptions = Array.from(languageSelect.options).map(opt => opt.value);
  
  // Check for saved language preference first
  const savedLanguage = localStorage.getItem('arnis-language');
  let languageToSet = 'en'; // Default to English
  
  if (savedLanguage && availableOptions.includes(savedLanguage)) {
    // Use saved language if it exists and is available
    languageToSet = savedLanguage;
  } else {
    // Otherwise use browser language
    const currentLang = navigator.language;
    
    // Try to match the exact language code first
    if (availableOptions.includes(currentLang)) {
      languageToSet = currentLang;
    }
    // Try to match just the base language code
    else if (availableOptions.includes(currentLang.split('-')[0])) {
      languageToSet = currentLang.split('-')[0];
    }
    // languageToSet remains 'en' as default
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

    // Send message to map iframe to change tile theme
    const mapIframe = document.querySelector('iframe[src="maps.html"]');
    if (mapIframe && mapIframe.contentWindow) {
      mapIframe.contentWindow.postMessage({
        type: 'changeTileTheme',
        theme: selectedTheme
      }, '*');
    }
  });

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
  function openLicense() {
    const licenseModal = document.getElementById("license-modal");
    const licenseContent = document.getElementById("license-content");

    // Render the license text as HTML
    licenseContent.innerHTML = licenseText;

    // Show the modal
    licenseModal.style.display = "flex";
    licenseModal.style.justifyContent = "center";
    licenseModal.style.alignItems = "center";
  }

  function closeLicense() {
    const licenseModal = document.getElementById("license-modal");
    licenseModal.style.display = "none";
  }

  window.openLicense = openLicense;
  window.closeLicense = closeLicense;
}

// World format selection (Java/Bedrock)
let selectedWorldFormat = 'java'; // Default to Java

function initWorldFormatToggle() {
  // Load saved format preference
  const savedFormat = localStorage.getItem('arnis-world-format');
  if (savedFormat && (savedFormat === 'java' || savedFormat === 'bedrock')) {
    selectedWorldFormat = savedFormat;
  }
  
  // Apply the saved selection to UI
  updateFormatToggleUI(selectedWorldFormat);
}

function setWorldFormat(format) {
  if (format !== 'java' && format !== 'bedrock') return;
  
  selectedWorldFormat = format;
  localStorage.setItem('arnis-world-format', format);
  updateFormatToggleUI(format);
}

function updateFormatToggleUI(format) {
  const javaBtn = document.getElementById('format-java');
  const bedrockBtn = document.getElementById('format-bedrock');

  const heightLimitToggle = document.getElementById('disable-height-limit-toggle');

  // Toggle now supported on both formats (Java datapack + Bedrock BP).
  if (heightLimitToggle) {
    heightLimitToggle.disabled = false;
    heightLimitToggle.parentElement.closest('.settings-row').style.opacity = '1';
  }

  if (format === 'java') {
    javaBtn.classList.add('format-active');
    bedrockBtn.classList.remove('format-active');
  } else {
    javaBtn.classList.remove('format-active');
    bedrockBtn.classList.add('format-active');
    // Clear world path for bedrock (auto-generated)
    worldPath = "";
  }
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
  };

  window.rejectTelemetry = () => {
    localStorage.setItem(key, 'false');
    modal.style.display = 'none';
    // Update settings toggle to reflect the consent
    const telemetryToggle = document.getElementById('telemetry-toggle');
    if (telemetryToggle) {
      telemetryToggle.checked = false;
    }
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
function initClearCacheButton() {
  const button = document.getElementById('clear-cache-button');
  if (!button) {
    return;
  }

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

/// Save path management
let savePath = "";

async function initSavePath() {
  // Check if user has a saved path in localStorage
  const saved = localStorage.getItem('arnis-save-path');
  if (saved) {
    // Validate the saved path still exists (handles upgrades / moved directories)
    try {
      const normalized = await invoke('gui_set_save_path', { path: saved });
      savePath = normalized;
      localStorage.setItem('arnis-save-path', savePath);
    } catch (_) {
      // Saved path is no longer valid – re-detect
      console.warn("Stored save path no longer valid, re-detecting...");
      localStorage.removeItem('arnis-save-path');
      try {
        savePath = await invoke('gui_get_default_save_path');
        localStorage.setItem('arnis-save-path', savePath);
      } catch (error) {
        console.error("Failed to detect save path:", error);
      }
    }
  } else {
    // Auto-detect on first run
    try {
      savePath = await invoke('gui_get_default_save_path');
      localStorage.setItem('arnis-save-path', savePath);
    } catch (error) {
      console.error("Failed to detect save path:", error);
    }
  }

  // Populate the save path input in settings
  const savePathInput = document.getElementById('save-path-input');
  if (savePathInput) {
    savePathInput.value = savePath;
  }
}

function initSavePathSetting() {
  const savePathInput = document.getElementById('save-path-input');
  if (!savePathInput) return;

  savePathInput.value = savePath;

  // Manual text input – validate on change, revert if invalid
  savePathInput.addEventListener('change', async () => {
    const newPath = savePathInput.value.trim();
    if (!newPath) {
      savePathInput.value = savePath;
      return;
    }

    try {
      const validated = await invoke('gui_set_save_path', { path: newPath });
      savePath = validated;
      localStorage.setItem('arnis-save-path', savePath);
    } catch (_) {
      // Invalid path – silently revert to previous value
      savePathInput.value = savePath;
    }
  });

  // Folder picker button
  const browseBtn = document.getElementById('save-path-browse');
  if (browseBtn) {
    browseBtn.addEventListener('click', async () => {
      try {
        const picked = await invoke('gui_pick_save_directory', { startPath: savePath });
        if (picked) {
          savePath = picked;
          savePathInput.value = savePath;
          localStorage.setItem('arnis-save-path', savePath);
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
        const [lng1, lat1, lng2, lat2] = mapSelectedBBox.split(" ").map(Number);
        const selectedSize = calculateBBoxSize(lng1, lat1, lng2, lat2);
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

        // Show custom bbox on the map
        let map_container = document.querySelector('.map-container');
        map_container.setAttribute('src', `maps.html#${lat1},${lng1},${lat2},${lng2}`);
        map_container.contentWindow.location.reload();

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
  });
}

/**
 * Calculates the approximate area of a bounding box in square meters
 * Uses the Haversine formula for geodesic calculations
 * @param {number} lng1 - First longitude coordinate
 * @param {number} lat1 - First latitude coordinate
 * @param {number} lng2 - Second longitude coordinate
 * @param {number} lat2 - Second latitude coordinate
 * @returns {number} Area in square meters
 */
function calculateBBoxSize(lng1, lat1, lng2, lat2) {
  // Approximate distance calculation using Haversine formula or geodesic formula
  const toRad = (angle) => (angle * Math.PI) / 180;
  const R = 6371000; // Earth radius in meters

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

const threshold1 = 44000000.00;  // Yellow warning threshold (~6.2km x 7km)
const threshold2 = 85000000.00;  // Red error threshold (~8.7km x 9.8km)
const threshold3 = 500000000.00; // Extreme warning threshold (500 km²)
let selectedBBox = "";
let mapSelectedBBox = "";  // Tracks bbox from map selection
let customBBoxValid = false;  // Tracks if custom input is valid

/**
 * Displays the appropriate bbox size status message based on area thresholds
 * @param {HTMLElement} bboxSelectionElement - The element to display the message in
 * @param {number} selectedSize - The calculated bbox area in square meters
 */
function displayBboxSizeStatus(bboxSelectionElement, selectedSize) {
  if (selectedSize > threshold3) {
    setBboxSelectionInfo(bboxSelectionElement, "area_extreme", "#ff4444");
  } else if (selectedSize > threshold2) {
    setBboxSelectionInfo(bboxSelectionElement, "area_too_large", "#fa7878");
  } else if (selectedSize > threshold1) {
    setBboxSelectionInfo(bboxSelectionElement, "area_extensive", "#fecc44");
  } else {
    setBboxSelectionInfo(bboxSelectionElement, "selection_confirmed", "#7bd864");
  }
}

// Function to handle incoming bbox data
function displayBboxInfoText(bboxText) {
  let [lng1, lat1, lng2, lat2] = bboxText.split(" ").map(Number);

  // Normalize longitudes
  lat1 = parseFloat(normalizeLongitude(lat1).toFixed(6));
  lat2 = parseFloat(normalizeLongitude(lat2).toFixed(6));
  mapSelectedBBox = `${lng1} ${lat1} ${lng2} ${lat2}`;

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
  if (lng1 === 0 && lat1 === 0 && lng2 === 0 && lat2 === 0) {
    setBboxSelectionInfo(bboxSelectionInfo, "select_area_prompt", "#ffffff");
    bboxCoordsInput.value = "";
    mapSelectedBBox = "";
    if (!customBBoxValid) {
      selectedBBox = "";
    }
    return;
  }

  // Update the custom bbox input with the map selection (comma-separated format)
  bboxCoordsInput.value = `${lng1},${lat1},${lng2},${lat2}`;

  // Calculate the size of the selected bbox
  const selectedSize = calculateBBoxSize(lng1, lat1, lng2, lat2);

  displayBboxSizeStatus(bboxSelectionInfo, selectedSize);
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
  try {
    if (generationButtonEnabled === false) {
      return;
    }

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
    var roof = document.getElementById("roof-toggle").checked;
    var fill_ground = document.getElementById("fillground-toggle").checked;
    var land_cover = document.getElementById("land-cover-toggle").checked;
    var disable_height_limit = document.getElementById("disable-height-limit-toggle").checked;
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

    // Pass the selected options to the Rust backend
    await invoke("gui_start_generation", {
        bboxText: selectedBBox,
        selectedWorld: worldPath,
        worldScale: scale,
        groundLevel: ground_level,
        terrainEnabled: terrain,
        skipOsmObjects: skipOsmObjects,
        interiorEnabled: interior,
        roofEnabled: roof,
        fillgroundEnabled: fill_ground,
        landCoverEnabled: land_cover,
        disableHeightLimit: disable_height_limit,
        isNewWorld: true,
        spawnPoint: spawnPoint,
        telemetryConsent: telemetryConsent || false,
        worldFormat: selectedWorldFormat,
        rotationAngle: rotationAngle
    });

    console.log("Generation process started.");
    generationButtonEnabled = false;
  } catch (error) {
    console.error("Error starting generation:", error);
    generationButtonEnabled = true;
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
  if (!worldPath) return;
  
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
