const fs = require('node:fs');
const path = require('node:path');
const zlib = require('node:zlib');

const ROOT = path.resolve(__dirname, '..', '..');
const args = process.argv.slice(2);
if (args.length !== 2) {
  throw new Error('Usage: node assets/planetary/build_features.cjs <extracted-moon-gis-dir> <Dune_Field.gml>');
}
const MOON_DIR = path.resolve(args[0]);
const DUNE_GML = path.resolve(args[1]);
const OUT_W = 3600;
const OUT_H = 1800;
const STEP = 0.1;
const R = 1_737_400;
const flags = {
  moonMare: 1 << 0,
  moonRille: 1 << 1,
  moonCraterRim: 1 << 2,
  moonRidge: 1 << 3,
  marsVolcanic: 1 << 4,
  marsDune: 1 << 5,
  marsPolarCap: 1 << 6,
  marsPolarLayer: 1 << 7,
  marsFluvial: 1 << 8,
  marsRille: 1 << 9,
};
const mask = new Uint16Array(OUT_W * OUT_H);

function normalizeRing(input, around = null) {
  const out = [];
  let previous = null;
  for (const point of input) {
    let lon = Number(point[0]);
    const lat = Number(point[1]);
    if (!Number.isFinite(lon) || !Number.isFinite(lat)) continue;
    if (previous !== null) {
      while (lon - previous > 180) lon -= 360;
      while (lon - previous < -180) lon += 360;
    }
    out.push([lon, lat]);
    previous = lon;
  }
  if (around !== null && out.length) {
    const center = out.reduce((s, p) => s + p[0], 0) / out.length;
    const shift = Math.round((around - center) / 360) * 360;
    if (shift) for (const p of out) p[0] += shift;
  }
  return out;
}

function contains(ring, x, y) {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i];
    const [xj, yj] = ring[j];
    if ((yi > y) !== (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

function setCell(col, row, bit) {
  if (row < 0 || row >= OUT_H) return;
  col = ((col % OUT_W) + OUT_W) % OUT_W;
  mask[row * OUT_W + col] |= bit;
}

function ringArea(ring) {
  let area = 0;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    area += ring[j][0] * ring[i][1] - ring[i][0] * ring[j][1];
  }
  return area / 2;
}

function polygonGroups(rawRings, geoJson = false) {
  const rings = rawRings.map(r => normalizeRing(r)).filter(r => r.length >= 4);
  if (!rings.length) return [];
  if (geoJson) {
    const outer = rings[0];
    const centerLon = outer.reduce((s, p) => s + p[0], 0) / outer.length;
    return [{ outer, holes: rings.slice(1).map(hole => normalizeRing(hole, centerLon)) }];
  }
  const outers = rings.filter(r => ringArea(r) < 0);
  const holes = rings.filter(r => ringArea(r) >= 0);
  const groups = outers.map(outer => ({ outer, holes: [] }));
  for (const hole of holes) {
    const centerLon = groups.length ? groups[0].outer.reduce((s, p) => s + p[0], 0) / groups[0].outer.length : 0;
    const aligned = normalizeRing(hole, centerLon);
    const group = groups.find(g => contains(g.outer, aligned[0][0], aligned[0][1]));
    if (group) group.holes.push(aligned);
    else groups.push({ outer: aligned, holes: [] });
  }
  return groups;
}

function scanIntervals(ring, lat, shift) {
  const xs = [];
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const [xi, yi] = ring[i], [xj, yj] = ring[j];
    if ((yi > lat) !== (yj > lat)) xs.push(xi + shift + ((lat - yi) * (xj - xi)) / (yj - yi));
  }
  xs.sort((a, b) => a - b);
  const spans = [];
  for (let i = 0; i + 1 < xs.length; i += 2) spans.push([xs[i], xs[i + 1]]);
  return spans;
}

function rasterPolygon(rawRings, bit, geoJson = false) {
  for (const { outer, holes } of polygonGroups(rawRings, geoJson)) {
    const lats = outer.map(p => p[1]);
    const minLat = Math.max(-90, Math.min(...lats)), maxLat = Math.min(90, Math.max(...lats));
    const minRow = Math.max(0, Math.floor((90 - maxLat) / STEP));
    const maxRow = Math.min(OUT_H - 1, Math.floor((90 - minLat) / STEP));
    for (let shift = -360; shift <= 360; shift += 360) {
      const ringLons = outer.map(p => p[0] + shift);
      if (Math.max(...ringLons) < -180 || Math.min(...ringLons) > 180) continue;
      for (let row = minRow; row <= maxRow; row++) {
        const lat = 90 - (row + 0.5) * STEP;
        const outerSpans = scanIntervals(outer, lat, shift);
        const holeSpans = holes.flatMap(hole => scanIntervals(hole, lat, shift));
        for (const [start, end] of outerSpans) {
          let segments = [[start, end]];
          for (const [holeStart, holeEnd] of holeSpans) {
            const next = [];
            for (const [lo, hi] of segments) {
              if (holeEnd <= lo || holeStart >= hi) next.push([lo, hi]);
              else {
                if (holeStart > lo) next.push([lo, holeStart]);
                if (holeEnd < hi) next.push([holeEnd, hi]);
              }
            }
            segments = next;
          }
          for (const [lo, hi] of segments) {
            const minCol = Math.max(0, Math.ceil((lo + 180) / STEP - 0.5));
            const maxCol = Math.min(OUT_W - 1, Math.ceil((hi + 180) / STEP - 0.5) - 1);
            for (let col = minCol; col <= maxCol; col++) setCell(col, row, bit);
          }
        }
      }
    }
  }
}
function rasterLine(input, bit, radius = 1) {
  const ring = normalizeRing(input);
  if (ring.length < 2) return;
  for (let i = 1; i < ring.length; i++) {
    const [aLon, aLat] = ring[i - 1];
    const [bLon, bLat] = ring[i];
    const n = Math.max(1, Math.ceil(Math.max(Math.abs(bLon - aLon), Math.abs(bLat - aLat)) / STEP * 2));
    for (let s = 0; s <= n; s++) {
      const t = s / n;
      const lon = aLon + (bLon - aLon) * t;
      const lat = aLat + (bLat - aLat) * t;
      for (let shift = -360; shift <= 360; shift += 360) {
        const x = lon + shift;
        if (x < -180 - STEP || x > 180 + STEP) continue;
        const col = Math.floor((x + 180) / STEP);
        const row = Math.floor((90 - lat) / STEP);
        for (let dy = -radius; dy <= radius; dy++) {
          for (let dx = -radius; dx <= radius; dx++) setCell(col + dx, row + dy, bit);
        }
      }
    }
  }
}

function rasterGeoJson(geometry, bit, isLine = false) {
  if (!geometry) return;
  const { type, coordinates } = geometry;
  if (type === 'Polygon') {
    if (isLine) return;
    rasterPolygon(coordinates, bit, true);
  } else if (type === 'MultiPolygon') {
    if (isLine) return;
    for (const polygon of coordinates) rasterPolygon(polygon, bit, true);
  } else if (type === 'LineString') {
    rasterLine(coordinates, bit);
  } else if (type === 'MultiLineString') {
    for (const line of coordinates) rasterLine(line, bit);
  }
}

function readDbf(file) {
  const b = fs.readFileSync(file);
  const count = b.readUInt32LE(4), header = b.readUInt16LE(8), record = b.readUInt16LE(10);
  const fields = [];
  for (let p = 32; p < header && b[p] !== 0x0d; p += 32) {
    fields.push({ name: b.subarray(p, p + 11).toString('ascii').replace(/\0.*$/, '').trim(), width: b[p + 16] });
  }
  const rows = [];
  for (let n = 0; n < count; n++) {
    const start = header + n * record;
    if (b[start] === 0x2a) { rows.push(null); continue; }
    let p = start + 1;
    const row = {};
    for (const field of fields) {
      row[field.name] = b.subarray(p, p + field.width).toString('utf8').trim();
      p += field.width;
    }
    rows.push(row);
  }
  return rows;
}

function *readShp(file) {
  const b = fs.readFileSync(file);
  for (let p = 100; p + 8 <= b.length;) {
    const length = b.readInt32BE(p + 4) * 2;
    const c = p + 8, type = b.readInt32LE(c);
    if (type === 0) { yield null; p = c + length; continue; }
    if (![3, 5, 13, 15, 23, 25].includes(type)) throw new Error(`Unsupported shapefile geometry type ${type}`);
    const partsCount = b.readInt32LE(c + 36), pointCount = b.readInt32LE(c + 40);
    const partAt = c + 44, pointAt = partAt + partsCount * 4;
    const starts = Array.from({ length: partsCount }, (_, i) => b.readInt32LE(partAt + 4 * i));
    const points = Array.from({ length: pointCount }, (_, i) => [b.readDoubleLE(pointAt + i * 16), b.readDoubleLE(pointAt + i * 16 + 8)]);
    const parts = starts.map((start, i) => points.slice(start, starts[i + 1] ?? pointCount));
    if (type === 3 || type === 13 || type === 23) yield { kind: 'line', parts: parts.map(line => line.map(([x, y]) => [x / R * 180 / Math.PI, y / R * 180 / Math.PI])) };
    else yield { kind: 'polygon', parts: parts.map(ring => ring.map(([x, y]) => [x / R * 180 / Math.PI, y / R * 180 / Math.PI])) };
    p = c + length;
  }
}

function shapefilePolygonRings(shape) {
  // The lunar source stores one outer boundary per feature; any island units
  // inside a mare are independently mapped as separate geologic polygons.
  return shape.parts;
}

async function fetchFeatures(collection, filter) {
  const url = new URL(`https://astrogeology.usgs.gov/pygeoapi/collections/${collection}/items`);
  url.searchParams.set('f', 'json');
  url.searchParams.set('limit', '1000');
  url.searchParams.set('filter-lang', 'cql-text');
  url.searchParams.set('filter', filter);
  const response = await fetch(url);
  const text = await response.text();
  if (!response.ok) throw new Error(`USGS ${response.status} for ${url}: ${text.slice(0, 200)}`);
  return JSON.parse(text).features;
}

async function main() {
  const unitsUrl = 'mars/sim3292_global_geologic_map/units';
  const structuresUrl = 'mars/global_geologic_map/structure';
  for (const [filter, bit, label] of [
    ["unitdesc LIKE '%volcanic%'", flags.marsVolcanic, 'Martian volcanic units'],
    ["unitdesc LIKE '%polar%'", flags.marsPolarLayer, 'Martian polar units'],
  ]) {
    const features = await fetchFeatures(unitsUrl, filter);
    let count = 0;
    for (const feature of features) {
      const desc = (feature.properties.unitdesc || '').toLowerCase();
      const unitBit = desc.includes('polar cap') ? flags.marsPolarCap | flags.marsPolarLayer : bit;
      rasterGeoJson(feature.geometry, unitBit);
      count++;
    }
    console.log(`${label}: ${count} mapped units`);
  }

  for (const [filter, bit, label] of [
    ["origin = 'Fluvial'", flags.marsFluvial, 'Martian fluvial structures'],
    ["interpreta = 'Rille'", flags.marsRille, 'Martian mapped rilles'],
  ]) {
    const features = await fetchFeatures(structuresUrl, filter);
    for (const feature of features) rasterGeoJson(feature.geometry, bit, true);
    console.log(`${label}: ${features.length} mapped lines`);
  }

  const duneGml = fs.readFileSync(DUNE_GML, 'utf8');
  let dunes = 0;
  for (const match of duneGml.matchAll(/<gml:coordinates>([^<]+)<\/gml:coordinates>/g)) {
    const coordinates = match[1].trim().split(/\s+/).map(pair => pair.split(',').map(Number));
    if (coordinates.length >= 4) { rasterPolygon([coordinates], flags.marsDune, true); dunes++; }
  }
  console.log(`Martian dune fields: ${dunes} mapped polygons`);

  const unitsDbf = readDbf(path.join(MOON_DIR, 'GeoUnits.dbf'));
  const unitsShp = readShp(path.join(MOON_DIR, 'GeoUnits.shp'));
  let mares = 0;
  let i = 0;
  for (const shape of unitsShp) {
    const row = unitsDbf[i++];
    if (!shape || !/mare/i.test(row?.FIRST_Un_2 || '')) continue;
    rasterPolygon(shape.parts, flags.moonMare);
    mares++;
  }
  console.log(`Lunar mare polygons: ${mares}`);

  const lineDbf = readDbf(path.join(MOON_DIR, 'Linear_Features.dbf'));
  const lineShp = readShp(path.join(MOON_DIR, 'Linear_Features.shp'));
  const types = new Map();
  let lineIndex = 0;
  for (const shape of lineShp) {
    const row = lineDbf[lineIndex++];
    if (!shape) continue;
    const type = (row?.TYPE || '').toLowerCase();
    const bit = type.includes('channel (volcanic)') ? flags.moonRille
      : type.includes('crest of crater rim') ? flags.moonCraterRim
      : type.includes('ridge crest') || type === 'basin ring' ? flags.moonRidge
      : 0;
    if (!bit) continue;
    types.set(type, (types.get(type) || 0) + 1);
    for (const line of shape.parts) rasterLine(line, bit);
  }
  console.log('Lunar mapped line classes:', Object.fromEntries(types));

  const raw = Buffer.from(mask.buffer, mask.byteOffset, mask.byteLength);
  const compressed = zlib.gzipSync(raw, { level: 9 });
  fs.writeFileSync(path.join(ROOT, 'assets', 'planetary', 'features.grid.gz'), compressed);
  console.log(`Wrote ${compressed.length} bytes gzip (raw ${raw.length} bytes), cells=${mask.length}`);
  console.log('Sample flags:', {
    moonMareCrisium: at(17, 59),
    moonTycho: at(-43.3, -11.2),
    marsOlympusMons: at(18.65, -133.8),
    marsNorthPolar: at(87, 0),
    marsSouthPoleCap: at(-87, 0),
    marsNiliPatera: at(8.7, 67.3),
  });
}

function at(lat, lon) {
  const col = ((Math.floor((lon + 180) / STEP) % OUT_W) + OUT_W) % OUT_W;
  const row = Math.max(0, Math.min(OUT_H - 1, Math.floor((90 - lat) / STEP)));
  return mask[row * OUT_W + col];
}

main().catch(error => { console.error(error); process.exit(1); });






