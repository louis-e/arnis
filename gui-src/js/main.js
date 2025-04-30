import { licenseText } from './license.js';

let invoke;
if (window.__TAURI__) {
  invoke = window.__TAURI__.core.invoke;
} else {
  function dummyFunc() {}
  window.__TAURI__ = { event: { listen: dummyFunc } };
  invoke = dummyFunc;
}

const DEFAULT_LOCALE_PATH = `./locales/en.json`;

// Initialize elements and start the demo progress
window.addEventListener("DOMContentLoaded", async () => {
  registerMessageEvent();
  window.selectWorld = selectWorld;
  window.startGeneration = startGeneration;
  setupProgressListener();
  initSettings();
  initWorldPicker();
  handleBboxInput();
  const localization = await getLocalization();
  await applyLocalization(localization);
  initFooter();
  await checkForUpdates();
});

/**
 * Checks if a JSON response is invalid or falls back to HTML
 * @param {Response} response - The fetch response object
 * @returns {boolean} True if the response is invalid JSON
 */
function invalidJSON(response) {
  // Workaround for Tauri always falling back to index.html for asset loading
  return !response.ok || response.headers.get("Content-Type") === "text/html";
}

/**
 * Fetches and returns localization data based on user's language
 * Falls back to English if requested language is not available
 * @returns {Promise<Object>} The localization JSON object
 */
async function getLocalization() {
  const lang = navigator.language;
  let response = await fetch(`./locales/${lang}.json`);

  // Try with only first part of language code
  if (invalidJSON(response)) {
    response = await fetch(`./locales/${lang.split('-')[0]}.json`);

    // Fallback to default English localization
    if (invalidJSON(response)) {
      response = await fetch(DEFAULT_LOCALE_PATH);
    }
  }

  const localization = await response.json();
  return localization;
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
    if (localizedStringKey in json) {
      element[attribute] = json[localizedStringKey];
    } else {
      // Fallback to default (English) string
      const response = await fetch(DEFAULT_LOCALE_PATH);
      const defaultJson = await response.json();
      element[attribute] = defaultJson[localizedStringKey];
    }
  }
}

async function applyLocalization(localization) {
  const localizationElements = {
    "h2[data-localize='select_location']": "select_location",
    "#bbox-text": "zoom_in_and_choose",
    "h2[data-localize='select_world']": "select_world",
    "span[id='choose_world']": "choose_world",
    "#selected-world": "no_world_selected",
    "#start-button": "start_generation",
    "h2[data-localize='progress']": "progress",
    "h2[data-localize='choose_world_modal_title']": "choose_world_modal_title",
    "button[data-localize='select_existing_world']": "select_existing_world",
    "button[data-localize='generate_new_world']": "generate_new_world",
    "h2[data-localize='customization_settings']": "customization_settings",
    "label[data-localize='world_scale']": "world_scale",
    "label[data-localize='custom_bounding_box']": "custom_bounding_box",
    "label[data-localize='floodfill_timeout']": "floodfill_timeout",
    "label[data-localize='ground_level']": "ground_level",
    ".footer-link": "footer_text",
    "button[data-localize='license_and_credits']": "license_and_credits",
    "h2[data-localize='license_and_credits']": "license_and_credits",

    // Placeholder strings
    "input[id='bbox-coords']": "placeholder_bbox",
    "input[id='floodfill-timeout']": "placeholder_floodfill",
    "input[id='ground-level']": "placeholder_ground"
  };

  for (const selector in localizationElements) {
    localizeElement(localization, { selector: selector }, localizationElements[selector]);
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
    footerElement.textContent =
      footerElement.textContent
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
      footer.style.marginTop = "15px";
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
  });
}

// Function to set up the progress bar listener
function setupProgressListener() {
  const progressBar = document.getElementById("progress-bar");
  const progressMessage = document.getElementById("progress-message");
  const progressDetail = document.getElementById("progress-detail");

  window.__TAURI__.event.listen("progress-update", (event) => {
    const { progress, message } = event.payload;

    if (progress != -1) {
      progressBar.style.width = `${progress}%`;
      progressDetail.textContent = `${Math.round(progress)}%`;
    }

    if (message != "") {
      progressMessage.textContent = message;

      if (message.startsWith("Error!")) {
        progressMessage.style.color = "#fa7878";
        generationButtonEnabled = true;
      } else if (message.startsWith("Done!")) {
        progressMessage.style.color = "#7bd864";
        generationButtonEnabled = true;
      } else {
        progressMessage.style.color = "";
      }
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
  
  window.openSettings = openSettings;
  window.closeSettings = closeSettings;

  // Update slider value display
  slider.addEventListener("input", () => {
    sliderValue.textContent = parseFloat(slider.value).toFixed(2);
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

function initWorldPicker() {
  // World Picker
  const worldPickerModal = document.getElementById("world-modal");
  
  // Open world picker modal
  function openWorldPicker() {
    worldPickerModal.style.display = "flex";
    worldPickerModal.style.justifyContent = "center";
    worldPickerModal.style.alignItems = "center";
  }

  // Close world picker modal
  function closeWorldPicker() {
    worldPickerModal.style.display = "none";
  }
  
  window.openWorldPicker = openWorldPicker;
  window.closeWorldPicker = closeWorldPicker;
}

/**
 * Validates and processes bounding box coordinates input
 * Supports both comma and space-separated formats
 * Updates the map display when valid coordinates are entered
 */
function handleBboxInput() {
  const inputBox = document.getElementById("bbox-coords");
  const bboxInfo = document.getElementById("bbox-info");

  inputBox.addEventListener("input", function () {
      const input = inputBox.value.trim();

      if (input === "") {
          bboxInfo.textContent = "";
          bboxInfo.style.color = "";
          selectedBBox = "";
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

              // Update the info text
              localizeElement(window.localization, { element: bboxInfo }, "custom_selection_confirmed");
              bboxInfo.style.color = "#7bd864";
          } else {
              // Valid numbers but invalid order or range
              localizeElement(window.localization, { element: bboxInfo }, "error_coordinates_out_of_range");
              bboxInfo.style.color = "#fecc44";
              selectedBBox = "";
          }
      } else {
          // Input doesn't match the required format
          localizeElement(window.localization, { element: bboxInfo }, "invalid_format");
          bboxInfo.style.color = "#fecc44";
          selectedBBox = "";
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

const threshold1 = 30000000.00;
const threshold2 = 45000000.00;
let selectedBBox = "";

// Function to handle incoming bbox data
function displayBboxInfoText(bboxText) {
  let [lng1, lat1, lng2, lat2] = bboxText.split(" ").map(Number);

  // Normalize longitudes
  lat1 = parseFloat(normalizeLongitude(lat1).toFixed(6));
  lat2 = parseFloat(normalizeLongitude(lat2).toFixed(6));
  selectedBBox = `${lng1} ${lat1} ${lng2} ${lat2}`;

  const bboxInfo = document.getElementById("bbox-info");

  // Reset the info text if the bbox is 0,0,0,0
  if (lng1 === 0 && lat1 === 0 && lng2 === 0 && lat2 === 0) {
    bboxInfo.textContent = "";
    selectedBBox = "";
    return;
  }

  // Calculate the size of the selected bbox
  const selectedSize = calculateBBoxSize(lng1, lat1, lng2, lat2);

  if (selectedSize > threshold2) {
    localizeElement(window.localization, { element: bboxInfo }, "area_too_large");
    bboxInfo.style.color = "#fa7878";
  } else if (selectedSize > threshold1) {
    localizeElement(window.localization, { element: bboxInfo }, "area_extensive");
    bboxInfo.style.color = "#fecc44";
  } else {
    localizeElement(window.localization, { element: bboxInfo }, "selection_confirmed");
    bboxInfo.style.color = "#7bd864";
  }
}

let worldPath = "";
let isNewWorld = false;

async function selectWorld(generate_new_world) {
  try {
    const worldName = await invoke('gui_select_world', { generateNew: generate_new_world } );
    if (worldName) {
      worldPath = worldName;
      isNewWorld = generate_new_world;
      const lastSegment = worldName.split(/[\\/]/).pop();
      document.getElementById('selected-world').textContent = lastSegment;
      document.getElementById('selected-world').style.color = "#fecc44";
    }
  } catch (error) {
    handleWorldSelectionError(error);
  }

  closeWorldPicker();
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
  const selectedWorld = document.getElementById('selected-world');
  localizeElement(window.localization, { element: selectedWorld }, errorKey);
  selectedWorld.style.color = "#fa7878";
  worldPath = "";
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
      const bboxInfo = document.getElementById('bbox-info');
      localizeElement(window.localization, { element: bboxInfo }, "select_location_first");
      bboxInfo.style.color = "#fa7878";
      return;
    }

    if (!worldPath || worldPath === "") {
      const selectedWorld = document.getElementById('selected-world');
      localizeElement(window.localization, { element: selectedWorld }, "select_minecraft_world_first");
      selectedWorld.style.color = "#fa7878";
      return;
    }

    var terrain = document.getElementById("terrain-toggle").checked;
    var fill_ground = document.getElementById("fillground-toggle").checked;
    var scale = parseFloat(document.getElementById("scale-value-slider").value);
    var floodfill_timeout = parseInt(document.getElementById("floodfill-timeout").value, 10);
    var ground_level = parseInt(document.getElementById("ground-level").value, 10);

    // Validate floodfill_timeout and ground_level
    floodfill_timeout = isNaN(floodfill_timeout) || floodfill_timeout < 0 ? 20 : floodfill_timeout;
    ground_level = isNaN(ground_level) || ground_level < -62 ? 20 : ground_level;

    // Pass the bounding box and selected world to the Rust backend
    await invoke("gui_start_generation", {
        bboxText: selectedBBox,
        selectedWorld: worldPath,
        worldScale: scale,
        groundLevel: ground_level,
        floodfillTimeout: floodfill_timeout,
        terrainEnabled: terrain,
        fillgroundEnabled: fill_ground,
        isNewWorld: isNewWorld
    });

    console.log("Generation process started.");
    generationButtonEnabled = false;
  } catch (error) {
    console.error("Error starting generation:", error);
    generationButtonEnabled = true;
  }
}
