// 3D terrain preview: renders the selected bbox's elevation (plus Overture
// buildings and optional ESA WorldCover colors) as interactive MapLibre 3D
// terrain — as a mini panel under the selection info (rendered on demand via
// the map's terrain toolbar button) and as a full-window modal. The backend
// sends one binary payload per bbox; DEM tiles are synthesized on demand via
// addProtocol. Land cover is fetched lazily when its modal toggle is enabled.
(function () {
  "use strict";

  const TILE_SIZE = 256;
  const PROTOCOL = "arnisdem";
  // Mirrors TERRAIN_PREVIEW_MAX_AREA_M2 in bbox.js. Generous because previews
  // always use AWS Terrain Tiles at a resolution-capped grid, so fetch cost
  // stays roughly constant regardless of bbox size.
  const MINI_MAX_AREA_M2 = 500000000;
  // Mirrors BUILDINGS_MAX_AREA_M2 in preview_3d.rs.
  const BUILDINGS_MAX_AREA_M2 = 10000000;
  // Arnis palette: panel gray hosts the mini, modal gray hosts the expanded view.
  const PANEL_BG = "#575757";
  const MODAL_BG = "#717171";
  // Terrain outside the bbox fades onto a flat plate over this fraction of the
  // bbox span, replacing the stretched clamp-to-edge look with a clean base.
  const EDGE_FADE_FRAC = 0.12;

  // ESA WorldCover 2021 class colors (official palette); 0/no-data stays transparent.
  const ESA_COLORS = {
    10: [0, 100, 0], // Tree cover
    20: [255, 187, 34], // Shrubland
    30: [255, 255, 76], // Grassland
    40: [240, 150, 255], // Cropland
    50: [250, 0, 0], // Built-up
    60: [180, 180, 180], // Bare / sparse vegetation
    70: [240, 240, 240], // Snow and ice
    80: [0, 100, 200], // Permanent water bodies
    90: [0, 150, 160], // Herbaceous wetland
    95: [0, 207, 117], // Mangroves
    100: [250, 230, 160], // Moss and lichen
  };

  // Datasets keyed by generation id so DEM tile URLs stay unambiguous when
  // the selection changes while a map instance is still alive.
  const datasets = new Map();
  let nextGen = 1;
  let payloadCache = { key: null, data: null };
  let buildingsCache = { key: null, geojson: null };
  let landCoverCache = { key: null, url: null };
  let miniView = null; // { map, gen }
  let modalView = null; // { map, gen }
  let miniToken = 0;
  let buildingsToken = 0;
  let generationRunning = false;
  let protocolRegistered = false;

  // ---------- payload parsing and DEM tile synthesis ----------

  // Normalizes a Tauri invoke result to an aligned ArrayBuffer and validates
  // the payload magic; every payload layout starts with magic + grid dims.
  function readPayloadHeader(buffer, expectedMagic) {
    if (buffer instanceof Uint8Array) {
      buffer =
        buffer.byteOffset === 0 && buffer.byteLength === buffer.buffer.byteLength
          ? buffer.buffer
          : buffer.buffer.slice(buffer.byteOffset, buffer.byteOffset + buffer.byteLength);
    } else if (Array.isArray(buffer)) {
      buffer = new Uint8Array(buffer).buffer;
    }
    const dv = new DataView(buffer);
    const magic = String.fromCharCode(dv.getUint8(0), dv.getUint8(1), dv.getUint8(2), dv.getUint8(3));
    if (magic !== expectedMagic) throw new Error("Unexpected preview payload format");
    return { buffer: buffer, dv: dv, gw: dv.getUint32(4, true), gh: dv.getUint32(8, true) };
  }

  function parsePayload(raw) {
    const { buffer, dv, gw, gh } = readPayloadHeader(raw, "APV1");
    const d = {
      gw,
      gh,
      minLat: dv.getFloat64(12, true),
      minLng: dv.getFloat64(20, true),
      maxLat: dv.getFloat64(28, true),
      maxLng: dv.getFloat64(36, true),
      minElev: dv.getFloat32(44, true),
      maxElev: dv.getFloat32(48, true),
      heights: new Uint16Array(buffer, 64, gw * gh),
    };
    d.mScale = d.maxElev > d.minElev ? (d.maxElev - d.minElev) / 65535 : 0;
    // Barely below the lowest terrain: the surroundings read as a flat plane
    // the island sits in, not a pedestal it towers over.
    d.plateElev = d.minElev - Math.max(0.5, (d.maxElev - d.minElev) * 0.01);
    return d;
  }

  // Bilinear height sample in meters; clamps to the grid edge outside the bbox.
  function sampleMeters(d, lat, lng) {
    const fx = ((lng - d.minLng) / (d.maxLng - d.minLng)) * (d.gw - 1);
    const fy = ((d.maxLat - lat) / (d.maxLat - d.minLat)) * (d.gh - 1);
    const x = Math.min(Math.max(fx, 0), d.gw - 1);
    const y = Math.min(Math.max(fy, 0), d.gh - 1);
    const x0 = Math.floor(x);
    const y0 = Math.floor(y);
    const x1 = Math.min(x0 + 1, d.gw - 1);
    const y1 = Math.min(y0 + 1, d.gh - 1);
    const tx = x - x0;
    const ty = y - y0;
    const h = d.heights;
    const v =
      h[y0 * d.gw + x0] * (1 - tx) * (1 - ty) +
      h[y0 * d.gw + x1] * tx * (1 - ty) +
      h[y1 * d.gw + x0] * (1 - tx) * ty +
      h[y1 * d.gw + x1] * tx * ty;
    return d.minElev + v * d.mScale;
  }

  // Inverse web-mercator: global fractional tile row -> latitude.
  function yToLat(yFrac, z) {
    const n = Math.PI - (2 * Math.PI * yFrac) / Math.pow(2, z);
    return (180 / Math.PI) * Math.atan(0.5 * (Math.exp(n) - Math.exp(-n)));
  }

  // Height with the edge treatment: real terrain inside the bbox, smooth
  // fade onto the flat base plate outside it.
  function terrainElevation(d, lat, lng) {
    let out = 0;
    if (lng < d.minLng) out = (d.minLng - lng) / ((d.maxLng - d.minLng) * EDGE_FADE_FRAC);
    else if (lng > d.maxLng) out = (lng - d.maxLng) / ((d.maxLng - d.minLng) * EDGE_FADE_FRAC);
    let outLat = 0;
    if (lat < d.minLat) outLat = (d.minLat - lat) / ((d.maxLat - d.minLat) * EDGE_FADE_FRAC);
    else if (lat > d.maxLat) outLat = (lat - d.maxLat) / ((d.maxLat - d.minLat) * EDGE_FADE_FRAC);
    out = Math.max(out, outLat);
    if (out >= 1) return d.plateElev;
    const h = sampleMeters(d, lat, lng);
    if (out <= 0) return h;
    const t = out * out * (3 - 2 * out);
    return h + (d.plateElev - h) * t;
  }

  // True when the tile lies fully outside the bbox + fade margin, i.e. every
  // pixel would be the constant plate elevation.
  function isPlateTile(d, z, x, y) {
    const scale = Math.pow(2, z);
    const tMinLng = (x / scale) * 360 - 180;
    const tMaxLng = ((x + 1) / scale) * 360 - 180;
    const tMaxLat = yToLat(y, z);
    const tMinLat = yToLat(y + 1, z);
    const padLng = (d.maxLng - d.minLng) * EDGE_FADE_FRAC;
    const padLat = (d.maxLat - d.minLat) * EDGE_FADE_FRAC;
    return (
      tMaxLng < d.minLng - padLng ||
      tMinLng > d.maxLng + padLng ||
      tMaxLat < d.minLat - padLat ||
      tMinLat > d.maxLat + padLat
    );
  }

  function renderDemTile(d, z, x, y) {
    const img = new ImageData(TILE_SIZE, TILE_SIZE);
    const px = img.data;
    const scale = Math.pow(2, z);
    if (isPlateTile(d, z, x, y)) {
      const e = Math.min(Math.max(d.plateElev + 32768, 0), 65535.99);
      const whole = Math.floor(e);
      const cr = Math.floor(whole / 256);
      const cg = whole % 256;
      const cb = Math.floor((e - whole) * 256);
      for (let i = 0; i < px.length; i += 4) {
        px[i] = cr;
        px[i + 1] = cg;
        px[i + 2] = cb;
        px[i + 3] = 255;
      }
      return img;
    }
    let i = 0;
    for (let r = 0; r < TILE_SIZE; r++) {
      const lat = yToLat(y + (r + 0.5) / TILE_SIZE, z);
      for (let c = 0; c < TILE_SIZE; c++) {
        const lng = ((x + (c + 0.5) / TILE_SIZE) / scale) * 360 - 180;
        // Terrarium encoding: elevation = (R * 256 + G + B / 256) - 32768
        const e = Math.min(Math.max(terrainElevation(d, lat, lng) + 32768, 0), 65535.99);
        const whole = Math.floor(e);
        px[i++] = Math.floor(whole / 256);
        px[i++] = whole % 256;
        px[i++] = Math.floor((e - whole) * 256);
        px[i++] = 255;
      }
    }
    return img;
  }

  // The terrain and hillshade sources request every tile with the same URL;
  // caching the synthesized ImageData halves the per-tile sampling work.
  const tileCache = new Map();
  const TILE_CACHE_MAX = 48;

  function ensureProtocol() {
    if (protocolRegistered) return;
    maplibregl.addProtocol(PROTOCOL, async (params) => {
      const m = params.url.match(/^arnisdem:\/\/(\d+)\/(\d+)\/(\d+)\/(\d+)/);
      const d = m && datasets.get(+m[1]);
      if (!d) throw new Error("No preview data available");
      let img = tileCache.get(params.url);
      if (!img) {
        img = renderDemTile(d, +m[2], +m[3], +m[4]);
        tileCache.set(params.url, img);
        if (tileCache.size > TILE_CACHE_MAX) {
          tileCache.delete(tileCache.keys().next().value);
        }
      }
      return { data: await createImageBitmap(img) };
    });
    protocolRegistered = true;
  }

  function landCoverUrl(grid, gw, gh) {
    const canvas = document.createElement("canvas");
    canvas.width = gw;
    canvas.height = gh;
    const ctx = canvas.getContext("2d");
    const img = ctx.createImageData(gw, gh);
    const px = img.data;
    for (let i = 0; i < grid.length; i++) {
      const rgb = ESA_COLORS[grid[i]];
      if (rgb) {
        const o = i * 4;
        px[o] = rgb[0];
        px[o + 1] = rgb[1];
        px[o + 2] = rgb[2];
        px[o + 3] = 255;
      }
    }
    ctx.putImageData(img, 0, 0);
    return canvas.toDataURL("image/png");
  }

  // Zoom at which one tile pixel matches the native grid resolution.
  function nativeMaxZoom(d) {
    const cellDeg = (d.maxLng - d.minLng) / d.gw;
    const z = Math.ceil(Math.log2(360 / (TILE_SIZE * cellDeg)));
    return Math.min(Math.max(z, 4), 17);
  }

  function hypsometricRamp(d, bgColor) {
    const stops = [
      [0.0, "#2f6b3c"],
      [0.25, "#96b566"],
      [0.5, "#d9c789"],
      [0.75, "#9c7a5b"],
      [0.9, "#8a8a8a"],
      [1.0, "#ffffff"],
    ];
    const expr = ["interpolate", ["linear"], ["elevation"]];
    // The base plate blends into the surrounding UI color.
    expr.push(d.plateElev, bgColor);
    for (const [f, color] of stops) {
      expr.push(d.minElev + f * (d.maxElev - d.minElev), color);
    }
    return expr;
  }

  function loadMapLibre() {
    if (window.maplibregl) return Promise.resolve();
    return new Promise((resolve, reject) => {
      const css = document.createElement("link");
      css.rel = "stylesheet";
      css.href = "./css/maps/maplibre-gl.css";
      document.head.appendChild(css);
      const s = document.createElement("script");
      s.src = "./js/maps/maplibre-gl.js";
      s.onload = resolve;
      s.onerror = () => reject(new Error("Failed to load MapLibre GL"));
      document.head.appendChild(s);
    });
  }

  // ---------- shared data loading ----------

  function parseBBoxText(bboxText) {
    const parts = String(bboxText || "").trim().split(/[,\s]+/).map(Number);
    if (parts.length !== 4 || parts.some((v) => !isFinite(v))) return null;
    return { minLat: parts[0], minLng: parts[1], maxLat: parts[2], maxLng: parts[3] };
  }

  function bboxAreaM2(bboxText) {
    const b = parseBBoxText(bboxText);
    if (!b) return NaN;
    const midLat = ((b.minLat + b.maxLat) / 2) * (Math.PI / 180);
    const hM = Math.abs(b.maxLat - b.minLat) * 111320;
    const wM = Math.abs(b.maxLng - b.minLng) * 111320 * Math.cos(midLat);
    return hM * wM;
  }

  // Normalized comparison key: the iframe and main.js format the same bbox
  // with different decimal padding, so raw strings can't be compared.
  function bboxKey(bboxText) {
    const b = parseBBoxText(bboxText);
    return b ? [b.minLat, b.minLng, b.maxLat, b.maxLng].map((v) => v.toFixed(6)).join(" ") : null;
  }

  function gcDatasets() {
    const keep = new Set();
    if (payloadCache.data) keep.add(payloadCache.data.gen);
    if (miniView) keep.add(miniView.gen);
    if (modalView) keep.add(modalView.gen);
    for (const g of Array.from(datasets.keys())) {
      if (!keep.has(g)) datasets.delete(g);
    }
  }

  async function fetchTerrain(bboxText) {
    if (payloadCache.key === bboxText && payloadCache.data) return payloadCache.data;
    const buffer = await window.__TAURI__.core.invoke("gui_get_terrain_preview", {
      bboxText: bboxText,
      awsOnly: true,
    });
    const d = parsePayload(buffer);
    d.gen = nextGen++;
    d.bboxText = bboxText;
    datasets.set(d.gen, d);
    payloadCache = { key: bboxText, data: d };
    gcDatasets();
    return d;
  }

  // Best-effort Overture buildings; every failure is silent by design.
  async function loadBuildings(bboxText) {
    try {
      if (buildingsCache.key !== bboxText) {
        if (bboxAreaM2(bboxText) > BUILDINGS_MAX_AREA_M2) return;
        const myToken = ++buildingsToken;
        const raw = await window.__TAURI__.core.invoke("gui_get_preview_buildings", {
          bboxText: bboxText,
        });
        if (myToken !== buildingsToken) return;
        const gj = JSON.parse(raw);
        if (!gj || !Array.isArray(gj.features) || gj.features.length === 0) return;
        buildingsCache = { key: bboxText, geojson: gj };
      }
      attachBuildings(miniView, bboxText);
      attachBuildings(modalView, bboxText);
    } catch (e) {
      console.warn("Building preview unavailable:", e);
    }
  }

  function attachBuildings(view, bboxText) {
    if (!view || !view.map || !buildingsCache.geojson || buildingsCache.key !== bboxText) return;
    const d = datasets.get(view.gen);
    if (!d || d.bboxText !== bboxText) return;
    const map = view.map;
    let attempts = 0;
    // isStyleLoaded()/once("load") gating is racy (load may have passed while
    // the style reports busy during tile streaming) — just try, retry briefly.
    const tryAdd = () => {
      if (view !== miniView && view !== modalView) return;
      try {
        if (!map.getSource("buildings")) {
          map.addSource("buildings", { type: "geojson", data: buildingsCache.geojson });
          map.addLayer(
            {
              id: "buildings",
              type: "fill-extrusion",
              source: "buildings",
              paint: {
                "fill-extrusion-color": "#d6d6d6",
                "fill-extrusion-height": ["get", "h"],
                "fill-extrusion-base": ["get", "b"],
                "fill-extrusion-opacity": 1.0,
              },
            },
            map.getLayer("bbox-line") ? "bbox-line" : undefined
          );
        }
        // The modal has a Buildings toggle; sync it once the layer exists.
        if (view === modalView) {
          const toggle = document.getElementById("preview3d-buildings");
          toggle.disabled = false;
          map.setLayoutProperty("buildings", "visibility", toggle.checked ? "visible" : "none");
        }
      } catch (e) {
        if (++attempts < 20) setTimeout(tryAdd, 300);
        else console.warn("Building layer failed:", e);
      }
    };
    tryAdd();
  }

  // Lazily fetched ESA WorldCover overlay for the modal (user-enabled toggle).
  async function setModalLandCover(enabled) {
    const toggle = document.getElementById("preview3d-landcover");
    if (!modalView || !modalView.map) return;
    const map = modalView.map;
    try {
      if (map.getLayer("landcover")) {
        map.setLayoutProperty("landcover", "visibility", enabled ? "visible" : "none");
        return;
      }
      if (!enabled) return;
      const d = datasets.get(modalView.gen);
      if (!d) return;

      toggle.disabled = true;
      let url = landCoverCache.key === d.bboxText ? landCoverCache.url : null;
      if (!url) {
        const raw = await window.__TAURI__.core.invoke("gui_get_preview_landcover", {
          bboxText: d.bboxText,
        });
        const { buffer, gw, gh } = readPayloadHeader(raw, "APL1");
        if (gw !== d.gw || gh !== d.gh) throw new Error("land cover grid mismatch");
        url = landCoverUrl(new Uint8Array(buffer, 12, gw * gh), gw, gh);
        landCoverCache = { key: d.bboxText, url: url };
      }
      if (!modalView || modalView.map !== map) return;
      map.addSource("landcover", {
        type: "image",
        url: url,
        coordinates: [
          [d.minLng, d.maxLat],
          [d.maxLng, d.maxLat],
          [d.maxLng, d.minLat],
          [d.minLng, d.minLat],
        ],
      });
      map.addLayer(
        {
          id: "landcover",
          type: "raster",
          source: "landcover",
          paint: { "raster-opacity": 0.75, "raster-resampling": "nearest" },
        },
        map.getLayer("hillshade") ? "hillshade" : undefined
      );
    } catch (e) {
      console.warn("Land cover unavailable:", e);
      toggle.checked = false;
    } finally {
      toggle.disabled = false;
    }
  }

  // ---------- map construction (shared by mini and modal) ----------

  function buildStyle(d, bgColor) {
    const lngPad = (d.maxLng - d.minLng) * 0.5;
    const latPad = (d.maxLat - d.minLat) * 0.5;
    // Two identical DEM sources: MapLibre wants terrain and hillshade separated.
    // Bounds match the camera's maxBounds so the base plate covers every
    // visible spot (outside it terrain drops to sea level as a cliff).
    const demSource = () => ({
      type: "raster-dem",
      tiles: [PROTOCOL + "://" + d.gen + "/{z}/{x}/{y}"],
      tileSize: TILE_SIZE,
      encoding: "terrarium",
      minzoom: 0,
      maxzoom: nativeMaxZoom(d),
      bounds: [
        d.minLng - lngPad * 2,
        d.minLat - latPad * 2,
        d.maxLng + lngPad * 2,
        d.maxLat + latPad * 2,
      ],
    });

    const style = {
      version: 8,
      sources: {
        dem: demSource(),
        demShade: demSource(),
        bbox: {
          type: "geojson",
          data: {
            type: "Feature",
            geometry: {
              type: "LineString",
              coordinates: [
                [d.minLng, d.minLat],
                [d.maxLng, d.minLat],
                [d.maxLng, d.maxLat],
                [d.minLng, d.maxLat],
                [d.minLng, d.minLat],
              ],
            },
          },
        },
      },
      layers: [
        { id: "bg", type: "background", paint: { "background-color": bgColor } },
        {
          id: "hillshade",
          type: "hillshade",
          source: "demShade",
          paint: { "hillshade-exaggeration": 0.55 },
        },
        {
          id: "bbox-line",
          type: "line",
          source: "bbox",
          paint: { "line-color": "#7bd864", "line-width": 2, "line-opacity": 0.9 },
        },
      ],
    };
    return style;
  }

  function createTerrainMap(container, d, opts) {
    ensureProtocol();
    const bgColor = opts.bgColor || MODAL_BG;
    const lngPad = (d.maxLng - d.minLng) * 0.5;
    const latPad = (d.maxLat - d.minLat) * 0.5;
    const maxZoom = nativeMaxZoom(d);

    const map = new maplibregl.Map({
      container: container,
      style: buildStyle(d, bgColor),
      center: [(d.minLng + d.maxLng) / 2, (d.minLat + d.maxLat) / 2],
      zoom: Math.max(maxZoom - 4, 2),
      maxZoom: maxZoom + 3,
      maxPitch: 85,
      maxBounds: [
        [d.minLng - lngPad * 2, d.minLat - latPad * 2],
        [d.maxLng + lngPad * 2, d.maxLat + latPad * 2],
      ],
      attributionControl: false,
      interactive: opts.interactive !== false,
    });

    map.on("error", (e) => console.warn("Preview3D map error:", e && e.error));

    map.on("load", () => {
      // Hypsometric tint below the hillshade; v5.6 layer type, degrade silently.
      if (d.maxElev - d.minElev >= 1) {
        try {
          map.addLayer(
            {
              id: "relief",
              type: "color-relief",
              source: "demShade",
              paint: {
                "color-relief-color": hypsometricRamp(d, bgColor),
                "color-relief-opacity": 0.85,
              },
            },
            "hillshade"
          );
        } catch (err) {
          console.warn("color-relief layer unavailable:", err);
        }
      }
      try {
        map.setTerrain({ source: "dem", exaggeration: currentExaggeration() });
      } catch (err) {
        console.warn("Terrain unavailable:", err);
      }
      const cam = map.cameraForBounds(
        [
          [d.minLng, d.minLat],
          [d.maxLng, d.maxLat],
        ],
        { padding: opts.padding }
      );
      // cameraForBounds fits at pitch 0; the tilted camera pushes content
      // back, so boost the zoom to fill the frame again.
      if (cam) {
        map.jumpTo({
          ...cam,
          zoom: cam.zoom + (opts.zoomBoost || 0),
          pitch: opts.pitch,
          bearing: -17,
        });
      }
      if (opts.onReady) opts.onReady();
    });

    return map;
  }

  function disposeView(view) {
    if (view && view.map) {
      try {
        view.map.remove();
      } catch (e) {
        console.warn("Failed to dispose preview map:", e);
      }
    }
  }

  function currentExaggeration() {
    const el = document.getElementById("preview3d-exaggeration");
    return (el && parseFloat(el.value)) || 1.2;
  }

  // ---------- mini preview (rendered on demand via the map's terrain button) ----------

  function miniLoader(text, isError) {
    const wrap = document.getElementById("preview3d-mini");
    const loader = document.getElementById("preview3d-mini-loader");
    wrap.style.display = "block";
    wrap.classList.add("preview3d-mini-visible");
    loader.style.display = "flex";
    loader.classList.remove("preview3d-mini-loader-hidden");
    loader.classList.toggle("preview3d-mini-loader-error", !!isError);
    document.getElementById("preview3d-mini-loader-text").textContent = text;
  }

  function fadeMiniLoader() {
    document.getElementById("preview3d-mini-loader").classList.add("preview3d-mini-loader-hidden");
  }

  function miniError(text, token) {
    miniLoader(text, true);
    setTimeout(() => {
      if (token === miniToken) hideMini();
    }, 2600);
  }

  function hideMini() {
    const wrap = document.getElementById("preview3d-mini");
    if (wrap) {
      wrap.classList.remove("preview3d-mini-visible");
      wrap.style.display = "none";
    }
    const loader = document.getElementById("preview3d-mini-loader");
    if (loader) loader.style.display = "none";
    disposeView(miniView);
    miniView = null;
    gcDatasets();
  }

  let lastRequestedBBox = null;

  function requestMiniRender(bboxText) {
    if (isModalOpen()) return;
    const token = ++miniToken;
    const area = bboxAreaM2(bboxText);
    if (!isFinite(area) || area <= 0 || area > MINI_MAX_AREA_M2) return;
    if (generationRunning) {
      miniError("Preview not available during generation", token);
      return;
    }
    lastRequestedBBox = bboxKey(bboxText);
    miniLoader("Rendering terrain preview…");
    loadMini(bboxText, token);
  }

  async function loadMini(bboxText, myToken) {
    try {
      // Buildings fetch is independent of the terrain fetch; run it in
      // parallel — renderMini attaches from the cache if it wins the race.
      loadBuildings(bboxText);
      await loadMapLibre();
      const d = await fetchTerrain(bboxText);
      if (myToken !== miniToken) return; // superseded; the newer request owns the panel
      if (generationRunning || isModalOpen()) {
        hideMini(); // don't leave the loader animating forever
        return;
      }
      renderMini(d, bboxText);
    } catch (e) {
      if (myToken === miniToken && String(e).indexOf("superseded") === -1) {
        console.warn("Mini preview unavailable:", e);
        miniError("Preview unavailable", myToken);
      }
    }
  }

  function renderMini(d, bboxText) {
    disposeView(miniView);
    miniView = null;
    try {
      const map = createTerrainMap("preview3d-mini-map", d, {
        padding: 8,
        pitch: 55,
        zoomBoost: 0.7,
        bgColor: PANEL_BG,
      });
      miniView = { map: map, gen: d.gen };
      // MapLibre's click only fires on clean clicks (not after drags), so the
      // mini stays fully interactive while a plain click expands to the modal.
      map.on("click", () => window.expandPreview3D());
      // Reveal after the first frame following "load" ("idle" never fires with
      // terrain enabled); the timer is a safety net so the loader can't linger.
      const reveal = () => {
        if (miniView && miniView.map === map) fadeMiniLoader();
      };
      map.once("load", () => map.once("render", () => requestAnimationFrame(reveal)));
      setTimeout(reveal, 2500);
      attachBuildings(miniView, bboxText);
    } catch (e) {
      console.warn("Mini preview render failed:", e);
      miniError("Preview unavailable", miniToken);
    }
    gcDatasets();
  }

  // Called by main.js on selection changes; rendering itself is button-driven.
  window.arnisPreview3D = {
    onBboxChanged: function (bboxText) {
      // Some iframe paths re-send an unchanged bbox; keep the render then.
      if (lastRequestedBBox && bboxKey(bboxText) === lastRequestedBBox) return;
      lastRequestedBBox = null;
      miniToken++; // cancel any in-flight render, the selection is stale
      hideMini();
    },
    onBboxCleared: function () {
      lastRequestedBBox = null;
      miniToken++;
      hideMini();
    },
    setGenerationRunning: function (running) {
      generationRunning = !!running;
    },
  };

  // The map iframe's terrain button asks for a render via postMessage.
  window.addEventListener("message", (ev) => {
    if (ev.data && ev.data.type === "renderTerrainPreview" && ev.data.previewBBoxText) {
      requestMiniRender(String(ev.data.previewBBoxText));
    }
  });

  // ---------- full modal ----------

  function isModalOpen() {
    const modal = document.getElementById("preview3d-modal");
    return !!modal && modal.style.display !== "none";
  }

  function setStatus(text, isError) {
    const status = document.getElementById("preview3d-status");
    status.style.display = text ? "flex" : "none";
    status.textContent = text || "";
    status.classList.toggle("preview3d-status-error", !!isError);
  }

  function openModalWithData(d, bboxText) {
    disposeView(modalView);
    modalView = null;
    const map = createTerrainMap("preview3d-map", d, {
      padding: 40,
      pitch: 60,
      zoomBoost: 0.4,
      bgColor: MODAL_BG,
      onReady: function () {
        setStatus(null);
        document.getElementById("preview3d-meta").textContent =
          d.gw + "×" + d.gh + " grid · " + Math.round(d.minElev) + "–" + Math.round(d.maxElev) + " m";
      },
    });
    // Land cover is opt-in and fetched lazily on first enable.
    const lcToggle = document.getElementById("preview3d-landcover");
    lcToggle.checked = false;
    lcToggle.disabled = false;
    // Disabled until the buildings layer attaches (async, silent); stays off
    // with an explanation when the area exceeds the buildings gate.
    const bToggle = document.getElementById("preview3d-buildings");
    const buildingsGated = bboxAreaM2(bboxText) > BUILDINGS_MAX_AREA_M2;
    bToggle.disabled = true;
    bToggle.checked = !buildingsGated;
    bToggle.parentElement.title = buildingsGated
      ? "Area too large for the buildings overlay (max 10 km²)"
      : "";
    modalView = { map: map, gen: d.gen };
    attachBuildings(modalView, bboxText);
    gcDatasets();
  }

  // Clicking the mini preview reuses the cached payload for an instant
  // modal open (same data, bigger canvas).
  window.expandPreview3D = function () {
    if (!payloadCache.data) return;
    const modal = document.getElementById("preview3d-modal");
    modal.style.display = "flex";
    setStatus(null);
    openModalWithData(payloadCache.data, payloadCache.data.bboxText);
    loadBuildings(payloadCache.data.bboxText);
  };

  window.closePreview3D = function () {
    document.getElementById("preview3d-modal").style.display = "none";
    disposeView(modalView);
    modalView = null;
    gcDatasets();
  };

  document.addEventListener("DOMContentLoaded", () => {
    const exag = document.getElementById("preview3d-exaggeration");
    exag.addEventListener("input", () => {
      document.getElementById("preview3d-exaggeration-value").textContent =
        parseFloat(exag.value).toFixed(1) + "×";
      for (const view of [modalView, miniView]) {
        if (view && view.map && view.map.getTerrain()) {
          try {
            view.map.setTerrain({ source: "dem", exaggeration: currentExaggeration() });
          } catch (e) { /* keep previous exaggeration */ }
        }
      }
    });

    document.getElementById("preview3d-landcover").addEventListener("change", (e) => {
      setModalLandCover(e.target.checked);
    });

    document.getElementById("preview3d-buildings").addEventListener("change", (e) => {
      if (modalView && modalView.map && modalView.map.getLayer("buildings")) {
        modalView.map.setLayoutProperty(
          "buildings",
          "visibility",
          e.target.checked ? "visible" : "none"
        );
      }
    });

    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && isModalOpen()) window.closePreview3D();
    });
  });
})();
