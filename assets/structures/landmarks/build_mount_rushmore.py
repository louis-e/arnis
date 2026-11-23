#!/usr/bin/env python3
"""Builds mount_rushmore.schem beside this script: the four carved heads of Mount Rushmore National
Memorial in the granite around them, the cliffs either side and the talus of carving rubble below,
as a piece of mountain that replaces the terrain under it.

    python3 assets/structures/landmarks/build_mount_rushmore.py [CACHE_DIR]

Needs numpy, laspy[lazrs] and pyproj. The shape is USGS 3DEP lidar (survey SD_BlackHills_D23, tile
13tfj230590, 271 MB, public domain; the Entwine index has no survey here), downloaded once into
CACHE_DIR ($ARNIS_LANDMARK_CACHE, else ~/.cache/arnis-landmarks); the points near the memorial and
the rock cut from them are kept there, so later runs skip the tile.

The scanner flew north-south and saw the faces from the east, 4 to 27 degrees off nadir: tops and
the fronts of the faces are measured, eye sockets and the undersides of brows, noses and chins are
in its shadow, and at 40 points a square metre the rest is noisy at the scale of a feature. So the
rock is the scan seen from above and from four directions in front of the faces; on each face the
scan gives the form at two metres and coarser, and brows, sockets, nose, lips and chin, carved
after photographs, give everything finer.

Axes: x east, y up, z south, one block per metre, north up. Layer y spans Z0 + y to Z0 + y + 1
metres (NAVD88). Model x 68, z 93 is 43.8786493 N, 103.4587754 W.
"""
import gzip
import io
import os
import struct
import sys
import urllib.request
import warnings

import numpy as np
from numpy.lib.stride_tricks import sliding_window_view as swv

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "mount_rushmore.schem")
TILE_URL = ("https://rockyweb.usgs.gov/vdelivery/Datasets/Staged/Elevation/LPC/Projects/SD_BlackHills_D23/"
            "SD_BlackHills_1_D23/LAZ/USGS_LPC_SD_BlackHills_D23_13tfj230590.laz")
TILE_BYTES = 271_436_856
DATA_VERSION = 4438

# Local frame: metres east and north of this point, NAD83(2011) taken as WGS84 like the terrain tiles.
LAT, LON = 43.8790, -103.4590
R_EARTH = 6_371_000.0
# Model box in that frame, and the half-metre grid the rock is cut on.
E0, E1, N0, N1, Z0, Z1 = -50.0, 86.0, -132.0, 54.0, 1588.0, 1750.0
RES = 0.5
# Area the model replaces: the carving with the summit rock behind it and the cliffs either side, and
# the talus below (OSM scree, way 823076723, widened by a few metres), short of the forest.
FOOTPRINT = [(-40, 40), (-10, 50), (20, 47), (38, 38), (54, 29), (64, 21), (74, 1), (82, -19), (72, -42), (64, -51),
             (56, -48), (50, -58), (44, -82), (43, -95), (30, -106), (4, -121), (-14, -128), (-20, -100),
             (-24, -70), (-38, -52), (-46, -20), (-46, 10)]
# Talus of carving rubble: OSM scree outline, way 823076723.
TALUS = [(16.1, 41.2), (33.6, 33.6), (50.6, 25.3), (59.4, 18.8), (69.2, 0.7), (78.1, -18.8), (69.3, -38.8),
         (63.1, -46.8), (55.3, -42.8), (47.8, -55.7), (40.2, -81.5), (39.7, -93.4), (28.0, -102.8), (2.4, -117.5),
         (-11.5, -124.5), (-12.3, -50.9), (-6.7, -50.6), (12.2, 26.2)]

# Carved sculpture: the OSM monument outline (way 550692883) taken west over the heads.
CARVED = [(24, 30), (6, 32), (-12, 22), (-24, 6), (-28, -20), (-24, -48), (-8, -62), (8, -62), (18, -44), (16, -12),
          (14, -2), (20, 4), (28, 6), (38, 12), (38, 26)]
CARVED_FLOOR = 1672.0


def enu(lat, lon):
    return (np.radians(lon - LON) * np.cos(np.radians(LAT)) * R_EARTH, np.radians(lat - LAT) * R_EARTH)


# ---------------------------------------------------------------- data

def cache_dir():
    d = sys.argv[1] if len(sys.argv) > 1 else os.environ.get("ARNIS_LANDMARK_CACHE") or os.path.expanduser(
        "~/.cache/arnis-landmarks")
    os.makedirs(d, exist_ok=True)
    return d


def download(url, path, size):
    if os.path.exists(path) and os.path.getsize(path) == size:
        return
    part = path + ".part"
    have = os.path.getsize(part) if os.path.exists(part) else 0
    req = urllib.request.Request(url, headers={"User-Agent": "arnis-landmarks", "Range": f"bytes={have}-"})
    print(f"downloading {url} ({size >> 20} MB; rockyweb serves about 100 kB/s to one connection)", file=sys.stderr)
    with urllib.request.urlopen(req) as r, open(part, "ab" if have else "wb") as f:
        while chunk := r.read(1 << 20):
            f.write(chunk)
    os.replace(part, path)


def points():
    """Points of the box and a margin, as e, n, z (metres) with class and return count."""
    cache = cache_dir()
    npz = os.path.join(cache, "rushmore_points.npz")
    if not os.path.exists(npz):
        import laspy
        from pyproj import Transformer
        laz = os.path.join(cache, os.path.basename(TILE_URL))
        download(TILE_URL, laz, TILE_BYTES)
        to_ll = Transformer.from_crs(6342, 6318, always_xy=True)
        keep = {k: [] for k in ("e", "n", "z", "cls", "nret")}
        with laspy.open(laz) as f:
            for p in f.chunk_iterator(2_000_000):
                lon, lat = to_ll.transform(np.asarray(p.x), np.asarray(p.y))
                e, n = enu(lat, lon)
                m = (e > E0 - 60) & (e < E1 + 60) & (n > N0 - 60) & (n < N1 + 60)
                for k, v in (("e", e), ("n", n), ("z", np.asarray(p.z)), ("cls", np.asarray(p.classification)),
                             ("nret", np.asarray(p.number_of_returns))):
                    keep[k].append(v[m])
        np.savez_compressed(npz, **{k: np.concatenate(v) for k, v in keep.items()})
    d = np.load(npz)
    return d["e"].astype(float), d["n"].astype(float), d["z"].astype(float), d["cls"], d["nret"]


def rock_points(e, n, z, cls, nret):
    """Ground class, and unclassified single returns no higher than the ground-class surface around them:
    the survey leaves trees unclassified."""
    cell = 0.5
    ix = ((e - e.min()) / cell).astype(int); iy = ((n - n.min()) / cell).astype(int)
    W, H = ix.max() + 1, iy.max() + 1
    g = cls == 2
    top = np.full(H * W, -np.inf)
    np.maximum.at(top, iy[g] * W + ix[g], z[g])
    top = swv(np.pad(top.reshape(H, W), 3, constant_values=-np.inf), (7, 7)).max(axis=(2, 3))
    veg = (cls == 1) & ((nret > 1) | (z > top[iy, ix] + 0.5))
    keep = ~veg & ~np.isin(cls, [7, 18])
    return np.stack([e[keep], n[keep], z[keep]], 1)


# ---------------------------------------------------------------- cutting the rock

def nanfilter(a, k, fn):
    w = swv(np.pad(a, k // 2, constant_values=np.nan), (k, k)).reshape(a.shape + (k * k,))
    with np.errstate(all="ignore"), warnings.catch_warnings():
        warnings.simplefilter("ignore")  # all-NaN windows are expected off the rock
        return fn(w, axis=-1)


class View:
    """Frontmost scanned rock seen along one direction, on a half-metre raster. Pixels that see far
    past their neighbours look through gaps in the sampling, so they are dropped and filled."""

    def __init__(self, pts, az, el, robust=True):
        a, b = np.radians(az), np.radians(el)
        self.d = -np.array([np.sin(a) * np.cos(b), np.cos(a) * np.cos(b), np.sin(b)])
        self.r = np.array([1.0, 0, 0]) if abs(self.d[2]) > 0.99 else np.cross(self.d, [0, 0, 1.0])
        self.r /= np.linalg.norm(self.r)
        self.u = np.cross(self.r, self.d)
        pu, pv, pd = pts @ self.r, pts @ self.u, pts @ self.d
        self.u0, self.v0 = pu.min() - 2, pv.min() - 2
        W = int((pu.max() - self.u0) / RES) + 5; H = int((pv.max() - self.v0) / RES) + 5
        iu = ((pu - self.u0) / RES).astype(int); iv = ((pv - self.v0) / RES).astype(int)
        D = np.full(H * W, np.inf); np.minimum.at(D, iv * W + iu, pd)
        D = D.reshape(H, W); D[~np.isfinite(D)] = np.nan
        if robust:
            ref = nanfilter(D, 7, lambda w, axis: np.nanpercentile(w, 30, axis=axis))
            D = np.where(D > ref + 1.5, np.nan, D)
        for _ in range(12):
            D = np.where(np.isnan(D), nanfilter(D, 3, np.nanmean), D)
        self.D = np.where(np.isnan(D), D, nanfilter(D, 3, np.nanmedian))

    def depth_at(self, q):
        """Depth of the scanned surface along this view's ray through each point of q (N, 3); NaN off the raster."""
        H, W = self.D.shape
        iu = ((q @ self.r - self.u0) / RES).astype(int); iv = ((q @ self.u - self.v0) / RES).astype(int)
        ok = (iu >= 0) & (iu < W) & (iv >= 0) & (iv < H)
        out = np.full(len(q), np.nan); out[ok] = self.D[iv[ok], iu[ok]]
        return out


def voxel_centres(shape, k):
    nx, ny, _ = shape
    E, N = np.meshgrid(E0 + (np.arange(nx) + 0.5) * RES, N0 + (np.arange(ny) + 0.5) * RES, indexing="ij")
    return np.stack([E.ravel(), N.ravel(), np.full(E.size, Z0 + (k + 0.5) * RES)], 1)


def carve(solid, view, tol=0.25):
    for k in range(solid.shape[2]):
        q = voxel_centres(solid.shape, k)
        D = view.depth_at(q)
        with np.errstate(invalid="ignore"):
            solid[:, :, k] &= ~(q @ view.d < D - tol).reshape(solid.shape[:2])


def blur(a):
    a = a.astype(np.float32)
    for ax in range(3):
        p = np.pad(a, [(1, 1) if i == ax else (0, 0) for i in range(3)], mode="edge")
        s = lambda lo, hi: tuple(slice(lo, hi) if i == ax else slice(None) for i in range(3))
        a = 0.25 * p[s(0, -2)] + 0.5 * p[s(1, -1)] + 0.25 * p[s(2, None)]
    return a


# ---------------------------------------------------------------- the heads

# Per head, measured on the scan: c a point on the face (e, n, z), az the way it faces, uc its middle
# line (u to the right of c), eo the eye offset either side, and the heights above c of the eyes, nose
# tip, mouth and chin (from the unseen sockets and nostril shadows); half the cheek half-width, nose how
# far its tip stands out, oval the half-extents of the face. Metres.
FACES = {
    "Washington": dict(c=(-4.2, -33.3, 1712.0), az=108, uc=-0.4, eo=2.0, eye=6.0, tip=2.2, mouth=0.1, chin=-2.8,
                       half=5.6, nose=2.4, oval=(6.5, 9.5)),
    "Jefferson": dict(c=(-9.2, -12.0, 1710.0), az=105, uc=-0.45, eo=2.4, eye=7.2, tip=3.4, mouth=1.0, chin=-1.8,
                      half=4.8, nose=2.2, oval=(6.0, 9.0)),
    "Roosevelt": dict(c=(-12.3, 4.8, 1705.0), az=105, uc=-1.2, eo=2.2, eye=5.4, tip=1.6, mouth=-0.8, chin=-3.4,
                      half=4.6, nose=2.2, moustache=True, oval=(5.8, 8.5)),
    "Lincoln": dict(c=(7.1, 14.4, 1708.0), az=110, uc=-1.9, eo=2.2, eye=2.6, tip=-1.2, mouth=-3.3, chin=-6.2,
                    half=4.8, nose=2.2, beard=True, oval=(6.2, 9.5)),
}


def bump(x, v, x0, v0, hx, hv, sharp=2.0):
    """1 inside an ellipse with a steep rim, 0 outside."""
    t = ((x - x0) / hx) ** 2 + ((v - v0) / hv) ** 2
    return np.clip((1 - t) * sharp, 0, 1)


def face_relief(f, u, v):
    """How far the carved face stands out at (u, v), in metres: brows, sockets and eyes, nose, lips, chin."""
    x = u - f["uc"]
    ve, vt, vm, vc, hw, eo = f["eye"], f["tip"], f["mouth"], f["chin"], f["half"], f["eo"]
    w = -0.6 * (x / hw) ** 2
    jaw = hw * np.clip(0.55 + 0.45 * (v - vc) / (ve - vc), 0.5, 1.0)
    w -= np.clip(np.abs(x) - jaw, 0, None) * 1.2
    brow = ve + 1.15
    w += 1.0 * (np.abs(v - brow) < 0.5) * (np.abs(x) < eo + 1.9)
    for sx in (-1, 1):
        w -= 2.2 * bump(x, v, sx * eo, ve, 1.7, 1.0)
        w += 1.0 * bump(x, v, sx * eo, ve - 0.15, 0.8, 0.5)
    mw = 2.7
    if f.get("moustache"):
        w += 1.4 * bump(x, v, 0, vm + 0.55, 3.1, 0.8, 3)
        w -= 0.8 * bump(x, v, 0, vm - 0.45, 1.8, 0.3, 3)
    else:
        w += 0.6 * bump(x, v, 0, vm + 0.5, mw, 0.4, 3)
        w -= 1.0 * bump(x, v, 0, vm, mw * 0.95, 0.25, 4)
        w += 0.5 * bump(x, v, 0, vm - 0.5, mw * 0.8, 0.35, 3)
    w += 1.0 * bump(x, v, 0, vc + 0.9, 2.3, 1.2)
    if f.get("beard"):
        w += 0.7 * ((v < vm - 0.4) & (v > vc - 0.8) & (np.abs(x) < jaw + 0.6))
    w -= np.clip(vc - 0.2 - v, 0, None) * 2.5
    # nose: a ridge from between the brows to the tip, wider at its base, nostril wings either side
    top = ve + 0.6
    t = np.clip((top - v) / (top - vt), 0, 1)
    wide = 0.5 + 0.6 * t
    nose = f["nose"] * (0.35 + 0.65 * t) * np.clip((1 - (x / wide) ** 2) * 3, 0, 1) * (v <= top) * (v >= vt - 0.3)
    for sx in (-1, 1):
        nose = np.maximum(nose, 1.2 * bump(x, v, sx * 1.0, vt + 0.2, 0.6, 0.5))
    return np.where(nose > 0, np.maximum(w, nose), w)


def gauss(a, sigma):
    """Gaussian blur of a 2D array (edges held), sigma in cells."""
    k = int(3 * sigma) + 1
    x = np.arange(-k, k + 1); g = np.exp(-x ** 2 / (2 * sigma ** 2)); g /= g.sum()
    p = np.pad(a, k, mode="edge")
    p = np.apply_along_axis(lambda r: np.convolve(r, g, mode="valid"), 1, p)
    return np.apply_along_axis(lambda r: np.convolve(r, g, mode="valid"), 0, p)


def head_frame(f):
    a = np.radians(f["az"])
    d = -np.array([np.sin(a), np.cos(a), 0.0])
    r = np.cross(d, [0, 0, 1.0]); r /= np.linalg.norm(r)
    return np.array(f["c"], float), d, r


def carve_heads(solid, pts):
    """Inside each face's oval the scan's coarse form takes the fine relief of the sculpted face, fading to
    the plain scan at the rim; rock standing in front of the face (a neighbouring head) is left alone."""
    for name, f in FACES.items():
        c, d, r = head_frame(f)
        ou, ov = f["oval"]
        cu, cv = f["uc"], (f["eye"] + f["chin"]) / 2 + 0.8
        u0, u1, v0, v1 = cu - ou - 1, cu + ou + 1, cv - ov - 1, cv + ov + 1
        rel = pts - c
        pu, pv, pd = rel @ r, rel[:, 2], rel @ d
        m = (pu > u0) & (pu < u1) & (pv > v0) & (pv < v1) & (pd > -15) & (pd < 25)
        W = int((u1 - u0) / RES); H = int((v1 - v0) / RES)
        iu = ((pu[m] - u0) / RES).astype(int); iv = ((pv[m] - v0) / RES).astype(int)
        ok = (iu >= 0) & (iu < W) & (iv >= 0) & (iv < H)
        D = np.full(H * W, np.inf); np.minimum.at(D, iv[ok] * W + iu[ok], pd[m][ok])
        D = D.reshape(H, W); D[~np.isfinite(D)] = np.nan
        ref = nanfilter(D, 7, lambda w, axis: np.nanpercentile(w, 30, axis=axis))
        D = np.where(D > ref + 1.5, np.nan, D)
        for _ in range(16):
            D = np.where(np.isnan(D), nanfilter(D, 3, np.nanmean), D)
        D = nanfilter(D, 5, np.nanmedian)
        uu, vv = np.meshgrid(u0 + (np.arange(W) + 0.5) * RES, v0 + (np.arange(H) + 0.5) * RES)
        rad = np.sqrt(((uu - cu) / ou) ** 2 + ((vv - cv) / ov) ** 2)
        alpha = np.clip((1.0 - rad) / 0.25, 0, 1)
        # The scan gives the form at two metres and coarser, the sculpted face everything finer: the scan
        # is noisy there and blind under brows, nose and chin.
        form = gauss(nanfilter(D, 7, np.nanmedian), 2.0 / RES)
        alpha = np.where(D < form - 2.0, 0.0, alpha)
        w = face_relief(f, uu, vv)
        fine = w - gauss(w, 2.0 / RES)
        target = alpha * (form - fine) + (1 - alpha) * D
        i0 = max(int((c[0] - 22 - E0) / RES), 0); i1 = min(int((c[0] + 22 - E0) / RES), solid.shape[0])
        j0 = max(int((c[1] - 22 - N0) / RES), 0); j1 = min(int((c[1] + 22 - N0) / RES), solid.shape[1])
        k0 = max(int((c[2] + v0 - Z0) / RES), 0); k1 = min(int((c[2] + v1 - Z0) / RES) + 1, solid.shape[2])
        E, N, Zc = np.meshgrid(E0 + (np.arange(i0, i1) + 0.5) * RES, N0 + (np.arange(j0, j1) + 0.5) * RES,
                               Z0 + (np.arange(k0, k1) + 0.5) * RES, indexing="ij")
        q = np.stack([E, N, Zc], -1) - c
        qu, qv, qd = q @ r, q[..., 2], q @ d
        iu = ((qu - u0) / RES).astype(int); iv = ((qv - v0) / RES).astype(int)
        ok = (iu >= 0) & (iu < W) & (iv >= 0) & (iv < H)
        tgt = np.full(qd.shape, np.nan); tgt[ok] = target[iv[ok], iu[ok]]
        a3 = np.zeros(qd.shape); a3[ok] = alpha[iv[ok], iu[ok]]
        with np.errstate(invalid="ignore"):
            near = ok & (a3 > 0) & (qd > tgt - 4) & (qd < tgt + 4)
            part = solid[i0:i1, j0:j1, k0:k1]
            part[near] = qd[near] >= tgt[near] - 0.25
        print(f"  {name}: shaped {int(near.sum())} half-metre cells", file=sys.stderr)


# ---------------------------------------------------------------- footprint

def inside(poly, x, y):
    c = np.zeros(x.shape, bool)
    for (x1, y1), (x2, y2) in zip(poly, poly[1:] + poly[:1]):
        with np.errstate(divide="ignore", invalid="ignore"):
            c ^= ((y1 > y) != (y2 > y)) & (x < (x2 - x1) * (y - y1) / (y2 - y1) + x1)
    return c


# ---------------------------------------------------------------- blocks

def bit(sx, sy, sz):
    return 1 << (sx + 2 * sz + 4 * sy)


def shape_table():
    allc = [(0, 0), (1, 0), (0, 1), (1, 1)]
    side = {"north": [(0, 0), (1, 0)], "south": [(0, 1), (1, 1)], "west": [(0, 0), (0, 1)], "east": [(1, 0), (1, 1)]}
    left = {"north": "west", "south": "east", "east": "north", "west": "south"}
    right = {v: k for k, v in left.items()}
    opp = {"north": "south", "south": "north", "east": "west", "west": "east"}
    q = lambda a, b: [c for c in side[a] if c in side[b]]
    lay = lambda sy, cells: sum(bit(sx, sy, sz) for sx, sz in cells)
    shapes = [(255, "full", None), (lay(0, allc), "slab", "type=bottom"), (lay(1, allc), "slab", "type=top")]
    for half, by, sy in (("bottom", 0, 1), ("top", 1, 0)):
        for f in ("north", "south", "east", "west"):
            for name, cells in (("straight", side[f]), ("outer_left", q(f, left[f])), ("outer_right", q(f, right[f])),
                                ("inner_left", side[f] + q(opp[f], left[f])), ("inner_right", side[f] + q(opp[f], right[f]))):
                shapes.append((lay(by, allc) | lay(sy, cells), "stairs", f"facing={f},half={half},shape={name}"))
    masks = np.array([s[0] for s in shapes])
    pop = np.array([bin(i).count("1") for i in range(256)])
    # Stairs only where they clearly fit: on a 1 m grid they mostly read as noise.
    cost = np.array([{"full": 0.0, "slab": 0.3, "stairs": 9.0}[s[1]] for s in shapes])
    best = np.zeros(256, np.int32)
    for p in range(256):
        err = pop[np.bitwise_xor(masks, p)] + cost
        best[p] = int(np.argmin(err)) if pop[p] > 2 else -1
        if pop[p] >= 7:
            best[p] = 0
    return shapes, best


def write_sponge(path, grid, names):
    w, h, l = grid.shape
    data = bytearray()
    for v in grid.transpose(1, 2, 0).reshape(-1).tolist():
        while v >= 0x80:
            data.append((v & 0x7F) | 0x80); v >>= 7
        data.append(v)

    def payload(out, tag, value):
        if tag == 2:
            out.write(struct.pack(">h", value))
        elif tag == 3:
            out.write(struct.pack(">i", value))
        elif tag == 7:
            out.write(struct.pack(">i", len(value)) + bytes(value))
        elif tag == 9:
            out.write(bytes([value[0]]) + struct.pack(">i", len(value[1])))
        elif tag == 10:
            for key, (t, v) in value.items():
                raw = key.encode(); out.write(bytes([t]) + struct.pack(">H", len(raw)) + raw); payload(out, t, v)
            out.write(b"\x00")
        elif tag == 11:
            out.write(struct.pack(">i", len(value)) + struct.pack(f">{len(value)}i", *value))

    blocks = {"Palette": (10, {k: (3, i) for i, k in enumerate(names)}), "Data": (7, data), "BlockEntities": (9, (10, []))}
    schem = {"Version": (3, 3), "DataVersion": (3, DATA_VERSION), "Width": (2, w), "Height": (2, h), "Length": (2, l),
             "Offset": (11, [0, 0, 0]), "Blocks": (10, blocks)}
    out = io.BytesIO(); out.write(b"\x0a" + struct.pack(">H", 0)); payload(out, 10, {"Schematic": (10, schem)})
    with open(path, "wb") as f:
        f.write(gzip.compress(out.getvalue(), mtime=0))


def hash01(*a):
    h = np.uint64(0x9E3779B97F4A7C15)
    for v in a:
        h = (h ^ (np.asarray(v).astype(np.int64).astype(np.uint64) + np.uint64(0x632BE59BD9B4E019))) * np.uint64(0xBF58476D1CE4E5B9)
        h ^= h >> np.uint64(29)
    return (h & np.uint64(0xFFFFFF)).astype(np.float64) / float(0xFFFFFF)


def smooth_noise(x, y, cell, salt):
    """Value noise over a 2D lattice, 0..1."""
    gx, gy = np.floor(x / cell), np.floor(y / cell)
    fx, fy = x / cell - gx, y / cell - gy
    fx, fy = fx * fx * (3 - 2 * fx), fy * fy * (3 - 2 * fy)
    v = lambda i, j: hash01(gx + i, gy + j, salt)
    return (v(0, 0) * (1 - fx) + v(1, 0) * fx) * (1 - fy) + (v(0, 1) * (1 - fx) + v(1, 1) * fx) * fy


def build_blocks(solid):
    nx2, ny2, nz2 = solid.shape
    X, Y, Z = nx2 // 2, nz2 // 2, ny2 // 2
    sub = solid[:X * 2, ::-1, :Y * 2].transpose(0, 2, 1)[:, :, :Z * 2].reshape(X, 2, Y, 2, Z, 2)
    pat = np.zeros((X, Y, Z), np.int32)
    for sx in (0, 1):
        for sy in (0, 1):
            for sz in (0, 1):
                pat |= sub[:, sx, :, sy, :, sz].astype(np.int32) << (sx + 2 * sz + 4 * sy)
    shapes, best = shape_table()
    kind = best[pat]
    ce = E0 + np.arange(X) + 0.5
    cn = N1 - (np.arange(Z) + 0.5)
    CE, CN = np.meshgrid(ce, cn, indexing="ij")
    foot = inside(FOOTPRINT, CE, CN)
    occ = (kind >= 0) & foot[:, None, :]
    ys = np.arange(Y)[None, :, None]
    # Each column keeps a shell of rock reaching three blocks under the lowest surface within three
    # blocks of it, so it always reaches into the terrain it replaces, even on a cliff.
    surface = np.where(foot, np.argmax(~occ, axis=1), 10_000).astype(float)
    low = swv(np.pad(surface, 3, mode="edge"), (7, 7)).min(axis=(2, 3))
    bottom = np.maximum(low - 3, 0).astype(int)
    occ &= ys >= bottom[:, None, :]
    # Drop rock not joined to the ground through rock.
    grounded = occ & (ys == bottom[:, None, :])
    while True:
        g = grounded.copy()
        g[1:] |= grounded[:-1]; g[:-1] |= grounded[1:]
        g[:, 1:] |= grounded[:, :-1]; g[:, :-1] |= grounded[:, 1:]
        g[:, :, 1:] |= grounded[:, :, :-1]; g[:, :, :-1] |= grounded[:, :, 1:]
        g &= occ
        if g.sum() == grounded.sum():
            break
        grounded = g
    print(f"  dropped {int(occ.sum() - grounded.sum())} loose blocks", file=sys.stderr)
    occ = grounded
    names, grid = materials(occ, kind, shapes, CE, CN, foot)
    floodlights(occ, grid, names)
    write_sponge(OUT, grid, names)
    top = int(np.nonzero(occ.any(axis=(0, 2)))[0].max())
    print(f"wrote {OUT}: {X}x{Y}x{Z}, {int(occ.sum())} blocks, {len(names)} states, top layer {top}", file=sys.stderr)
    print("  " + ", ".join(names[1:]), file=sys.stderr)
    return occ, grid, names


STAIRS = {"polished_diorite": "polished_diorite_stairs", "diorite": "diorite_stairs"}
SLABS = {"stone": "stone_slab", "andesite": "andesite_slab", "diorite": "polished_diorite_slab",
         "polished_diorite": "polished_diorite_slab", "cobblestone": "cobblestone_slab"}


# Where the night floodlights fall on each head, in its frame (u right of c, v up from c), metres.
LIT = [(-3.0, 1.0), (3.0, 1.0), (0.0, -4.0)]


def floodlights(occ, grid, names):
    """Invisible light blocks a few metres out from each face, as the floodlights from below light
    them; only into open air in front of the rock."""
    names.append("minecraft:light[level=15]")
    light = len(names) - 1
    X, Y, Z = occ.shape
    cell = lambda p: (int(np.floor(p[0] - E0)), int(np.floor(p[2] - Z0)), int(np.floor(N1 - p[1])))
    inside_box = lambda x, y, z: 0 <= x < X and 0 <= y < Y and 0 <= z < Z
    placed = 0
    for name, f in FACES.items():
        c, d, r = head_frame(f)
        for du, dv in LIT:
            p = c + r * (f["uc"] + du) + np.array([0, 0, (f["eye"] + f["chin"]) / 2 + dv]) + d * 10
            rock = False
            for k in range(80):
                q = p - d * k * 0.5
                x, y, z = cell(q)
                if not inside_box(x, y, z):
                    break
                rock |= occ[x, y, z]
                if rock and not occ[x, y, z]:
                    x, y, z = cell(q - d * 3)
                    x2, y2, z2 = cell(q - d * 4)
                    if inside_box(x, y, z) and inside_box(x2, y2, z2) and not occ[x, y, z] and not occ[x2, y2, z2]:
                        grid[x, y, z] = light
                        placed += 1
                    else:
                        print(f"  no room for a light at {name} {du:+} {dv:+}", file=sys.stderr)
                    break
    print(f"  {placed} floodlight cells", file=sys.stderr)


def materials(occ, kind, shapes, CE, CN, foot):
    """Pale carved granite on the sculpture, pale rubble on the talus, weathered grey granite with
    darker run-off streaks elsewhere; stone inside."""
    X, Y, Z = occ.shape
    air = np.ones((X + 2, Y + 2, Z + 2), bool); air[1:-1, 1:-1, 1:-1] = ~occ
    exposed = occ & (air[:-2, 1:-1, 1:-1] | air[2:, 1:-1, 1:-1] | air[1:-1, :-2, 1:-1] | air[1:-1, 2:, 1:-1]
                     | air[1:-1, 1:-1, :-2] | air[1:-1, 1:-1, 2:])
    elev = Z0 + np.arange(Y)[None, :, None] + 0.5
    carved = inside(CARVED, CE, CN)[:, None, :] & (elev > CARVED_FLOOR)
    top = Y - 1 - np.argmax(occ[:, ::-1, :], axis=1)
    gy, gx = np.gradient(top.astype(float))
    talus = (inside(TALUS, CE, CN) & (np.hypot(gx, gy) < 1.2))[:, None, :] & ~carved
    names = ["minecraft:air", "minecraft:stone"]
    index = {n: i for i, n in enumerate(names)}

    def idx(name):
        if name not in index:
            index[name] = len(names); names.append(name)
        return index[name]

    grid = np.zeros(occ.shape, np.int32)
    grid[occ] = 1
    E3 = np.broadcast_to(CE[:, None, :], occ.shape); N3 = np.broadcast_to(CN[:, None, :], occ.shape)
    H3 = np.broadcast_to(np.arange(Y)[None, :, None], occ.shape)
    xs, ys, zs = np.nonzero(exposed)
    e, n, h = E3[xs, ys, zs], N3[xs, ys, zs], H3[xs, ys, zs]
    rv = hash01(np.floor(e), h, np.floor(n), 11)
    head = np.zeros(len(e), bool)
    for f in FACES.values():
        c, d, r = head_frame(f)
        q = np.stack([e, n, Z0 + h + 0.5], 1) - c
        u, v, w = q @ r, q[:, 2], -(q @ d)
        cv = (f["eye"] + f["chin"]) / 2 + 0.8
        head |= (((u - f["uc"]) / (f["oval"][0] * 1.15)) ** 2 + ((v - cv - 2) / (f["oval"][1] * 1.3)) ** 2 < 1) & (abs(w) < 14)
    streak = smooth_noise(e + n * 0.3, n - e * 0.3, 2.5, 5) + 0.25 * smooth_noise(e, h, 7.0, 6)
    patch = smooth_noise(e + h * 0.2, n, 6.0, 7)
    for x, y, z, r, st, pa, hd in zip(xs, ys, zs, rv, streak, patch, head):
        form = shapes[kind[x, y, z]][1]
        if form == "stairs" and not carved[x, y, z]:
            form = "full"
        if carved[x, y, z] and hd:
            base = "calcite"
        elif carved[x, y, z]:
            base = "diorite" if pa > 0.72 else "polished_diorite"
        elif talus[x, y, z]:
            base = "andesite" if r < 0.45 else "diorite" if r < 0.8 else "polished_andesite"
            form = "full"
        else:
            base = ("tuff" if st > 0.84 else "cobblestone" if st > 0.77 else "andesite" if pa > 0.62
                    else "stone")
        if form == "slab" and base in SLABS:
            half = shapes[kind[x, y, z]][2]
            grid[x, y, z] = idx(f"minecraft:{SLABS[base]}[{half},waterlogged=false]")
        elif form == "stairs" and base in STAIRS:
            grid[x, y, z] = idx(f"minecraft:{STAIRS[base]}[{shapes[kind[x, y, z]][2]},waterlogged=false]")
        else:
            grid[x, y, z] = idx(f"minecraft:{base}")
    return names, grid


def smooth_faces(solid, passes=8):
    """Round off the scan noise on the carved sculpture; the natural rock keeps its roughness."""
    i0, i1 = max(int((-45 - E0) / RES), 0), int((45 - E0) / RES)
    j0, j1 = int((-70 - N0) / RES), int((45 - N0) / RES)
    k0, k1 = int((CARVED_FLOOR - 6 - Z0) / RES), solid.shape[2]
    part = solid[i0:i1, j0:j1, k0:k1]
    soft = part.astype(np.float32)
    for _ in range(passes):
        soft = blur(soft)
    E, N = np.meshgrid(E0 + (np.arange(i0, i1) + 0.5) * RES, N0 + (np.arange(j0, j1) + 0.5) * RES, indexing="ij")
    zone = inside(CARVED, E, N)[:, :, None] & (Z0 + (np.arange(k0, k1) + 0.5) * RES > CARVED_FLOOR - 4)[None, None, :]
    solid[i0:i1, j0:j1, k0:k1] = np.where(zone, soft > 0.5, part)
    return solid


def envelope(pts):
    """Rock cut from the scan seen from above and from the front, cached: it only depends on the scan."""
    path = os.path.join(cache_dir(), f"rushmore_envelope_{E0:g}_{E1:g}_{N0:g}_{N1:g}_{Z0:g}_{Z1:g}.npy")
    if os.path.exists(path):
        return np.load(path)
    shape = (int((E1 - E0) / RES), int((N1 - N0) / RES), int((Z1 - Z0) / RES))
    solid = np.ones(shape, bool)
    carve(solid, View(pts, 0, 90, robust=False))
    for az, el in ((100, 0), (120, 0), (140, 0), (120, 25)):
        carve(solid, View(pts, az, el))
    np.save(path, solid)
    return solid


def main():
    e, n, z, cls, nret = points()
    pts = rock_points(e, n, z, cls, nret)
    box = (pts[:, 0] > E0 - 40) & (pts[:, 0] < E1 + 40) & (pts[:, 1] > N0 - 40) & (pts[:, 1] < N1 + 40)
    pts = pts[box]
    print("cutting the rock", file=sys.stderr)
    solid = smooth_faces(envelope(pts))
    carve_heads(solid, pts)
    build_blocks(solid)


if __name__ == "__main__":
    main()
