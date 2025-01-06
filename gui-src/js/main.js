const { invoke } = window.__TAURI__.core;

// Initialize elements and start the demo progress
window.addEventListener("DOMContentLoaded", async () => {
  registerMessageEvent();
  window.selectWorld = selectWorld;
  window.startGeneration = startGeneration;
  setupProgressListener();
  initSettings();
  initWorldPicker();
  handleBboxInput();
  const language = detectLanguage();
  const localization = await loadLocalization(language);
  await applyLocalization(localization);
  initFooter();
  await checkForUpdates();
});

async function loadLocalization(language) {
  const response = await fetch(`./locales/${language}.json`);
  const localization = await response.json();
  return localization;
}

async function applyLocalization(localization) {
  const selectLocationElement = document.querySelector("h2[data-localize='select_location']");
  if (selectLocationElement) {
    selectLocationElement.textContent = localization.select_location;
  }

  const bboxTextElement = document.getElementById("bbox-text");
  if (bboxTextElement) {
    bboxTextElement.textContent = localization.zoom_in_and_choose;
  }

  const selectWorldElement = document.querySelector("h2[data-localize='select_world']");
  if (selectWorldElement) {
    selectWorldElement.textContent = localization.select_world;
  }

  const chooseWorldButton = document.querySelector("button[data-localize='choose_world']");
  if (chooseWorldButton) {
    chooseWorldButton.firstChild.textContent = localization.choose_world;
  }

  const selectedWorldElement = document.getElementById("selected-world");
  if (selectedWorldElement) {
    selectedWorldElement.textContent = localization.no_world_selected;
  }

  const startButtonElement = document.getElementById("start-button");
  if (startButtonElement) {
    startButtonElement.textContent = localization.start_generation;
  }

  const progressElement = document.querySelector("h2[data-localize='progress']");
  if (progressElement) {
    progressElement.textContent = localization.progress;
  }

  const chooseWorldModalTitle = document.querySelector("h2[data-localize='choose_world_modal_title']");
  if (chooseWorldModalTitle) {
    chooseWorldModalTitle.textContent = localization.choose_world_modal_title;
  }

  const selectExistingWorldButton = document.querySelector("button[data-localize='select_existing_world']");
  if (selectExistingWorldButton) {
    selectExistingWorldButton.textContent = localization.select_existing_world;
  }

  const generateNewWorldButton = document.querySelector("button[data-localize='generate_new_world']");
  if (generateNewWorldButton) {
    generateNewWorldButton.textContent = localization.generate_new_world;
  }

  const customizationSettingsTitle = document.querySelector("h2[data-localize='customization_settings']");
  if (customizationSettingsTitle) {
    customizationSettingsTitle.textContent = localization.customization_settings;
  }

  const winterModeLabel = document.querySelector("label[data-localize='winter_mode']");
  if (winterModeLabel) {
    winterModeLabel.textContent = localization.winter_mode;
  }

  const worldScaleLabel = document.querySelector("label[data-localize='world_scale']");
  if (worldScaleLabel) {
    worldScaleLabel.textContent = localization.world_scale;
  }

  const customBoundingBoxLabel = document.querySelector("label[data-localize='custom_bounding_box']");
  if (customBoundingBoxLabel) {
    customBoundingBoxLabel.textContent = localization.custom_bounding_box;
  }

  const floodfillTimeoutLabel = document.querySelector("label[data-localize='floodfill_timeout']");
  if (floodfillTimeoutLabel) {
    floodfillTimeoutLabel.textContent = localization.floodfill_timeout;
  }

  const groundLevelLabel = document.querySelector("label[data-localize='ground_level']");
  if (groundLevelLabel) {
    groundLevelLabel.textContent = localization.ground_level;
  }

  const footerLinkElement = document.querySelector(".footer-link");
  if (footerLinkElement) {
    footerLinkElement.innerHTML = localization.footer_text.replace("{year}", '<span id="current-year"></span>').replace("{version}", '<span id="version-placeholder"></span>');
  }

  // Update error messages
  window.localization = localization;
}

function detectLanguage() {
  const lang = navigator.language || navigator.userLanguage;
  const langCode = lang.split('-')[0];
  switch (langCode) {
    case 'es':
      return 'es';
    case 'ru':
      return 'ru';
    case 'de':
      return 'de';
    case 'zh':
      return 'zh';
    case 'uk':
      return 'ua';
    case 'pl':
      return 'pl';
    default:
      return 'en';
  }
}

// Function to initialize the footer with the current year and version
async function initFooter() {
  const currentYear = new Date().getFullYear();
  const currentYearElement = document.getElementById("current-year");
  if (currentYearElement) {
    currentYearElement.textContent = currentYear;
  }

  try {
    const version = await invoke('gui_get_version');
    const versionPlaceholder = document.getElementById("version-placeholder");
    if (versionPlaceholder) {
      versionPlaceholder.textContent = version;
    }
  } catch (error) {
    console.error("Failed to fetch version:", error);
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

      updateMessage.textContent = window.localization.new_version_available;
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

// Function to validate and handle bbox input
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
              // Input is valid; trigger the event
              const bboxText = `${lat1} ${lng1} ${lat2} ${lng2}`;
              window.dispatchEvent(new MessageEvent('message', { data: { bboxText } }));

              // Update the info text
              bboxInfo.textContent = window.localization.custom_selection_confirmed;
              bboxInfo.style.color = "#7bd864";
          } else {
              // Valid numbers but invalid order or range
              bboxInfo.textContent = window.localization.error_coordinates_out_of_range;
              bboxInfo.style.color = "#fecc44";
              selectedBBox = "";
          }
      } else {
          // Input doesn't match the required format
          bboxInfo.textContent = window.localization.invalid_format;
          bboxInfo.style.color = "#fecc44";
          selectedBBox = "";
      }
  });
}

// Function to calculate the bounding box "size" in square meters based on latitude and longitude
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

// Function to normalize longitude to the range [-180, 180]
function normalizeLongitude(lon) {
  return ((lon + 180) % 360 + 360) % 360 - 180;
}

const threshold1 = 35000000.00;
const threshold2 = 50000000.00;
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
    bboxInfo.textContent = window.localization.area_too_large;
    bboxInfo.style.color = "#fa7878";
  } else if (selectedSize > threshold1) {
    bboxInfo.textContent = window.localization.area_extensive;
    bboxInfo.style.color = "#fecc44";
  } else {
    bboxInfo.textContent = window.localization.selection_confirmed;
    bboxInfo.style.color = "#7bd864";
  }
}

let worldPath = "";
async function selectWorld(generate_new_world) {
  try {
    const worldName = await invoke('gui_select_world', { generateNew: generate_new_world } );
    if (worldName) {
      worldPath = worldName;
      const lastSegment = worldName.split(/[\\/]/).pop();
      document.getElementById('selected-world').textContent = lastSegment;
      document.getElementById('selected-world').style.color = "#fecc44";
    }
  } catch (error) {
    handleWorldSelectionError(error);
  }

  closeWorldPicker();
}

function handleWorldSelectionError(errorCode) {
  const errorMessages = {
    1: window.localization.minecraft_directory_not_found,
    2: window.localization.world_in_use,
    3: window.localization.failed_to_create_world,
    4: window.localization.no_world_selected_error
  };

  const errorMessage = errorMessages[errorCode] || "Unknown error";
  document.getElementById('selected-world').textContent = errorMessage;
  document.getElementById('selected-world').style.color = "#fa7878";
  worldPath = "";
  console.error(error);
}

let generationButtonEnabled = true;
async function startGeneration() {
  try {
    if (generationButtonEnabled === false) {
      return;
    }

    if (!selectedBBox || selectedBBox == "0.000000 0.000000 0.000000 0.000000") {
      document.getElementById('bbox-info').textContent = window.localization.select_location_first;
      document.getElementById('bbox-info').style.color = "#fa7878";
      return;
    }

    if (!worldPath || worldPath === "") {
      document.getElementById('selected-world').textContent = window.localization.select_minecraft_world_first;
      document.getElementById('selected-world').style.color = "#fa7878";
      return;
    }

    var winter_mode = document.getElementById("winter-toggle").checked;
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
        winterMode: winter_mode,
        floodfillTimeout: floodfill_timeout,
    });

    console.log("Generation process started.");
    generationButtonEnabled = false;
  } catch (error) {
    console.error("Error starting generation:", error);
    generationButtonEnabled = true;
  }
}
