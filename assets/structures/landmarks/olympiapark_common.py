"""Shared voxel model, Sponge writer and survey helpers for the Olympiapark generators.

build_olympiaturm.py, build_olympiahalle.py and build_olympia_schwimmhalle.py import this.
Deps: numpy; the halls also need laspy[lazrs], pyproj and pillow (laser tiles, OSM outlines).
"""
import gzip
import io
import os
import re
import struct
import sys
import urllib.request

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
SCHEMATIC_RS = os.path.join(HERE, "..", "..", "..", "src", "structures", "schematic.rs")
DATA_VERSION = 4438

LASER_URL = "https://geodaten.bayern.de/odd_data/laser/{}_{}.laz"
OSM_URL = "https://www.openstreetmap.org/api/0.6/way/{}/full.json"
# The Olympiastadion model's local frame, so all three share one grid.
FRAME_LAT, FRAME_LON = 48.1731023, 11.5464961


def state(name, **props):
    name = name if ":" in name else "minecraft:" + name
    if not props:
        return name
    inner = ",".join(f"{k}={str(v).lower()}" for k, v in sorted(props.items()))
    return f"{name}[{inner}]"


def base(s):
    return s.split("[")[0]


class Model:
    def __init__(self, w, h, l):
        self.v = np.zeros((w, h, l), np.uint16)
        self.palette = ["minecraft:air"]
        self.index = {"minecraft:air": 0}

    def sid(self, s):
        s = state(s) if ":" not in s else s
        if s not in self.index:
            self.index[s] = len(self.palette)
            self.palette.append(s)
        return self.index[s]

    def set(self, x, y, z, s):
        w, h, l = self.v.shape
        if 0 <= x < w and 0 <= y < h and 0 <= z < l:
            self.v[x, y, z] = self.sid(s)

    def get(self, x, y, z):
        w, h, l = self.v.shape
        if 0 <= x < w and 0 <= y < h and 0 <= z < l:
            return self.palette[self.v[x, y, z]]
        return "minecraft:air"

    def layer(self, y, mask, s, only_air=False):
        """Fill the 2D (x, z) mask at layer y."""
        i = self.sid(s)
        sl = self.v[:, y, :]
        m = mask & (sl == 0) if only_air else mask
        sl[m] = i

    def connect(self):
        """Give panes, bars, walls and fences the connections their neighbours call for."""
        v = self.v
        n0 = len(self.palette)
        links = np.array([base(s).endswith(("_bars", "_pane", "_wall", "_fence")) for s in self.palette])
        solid = np.array([s != "minecraft:air" and not any(k in s for k in (
            "slab", "stairs", "trapdoor", "lantern", "rod", "chain", "carpet", "button"))
            for s in self.palette]) | links
        w, h, l = v.shape
        xs, ys, zs = np.nonzero(links[v])
        new = []
        cache = {}
        for x, y, z in zip(xs, ys, zs):
            s = self.palette[v[x, y, z]]
            b = base(s)
            near = {}
            for d, (dx, dz) in {"north": (0, -1), "south": (0, 1), "west": (-1, 0), "east": (1, 0)}.items():
                nx, nz = x + dx, z + dz
                n = v[nx, y, nz] if 0 <= nx < w and 0 <= nz < l else 0
                if n >= n0:
                    n = new[n - n0]
                near[d] = bool(solid[n])
            if b.endswith("_wall"):
                above = v[x, y + 1, z] if y + 1 < h else 0
                above = new[above - n0] if above >= n0 else above
                straight = near == {"north": True, "south": True, "west": False, "east": False} or \
                    near == {"north": False, "south": False, "west": True, "east": True}
                props = {d: ("low" if c else "none") for d, c in near.items()}
                props["up"] = (not straight) or bool(solid[above])
                props["waterlogged"] = False
            else:
                props = dict(near)
                props["waterlogged"] = False
            key = (b, tuple(sorted(props.items())))
            if key not in cache:
                cache[key] = self.sid(state(b, **props))
                if cache[key] >= n0 + len(new):
                    new.append(v[x, y, z])
            v[x, y, z] = cache[key]

    def trim(self):
        """Drop empty margins; returns the (x, y, z) offset removed."""
        occ = self.v != 0
        xs = np.nonzero(occ.any((1, 2)))[0]
        ys = np.nonzero(occ.any((0, 2)))[0]
        zs = np.nonzero(occ.any((0, 1)))[0]
        x0, y0, z0 = xs[0], 0, zs[0]
        self.v = self.v[x0:xs[-1] + 1, :ys[-1] + 1, z0:zs[-1] + 1]
        return x0, y0, z0

    def compact(self):
        """Renumber the palette to the states in use."""
        used = np.unique(self.v)
        lut = np.zeros(len(self.palette), np.uint16)
        lut[used] = np.arange(len(used))
        self.palette = [self.palette[i] for i in used]
        self.index = {s: i for i, s in enumerate(self.palette)}
        self.v = lut[self.v]

    def counts(self):
        c = np.bincount(self.v.ravel(), minlength=len(self.palette))
        return sorted(((int(c[i]), s) for i, s in enumerate(self.palette) if c[i]), reverse=True)

    def save(self, path):
        self.compact()
        if self.palette[0] != "minecraft:air":
            raise ValueError("air must stay slot 0")
        if len(self.palette) > 256:
            raise ValueError(f"{len(self.palette)} block states, the loader takes 256")
        with open(path, "wb") as f:
            f.write(sponge(self.v, self.palette))


def _payload(out, tag, value):
    if tag == 2:
        out.write(struct.pack(">h", value))
    elif tag == 3:
        out.write(struct.pack(">i", value))
    elif tag == 7:
        out.write(struct.pack(">i", len(value)) + bytes(value))
    elif tag == 8:
        raw = value.encode()
        out.write(struct.pack(">H", len(raw)) + raw)
    elif tag == 9:
        kind, items = value
        out.write(bytes([kind]) + struct.pack(">i", len(items)))
        for item in items:
            _payload(out, kind, item)
    elif tag == 10:
        for key, (t, v) in value.items():
            raw = key.encode()
            out.write(bytes([t]) + struct.pack(">H", len(raw)) + raw)
            _payload(out, t, v)
        out.write(b"\x00")
    elif tag == 11:
        out.write(struct.pack(">i", len(value)) + struct.pack(f">{len(value)}i", *value))
    else:
        raise ValueError(tag)


def _varints(ids):
    out = bytearray()
    for n in ids.tolist():
        while True:
            byte = n & 0x7F
            n >>= 7
            if n:
                out.append(byte | 0x80)
            else:
                out.append(byte)
                break
    return out


def sponge(v, palette):
    w, h, l = v.shape
    # Sponge index = (y * L + z) * W + x
    data = _varints(np.ascontiguousarray(v.transpose(1, 2, 0)).ravel())
    blocks = {
        "Palette": (10, {k: (3, i) for i, k in enumerate(palette)}),
        "Data": (7, data),
        "BlockEntities": (9, (10, [])),
    }
    schematic = {
        "Version": (3, 3),
        "DataVersion": (3, DATA_VERSION),
        "Width": (2, w),
        "Height": (2, h),
        "Length": (2, l),
        "Offset": (11, [0, 0, 0]),
        "Blocks": (10, blocks),
    }
    out = io.BytesIO()
    out.write(b"\x0a" + struct.pack(">H", 0))
    _payload(out, 10, {"Schematic": (10, schematic)})
    return gzip.compress(out.getvalue(), mtime=0)


def unmapped(palette):
    """Block names the Arnis loader would silently drop."""
    src = open(SCHEMATIC_RS).read()
    body = src[src.index("fn map_structure_block"):]
    names = set(re.findall(r'"([a-z_]+)"(?:\s*\|\s*"[a-z_]+")*\s*=>', body))
    names |= set(re.findall(r'\|\s*"([a-z_]+)"', body))
    return sorted({base(s)[10:] for s in palette if s != "minecraft:air"} - names)


def report(model, path):
    print(f"{os.path.basename(path)}: {model.v.shape[0]}x{model.v.shape[1]}x{model.v.shape[2]}, "
          f"{len(model.palette)} states, {int((model.v != 0).sum())} blocks")
    missing = unmapped(model.palette)
    if missing:
        print("  not mapped in schematic.rs:", ", ".join(missing))


# ---------------------------------------------------------------- geometry


def dist_grid(w, l, cx, cz):
    """Distance of every cell centre from (cx, cz), indexed [x, z]."""
    x = np.arange(w)[:, None] + 0.5
    z = np.arange(l)[None, :] + 0.5
    return np.hypot(x - cx, z - cz)


def erode(mask, r=1):
    out = mask.copy()
    for _ in range(r):
        m = out.copy()
        m[1:, :] &= out[:-1, :]
        m[:-1, :] &= out[1:, :]
        m[:, 1:] &= out[:, :-1]
        m[:, :-1] &= out[:, 1:]
        out = m
    return out


def dilate(mask, r=1):
    return ~erode(~mask, r)


def erode8(mask):
    out = erode(mask, 1)
    out[1:, 1:] &= mask[:-1, :-1]
    out[:-1, :-1] &= mask[1:, 1:]
    out[1:, :-1] &= mask[:-1, 1:]
    out[:-1, 1:] &= mask[1:, :-1]
    return out


def outline(mask):
    """Cells of the mask touching the outside, corners included, so the ring is closed side to side."""
    return mask & ~erode8(mask)


# ---------------------------------------------------------------- survey data


def cache_dir(argv):
    d = argv[1] if len(argv) > 1 else os.environ.get("ARNIS_SURVEY_CACHE", os.path.expanduser("~/.cache/arnis-survey"))
    os.makedirs(d, exist_ok=True)
    return d


def fetch(url, path):
    if not os.path.exists(path):
        print("downloading", url, file=sys.stderr)
        req = urllib.request.Request(url, headers={"User-Agent": "arnis-landmark-builder"})
        with urllib.request.urlopen(req, timeout=600) as r, open(path + ".part", "wb") as f:
            while chunk := r.read(1 << 20):
                f.write(chunk)
        os.replace(path + ".part", path)
    return path


class Frame:
    """Local metres round FRAME_LAT/LON: x east, y north along true north (UTM north is 1.9 deg off here)."""

    def __init__(self):
        from pyproj import CRS, Transformer
        aeqd = CRS.from_proj4(f"+proj=aeqd +lat_0={FRAME_LAT} +lon_0={FRAME_LON} +ellps=WGS84 +units=m")
        self.from_utm = Transformer.from_crs("EPSG:25832", aeqd, always_xy=True)
        self.from_ll = Transformer.from_crs("EPSG:4326", aeqd, always_xy=True)
        self.to_ll = Transformer.from_crs(aeqd, "EPSG:4326", always_xy=True)

    def utm(self, e, n):
        return self.from_utm.transform(e, n)

    def ll(self, lat, lon):
        return self.from_ll.transform(lon, lat)

    def latlon(self, x, y):
        lon, lat = self.to_ll.transform(x, y)
        return lat, lon


def laser_points(cache, frame, tiles, box, name):
    """Laser points inside box (local x0, x1, y0, y1) from the given 1 km tiles: x, y, z, class."""
    npz = os.path.join(cache, f"olympiapark_{name}_pts.npz")
    if os.path.exists(npz):
        p = np.load(npz)
        return p["x"], p["y"], p["z"], p["c"]
    import laspy
    out = []
    for te, tn in tiles:
        las = laspy.read(fetch(LASER_URL.format(te, tn), os.path.join(cache, f"{te}_{tn}.laz")))
        x, y = frame.utm(np.asarray(las.x), np.asarray(las.y))
        m = (x >= box[0]) & (x < box[1]) & (y >= box[2]) & (y < box[3])
        out.append((x[m], y[m], np.asarray(las.z)[m], np.asarray(las.classification)[m]))
    x, y, z, c = (np.concatenate([o[i] for o in out]) for i in range(4))
    np.savez(npz, x=x.astype(np.float32), y=y.astype(np.float32), z=z.astype(np.float32), c=c.astype(np.uint8))
    p = np.load(npz)
    return p["x"], p["y"], p["z"], p["c"]


def osm_area(cache, frame):
    """Ways round the Olympiapark halls as {id: (tags, local ring)}. OpenStreetMap contributors, ODbL."""
    import json
    url = "https://www.openstreetmap.org/api/0.6/map.json?bbox=11.5430,48.1695,11.5575,48.1775"
    els = json.load(open(fetch(url, os.path.join(cache, "olympiapark_osm.json"))))["elements"]
    nodes = {e["id"]: (e["lat"], e["lon"]) for e in els if e["type"] == "node"}
    ways = {}
    for e in els:
        if e["type"] == "way" and all(i in nodes for i in e["nodes"]):
            ll = np.array([nodes[i] for i in e["nodes"]])
            x, y = frame.ll(ll[:, 0], ll[:, 1])
            ways[e["id"]] = (e.get("tags", {}), np.c_[x, y])
    return ways


class Grid:
    """Model columns: cell (i, k) is local x = x0 + i + 0.5, y = y1 - k - 0.5."""

    def __init__(self, x0, y1, w, l):
        self.x0, self.y1, self.w, self.l = x0, y1, w, l

    def cell(self, x, y):
        return np.floor(np.asarray(x) - self.x0).astype(int), np.floor(self.y1 - np.asarray(y)).astype(int)

    def centre(self, i, k):
        return self.x0 + i + 0.5, self.y1 - k - 0.5

    def polygon(self, ring):
        from PIL import Image, ImageDraw
        im = Image.new("L", (self.w, self.l), 0)
        ImageDraw.Draw(im).polygon([(x - self.x0, self.y1 - y) for x, y in ring], fill=1, outline=1)
        return np.array(im, bool).T

    def stat(self, x, y, v, fn, init):
        i, k = self.cell(x, y)
        m = (i >= 0) & (i < self.w) & (k >= 0) & (k < self.l)
        a = np.full((self.w, self.l), init, np.float64)
        fn.at(a, (i[m], k[m]), v[m])
        return a


def shift(a, di, dk, fill=np.nan):
    out = np.full_like(a, fill)
    w, l = a.shape
    out[max(di, 0):w + min(di, 0), max(dk, 0):l + min(dk, 0)] = a[max(-di, 0):w + min(-di, 0), max(-dk, 0):l + min(-dk, 0)]
    return out


def window(a, r, fn):
    """fn (np.fmax/np.fmin) over the (2r+1) square round each cell."""
    out = a.copy()
    for di in range(-r, r + 1):
        for dk in range(-r, r + 1):
            if di or dk:
                out = fn(out, shift(a, di, dk))
    return out


def plus(a, fn):
    """fn over each cell and its four side neighbours: opening with it drops one-cell lines."""
    out = a.copy()
    for di, dk in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        out = fn(out, shift(a, di, dk))
    return out


def fill_nan(a, mask=None, iters=500):
    """Grow known values into unknown cells (inside mask) by neighbour means."""
    a = a.copy()
    for _ in range(iters):
        miss = np.isnan(a) if mask is None else (np.isnan(a) & mask)
        if not miss.any():
            break
        nb = np.stack([shift(a, 1, 0), shift(a, -1, 0), shift(a, 0, 1), shift(a, 0, -1)])
        cnt = np.isfinite(nb).sum(0)
        mean = np.where(cnt > 0, np.nansum(nb, 0) / np.maximum(cnt, 1), np.nan)
        a[miss] = mean[miss]
    return a


def smooth3(a):
    valid = np.isfinite(a)
    num = np.zeros_like(a); den = np.zeros_like(a)
    for di in (-1, 0, 1):
        for dk in (-1, 0, 1):
            s = shift(np.where(valid, a, 0.0), di, dk, 0.0)
            num += s
            den += shift(valid.astype(float), di, dk, 0.0)
    return np.where(valid, num / np.maximum(den, 1e-9), np.nan)


def seg_dist(p, a, b):
    a, b = np.asarray(a, float), np.asarray(b, float)
    d = b - a
    ln = np.linalg.norm(d)
    d = d / ln
    s = np.clip((p - a) @ d, 0, ln)
    return np.linalg.norm(p - a - np.outer(s, d), axis=1)


def ground_surface(g, x, y, z, c):
    """Bare-earth height per column: lowest ground return, holes grown in from round them."""
    gmin = g.stat(x[c == 2], y[c == 2], z[c == 2], np.fmin, np.nan)
    return smooth3(fill_nan(gmin))


def roof_surface(g, x, y, z, c, ground, area, masts, ties=()):
    """Top of the cable-net skin inside the mapped roof outlines: highest non-ground return, with
    masts, tie-down struts and anything one cell wide (cables, clamps) taken out. Gaps between
    neighbouring outlines are closed where the scan shows the net carrying on."""
    keep = c != 2
    p = np.c_[x, y, z].astype(float)
    for head, foot in masts:
        keep &= seg_dist(p, head, foot) > 4.5
    for a, b in ties:
        keep &= seg_dist(p, a, b) > 1.6
    top = g.stat(x[keep], y[keep], z[keep], np.fmax, np.nan)
    net = np.isfinite(top) & (top - ground > 1.5)
    closed = erode(dilate(area, 3), 3)
    area = area | (closed & net)
    top = np.where(area & net, top, np.nan)
    filled = np.where(np.isfinite(top), top, -1e4)
    opened = plus(plus(filled, np.fmin), np.fmax)
    broad = window(window(filled, 1, np.fmin), 1, np.fmax)
    # Cone tips stand at most 2.5 m above what three cells across carries, so no needle is left.
    top = np.where(np.isfinite(top) & (opened > 0), np.minimum(np.minimum(top, opened + 0.3), broad + 2.5), np.nan)
    # Near the outline, cells far below their neighbours are things under the edge (trees, lamps).
    rim = area & ~erode(area, 5)
    for _ in range(6):
        hi = plus(np.where(np.isfinite(top), top, -1e4), np.fmax)
        drop = rim & np.isfinite(top) & (hi - top > 2.5)
        if not drop.any():
            break
        top[drop] = np.nan
        area = area & ~drop
    return np.where(area, smooth3(fill_nan(top, area)), np.nan)


# ---------------------------------------------------------------- tent structure (shared palette)

SKIN = "white_stained_glass"
CABLE = "light_gray_concrete"
MAST = "iron_block"
NODE = "polished_andesite"
ANCHOR = "smooth_stone"
# Under each hall's glass wall only, so the landmark entry can mark the hall with it.
SILL = "polished_basalt"


def skin(m, g, h, z0):
    """One glass type, one block thick, closed down to the lower neighbour."""
    mask = np.isfinite(h)
    top = np.where(mask, np.round(h - z0).astype(int) - 1, -1)
    low = top.copy()
    for di, dk in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        nb = shift(np.where(mask, top, 10 ** 6).astype(float), di, dk, 10 ** 6).astype(int)
        low = np.minimum(low, nb)
    low = np.clip(np.maximum(low + 1, top - 4), 0, top)
    a = m.sid(SKIN)
    for i, k in zip(*np.nonzero(mask)):
        m.v[i, low[i, k]:top[i, k] + 1, k] = a
    return top


def voxel(g, z0, p):
    i, k = g.cell(p[0], p[1])
    return int(i), int(np.floor(p[2] - z0)), int(k)


def cylinder(m, g, z0, a, b, r, block):
    a, b = np.asarray(a, float), np.asarray(b, float)
    lo = np.minimum(a, b) - r - 1
    hi = np.maximum(a, b) + r + 1
    i0, _, k1 = voxel(g, z0, (lo[0], lo[1], lo[2]))
    i1, _, k0 = voxel(g, z0, (hi[0], hi[1], hi[2]))
    y0, y1 = int(np.floor(lo[2] - z0)), int(np.floor(hi[2] - z0))
    ii, yy, kk = np.meshgrid(np.arange(i0, i1 + 1), np.arange(y0, y1 + 1), np.arange(k0, k1 + 1), indexing="ij")
    cx, cy = g.centre(ii, kk)
    pts = np.c_[cx.ravel(), cy.ravel(), (yy + 0.5 + z0).ravel()]
    inside = seg_dist(pts, a, b) <= r
    sid = m.sid(block)
    w, h, l = m.v.shape
    for i, y, k in zip(ii.ravel()[inside], yy.ravel()[inside], kk.ravel()[inside]):
        if 0 <= i < w and 0 <= y < h and 0 <= k < l:
            m.v[i, y, k] = sid


def cable(m, g, z0, a, b, block=CABLE):
    """A face-connected line of blocks (voxel traversal), so it reads as one cable in game."""
    a, b = np.asarray(a, float), np.asarray(b, float)
    # Grid space: i east, y up, k south, cell centres at +0.5.
    pa = np.array([a[0] - g.x0, a[2] - z0, g.y1 - a[1]])
    pb = np.array([b[0] - g.x0, b[2] - z0, g.y1 - b[1]])
    d = pb - pa
    cell = np.floor(pa).astype(int)
    end = np.floor(pb).astype(int)
    step = np.sign(d).astype(int)
    with np.errstate(divide="ignore", invalid="ignore"):
        tdelta = np.where(d != 0, np.abs(1.0 / d), np.inf)
        nxt = np.where(step > 0, cell + 1 - pa, pa - cell)
        tmax = np.where(d != 0, nxt * tdelta, np.inf)
    for _ in range(int(np.abs(end - cell).sum()) + 1):
        i, y, k = int(cell[0]), int(cell[1]), int(cell[2])
        if m.get(i, y, k) == "minecraft:air":
            m.set(i, y, k, block)
        if (cell == end).all():
            break
        ax = int(np.argmin(tmax))
        cell[ax] += step[ax]
        tmax[ax] += tdelta[ax]


def mast(m, g, z0, head, foot, r=1.15):
    head, foot = np.asarray(head, float), np.asarray(foot, float)
    cylinder(m, g, z0, foot, head, r, MAST)
    i, y, k = voxel(g, z0, head)
    for di in (-1, 0, 1):
        for dk in (-1, 0, 1):
            m.set(i + di, y - 1, k + dk, NODE)
    m.set(i, y, k, MAST)
    footing(m, g, z0, foot)


def footing(m, g, z0, p, top_block=NODE):
    """A 3x3 concrete block two high, its top at p, the fitting on top in the middle."""
    i, y, k = voxel(g, z0, p)
    for di in (-1, 0, 1):
        for dk in (-1, 0, 1):
            for dy in (-2, -1):
                m.set(i + di, y + dy, k + dk, ANCHOR)
    m.set(i, y, k, top_block)


# ---------------------------------------------------------------- one tent model


def distance_from(mask, limit=120):
    """Steps (4-neighbour) from each cell to the nearest mask cell."""
    d = np.where(mask, 0.0, np.inf)
    for step in range(1, limit):
        grown = dilate(np.isfinite(d), 1) & ~np.isfinite(d)
        if not grown.any():
            break
        d[grown] = step
    return d


class Tent:
    """The survey, frame and grid one hall model is built on, and its roof structure."""

    def __init__(self, cache, name, tiles, utm_box, z0, roofs, masts, edge_anchors):
        self.frame = Frame()
        self.z0 = z0
        xs, ys = zip(*[self.utm(e, n) for e in utm_box[:2] for n in utm_box[2:]])
        x0, x1, y0, y1 = int(min(xs)) - 2, int(max(xs)) + 3, int(min(ys)) - 2, int(max(ys)) + 3
        self.g = g = Grid(x0, y1, x1 - x0, y1 - y0)
        x, y, z, c = laser_points(cache, self.frame, tiles, (x0, x1, y0, y1), name)
        self.ways = osm_area(cache, self.frame)
        self.ground = ground_surface(g, x, y, z, c)
        self.masts = {k: (np.array([*self.utm(*h[:2]), h[2]]), np.array([*self.utm(*f[:2]), f[2]]))
                      for k, (h, f) in masts.items()}
        area = np.zeros((g.w, g.l), bool)
        for wid in roofs:
            area |= g.polygon(self.ways[wid][1])
        self.roof = roof_surface(g, x, y, z, c, self.ground, area, self.masts.values())
        self.edge_anchors = edge_anchors
        ties = [t for t in self.ties() if np.all(np.isfinite(t[1]))]
        self.roof = roof_surface(g, x, y, z, c, self.ground, area, self.masts.values(), ties)
        top = int(np.ceil(max(h[2] for h, _ in self.masts.values()) - z0)) + 3
        self.m = Model(g.w, top, g.l)

    def utm(self, e, n):
        """Local frame from EPSG:25832 metres east of 689000 and north of 5338000."""
        return self.frame.utm(689000 + e, 5338000 + n)

    def at(self, grid, p, r=3):
        i, k = self.g.cell(*p)
        v = grid[np.clip(i, 0, self.g.w - 1), np.clip(k, 0, self.g.l - 1)]
        if np.isfinite(v):
            return v
        win = grid[max(i - r, 0):i + r + 1, max(k - r, 0):k + r + 1]
        return np.nanmax(win) if np.isfinite(win).any() else np.nan

    def ties(self):
        """Each edge anchor's footing top and the net point it holds down."""
        out = []
        for wid, (ae, an), (ee, en) in self.edge_anchors:
            a, e = np.array(self.utm(ae, an)), np.array(self.utm(ee, en))
            out.append((np.array([*a, self.at(self.ground, a) + 0.5]), np.array([*e, self.at(self.roof, e, 6) - 0.5])))
        return out

    def structure(self, cables, footings):
        m, g, z0 = self.m, self.g, self.z0
        skin(m, g, self.roof, z0)
        for h, f in self.masts.values():
            mast(m, g, z0, h, f)
        for key, (e, n), kind in cables:
            head = self.masts[key][0]
            p = np.array(self.utm(e, n))
            if kind == "ground":
                end = np.array([*p, self.at(self.ground, p) + 0.5])
                footing(m, g, z0, end)
            else:
                end = np.array([*p, self.at(self.roof, p, 6)])
            if np.all(np.isfinite(end)):
                cable(m, g, z0, head, end)
        for foot, net in self.ties():
            if np.all(np.isfinite(net)):
                footing(m, g, z0, foot)
                cable(m, g, z0, foot, net)
        for wid in footings:
            if wid in self.ways:
                p = self.ways[wid][1].mean(0)
                footing(m, g, z0, np.array([*p, self.at(self.ground, p) + 0.5]))

    def glass_wall(self, hall, base, lid, frame_block, glass_block):
        """A curtain wall on the outline: mullions every 3 m and transoms every 4, panes between, on a
        sill that closes the ring (the landmark's interior marker)."""
        m, g = self.m, self.g
        ring = outline(hall)
        xs, ks = np.nonzero(ring)
        cx, cy = g.centre(xs, ks)
        order = np.argsort(np.arctan2(cy - cy.mean(), cx - cx.mean()))
        fid, gid = m.sid(frame_block), m.sid(glass_block)
        for n, idx in enumerate(order):
            i, k = xs[idx], ks[idx]
            b, t = base[i, k], lid[i, k]
            m.set(i, b - 1, k, SILL)
            for yy in range(b, t + 1):
                m.v[i, yy, k] = fid if (n % 3 == 0 or (yy - b) % 4 == 3) else gid
        return ring

    def light_interior(self, hall, lid, spacing=8, lamps="sea_lantern", level=15):
        """Ceiling lamps set into the lining and invisible light blocks three above each floor,
        on a sparse grid, so the hall glows through its glass at night."""
        m, g = self.m, self.g
        inside = hall & ~outline(hall)
        light = f"light[level={level},waterlogged=false]"
        for i, k in zip(*np.nonzero(inside)):
            on = (g.x0 + i) % spacing == 0 and (self.g.y1 - k) % spacing == 0
            half = (g.x0 + i) % spacing == spacing // 2 and (self.g.y1 - k) % spacing == spacing // 2
            top = lid[i, k]
            if top <= 0 or not (on or half):
                continue
            if on:
                m.set(i, top, k, lamps)
                continue
            y = top - 1
            while y > 0 and m.get(i, y, k) == "minecraft:air":
                y -= 1
            if y + 3 < top and m.get(i, y + 3, k) == "minecraft:air":
                m.set(i, y + 3, k, light)

    def placement(self, hall, half_x, half_z):
        """Anchor on the hall's centroid; ground_y the layer at the median ground Arnis samples."""
        xs, ks = np.nonzero(hall)
        ax, az = xs.mean() + 0.5, ks.mean() + 0.5
        lat, lon = self.frame.latlon(*self.g.centre(ax - 0.5, az - 0.5))
        samples = [self.ground[int(ax + a * half_x), int(az + b * half_z)]
                   for a in np.linspace(-1, 1, 17) for b in np.linspace(-1, 1, 17)]
        med = float(np.median(samples))
        rows = np.nonzero((self.m.v != 0).any((0, 2)))[0]
        # The loader drops empty bottom layers, so layers count from the lowest filled one.
        low, top = int(rows.min()), int(rows.max())
        gy = int(round(med - self.z0)) - low
        cols = np.nonzero((self.m.v != 0).any(1))
        reach = float(np.sqrt(((cols[0] + 0.5 - ax) ** 2 + (cols[1] + 0.5 - az) ** 2).max()))
        return dict(lat=round(lat, 7), lon=round(lon, 7), anchor_x=round(float(ax), 1), anchor_z=round(float(az), 1),
                    median_ground_m=round(med, 2), ground_y=gy, top_layer=top - low, height=top - low + 1 - gy, suppress=(half_x, half_z),
                    reach_m=int(np.ceil(reach)))
