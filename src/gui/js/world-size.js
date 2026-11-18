// The size estimate under the map: what the backend is asked (its table in world_utils.rs is the one the disk check uses) and how the answer reads.

// Metres across a block on the Moon and Mars.
const BODY_METRES = { moon: 200, mars: 500 };

// The request for gui_world_size_estimate, the settings as a run takes them: off Earth no caves, interiors or extended height, interiors only where buildings are, Disasters fill the ground from -40.
export function sizeRequest({ km2, scale = 1, body = 'earth', format = 'java', mode = 'geo-terrain', groundLevel = -62, tall = true,
  fill = false, caves = false, interior = false, light = false, voxy = false, dh = false, disasters = false }) {
  if ((format !== 'java' && format !== 'bedrock') || !(km2 > 0)) return null;
  const earth = !BODY_METRES[body];
  const cave = caves && earth;
  const shake = disasters && earth;
  return {
    blocksKm2: earth ? km2 * scale * scale : km2 / BODY_METRES[body] ** 2,
    worldFormat: format,
    interior: interior && earth && (mode === 'geo-terrain' || mode === 'geo-only'),
    bakeLighting: light,
    voxyLod: voxy,
    dhLod: dh,
    caves: cave,
    fillground: fill || cave || shake,
    // Only Java builds deeper with the extended height; Bedrock raises its top alone.
    tall: tall && earth && format === 'java',
    groundLevel: shake ? Math.max(groundLevel, -40) : groundLevel,
  };
}

const sig2 = (v) => {
  const p = 10 ** (Math.floor(Math.log10(v)) - 1);
  return Math.round(v / p) * p;
};

// Two significant figures, never under 1 MB: "150 MB", "1.3 GB", "23 GB".
export function formatMb(mb) {
  let v = Math.max(1, mb);
  let unit = 0;
  while (sig2(v) >= 1000 && unit < 2) {
    v /= 1024;
    unit += 1;
  }
  const n = sig2(v);
  const text = n >= 10 ? String(Math.round(n)) : unit ? n.toFixed(1) : String(Math.round(v));
  return `${text} ${['MB', 'GB', 'TB'][unit]}`;
}

// "327–388 MB": a unit both ends share is said once (the cells plan reads its ranges the same way).
export const rangeText = ([lo, hi], unit) => {
  const [a, b] = [unit(lo), unit(hi)];
  if (a === b) return a;
  const u = a.match(/^[\d.,]+( \S+)$/);
  return u && b.endsWith(u[1]) ? `${a.slice(0, -u[1].length)}–${b}` : `${a}–${b}`;
};

export const formatSizeRange = (mib) => rangeText(mib, formatMb);

// Ground area of drawn shapes ([lat, lng] rings) in km², on a body of radius `r` metres.
export function ringsKm2(rings, r) {
  const rad = Math.PI / 180;
  return rings.reduce((sum, ring) => {
    const lat = (ring.reduce((s, p) => s + p[0], 0) / ring.length) * rad;
    let twice = 0;
    ring.forEach(([y1, x1], i) => {
      const [y2, x2] = ring[(i + 1) % ring.length];
      twice += x1 * y2 - x2 * y1;
    });
    return sum + (Math.abs(twice) / 2) * rad * rad * r * r * Math.cos(lat) / 1e6;
  }, 0);
}
