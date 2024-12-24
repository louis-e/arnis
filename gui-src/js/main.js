const { invoke } = window.__TAURI__.core;

// Initialize elements and start the demo progress
window.addEventListener("DOMContentLoaded", async () => {
  const currentYear = new Date().getFullYear();
  document.getElementById("current-year").textContent = currentYear;

  // Update displayed version
  try {
    const version = await invoke('gui_get_version');
    const footerLink = document.querySelector(".footer-link");
    footerLink.textContent = `© ${currentYear} Arnis v${version} by louis-e`;
  } catch (error) {
    console.error("Failed to fetch version:", error);
  }

  // Check for updates
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

      updateMessage.textContent = "There's a new version available! Click here to download it.";
      footer.style.marginTop = "15px";
      footer.appendChild(updateMessage);
    }
  } catch (error) {
    console.error("Failed to check for updates: ", error);
  }

  // Register bbox update event for iframe map
  window.addEventListener('message', function (event) {
    const bboxText = event.data.bboxText;

    if (bboxText) {
      console.log("Updated BBOX Coordinates:", bboxText);
      displayBboxInfoText(bboxText);
    }
  });

  window.pickDirectory = pickDirectory;
  window.startGeneration = startGeneration;

  const progressBar = document.getElementById("progress-bar");
  const progressMessage = document.getElementById("progress-message");
  const progressDetail = document.getElementById("progress-detail");

  // Listen for progress-update events
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
      } else {
        progressMessage.style.color = "";
      }
    }
  });
});

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

const threshold1 = 12332660.00;
const threshold2 = 36084700.00;
let selectedBBox = "";

// Function to handle incoming bbox data
function displayBboxInfoText(bboxText) {
  selectedBBox = bboxText;

  const [lng1, lat1, lng2, lat2] = bboxText.split(" ").map(Number);
  const bboxInfo = document.getElementById("bbox-info");

  // Reset the info text if the bbox is 0,0,0,0
  if (lng1 === 0 && lat1 === 0 && lng2 === 0 && lat2 === 0) {
    bboxInfo.textContent = "";
    return;
  }

  // Calculate the size of the selected bbox
  const selectedSize = calculateBBoxSize(lng1, lat1, lng2, lat2);

  if (selectedSize > threshold2) {
    bboxInfo.textContent = "This area is very large and could exceed typical computing limits.";
    bboxInfo.style.color = "#fa7878";
  } else if (selectedSize > threshold1) {
    bboxInfo.textContent = "The area is quite extensive and may take significant time and resources.";
    bboxInfo.style.color = "#fecc44";
  } else {
    bboxInfo.textContent = "Selection confirmed!";
    bboxInfo.style.color = "#7bd864";
  }
}

let worldPath = "";

async function pickDirectory() {
  try {
    const worldName = await invoke('gui_pick_directory');
    if (worldName) {
      worldPath = worldName;
      const lastSegment = worldName.split(/[\\/]/).pop();
      document.getElementById('selected-world').textContent = lastSegment;
      document.getElementById('selected-world').style.color = "#fecc44";
    }
  } catch (error) {
    console.error(error);
    document.getElementById('selected-world').textContent = error;
    document.getElementById('selected-world').style.color = "#fa7878";
  }
}

async function startGeneration() {
  try {
    if (worldPath === "No world selected" || worldPath == "Invalid Minecraft world" || worldPath === "") {
      document.getElementById('selected-world').textContent = "Select a Minecraft world first!";
      document.getElementById('selected-world').style.color = "#fa7878";
      return;
    }

    if (!selectedBBox || selectedBBox == "0.000000 0.000000 0.000000 0.000000") {
      document.getElementById('bbox-info').textContent = "Select a location firsta using the rectangle tool!";
      document.getElementById('bbox-info').style.color = "#fa7878";
      return;
    }

    // Pass the bounding box and selected world to the Rust backend
    await invoke("gui_start_generation", { bboxText: selectedBBox, selectedWorld: worldPath });
    
    // Update the UI or show a loading/progress message if needed
    console.log("Generation process started.");
  } catch (error) {
    console.error("Error starting generation:", error);
  }
}
