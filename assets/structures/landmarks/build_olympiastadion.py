#!/usr/bin/env python3
"""Builds olympiastadion_munich.schem, the Olympiastadion München landmark, beside this script.

    python3 build_olympiastadion.py [--cache DIR] [--out PATH]

Shape comes from the Bavarian laser scan (tile 689_5338, about 20 points per m2), downloaded once
into the cache dir (--cache, else $ARNIS_SURVEY_CACHE, else ~/.cache/arnis-survey): pitch level,
moat, stand surface and rim, the control booth and the cable-net roof surface are read from it,
the roof clipped to its OSM outline. Masts, guy anchors, floodlight towers, aisles, vomitories and
tunnels were measured from the same points and photos and are listed as constants below.
--preview also writes the model set into the survey ground, for renders.

Data: Bayerische Vermessungsverwaltung - www.geodaten.bayern.de, CC BY 4.0;
roof outline (way 25001469) (c) OpenStreetMap contributors, ODbL.
Deps: numpy, laspy[lazrs], pyproj.

Model frame: north-up at 1 block per metre in a local azimuthal frame (true north, not the
UTM grid, which is turned 1.9 degrees here). x east, y up, z south. The pitch surface is the top
of layer PITCH; LAT/LON lands on block (ANCHOR_X, ANCHOR_Z).
"""
import argparse
import gzip
import io
import math
import os
import struct
import sys
import urllib.request

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
TILE = (689, 5338)
LASER_URL = "https://geodaten.bayern.de/odd_data/laser/{}_{}.laz"
LAT, LON = 48.1731023, 11.5464961
XMIN, XMAX, YMIN, YMAX = -196, 150, -206, 206
PITCH = 4
W, L = XMAX - XMIN, YMAX - YMIN
ANCHOR_X, ANCHOR_Z = -XMIN, YMAX
H = 90
ROOF_WAY = 25001469


# ---------------------------------------------------------------- data

def fetch(cache):
    os.makedirs(cache, exist_ok=True)
    path = os.path.join(cache, "%d_%d.laz" % TILE)
    if not os.path.exists(path):
        print("downloading", LASER_URL.format(*TILE), file=sys.stderr)
        urllib.request.urlretrieve(LASER_URL.format(*TILE), path + ".part")
        os.replace(path + ".part", path)
    return path


def load_points(cache):
    """Points around the stadium in the local frame: x east, y north, z, class."""
    npy = os.path.join(cache, "olympiastadion_pts.npy")
    if os.path.exists(npy):
        return np.load(npy)
    import laspy
    from pyproj import CRS, Transformer
    aeqd = CRS.from_proj4(f"+proj=aeqd +lat_0={LAT} +lon_0={LON} +ellps=WGS84 +units=m")
    t = Transformer.from_crs("EPSG:25832", aeqd, always_xy=True)
    keep = []
    with laspy.open(fetch(cache)) as f:
        for p in f.chunk_iterator(4_000_000):
            x, y = np.asarray(p.x), np.asarray(p.y)
            m = (x > 689080) & (x < 689560) & (y > 5338430) & (y < 5338920)
            lx, ly = t.transform(x[m], y[m])
            keep.append(np.stack([lx, ly, np.asarray(p.z)[m], np.asarray(p.classification)[m]], 1).astype(np.float32))
    a = np.concatenate(keep)
    np.save(npy, a)
    return a


def cell_stats(x, y, z, qs):
    """Per model column percentiles of z (NaN where empty) and counts."""
    ix = np.floor(x - XMIN).astype(np.int64)
    iz = np.floor(YMAX - y).astype(np.int64)
    ok = (ix >= 0) & (ix < W) & (iz >= 0) & (iz < L)
    cell = ix[ok] * L + iz[ok]
    zz = z[ok]
    order = np.lexsort((zz, cell))
    cell, zz = cell[order], zz[order]
    n = np.bincount(cell, minlength=W * L)
    start = np.concatenate([[0], np.cumsum(n)[:-1]])
    out = []
    for q in qs:
        v = np.full(W * L, np.nan, np.float32)
        has = n > 0
        v[has] = zz[start[has] + np.floor(q * (n[has] - 1)).astype(np.int64)]
        out.append(v.reshape(W, L))
    return out, n.reshape(W, L)


def win(a, r, fn):
    """fn over a (2r+1) square window, NaN-aware, edges padded."""
    p = np.pad(a, r, mode="edge")
    v = np.lib.stride_tricks.sliding_window_view(p, (2 * r + 1, 2 * r + 1))
    return fn(v.reshape(a.shape + (-1,)), axis=-1)


def fill_nan(a, iters=400):
    a = a.copy()
    for _ in range(iters):
        bad = np.isnan(a)
        if not bad.any():
            break
        p = np.pad(a, 1, mode="edge")
        nb = np.stack([p[:-2, 1:-1], p[2:, 1:-1], p[1:-1, :-2], p[1:-1, 2:]])
        with np.errstate(all='ignore'):
            m = np.nanmean(nb, axis=0)
        a[bad] = m[bad]
    return np.nan_to_num(a, nan=0.0)


def box(a, r):
    p = np.pad(a, r, mode="edge")
    c = np.cumsum(np.cumsum(np.pad(p, ((1, 0), (1, 0))), 0), 1)
    k = 2 * r + 1
    return (c[k:, k:] - c[:-k, k:] - c[k:, :-k] + c[:-k, :-k]) / (k * k)


def components(mask, seed):
    """4-connected component of mask containing seed (x, z)."""
    out = np.zeros_like(mask)
    st = [seed]
    while st:
        x, z = st.pop()
        if x < 0 or z < 0 or x >= mask.shape[0] or z >= mask.shape[1] or out[x, z] or not mask[x, z]:
            continue
        out[x, z] = True
        st += [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]
    return out


def enclosed(mask):
    """mask plus the holes it encloses."""
    outside = components(~np.pad(mask, 1), (0, 0))[1:-1, 1:-1]
    return ~outside


def dilate(m, r):
    return win(m.astype(np.uint8), r, np.max) > 0


def erode(m, r):
    return win(m.astype(np.uint8), r, np.min) > 0


def osm_mask(cache, way):
    """Columns inside an OSM way (the roof outline), fetched once into the cache."""
    import json
    path = os.path.join(cache, "osm_way_%d.json" % way)
    if not os.path.exists(path):
        req = urllib.request.Request("https://www.openstreetmap.org/api/0.6/way/%d/full.json" % way,
                                     headers={"User-Agent": "arnis-landmark-builder"})
        open(path, "wb").write(urllib.request.urlopen(req).read())
    from pyproj import CRS, Transformer
    aeqd = CRS.from_proj4(f"+proj=aeqd +lat_0={LAT} +lon_0={LON} +ellps=WGS84 +units=m")
    t = Transformer.from_crs("EPSG:4326", aeqd, always_xy=True)
    els = json.load(open(path))["elements"]
    nodes = {e["id"]: e for e in els if e["type"] == "node"}
    w = next(e for e in els if e["type"] == "way")
    poly = np.array([t.transform(nodes[n]["lon"], nodes[n]["lat"]) for n in w["nodes"]])
    X, Y = col_xy()
    inside = np.zeros(X.shape, bool)
    for (x1, y1), (x2, y2) in zip(poly, np.roll(poly, -1, 0)):
        if y1 == y2:
            continue
        inside ^= ((y1 > Y) != (y2 > Y)) & (X < x1 + (Y - y1) * (x2 - x1) / (y2 - y1))
    return inside


def col_xy():
    xs = XMIN + np.arange(W) + 0.5
    ys = YMAX - np.arange(L) - 0.5
    return np.meshgrid(xs, ys, indexing="ij")


class Survey:
    """Surfaces in model columns [x, z], heights over the pitch in metres."""

    def __init__(self, cache):
        a = load_points(cache)
        x, y, z, c = a[:, 0].astype(np.float64), a[:, 1].astype(np.float64), a[:, 2].astype(np.float64), a[:, 3]
        centre = np.hypot(x - EC[0], y - EC[1]) < 25
        self.pitch_z = float(np.median(z[centre & (c == 2)]))
        z = z - self.pitch_z
        gr = (c == 2) | (c == 6)
        (lo, p25), _ = cell_stats(x[gr], y[gr], z[gr], (0.0, 0.25))
        self.gmin = lo
        g2 = c == 2
        (d50,), _ = cell_stats(x[g2], y[g2], z[g2], (0.5,))
        self.dtm = box(fill_nan(d50), 6)
        med = win(p25, 2, np.nanmedian)
        self.ground = fill_nan(np.where(np.isnan(p25), med, np.where(np.abs(p25 - med) > 1.5, med, p25)))
        o = c == 20
        (r10, r20, r50, r90), n20 = cell_stats(x[o], y[o], z[o], (0.1, 0.2, 0.5, 0.9))
        membrane = (n20 >= 3) & (r90 - r10 < 1.0) & (r50 - self.ground > 3.0)
        closed = erode(dilate(membrane, 2), 2)
        cx, cz = np.nonzero(closed)
        k = np.argmin(np.hypot(cx - (-100 - XMIN), cz - YMAX))
        roof = components(closed, (int(cx[k]), int(cz[k])))
        roof = erode(dilate(enclosed(roof), 2), 2)
        roof &= dilate(osm_mask(cache, ROOF_WAY), 2)
        roof = dilate(erode(roof, 1), 1) & roof
        # the lower points of a cell: cables over the net only add points above it
        r = np.where(roof & (n20 >= 3) & (r50 - r10 < 0.6), r20, np.nan)
        # cable streaks stand as ridges over the net, lamps and rigging hang as pits under it
        r[np.abs(r - win(r, 4, np.nanmedian)) > 2.5] = np.nan
        r = box(win(fill_nan(r), 1, np.median), 1)
        # things under the inner edge (cranes, the camera rig) are no part of the net
        _, rho = bowl(*col_xy())
        roof &= ~((rho < 30) & (r < self.ground + 6)) & (rho > 1)
        self.roof_mask = roof
        self.roof = np.where(roof, r, np.nan)


# ---------------------------------------------------------------- bowl frame

# Moat ellipse fitted to the laser points in the 4 m trench round the field.
EC = (4.1575, 3.0175)
EA, EB = 93.6467, 63.6706
ETH = math.radians(102.3251)
EU = (math.cos(ETH), math.sin(ETH))

_TT = np.linspace(0, 2 * np.pi, 40001)
_DS = np.hypot(-EA * np.sin(_TT), EB * np.cos(_TT))
_S = np.concatenate([[0], np.cumsum((_DS[1:] + _DS[:-1]) / 2 * (_TT[1] - _TT[0]))])
PERIM = float(_S[-1])


def to_uv(x, y):
    """u along the long axis (NNW), v to its left (WSW)."""
    dx, dy = x - EC[0], y - EC[1]
    return dx * EU[0] + dy * EU[1], -dx * EU[1] + dy * EU[0]


def from_uv(u, v):
    return EC[0] + u * EU[0] - v * EU[1], EC[1] + u * EU[1] + v * EU[0]


def bowl(x, y):
    """Perimeter position s along the moat and distance rho out from it."""
    u, v = to_uv(x, y)
    t = np.arctan2(v / EB, u / EA)
    for _ in range(10):
        f = (EB * EB - EA * EA) * np.sin(t) * np.cos(t) + u * EA * np.sin(t) - v * EB * np.cos(t)
        fp = (EB * EB - EA * EA) * np.cos(2 * t) + u * EA * np.cos(t) + v * EB * np.sin(t)
        t = t - f / np.where(np.abs(fp) < 1e-6, 1e-6, fp)
    pu, pv = EA * np.cos(t), EB * np.sin(t)
    rho = np.hypot(u - pu, v - pv) * np.sign((u / EA) ** 2 + (v / EB) ** 2 - 1)
    return np.interp(t % (2 * np.pi), _TT, _S), rho


def bowl_point(s, rho=0.0):
    t = np.interp(np.asarray(s) % PERIM, _S, _TT)
    nu, nv = EB * np.cos(t), EA * np.sin(t)
    n = np.hypot(nu, nv)
    return from_uv(EA * np.cos(t) + rho * nu / n, EB * np.sin(t) + rho * nv / n)


def sdist(a, b):
    """Signed perimeter distance a - b, wrapped."""
    return (a - b + PERIM / 2) % PERIM - PERIM / 2


# ---------------------------------------------------------------- measured layout

# West stand vomitories (perimeter s): aisle below through the middle, one above on each side.
VOMITORIES = [36.8, 54.2, 72.8, 92.5, 112.8, 133.5, 153.5, 173.5, 192.2, 210.0]
VOM_RHO = (33.0, 39.5)
VOM_HALF = 3.0
# Full-height aisles elsewhere: the open east read off the seat points, the curves spaced alike.
AISLES = [6.0, 20.5, 225.0, 241.0, 257.0, 271.0, 287.0, 304.5, 322.5, 342.0, 362.5, 383.5, 404.0,
          423.5, 444.0, 459.7, 474.5, 489.5]
AISLE_HALF = 1.05
# Athletes' tunnels cut through the lower west stand: (s, half width, rho of the mouth).
TUNNELS = [(54.2, 5.5, 17.5), (194.2, 4.0, 14.0)]
# Press tribune between two lower aisles, its field-level forecourt, and the camera box above.
PRESS = (114.2, 132.2, 8.5, 31.0)
CAMERA = (121.0, 125.5, 25.5, 29.0)

# Roof masts: head (x, y, z over the pitch) and foot, fitted to the laser points of each tube.
MASTS = [
    ((8.8, 170.2, 54.1), (4.9, 161.5)),
    ((-45.7, 149.7, 65.4), (-47.1, 144.4)),
    ((-93.2, 108.9, 74.6), (-95.1, 106.3)),
    ((-123.7, 54.3, 81.3), (-132.5, 55.7)),
    ((-126.0, -8.3, 82.4), (-139.1, -10.4)),
    ((-108.5, -70.3, 79.3), (-116.0, -71.0)),
    ((-66.9, -119.0, 68.9), (-70.9, -113.5)),
    ((-19.4, -148.2, 55.9), (-18.2, -140.8)),
]
# Backstay anchors: cable foot from the laser, block length across the cable and width from OSM.
ANCHORS = [(-5.7, 199.3, 5.7, 2.7), (-86.1, 195.3, 7.0, 3.0), (-149.0, 143.1, 5.0, 3.3),
           (-185.5, 69.1, 13.6, 2.9), (-187.7, -14.3, 12.9, 2.6), (-163.7, -93.1, 13.5, 2.7),
           (-110.7, -153.8, 8.2, 1.7), (-45.0, -200.2, 5.9, 5.1)]
# Lattice floodlight towers on the east rim: lamp head top and foot.
TOWERS = [((51.5, 99.6, 67.7), (74.0, 114.1)), ((86.1, -61.7, 67.8), (109.5, -69.3))]
# Lamp clusters standing on the inner roof edge in the west.
ROOF_LIGHTS = [(-48.3, 55.9, 54.3), (-24.9, -69.0, 49.8)]
# Video screens: the south one from the laser, the north one mirrored through the bowl centre.
SCREENS = [((-16.0, -128.0), (12.0, -128.0)), ((24.4, 134.0), (-3.7, 134.1))]


# ---------------------------------------------------------------- blocks

class Palette:
    def __init__(self):
        self.names = ["minecraft:air"]

    def __call__(self, name, **props):
        name = name if ":" in name else "minecraft:" + name
        if props:
            name += "[" + ",".join(f"{k}={v}" for k, v in sorted(props.items())) + "]"
        if name not in self.names:
            self.names.append(name)
        return self.names.index(name)


B = Palette()
SEAT = B("lime_concrete")
SEAT_MIX = [(SEAT, 0.55), (B("lime_concrete_powder"), 0.30), (B("lime_wool"), 0.15)]
AISLE = B("smooth_sandstone")
CONCRETE = B("light_gray_concrete")
PAVING = B("smooth_stone")
APRON = B("polished_andesite")
TRACK = B("red_terracotta")
LINE = B("white_concrete")
GRASS = B("grass_block")
STRIPE = B("moss_block")
DIRT = B("dirt")
SAND = B("sand")
RAIL = B("iron_bars")
DARK = B("black_concrete")
PRESS_DESK = B("gray_concrete")
SIGN = B("light_blue_concrete")
PANEL = B("white_stained_glass")
STEEL = B("iron_block")
NODE = B("polished_andesite")
FOOTING = B("smooth_stone")
LAMP = B("sea_lantern")
LIT = B("white_concrete")
CABLE = B("light_gray_concrete")
GLAZING = B("gray_stained_glass")


def stair(facing):
    return B("smooth_sandstone_stairs", facing=facing, half="bottom", shape="straight")


def light(level):
    return B("light", level=level)


# ---------------------------------------------------------------- model

class Model:
    def __init__(self):
        self.v = np.zeros((W, H, L), np.uint16)

    def put(self, x, y, z, b, over=True):
        if 0 <= x < W and 0 <= y < H and 0 <= z < L and (over or self.v[x, y, z] == 0):
            self.v[x, y, z] = b

    def column(self, x, z, y0, y1, b):
        if 0 <= x < W and 0 <= z < L:
            self.v[x, max(0, y0):min(H, y1), z] = b

    def line(self, p0, p1, b, step=0.33, over=False):
        """Voxel line between points given as (east, north, height over the pitch)."""
        p0, p1 = np.asarray(p0, float), np.asarray(p1, float)
        n = max(1, int(np.linalg.norm(p1 - p0) / step))
        for i in range(n + 1):
            x, y, z = p0 + (p1 - p0) * i / n
            self.put(int(math.floor(x - XMIN)), PITCH + int(math.floor(z)) + 1, int(math.floor(YMAX - y)), b, over)


def fline(m, p0, p1, b, over=False):
    """Face-connected voxel line (3D DDA) between (east, north, height) points."""
    a = np.array([p0[0] - XMIN, PITCH + 1 + p0[2], YMAX - p0[1]], float)
    c = np.array([p1[0] - XMIN, PITCH + 1 + p1[2], YMAX - p1[1]], float)
    v, end = np.floor(a).astype(int), np.floor(c).astype(int)
    d = c - a
    step = np.sign(d).astype(int)
    nz = np.where(d != 0, d, 1.0)
    tmax = np.where(d != 0, (v + (step > 0) - a) / nz, np.inf)
    tdelta = np.where(d != 0, np.abs(1.0 / nz), np.inf)
    cells = [tuple(v)]
    for _ in range(int(np.abs(end - v).sum())):
        k = int(np.argmin(tmax))
        v[k] += step[k]
        tmax[k] += tdelta[k]
        cells.append(tuple(v))
    if b is not None:
        for x, y, z in cells:
            m.put(x, y, z, b, over)
    return cells


def layer(h):
    """Layer whose block top is nearest height h over the pitch."""
    return PITCH + np.round(h).astype(int)


def mx(x):
    return int(math.floor(x - XMIN))


def mz(y):
    return int(math.floor(YMAX - y))


def ly(z):
    return PITCH + int(math.floor(z)) + 1


# ---------------------------------------------------------------- build

def profiles(sv):
    """Seat top and outer edge of the ring (rho) for every metre of perimeter."""
    ss = np.arange(0, PERIM, 1.0)
    rr = np.arange(0, 110, 0.5)
    S, R = np.meshgrid(ss, rr, indexing="ij")
    X, Y = bowl_point(S, R)
    ix = np.clip(np.floor(X - XMIN).astype(int), 0, W - 1)
    iz = np.clip(np.floor(YMAX - Y).astype(int), 0, L - 1)
    h = sv.ground[ix, iz]
    hs = np.apply_along_axis(lambda a: np.convolve(np.pad(a, 2, mode="edge"), np.ones(5) / 5, "valid"), 1, h)
    top = np.zeros(len(ss)); out = np.zeros(len(ss))
    for i in range(len(ss)):
        p = hs[i]
        k = 16
        while k < len(rr) - 17 and (p[k:k + 17].max() - p[k] > 1.0 or p[k] < 1.0):
            k += 1
        top[i] = rr[k]
        lvl = p[k]
        j = k
        while j < len(rr) - 1 and abs(p[j + 1] - lvl) < 1.2 and rr[j] < rr[k] + 30:
            j += 1
        out[i] = rr[j]
    med = lambda a, r: np.array([np.median(np.take(a, range(i - r, i + r + 1), mode="wrap")) for i in range(len(a))])
    top = med(top, 4)
    out = np.minimum(med(out, 8), top + 22)
    k = np.ones(15) / 15
    out = np.convolve(np.concatenate([out[-7:], out, out[:7]]), k, "valid")
    out = np.maximum(out, top + 1)
    return ss, top, out


def build(sv):
    m = Model()
    X, Y = col_xy()
    S, RHO = bowl(X, Y)
    U, V = to_uv(X, Y)
    ss, top_s, out_s = profiles(sv)
    TOP = np.interp(S, ss, top_s, period=PERIM)
    OUT = np.interp(S, ss, out_s, period=PERIM)

    # where features cut the stand, the stand surface is carried over them first
    def near(s0, half):
        return np.abs(sdist(S, s0)) <= half
    feat = np.zeros((W, L), bool)
    for s0 in VOMITORIES:
        feat |= near(s0, VOM_HALF + 0.6) & (RHO > VOM_RHO[0] - 1) & (RHO < VOM_RHO[1] + 1)
    for s0, half, r1 in TUNNELS:
        feat |= near(s0, half + 0.6) & (RHO > -3) & (RHO < r1 + 1)
    press = (S > PRESS[0] - 1) & (S < PRESS[1] + 1) & (RHO > -3) & (RHO < PRESS[2] + 1)
    feat |= press
    # the glazed control booth standing behind the top rows of the main stand
    booth = (S > 95) & (S < 160) & (RHO > 55) & (RHO < 75) & (sv.ground > 32.5)
    booth = erode(dilate(booth, 1), 1) & (S > 95) & (S < 160) & (RHO > 55)
    feat |= booth
    g = sv.ground.copy()
    g[feat] = np.nan
    stand_h = box(fill_nan(g), 2)
    walk_h = win(sv.ground, 1, np.median)

    moat = (sv.gmin < -1.2) & (RHO > -6) & (RHO < 5)
    moat = erode(dilate(moat, 1), 1) & (RHO > -6) & (RHO < 5)
    field = (RHO < 2.5) & ~moat
    stand = (RHO >= -1) & (RHO <= TOP) & ~moat & ~field
    walk = (RHO > TOP) & (RHO <= OUT)
    # the concourse roof behind the main stand, a storey over the ground outside
    walk |= (RHO > TOP) & (RHO <= TOP + 8) & (sv.ground > sv.dtm + 2.5)
    ring = stand | walk | moat

    stand |= booth
    ring = ring | booth
    Lt = np.where(stand, layer(stand_h), layer(walk_h))
    Lt = np.maximum(Lt, PITCH)
    floor_ = np.clip(np.minimum(Lt - 3, PITCH + np.floor(sv.dtm).astype(int) - 2), PITCH - 3, None)
    floor_ = np.where(ring & ~erode(ring, 3), PITCH - 3, floor_)

    # ---- field
    field_block(m, field, U, V)

    # ---- moat: floor two below the pitch, walls up to both sides
    for x, z in zip(*np.nonzero(moat)):
        m.column(x, z, PITCH - 3, PITCH - 1, CONCRETE)
        m.put(x, PITCH - 2, z, PAVING)
    edge_in = field & dilate(moat, 1)
    for x, z in zip(*np.nonzero(edge_in)):
        m.put(x, PITCH + 1, z, RAIL)

    # ---- stand and walkway fill (the fill marks the ring for the excavation)
    Y3 = np.arange(H)[None, :, None]
    fill = (ring & ~moat)[:, None, :] & (Y3 >= floor_[:, None, :]) & (Y3 < Lt[:, None, :])
    m.v[fill] = CONCRETE
    top_b = np.where(stand, SEAT, PAVING)
    top_b = np.where(booth, CONCRETE, top_b)
    top_b = np.where(stand & (Lt == PITCH), APRON, top_b)
    xs, zs = np.nonzero(ring & ~moat)
    m.v[xs, Lt[xs, zs], zs] = top_b[xs, zs]
    xs, zs = np.nonzero(top_b == SEAT)
    m.v[xs, Lt[xs, zs], zs] = seat_mix(xs, Lt[xs, zs], zs)

    # ---- aisles
    aisle = np.zeros((W, L), bool)
    for s0 in AISLES:
        aisle |= aisle_line(X, Y, s0) & stand
    for s0 in VOMITORIES:
        aisle |= aisle_line(X, Y, s0) & stand & (RHO < VOM_RHO[0])
        for side in (-1, 1):
            aisle |= aisle_line(X, Y, s0 + side * (VOM_HALF + AISLE_HALF + 0.2)) & stand & (RHO > VOM_RHO[1])
    aisle &= Lt > PITCH
    xs, zs = np.nonzero(aisle)
    m.v[xs, Lt[xs, zs], zs] = AISLE
    outward = outward_dir(X, Y)
    for x, z in zip(xs, zs):
        dx, dz = outward[x, z]
        nx, nz = x + dx, z + dz
        if 0 <= nx < W and 0 <= nz < L and Lt[nx, nz] > Lt[x, z] and stand[nx, nz]:
            m.put(x, Lt[x, z] + 1, z, stair(FACING[(dx, dz)]))

    pr = (S >= PRESS[0]) & (S <= PRESS[1]) & stand & (RHO <= PRESS[3])
    # ---- press tribune: grey desks in rows, forecourt at field level with the sign and doors
    xs, zs = np.nonzero(pr & (RHO > PRESS[2]))
    m.v[xs, Lt[xs, zs], zs] = np.where(Lt[xs, zs] % 2 == 0, PRESS_DESK, APRON)
    fc = (S >= PRESS[0]) & (S <= PRESS[1]) & (RHO <= PRESS[2]) & (RHO > -1.5) & ~moat
    for x, z in zip(*np.nonzero(fc)):
        m.column(x, z, PITCH + 1, H, 0)
        m.put(x, PITCH, z, APRON)
    for x, z in zip(*np.nonzero(pr & (RHO > PRESS[2]) & dilate(fc, 1))):
        top = Lt[x, z]
        for y in range(PITCH + 1, top + 1):
            m.put(x, y, z, CONCRETE)
        m.put(x, PITCH + 3, z, SIGN)
        sc = sdist(S[x, z], (PRESS[0] + PRESS[1]) / 2)
        if 2.0 <= abs(sc) <= 4.5:
            m.put(x, PITCH + 1, z, DARK)
            m.put(x, PITCH + 2, z, DARK)
    cam = (S >= CAMERA[0]) & (S <= CAMERA[1]) & (RHO >= CAMERA[2]) & (RHO <= CAMERA[3])
    for x, z in zip(*np.nonzero(cam)):
        base = Lt[x, z]
        m.column(x, z, base + 1, base + 3, PRESS_DESK)
        m.put(x, base + 3, z, CONCRETE)
    for x, z in zip(*np.nonzero(cam & ~erode(cam, 1) & (RHO < CAMERA[2] + 1))):
        m.put(x, Lt[x, z] + 2, z, DARK)

    # ---- vomitories: landing cut into the seats, walls, a dark doorway and a railing
    for s0 in VOMITORIES:
        vm = near(s0, VOM_HALF) & (RHO >= VOM_RHO[0]) & (RHO <= VOM_RHO[1]) & stand
        if not vm.any():
            continue
        fx, fz = bowl_point(s0, VOM_RHO[0])
        lf = int(layer(stand_h[mx(fx), mz(fz)]))
        rim = dilate(vm, 1) & ~vm & stand & (RHO >= VOM_RHO[0] - 0.5)
        for x, z in zip(*np.nonzero(vm)):
            m.column(x, z, lf + 1, H, 0)
            m.column(x, z, floor_[x, z], lf, CONCRETE)
            m.put(x, lf, z, PAVING)
        for x, z in zip(*np.nonzero(rim)):
            back = RHO[x, z] > VOM_RHO[1] - 0.5
            side = not back and RHO[x, z] > VOM_RHO[0] + 0.5
            if back or side:
                for y in range(lf + 1, Lt[x, z] + 2):
                    m.put(x, y, z, LIT)
                if back and abs(sdist(S[x, z], s0)) <= 1.2:
                    for y in range(lf + 1, min(lf + 3, Lt[x, z] + 1)):
                        m.put(x, y, z, PRESS_DESK)
        for x, z in zip(*np.nonzero(vm & (RHO < VOM_RHO[0] + 1.0))):
            m.put(x, lf + 1, z, RAIL)
        for x, z in zip(*np.nonzero(rim & (RHO > VOM_RHO[0] + 0.5))):
            m.put(x, max(Lt[x, z] + 2, lf + 2), z, RAIL)

    # ---- athletes' tunnels through the lower stand
    for s0, half, r1 in TUNNELS:
        tn = near(s0, half) & (RHO > -3) & (RHO <= r1) & ~moat
        wall = dilate(tn, 1) & ~tn & stand
        for x, z in zip(*np.nonzero(tn)):
            m.column(x, z, PITCH + 1, H, 0)
            m.put(x, PITCH, z, APRON)
        for x, z in zip(*np.nonzero(wall)):
            for y in range(PITCH + 1, Lt[x, z] + 1):
                m.put(x, y, z, CONCRETE)
            m.put(x, Lt[x, z] + 1, z, RAIL)
            if RHO[x, z] > r1 and abs(sdist(S[x, z], s0)) <= half - 1.0:
                for y in range(PITCH + 1, PITCH + 4):
                    m.put(x, y, z, DARK)

    # ---- rail along the back of the seats where the walkway drops away
    edge = stand & dilate(walk, 1)
    for x, z in zip(*np.nonzero(edge)):
        nb = Lt[max(0, x - 1):x + 2, max(0, z - 1):z + 2][walk[max(0, x - 1):x + 2, max(0, z - 1):z + 2]]
        if len(nb) and nb.min() < Lt[x, z] - 1:
            m.put(x, Lt[x, z] + 1, z, RAIL)

    # ---- booth: dark glazing towards the pitch under a flat roof
    roof_l = layer(sv.ground)
    front_b = booth & dilate(stand & ~booth, 1)
    for x, z in zip(*np.nonzero(booth)):
        top = int(roof_l[x, z])
        for y in range(Lt[x, z] + 1, top):
            glazing = front_b[x, z] and Lt[x, z] + 2 <= y <= top - 2
            m.put(x, y, z, GLAZING if glazing else CONCRETE)
        m.put(x, top, z, PAVING)

    # ---- night lighting: the floodlit field and stands, vomitories and tunnels
    X0, Z0 = np.meshgrid(np.arange(W), np.arange(L), indexing="ij")
    for x, z in zip(*np.nonzero(field & (X0 % 12 == 6) & (Z0 % 12 == 6))):
        m.put(x, PITCH + 3, z, light(15), over=False)
    for x, z in zip(*np.nonzero(stand & ~booth & (Lt > PITCH) & (X0 % 14 == 3) & (Z0 % 14 == 3))):
        m.put(x, Lt[x, z] + 2, z, light(12), over=False)
    for s0 in VOMITORIES:
        fx, fz = bowl_point(s0, VOM_RHO[0])
        lf = int(layer(stand_h[mx(fx), mz(fz)]))
        fx, fz = bowl_point(s0, sum(VOM_RHO) / 2)
        m.put(mx(fx), lf + 2, mz(fz), light(12), over=False)
    for s0, half, r1 in TUNNELS:
        fx, fz = bowl_point(s0, r1 - 2.0)
        m.put(mx(fx), PITCH + 2, mz(fz), light(12), over=False)

    roof(m, sv)
    masts(m, sv)
    towers(m)
    screens(m, walk_h)
    return m, ring


def seat_mix(x, y, z):
    """Seat colours hashed per block, weighted to lime."""
    with np.errstate(over="ignore"):
        h = (np.asarray(x, np.uint64) * np.uint64(0x9E3779B1) + np.asarray(y, np.uint64) * np.uint64(0x85EBCA77)
             + np.asarray(z, np.uint64) * np.uint64(0xC2B2AE3D)) & np.uint64(0xFFFFFFFF)
        h = ((h ^ (h >> np.uint64(15))) * np.uint64(0x2C1B3C6D)) & np.uint64(0xFFFFFFFF)
        h ^= h >> np.uint64(12)
    f = (h & np.uint64(0xFFFF)).astype(float) / 65536.0
    out = np.empty(f.shape, np.uint16)
    acc = 0.0
    for b, w in SEAT_MIX:
        out[f >= acc] = b
        acc += w
    return out


FACING = {(1, 0): "east", (-1, 0): "west", (0, 1): "south", (0, -1): "north"}


def outward_dir(X, Y):
    """Grid step that leads away from the pitch, per column."""
    s, r = bowl(X, Y)
    x2, y2 = bowl_point(s, r + 1.0)
    dx, dy = x2 - X, y2 - Y
    out = np.zeros(X.shape + (2,), int)
    horiz = np.abs(dx) >= np.abs(dy)
    out[..., 0] = np.where(horiz, np.sign(dx), 0)
    out[..., 1] = np.where(horiz, 0, -np.sign(dy))
    return out


def aisle_line(X, Y, s0):
    """Columns within half an aisle of the normal through perimeter point s0."""
    px, py = bowl_point(s0, 0.0)
    qx, qy = bowl_point(s0, 1.0)
    nx, ny = qx - px, qy - py
    d = np.abs((X - px) * ny - (Y - py) * nx)
    ahead = (X - px) * nx + (Y - py) * ny > -2
    return (d <= AISLE_HALF) & ahead


def field_block(m, field, U, V):
    au, av = np.abs(U), np.abs(V)
    straight = au <= 42.195
    dcurve = np.hypot(au - 42.195, av)
    d = np.where(straight, av, dcurve) - 36.5
    track = (d >= 0) & (d < 9.76)
    sprint = (V > 36.5) & (V < 46.26) & (U > 42.0) & (U < 60.0)
    track |= sprint
    pitch = (au <= 52.5) & (av <= 34.0)
    inner = d < 0
    kerb = track & (dilate(inner & ~track, 1) | dilate(~track & ~inner, 1))
    xs, zs = np.nonzero(field)
    for x, z in zip(xs, zs):
        u, v = U[x, z], V[x, z]
        if track[x, z]:
            dd = d[x, z]
            b = TRACK
            if kerb[x, z] or (sprint[x, z] and abs(u - 57.8) < 0.55):
                b = LINE
            elif abs(u + 42.195) < 0.55 and v > 0:
                b = LINE
            # eight 1.22 m lanes do not fit whole blocks: every other boundary, dotted
            elif (x + z) % 2 == 0 and any(abs(dd - 1.22 * k) < 0.35 for k in (2, 4, 6)):
                b = LINE
        elif pitch[x, z]:
            b = pitch_block(u, v)
        elif inner[x, z]:
            b = GRASS if av[x, z] <= 36.5 and au[x, z] <= 52.5 else TRACK
            if b == TRACK:
                for cu in (-1, 1):
                    r = math.hypot(u - cu * 70.0, v - 8.0)
                    if abs(r - 1.6) < 0.5 or abs(math.hypot(u - cu * 70.0, v + 10.0) - 2.5) < 0.5:
                        b = LINE
        else:
            west = v > 46.26 and abs(u) < 42.195
            east = v < -46.26 and abs(u) < 42.195
            b = APRON
            if west:
                b = GRASS
                if -40 < u < 22 and (abs(v - 49.5) < 0.6 or abs(v - 52.0) < 0.6):
                    b = TRACK
                if -46 < u <= -40 and 48.5 < v < 53.0:
                    b = SAND
            elif east:
                if -14 < u < 41 and abs(v + 50.5) < 0.6:
                    b = TRACK
                if 41 <= u < 47 and -52.5 < v < -48.5:
                    b = SAND
        m.put(x, PITCH, z, b)
        m.put(x, PITCH - 1, z, DIRT)
        m.put(x, PITCH - 2, z, DIRT)
    # goals
    for side in (-1, 1):
        a, b = from_uv(side * 52.8, -3.66), from_uv(side * 52.8, 3.66)
        fline(m, (a[0], a[1], 2.0), (b[0], b[1], 2.0), RAIL)
        for gx, gy in (a, b):
            m.put(mx(gx), PITCH + 1, mz(gy), RAIL)
            m.put(mx(gx), PITCH + 2, mz(gy), RAIL)


def pitch_block(u, v):
    au, av = abs(u), abs(v)
    line = lambda d: abs(d) <= 0.5
    r = math.hypot(u, v)
    pen = 52.5 - 16.5
    goal = 52.5 - 5.5
    marked = (au > 52.0 or av > 33.5 or line(u) or line(r - 9.15)
              or (line(au - pen) and av <= 20.16) or (av - 20.16 <= 0.5 and av >= 19.66 and au >= pen)
              or (line(au - goal) and av <= 9.16) or (av - 9.16 <= 0.5 and av >= 8.66 and au >= goal)
              or (math.hypot(au - 41.5, v) < 0.6)
              or (line(math.hypot(au - 41.5, v) - 9.15) and au < pen))
    if marked:
        return LINE
    return STRIPE if int((u + 52.5) // 5.83) % 2 else GRASS


def roof(m, sv):
    mask = sv.roof_mask
    lr = np.where(mask, layer(np.nan_to_num(sv.roof)), -1)
    xs, zs = np.nonzero(mask)
    # the edge cable runs along the inner edge: roof cells with open sky on their pitch side
    _, rho = bowl(*col_xy())
    edge = np.zeros_like(mask)
    for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        nm, nr = np.roll(mask, (-dx, -dz), (0, 1)), np.roll(rho, (-dx, -dz), (0, 1))
        edge |= mask & ~nm & (nr < rho)
    edge &= rho < 45
    for x, z in zip(xs, zs):
        top = lr[x, z]
        lo = top
        for dx, dz in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            nx, nz = x + dx, z + dz
            if 0 <= nx < W and 0 <= nz < L and mask[nx, nz]:
                lo = min(lo, lr[nx, nz] + 1)
        b = CABLE if edge[x, z] else PANEL
        for y in range(lo, top + 1):
            m.put(x, y, z, b)


def masts(m, sv):
    for (head, foot), anchor in zip(MASTS, ANCHORS):
        hx, hy, hz = head
        fz = sv.ground[mx(foot[0]), mz(foot[1])]
        # carried below ground to pitch level so the engine fills under it wherever the terrain sits
        tube(m, (foot[0], foot[1], min(fz - 2.0, 0.0)), head, 1.15, STEEL)
        cx, cz, top = mx(hx), mz(hy), ly(hz)
        for dx in (-1, 0, 1):
            for dz in (-1, 0, 1):
                m.put(cx + dx, top - 2, cz + dz, NODE)
        m.put(cx, top, cz, STEEL)
        ax, ay, length, width = anchor
        az = sv.ground[mx(ax), mz(ay)]
        cable(m, (hx, hy, hz - 0.5), (ax, ay, az + 1.5))
        anchor_block(m, (ax, ay, az), (ax - hx, ay - hy), length, width)
        hump = roof_hump(sv, hx, hy)
        if hump is not None:
            cable(m, (hx, hy, hz - 0.5), hump)


def roof_hump(sv, hx, hy):
    """Highest point of the net within reach on the bowl side of a mast head."""
    X, Y = col_xy()
    d = np.hypot(X - hx, Y - hy)
    inward = ((EC[0] - hx) * (X - hx) + (EC[1] - hy) * (Y - hy)) / np.maximum(d, 1e-6) / math.hypot(EC[0] - hx, EC[1] - hy)
    ok = sv.roof_mask & (d > 8) & (d < 55) & (inward > 0.6)
    if not ok.any():
        return None
    score = np.where(ok, np.nan_to_num(sv.roof) - 0.12 * d, -1e9)
    x, z = np.unravel_index(np.argmax(score), score.shape)
    return (X[x, z], Y[x, z], sv.roof[x, z] + 0.6)


def tube(m, p0, p1, r, b):
    p0, p1 = np.array(p0, float), np.array(p1, float)
    lo, hi = np.minimum(p0, p1) - r - 1, np.maximum(p0, p1) + r + 1
    d = p1 - p0
    for x in range(int(lo[0]), int(hi[0]) + 1):
        for y in range(int(lo[1]), int(hi[1]) + 1):
            for z in range(int(lo[2]), int(hi[2]) + 1):
                p = np.array([x + 0.5, y + 0.5, z + 0.5])
                t = np.clip((p - p0) @ d / (d @ d), 0, 1)
                if np.linalg.norm(p - (p0 + t * d)) <= r:
                    m.put(mx(p[0]), ly(p[2] - 0.5), mz(p[1]), b)


def cable(m, p0, p1):
    fline(m, p0, p1, CABLE)


def anchor_block(m, p, toward, length, width):
    """Concrete beam across the cable, 1.5 m over the ground, footed down to pitch level."""
    ax, ay, az = p
    n = math.hypot(*toward)
    ux, uy = toward[0] / n, toward[1] / n
    cols = set()
    for a in np.arange(-length / 2, length / 2 + 0.01, 0.33):
        for b in np.arange(-width / 2, width / 2 + 0.01, 0.33):
            cols.add((mx(ax - uy * a + ux * b), mz(ay + ux * a + uy * b)))
    top = ly(az) + 1
    for x, z in cols:
        m.column(x, z, min(PITCH, top - 3), top + 1, FOOTING)
    m.put(mx(ax), top + 1, mz(ay), NODE)


def lattice(m, base, top, half):
    """Square mast of four steel corner lines tied by a steel ring every 4 m."""
    base, top = np.array(base, float), np.array(top, float)
    axis = (top - base) / np.linalg.norm(top - base)
    side = np.cross(axis, [0, 0, 1.0])
    side /= np.linalg.norm(side)
    other = np.cross(axis, side)
    corners = [side * a * half + other * b * half for a, b in ((-1, -1), (1, -1), (1, 1), (-1, 1))]
    for c in corners:
        fline(m, base + c, top + c, STEEL, over=True)
    for z in np.arange(base[2] + 2.0, top[2] - 0.5, 4.0):
        pts = [base + c + (top - base) * (z - base[2] - c[2]) / (top[2] - base[2]) for c in corners]
        for k in range(4):
            p, q = pts[k].copy(), pts[(k + 1) % 4].copy()
            p[2] = q[2] = z
            fline(m, p, q, STEEL)


def lamp_head(m, top, toward, wide, tall):
    """Floodlight frame facing the pitch, tilted down a block per two rows."""
    hx, hy, hz = top
    n = math.hypot(*toward)
    fx, fy = toward[0] / n, toward[1] / n
    sx, sy = -fy, fx
    w = wide / 2.0
    for j in range(tall):
        lean = -j / 2.0
        cx, cy, z = hx + fx * lean, hy + fy * lean, hz - j
        fline(m, (cx - sx * w - fx, cy - sy * w - fy, z), (cx + sx * w - fx, cy + sy * w - fy, z), RAIL)
        cells = fline(m, (cx - sx * w, cy - sy * w, z), (cx + sx * w, cy + sy * w, z), None)
        for i, (x, y, zz) in enumerate(cells):
            rim = i == 0 or i == len(cells) - 1 or j in (0, tall - 1)
            m.put(x, y, zz, STEEL if rim else (LAMP if (i + j) % 2 == 0 else RAIL))


def towers(m):
    for head, foot in TOWERS:
        hx, hy, hz = head
        fx, fy = foot
        lattice(m, (fx, fy, 13.0), (hx, hy, hz - 6.5), 1.6)
        m.line((fx, fy, -1.0), (fx, fy, 13.0), FOOTING, over=True)
        lamp_head(m, (hx, hy, hz), (EC[0] - hx, EC[1] - hy), 11, 8)
    for x, y, z in ROOF_LIGHTS:
        toward = (EC[0] - x, EC[1] - y)
        n = math.hypot(*toward)
        bx, by = x - toward[0] / n * 2.0, y - toward[1] / n * 2.0
        fline(m, (bx, by, z - 16.0), (bx, by, z + 2.0), STEEL, over=True)
        for k in (-3.0, 3.0):
            px, py = bx - toward[1] / n * k, by + toward[0] / n * k
            fline(m, (px, py, z - 13.0), (bx, by, z - 5.0), STEEL)
        lamp_head(m, (x, y, z), toward, 9, 5)


def screens(m, walk_h):
    for (x0, y0), (x1, y1) in SCREENS:
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        base = walk_h[mx(cx), mz(cy)]
        dx, dy = x1 - x0, y1 - y0
        n = math.hypot(dx, dy)
        dx, dy = dx / n, dy / n
        half = 8.0
        bottom, top = base + 3.0, base + 11.0
        # lit face towards the pitch, grey casing behind
        nx, ny = -dy, dx
        if nx * (EC[0] - cx) + ny * (EC[1] - cy) < 0:
            nx, ny = -nx, -ny
        for k in np.arange(-half, half + 0.01, 0.5):
            x, y = cx + dx * k, cy + dy * k
            for z in np.arange(bottom, top + 0.01, 1.0):
                frame = abs(k) > half - 0.6 or z < bottom + 0.5 or z > top - 0.5
                m.put(mx(x), ly(z), mz(y), PRESS_DESK if frame else LAMP)
                m.put(mx(x - nx), ly(z), mz(y - ny), PRESS_DESK)
        for k in (-half + 2, half - 2):
            x, y = cx + dx * k, cy + dy * k
            for z in np.arange(base - 1.0, bottom, 1.0):
                m.put(mx(x), ly(z), mz(y), STEEL)


# ---------------------------------------------------------------- output

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


def write_schem(path, vol, names):
    w, h, l = vol.shape
    flat = np.ascontiguousarray(vol.transpose(1, 2, 0)).ravel().astype(np.int64)
    two = flat >= 128
    data = np.empty(len(flat) + int(two.sum()), np.uint8)
    pos = np.arange(len(flat)) + np.concatenate([[0], np.cumsum(two)[:-1]])
    data[pos] = np.where(two, (flat & 0x7F) | 0x80, flat).astype(np.uint8)
    data[pos[two] + 1] = (flat[two] >> 7).astype(np.uint8)
    blocks = {"Palette": (10, {n: (3, i) for i, n in enumerate(names)}),
              "Data": (7, data.tobytes()), "BlockEntities": (9, (10, []))}
    schem = {"Version": (3, 3), "DataVersion": (3, 4438), "Width": (2, w), "Height": (2, h),
             "Length": (2, l), "Offset": (11, [0, 0, 0]), "Blocks": (10, blocks)}
    out = io.BytesIO()
    out.write(b"\x0a" + struct.pack(">H", 0))
    _payload(out, 10, {"Schematic": (10, schem)})
    with open(path, "wb") as f:
        f.write(gzip.compress(out.getvalue(), compresslevel=9, mtime=0))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cache", default=os.environ.get("ARNIS_SURVEY_CACHE", os.path.expanduser("~/.cache/arnis-survey")))
    ap.add_argument("--out", default=os.path.join(HERE, "olympiastadion_munich.schem"))
    ap.add_argument("--preview", help="also write the model set in survey ground, for renders")
    args = ap.parse_args()
    sv = Survey(args.cache)
    print("pitch at %.2f m" % sv.pitch_z, file=sys.stderr)
    m, ring = build(sv)
    xs, ys, zs = np.nonzero(m.v)
    x0, y0, z0 = xs.min(), ys.min(), zs.min()
    vol = m.v[x0:xs.max() + 1, y0:ys.max() + 1, z0:zs.max() + 1]
    print("voxels %d, states %d, size %s, anchor block (%d, %d), pitch layer %d, top layer %d" % (
        int((vol > 0).sum()), len(B.names), vol.shape, ANCHOR_X - x0, ANCHOR_Z - z0, PITCH - y0,
        vol.shape[1] - 1), file=sys.stderr)
    write_schem(args.out, vol, B.names)
    if args.preview:
        pv = m.v.copy()
        pv[np.isin(pv, [i for i, n in enumerate(B.names) if n.startswith("minecraft:light[")])] = 0
        g = PITCH + np.round(sv.dtm).astype(int)
        inner = erode(ring, 1)
        for x in range(W):
            for z in range(L):
                if inner[x, z]:
                    continue
                top = int(np.clip(g[x, z], 1, H - 1))
                col = pv[x, :top + 1, z]
                col[col == 0] = DIRT
                if pv[x, top, z] == DIRT:
                    pv[x, top, z] = GRASS
        write_schem(args.preview, pv, B.names)


if __name__ == "__main__":
    main()
