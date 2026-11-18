// What a world takes on disk before it is generated, in MiB per km² of blocks (ground area times scale²), from 1 km² runs of the 3.4 build with the window's defaults.

// Farmland 15.6, sea 15.6, suburb 16.0, Alps 16.3, Munich 16.6, Manhattan 18.8, forest 21.7, Moon 16.1, Mars 15.9.
const BASE = 17;
// Interiors add 4.7 (Munich) to 16.5 (Manhattan), baked light 11.3, and Voxy and Distant Horizons 11.6 and 8.1 on top of it.
const INTERIOR = 10;
const LIGHT = 11;
const VOXY = 12;
const DH = 8;
// On flat land filled ground and caves grow with the rock under the base: Munich 0.2 and 3.0 at the default level, 1.6 and 16.5 at 0, 2.5 and 18.1 at 63.
const FILL_FLAT = [0.1, 0.03];
const CAVES_FLAT = [1.5, 0.22];
// In the Alps (Zermatt) they come to 8.4 and 25.9 at vanilla height, and 20.4 and 47.7 with the extended build height.
const FILL_ALPS = { vanilla: 8, tall: 20 };
const CAVES_ALPS = { vanilla: 26, tall: 48 };
// A Bedrock .mcworld came to 0.39 (Munich) to 0.51 (Munich with caves) of the Java world.
const BEDROCK = 0.45;
// Metres across a block on the Moon and Mars.
const BODY_METRES = { moon: 200, mars: 500 };

// Blocks of rock between the base and the bedrock plane, which sits 64 under it on a section line and never below -64.
const rockUnder = (level) => level - Math.max(-64, Math.floor((level - 64) / 16) * 16);

// [low, high] MiB for `km2` of ground, or null where nothing was measured; with filled ground or caves the high end is for mountains, as only the run knows the terrain.
export function worldSizeMb({
  km2, scale = 1, body = 'earth', format = 'java', groundLevel = -62, tall = true,
  fill = false, caves = false, interior = false, light = false, voxy = false, dh = false,
}) {
  if ((format !== 'java' && format !== 'bedrock') || !(km2 > 0)) return null;
  const earth = !BODY_METRES[body];
  const blocks = earth ? km2 * scale * scale : km2 / BODY_METRES[body] ** 2;
  // Off Earth there are no caves or interiors, and the build height stays vanilla.
  const cave = caves && earth;
  const height = tall && earth ? 'tall' : 'vanilla';
  let low = BASE + (interior && earth ? INTERIOR : 0) + (light || voxy || dh ? LIGHT : 0) + (voxy ? VOXY : 0) + (dh ? DH : 0);
  let high = low;
  const rock = rockUnder(groundLevel) - rockUnder(-62);
  if (cave) {
    low += CAVES_FLAT[0] + CAVES_FLAT[1] * rock;
    high += CAVES_ALPS[height];
  } else if (fill) {
    low += FILL_FLAT[0] + FILL_FLAT[1] * rock;
    high += FILL_ALPS[height];
  }
  const f = blocks * (format === 'bedrock' ? BEDROCK : 1);
  return [low * f, Math.max(low, high) * f];
}

// Two significant figures, never under 1 MB: "150 MB", "1.3 GB", "23 GB".
export function formatMb(mb) {
  const sig2 = (v) => {
    const p = 10 ** (Math.floor(Math.log10(v)) - 1);
    return Math.round(v / p) * p;
  };
  let v = Math.max(1, mb);
  let unit = 0;
  while (sig2(v) >= 1000 && unit < 2) {
    v /= 1024;
    unit += 1;
  }
  const n = v < 10 ? (unit ? v.toFixed(1) : String(Math.round(v))) : String(Math.round(sig2(v)));
  return `${n} ${['MB', 'GB', 'TB'][unit]}`;
}

// "150 MB", or "20–65 MB" with the unit said once; ends that round alike are one figure.
export function formatSizeRange([low, high]) {
  const a = formatMb(low);
  const b = formatMb(high);
  if (a === b) return a;
  const [an, au] = a.split(' ');
  const [bn, bu] = b.split(' ');
  return au === bu ? `${an}–${bn} ${bu}` : `${a}–${b}`;
}

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
