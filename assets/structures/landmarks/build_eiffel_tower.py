#!/usr/bin/env python3
"""Builds eiffel_tower.schem beside this script: the Eiffel Tower at one block per metre.

    python3 assets/structures/landmarks/build_eiffel_tower.py [OUT.schem]

Shape from IGN LiDAR HD (2025 flight, tile LHD_FXX_0648_6863, Licence Ouverte Etalab 2.0):
leg, floor and pillar profiles, the arch intrados and the 44.3 degree turn were measured from
the point cloud and are baked in below. Published floor heights (57.63, 115.73, 276.13 m),
330 m to the antenna tip. Deps: numpy.

Model axes: x east, y up, z south. The tower axis is at x = z = AXIS, ground at layer GY.
"""
import gzip
import io
import itertools
import math
import os
import struct
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
DATA_VERSION = 4438

# Faces look out at 44.3/134.3/224.3/314.3 degrees, so the legs stand N, E, S and W.
TURN = 44.3
GY = 2
AXIS = 92
NX = NZ = 2 * AXIS
NY = 330 + GY

CT, ST = math.cos(math.radians(TURN)), math.sin(math.radians(TURN))

# ---------------------------------------------------------------- blocks

PALETTE = ["minecraft:air"]
_INDEX = {"minecraft:air": 0}


def blk(state):
    if state not in _INDEX:
        _INDEX[state] = len(PALETTE)
        PALETTE.append(state)
    return _INDEX[state]


IRON = blk("minecraft:brown_mushroom_block[down=true,east=true,north=true,south=true,up=true,west=true]")
FENCE = blk("minecraft:jungle_fence")
STONE = blk("minecraft:smooth_sandstone")
PAVING = blk("minecraft:smooth_stone_slab[type=bottom,waterlogged=false]")
GLASS = blk("minecraft:glass")
DARK_GLASS = blk("minecraft:gray_stained_glass_pane")
PANE = blk("minecraft:glass_pane")
DECK = blk("minecraft:spruce_planks")
CANOPY = blk("minecraft:jungle_slab[type=bottom,waterlogged=false]")
LAMP = blk("minecraft:sea_lantern")
LIGHT = blk("minecraft:light[level=15]")

grid = np.zeros((NX, NY, NZ), np.uint16)

# ---------------------------------------------------------------- frames


def to_model(u, v, h):
    """Tower frame (u toward 134.3 deg, v toward 224.3 deg, h up) to model coordinates."""
    return (u * CT - v * ST + AXIS, h + GY, u * ST + v * CT + AXIS)


def quad(q, u, v):
    """(u, v) turned q quarter turns about the axis."""
    for _ in range(q % 4):
        u, v = -v, u
    return u, v


_cx = np.arange(NX) + 0.5 - AXIS
_cz = np.arange(NZ) + 0.5 - AXIS
_X, _Z = np.meshgrid(_cx, _cz, indexing="ij")
U = _X * CT + _Z * ST
V = -_X * ST + _Z * CT
T = np.maximum(np.abs(U), np.abs(V))


def layer(h):
    return int(math.floor(h)) + GY


# ---------------------------------------------------------------- drawing


def put(i, j, k, b, under=False):
    i, j, k = np.asarray(i), np.asarray(j), np.asarray(k)
    ok = (i >= 0) & (i < NX) & (j >= 0) & (j < NY) & (k >= 0) & (k < NZ)
    i, j, k = i[ok], j[ok], k[ok]
    if under:
        free = grid[i, j, k] == 0
        i, j, k = i[free], j[free], k[free]
    grid[i, j, k] = b


def _seg_dist(q, a, c):
    d = c - a
    t = min(1.0, max(0.0, float((q - a) @ d) / float(d @ d))) if d @ d > 0 else 0.0
    return float(np.linalg.norm(q - (a + t * d)))


def line(p0, p1, b, under=False):
    """Voxel line between two tower-frame points, every step face to face so it never breaks
    up; where a step changes several axes, the in-between voxels hug the true line."""
    a = np.array(to_model(*p0))
    c = np.array(to_model(*p1))
    n = 2 * int(np.ceil(np.abs(c - a).max())) + 2
    t = np.linspace(0.0, 1.0, n)[:, None]
    p = np.floor(a + (c - a) * t).astype(int)
    path = [p[0]]
    for q in p[1:]:
        cur = path[-1]
        diff = q - cur
        axes = [ax for ax in range(3) if diff[ax]]
        if not axes:
            continue
        best = None
        for perm in itertools.permutations(axes):
            v, inter = cur.copy(), []
            for ax in perm[:-1]:
                v = v.copy()
                v[ax] += diff[ax]
                inter.append(v)
            cost = sum(_seg_dist(w + 0.5, a, c) for w in inter)
            if best is None or cost < best[0]:
                best = (cost, inter)
        path.extend(best[1])
        path.append(q)
    p = np.array(path)
    put(p[:, 0], p[:, 1], p[:, 2], b, under)


def rod(p0, p1, r, b, under=False):
    """Voxels whose centre lies within r of the segment."""
    a = np.array(to_model(*p0))
    c = np.array(to_model(*p1))
    lo = np.floor(np.minimum(a, c) - r).astype(int)
    hi = np.ceil(np.maximum(a, c) + r).astype(int)
    ii, jj, kk = np.meshgrid(
        np.arange(lo[0], hi[0] + 1), np.arange(lo[1], hi[1] + 1), np.arange(lo[2], hi[2] + 1),
        indexing="ij",
    )
    p = np.stack([ii, jj, kk], -1).reshape(-1, 3)
    q = p + 0.5
    d = c - a
    dd = float(d @ d)
    t = np.clip(((q - a) @ d) / dd, 0, 1) if dd > 0 else np.zeros(len(q))
    dist = np.linalg.norm(q - (a + t[:, None] * d), axis=1)
    p = p[dist <= r]
    put(p[:, 0], p[:, 1], p[:, 2], b, under)
    line(p0, p1, b, under)


def polyline(points, b, r=0.0, under=False):
    for p0, p1 in zip(points, points[1:]):
        if r > 0:
            rod(p0, p1, r, b, under)
        else:
            line(p0, p1, b, under)


def fill_layer(h, mask, b, under=False):
    j = layer(h)
    sl = grid[:, j, :]
    if under:
        mask = mask & (sl == 0)
    sl[mask] = b


def interp(table, h):
    hs, vs = zip(*table)
    return float(np.interp(h, hs, vs))


# ---------------------------------------------------------------- face plates

SQ2 = math.sqrt(2.0)
CW = 1.0 / SQ2
_NU = np.rint(_X + _Z).astype(int)
_NV = np.rint(_Z - _X).astype(int)


class Plate:
    """A face as a staircase one voxel per diagonal column (0.71 m), so its members rasterize alike.
    t(h, s) is the plane; `behind` is the share of the staircase inside it."""

    def __init__(self, t_axis, s_axis, t, behind=0.5):
        self.S = U * s_axis[0] + V * s_axis[1]
        self.T = U * t_axis[0] + V * t_axis[1]
        self.N = _NU * s_axis[0] + _NV * s_axis[1]
        self.ta, self.sa, self.t, self.behind = t_axis, s_axis, t, behind

    def _t(self, h, s):
        return np.broadcast_to(np.asarray(self.t(h, s), float), np.shape(s))

    def window(self, j):
        # A sloping plane takes both its layer edges, so columns stay joined layer to layer.
        h = j - GY + 0.5
        a, b = self._t(h - 0.5, self.S), self._t(h + 0.5, self.S)
        return (self.T > np.minimum(a, b) - SQ2 * self.behind) & (self.T <= np.maximum(a, b) + SQ2 * (1 - self.behind))

    def col(self, s, h):
        """Column holding face point (s, h)."""
        t = float(self._t(h, np.array([float(s)]))[0])
        x, _, z = to_model(s * self.sa[0] + t * self.ta[0], s * self.sa[1] + t * self.ta[1], h)
        return int(self.N[int(math.floor(x)), int(math.floor(z))])

    def cell(self, s, h):
        return (self.col(s, h), layer(h))

    def paint(self, cells, b, under=False):
        rows = {}
        for n, j in cells:
            rows.setdefault(j, set()).add(n)
        for j, ns in rows.items():
            if 0 <= j < NY:
                m = self.window(j) & np.isin(self.N, sorted(ns))
                sl = grid[:, j, :]
                if under:
                    m &= sl == 0
                sl[m] = b

    def pattern(self, j0, j1, fn):
        """fn(S, N, h) gives a block per plate voxel of a layer, 0 to leave it."""
        for j in range(max(j0, 0), min(j1, NY - 1) + 1):
            ii, kk = np.nonzero(self.window(j))
            if len(ii):
                b = fn(self.S[ii, kk], self.N[ii, kk], j - GY + 0.5)
                sel = b > 0
                grid[ii[sel], j, kk[sel]] = b[sel]


def pix_line(a, b):
    """Cells from a to b in (column, layer), one per step of the longer axis."""
    (n0, j0), (n1, j1) = a, b
    k = max(abs(n1 - n0), abs(j1 - j0))
    if k == 0:
        return [a]
    return [(n0 + math.floor((n1 - n0) * i / k + 0.5), j0 + math.floor((j1 - j0) * i / k + 0.5))
            for i in range(k + 1)]


def face_plate(q, t, behind=0.5):
    """Plate of tower face q at half-width t(h)."""
    return Plate(quad(q, 0.0, 1.0), quad(q, 1.0, 0.0), lambda h, s: t(h), behind)


def truss(plate, edges, levels, crosses=1, top=True):
    """Strut and bracing cells for each panel; crosses=2 puts two crosses side by side."""
    lo, hi = edges
    struts, braces = [], []
    for h0, h1 in zip(levels, levels[1:]):
        a0, b0 = plate.cell(lo(h0), h0), plate.cell(hi(h0), h0)
        a1, b1 = plate.cell(lo(h1), h1), plate.cell(hi(h1), h1)
        struts += pix_line(a0, b0)
        if crosses == 1:
            braces += pix_line(a0, b1) + pix_line(b0, a1)
        else:
            m0 = plate.cell((lo(h0) + hi(h0)) / 2, h0)
            m1 = plate.cell((lo(h1) + hi(h1)) / 2, h1)
            braces += pix_line(a0, m1) + pix_line(m0, a1) + pix_line(m0, b1) + pix_line(b0, m1)
    if top:
        h = levels[-1]
        struts += pix_line(plate.cell(lo(h), h), plate.cell(hi(h), h))
    return struts, braces


# ---------------------------------------------------------------- measured profiles

# Leg corners in the folded quadrant: the leg spans [inner, outer] on both u and v.
LEG_OUTER = [(5.0, 57.0), (51.0, 34.0), (64.0, 29.25), (72.0, 26.75), (80.0, 24.25),
             (90.0, 21.75), (100.0, 19.25), (104.0, 18.25), (110.0, 17.15), (116.0, 16.2)]
# Above the first floor the legs are not square: the arrises on the faces stand further in
# than the inner corner, as the face-on and corner LiDAR sections both show.
LEG_INNER = [(5.0, 40.45), (51.0, 18.8), (58.0, 16.5), (63.0, 15.6), (70.0, 14.4), (74.0, 13.6),
             (80.0, 12.0), (90.0, 10.0), (100.0, 8.6), (104.0, 8.0), (110.0, 7.2)]
LEG_SIDE = [(5.0, 40.45), (51.0, 18.8), (58.0, 14.6), (63.0, 13.4), (70.0, 12.2), (74.0, 11.2),
            (78.0, 10.5), (82.0, 9.9), (86.0, 9.1), (90.0, 8.5), (94.0, 8.0), (100.0, 7.3),
            (104.0, 7.0), (110.0, 6.4)]
# Upper pillar outer face; the corner columns carry on from the legs' outer arris.
PILLAR = [(116.0, 16.2), (120.0, 15.8), (125.0, 15.1), (130.0, 14.3), (137.1, 13.3),
          (147.5, 12.1), (158.3, 11.0), (168.75, 10.2), (178.75, 9.6), (188.75, 9.0),
          (197.5, 8.5), (207.5, 8.1), (215.8, 7.75), (224.2, 7.35), (231.25, 7.05),
          (239.2, 6.67), (246.25, 6.35), (252.5, 6.06), (259.2, 5.7), (262.0, 5.55)]
# Flare under the third floor, out to the gallery.
FLARE = [(262.0, 5.55), (266.0, 5.75), (269.0, 6.2), (271.5, 6.9), (273.5, 7.8), (275.5, 8.9)]

LOWER_STRUTS = [6.3 + 5.5 * k for k in range(8)]
UPPER_STRUTS = [57.6, 68.2, 78.8, 89.4, 100.0]
PILLAR_STRUTS = [116.0, 125.3, 137.1, 147.5, 158.3, 168.75, 178.75, 188.75, 197.5, 207.5,
                 215.8, 224.2, 231.25, 239.2, 246.25, 252.5, 259.2]

FLOOR1, FLOOR2, FLOOR3 = 57.63, 115.73, 276.13
# Arch intrados: a circle fitted to the LiDAR, centred on the axis 2.03 m up.
ARCH_C, ARCH_R = 2.03, 36.87


def leg_outer(h):
    return interp(LEG_OUTER, h)


def leg_inner(h):
    return interp(LEG_INNER, h)


def leg_side(h):
    """Inner edge of a leg as seen on the tower face."""
    return interp(LEG_SIDE, h)


def pillar(h):
    return interp(PILLAR + FLARE[1:], h)


# ---------------------------------------------------------------- legs


def arris_path(cu, cv, hs, r):
    """Points along one leg arris. cu/cv pick the inner or outer face on each axis."""
    pts = []
    for h in hs:
        o = leg_outer(h)
        i = leg_inner(h) if cu == cv == "i" else leg_side(h)
        pu = o - r if cu == "o" else i + r
        pv = o - r if cv == "o" else i + r
        pts.append((pu, pv, h))
    return pts


def leg(q):
    """One leg in quadrant q: feet on limestone piers, arrises, face lattice, glazed base halls."""
    r = 0.85
    hs = [5.0, 15.0, 25.0, 35.0, 45.0, 51.0, 58.0, 64.0, 72.0, 80.0, 90.0, 100.0, 104.0, 110.0]

    def at(c, h, rr=r):
        p = arris_path(c[0], c[1], [h], rr)[0]
        return (*quad(q, p[0], p[1]), h)

    for c in ("ii", "io", "oi", "oo"):
        # The outer arris goes on through the second floor as a pillar corner.
        polyline([at(c, h) for h in hs + ([116.0] if c == "oo" else [])], IRON, r=r)
    # Feet: each arris splays onto its own limestone pier.
    for cu in "io":
        for cv in "io":
            top = arris_path(cu, cv, [5.0], r)[0]
            fu = 60.6 if cu == "o" else 38.9
            fv = 60.6 if cv == "o" else 38.9
            mid = ((fu * 2 + top[0]) / 3, (fv * 2 + top[1]) / 3, 2.2)
            foot = [(fu, fv, 0.5), mid, top]
            polyline([(*quad(q, p[0], p[1]), p[2]) for p in foot], IRON, r=1.2)
            su, sv = quad(q, 1.0, 0.0)
            tu, tv = quad(q, 0.0, 1.0)
            cu_, cv_ = quad(q, fu, fv)
            pier = (np.abs((U - cu_) * su + (V - cv_) * sv) <= 1.9) & (np.abs((U - cu_) * tu + (V - cv_) * tv) <= 1.9)
            for h in (-1.5, -0.5, 0.5):
                fill_layer(h, pier, STONE)
    # Glazed halls under the leg.
    lo_in, lo_out = 38.9 + 1.6, 60.6 - 1.6
    su, sv = quad(q, 1.0, 0.0)
    tu, tv = quad(q, 0.0, 1.0)
    A = U * su + V * sv
    B = U * tu + V * tv
    inside = (A >= lo_in) & (A <= lo_out) & (B >= lo_in) & (B <= lo_out)
    # Walls a full diagonal step thick, so the glass runs join side to side.
    w = 1.45
    edge = inside & ~((A >= lo_in + w) & (A <= lo_out - w) & (B >= lo_in + w) & (B <= lo_out - w))
    door = (np.abs(A - (lo_in + lo_out) / 2) < 2.0) | (np.abs(B - (lo_in + lo_out) / 2) < 2.0)
    for h in (0.5, 1.5, 2.5):
        fill_layer(h, edge & ~(door & ((A < lo_in + 1) | (B < lo_in + 1))), PANE)
    fill_layer(3.5, edge, IRON)
    fill_layer(0.5, inside & ~edge, PAVING)
    # Face lattice: a strut at each level and one cross a panel, on all four faces.
    levels = [4.5] + LOWER_STRUTS + [51.0] + UPPER_STRUTS[1:] + [104.0]
    # Inner faces keep only struts; their crosses just cluttered the view through the legs.
    for k, (plate, edges) in enumerate(leg_plates(q)):
        struts, braces = truss(plate, edges, levels)
        plate.paint(struts + (braces if k < 2 else []), IRON)


def leg_plates(q, r=0.7):
    """The four faces of leg q as plates, each with its two edge arrises as s(h)."""
    a, b = quad(q, 1.0, 0.0), quad(q, 0.0, 1.0)

    def out(h):
        return leg_outer(h) - r

    def side(h):
        return leg_side(h) + r

    def inner(h):
        return leg_inner(h) + r

    def skew(h, s):
        # Above the first floor the inner corner sits deeper than the side ones.
        f = np.clip((np.asarray(s) - inner(h)) / max(out(h) - inner(h), 1e-6), 0.0, 1.0)
        return inner(h) + (side(h) - inner(h)) * f

    return [
        (Plate(b, a, lambda h, s: out(h)), (side, out)),
        (Plate(a, b, lambda h, s: out(h)), (side, out)),
        (Plate(a, b, skew), (inner, out)),
        (Plate(b, a, skew), (inner, out)),
    ]


def leg_lift(q, top, cabin_h):
    """Inclined lift up the middle of a leg: two rails and one cabin, kept level."""
    def centre(h):
        return (leg_outer(h) + leg_inner(h)) / 2

    hs = [h for h in (3.0, 15.0, 25.0, 35.0, 45.0, 51.0, 58.0, 64.0, 72.0, 80.0, 90.0, 100.0, 110.0) if h <= top]
    for off in (-1.3, 1.3):
        pts = []
        for h in hs:
            c = centre(h)
            pts.append((*quad(q, c + off / math.sqrt(2), c - off / math.sqrt(2)), h))
        polyline(pts, IRON)
    c = centre(cabin_h)
    su, sv = quad(q, 1.0, 0.0)
    tu, tv = quad(q, 0.0, 1.0)
    cu_, cv_ = quad(q, c, c)
    A = (U - cu_) * su + (V - cv_) * sv
    B = (U - cu_) * tu + (V - cv_) * tv
    box = (np.abs(A) <= 2.0) & (np.abs(B) <= 2.0)
    shell = box & ~((np.abs(A) <= 0.55) & (np.abs(B) <= 0.55))
    fill_layer(cabin_h, box, IRON)
    for h in (cabin_h + 1, cabin_h + 2):
        fill_layer(h, shell, PANE)
    fill_layer(cabin_h + 3, box, IRON)


# ---------------------------------------------------------------- arches and first floor

# 18 name bays (3.93 m) would be 5.56 columns on the turned face; 6 (4.24 m) keeps bays alike.
PITCH = 6
F1_OUT = 35.35
GIRDER_LOW, GIRDER_TOP = 44.8, 50.5
ARCH_BAND = 4.2
# Radial posts of the arch band, along the arc.
FAN = 2.9


def face_point(q, s, t, h):
    """Point on face q: s along the face, t out from the axis."""
    return (*quad(q, s, t), h)


def ff_face(q):
    """Arch, spandrel and lattice girder of face q, all on the face's outer plate."""
    p = face_plate(q, lambda h: max(leg_outer(h), F1_OUT), behind=1.0)
    m0 = p.col(0.0, 47.0)
    j_low, j_top = layer(GIRDER_LOW), layer(GIRDER_TOP) - 1
    r_in, r_out = ARCH_R + 0.5, ARCH_R + ARCH_BAND - 0.5

    def sill(m):
        return layer(ARCH_C + math.sqrt(max((r_out + 1.0) ** 2 - (m * CW) ** 2, 0.0)) + 0.5)

    # Spandrel openings: a pier every PITCH columns, round-headed, kept only where whole.
    holes = set()
    for k in range(-8, 8):
        ms = list(range(PITCH * k + 1, PITCH * (k + 1)))
        if max(abs(m) for m in ms) * CW > leg_side(GIRDER_LOW) - 0.3:
            continue
        if j_low - sill(ms[2]) < 2:
            continue
        for m in ms:
            edge = m in (ms[0], ms[-1])
            for j in range(sill(m), j_low - (1 if edge else 0)):
                holes.add((m, j))

    def fn(S, N, h):
        out = np.zeros(len(S), np.uint16)
        m = N - m0
        j = layer(h)
        inside = np.abs(S) < leg_side(h) + 0.7
        rr = np.hypot(S, h - ARCH_C)
        if j < j_low:
            sp = inside & (rr >= r_out)
            hole = np.array([(int(mm), j) in holes for mm in m], bool)
            out[sp & ~hole] = IRON
        elif j <= j_top:
            mm = np.mod(m, PITCH)
            chord = j in (j_low, j_top)
            arm = (j in (j_low + 1, j_top - 1)) & ((mm == 1) | (mm == PITCH - 1))
            out[inside & (chord | (mm == 0) | arm)] = IRON
        return out

    p.pattern(layer(0.6), j_top, fn)
    # Flanges and the radial posts between them, as single joined lines.
    span = math.pi / 2 + 0.04
    cells = []
    for rad in (r_in, r_out):
        prev = None
        for ph in np.linspace(-span, span, 1200):
            sx, hx = rad * math.sin(ph), ARCH_C + rad * math.cos(ph)
            if hx < 0.6:
                prev = None
                continue
            c = p.cell(sx, hx)
            if prev is not None and c != prev:
                cells += pix_line(prev, c)
            prev = c
    dphi = FAN / (ARCH_R + ARCH_BAND / 2)
    for k in range(-int(span / dphi), int(span / dphi) + 1):
        ph = k * dphi
        a = p.cell(r_in * math.sin(ph), ARCH_C + r_in * math.cos(ph))
        b = p.cell(r_out * math.sin(ph), ARCH_C + r_out * math.cos(ph))
        if ARCH_C + r_in * math.cos(ph) >= 0.6:
            cells += pix_line(a, b)
    p.paint(cells, IRON)


def first_floor():
    t_out = F1_OUT
    band2 = (T <= t_out + 0.05) & (T > t_out - 1.5)
    for h in (50.5, 51.5):
        fill_layer(h, band2, IRON)
    recess = (T <= t_out - 1.0) & (T > t_out - 2.5)
    for h in (52.5, 53.5, 54.5, 55.5):
        fill_layer(h, recess, IRON)
    fill_layer(56.5, (T <= t_out + 0.45) & (T > t_out - 2.5), IRON)
    # A console over each girder post, standing proud of the band so it reads.
    plates = [face_plate(q, lambda h: t_out, behind=1.0) for q in range(4)]
    posts = [[p.col(0.0, 54.0) + PITCH * k for k in range(-8, 9)] for p in plates]
    for q, (p, ns) in enumerate(zip(plates, posts)):
        cells = [(n, j) for n in ns for j in range(layer(52.5), layer(55.5) + 1)]
        p.paint(cells, IRON)
        face_plate(q, lambda h: t_out + 0.7, behind=1.0).paint(cells, IRON)
    deck = (T <= t_out + 0.45) & (T >= 17.0)
    fill_layer(FLOOR1, deck, DECK)
    # Glass floor round the void and a glass balustrade on it.
    fill_layer(FLOOR1, (T >= 17.0) & (T < 18.6), GLASS)
    ring_line(17.4, FLOOR1 + 1, PANE)
    ring_line(t_out - 0.1, FLOOR1 + 1, FENCE)
    # Gallery: a post at each console, a light canopy over the walk.
    for p, ns in zip(plates, posts):
        p.paint([(n, j) for n in ns for j in range(layer(59.5), layer(61.5) + 1)], FENCE)
    fill_layer(62.0, (T <= t_out + 0.4) & (T >= 31.0), CANOPY)
    # Ring girder round the void and cross girders under the deck.
    for h in np.arange(53.0, 57.0, 1.0):
        ring_line(17.5, h, IRON)
    for q in range(4):
        for k in range(-4, 5):
            s = k * PITCH * CW
            line(face_point(q, s, 17.5, 56.5), face_point(q, s, t_out - 1.5, 56.5), IRON)


def pavilions():
    """Glass pavilions of the first floor on the NE, SW and NW faces."""
    s0, s1, t0, t1 = -13.0, 13.0, 18.6, 31.5
    for q in (2, 0, 1):
        su, sv = quad(q, 1.0, 0.0)
        tu, tv = quad(q, 0.0, 1.0)
        S = U * su + V * sv
        Tq = U * tu + V * tv
        inside = (S >= s0) & (S <= s1) & (Tq >= t0) & (Tq <= t1)
        wall = inside & ~((S >= s0 + 1.45) & (S <= s1 - 1.45) & (Tq >= t0 + 1.45) & (Tq <= t1 - 1.45))
        for h in np.arange(58.0, 63.0, 1.0):
            fill_layer(h, wall, PANE)
        fill_layer(63.0, inside, IRON)
        for s in np.arange(s0, s1 + 0.1, 2.2):
            for t in (t0 + 0.5, t1 - 0.5):
                line(face_point(q, s, t, 58.0), face_point(q, s, t, 62.5), IRON)


# ---------------------------------------------------------------- second floor and pillar


def ring(t0, t1):
    """Plan band between two outer half-widths."""
    return (T <= t1) & (T > t0)


def ring_line(t, h, b):
    """A closed square at half-width t, drawn as joined lines so it never breaks into steps."""
    for q in range(4):
        line(face_point(q, -t, t, h), face_point(q, t, t, h), b)


def lift_hole():
    return (np.abs(U) < 3.2) & (np.abs(V) < 2.2)


def second_floor():
    t_out = 20.48
    # Crossed girders between the legs, under the plate girder.
    # Three panels a face, each with a post either side and one cross.
    half = leg_side(107.0) + 0.5
    for q in range(4):
        p = face_plate(q, lambda h: 19.4)
        cells = []
        for k in range(3):
            s0, s1 = -half + 2 * half * k / 3, -half + 2 * half * (k + 1) / 3
            cells += sum(truss(p, (lambda h, s0=s0: s0, lambda h, s1=s1: s1), [104.5, 109.5], top=False), [])
            for s_ in (s0, s1):
                cells += pix_line(p.cell(s_, 104.5), p.cell(s_, 109.5))
        p.paint(cells, IRON)
    # Plate girder, cornice and deck.
    for h in np.arange(109.5, 114.6, 1.0):
        fill_layer(h, ring(t_out - 1.6, t_out - 0.1), IRON)
    fill_layer(114.5, ring(t_out - 1.5, t_out + 0.4), IRON)
    fill_layer(FLOOR2 - 0.5, T <= t_out + 0.4, DECK)
    fill_layer(FLOOR2 - 0.5, lift_hole(), 0)
    # Balustrade, then the safety fence on posts.
    ring_line(t_out - 0.1, FLOOR2 + 0.5, FENCE)
    for q in range(4):
        for k in range(-7, 8):
            s = k * 2 * math.sqrt(2)
            line(face_point(q, s, t_out - 0.2, 116.5), face_point(q, s, t_out - 0.2, 118.5), FENCE)
    ring_line(t_out - 0.2, 118.5, FENCE)
    # Upper level: glazed rooms under a deck with its own railing.
    for h in (116.5, 117.5, 118.5):
        ring_line(17.0, h, PANE)
    for q in range(4):
        for k in range(-6, 7):
            s = k * 2 * math.sqrt(2)
            line(face_point(q, s, 17.0, 116.5), face_point(q, s, 17.0, 118.5), IRON)
    fill_layer(119.5, T <= 17.8, DECK)
    fill_layer(119.5, lift_hole(), 0)
    ring_line(17.4, 120.5, FENCE)


def upper_pillar():
    hs = [h for h, _ in PILLAR] + [h for h, _ in FLARE[1:]]
    for q in range(4):
        pts = []
        for h in hs:
            r = 0.85 if h < 200 else 0.7
            c = pillar(h) - r
            pts.append(face_point(q, c, c, h))
        for p0, p1 in zip(pts, pts[1:]):
            rod(p0, p1, 0.85 if p0[2] < 200 else 0.7, IRON)
        # Wide panels low down carry two crosses meeting mid-face, the rest one.
        p = face_plate(q, lambda h: pillar(h) - 0.85)
        edges = (lambda h: -(pillar(h) - 0.85), lambda h: pillar(h) - 0.85)
        low = [h for h in PILLAR_STRUTS if h <= 188.75]
        high = [h for h in PILLAR_STRUTS if h >= 188.75] + [262.0, 266.5, 270.5]
        p.paint(sum(truss(p, edges, low, crosses=2, top=False) + truss(p, edges, high), []), IRON)
        # Under the gallery: a middle post and two arches a face, on the flaring surface.
        mid = [face_point(q, 0.0, pillar(h) - 0.4, h) for h in (259.2, 262.0, 266.0, 269.0, 272.0, 275.0)]
        polyline(mid, IRON)
        for sgn in (-1, 1):
            arcp = []
            for k in range(17):
                a = k / 16 * math.pi
                h = 268.5 + 6.5 * math.sin(a)
                c = pillar(h) - 0.4
                arcp.append(face_point(q, sgn * c * (1 - math.cos(a)) / 2, c, h))
            polyline(arcp, IRON, r=0.5)
    # Spandrels close the arches under the gallery.
    for h in np.arange(268.5, 275.0, 1.0):
        c = pillar(h)
        band = ring(c - 1.5, c)
        S0 = np.minimum(np.abs(U), np.abs(V))
        crown = 268.5 + 6.5 * np.sin(np.pi * np.clip(S0 / max(c, 1.0), 0, 1))
        fill_layer(h, band & (h + 0.5 >= crown), IRON)
    # Lift guides up the middle, with the two cabins of the duo lift.
    for su, sv in ((-2.5, -1.5), (2.5, -1.5), (-2.5, 1.5), (2.5, 1.5)):
        line((su, sv, FLOOR2), (su, sv, FLOOR3 - 1.5), FENCE)
    for h in PILLAR_STRUTS[1::3]:
        frame = [(-2.5, -1.5, h), (2.5, -1.5, h), (2.5, 1.5, h), (-2.5, 1.5, h), (-2.5, -1.5, h)]
        polyline(frame, IRON)
    for (su, h0) in ((-1.2, 180.0), (1.2, 232.0)):
        for h in (h0, h0 + 1, h0 + 2):
            fill_layer(h, (np.abs(U - su) < 1.1) & (np.abs(V) < 1.1), PANE)
        fill_layer(h0 + 3, (np.abs(U - su) < 1.1) & (np.abs(V) < 1.1), IRON)
    # Platform inside the pillar at 196 m, clear in the LiDAR.
    fill_layer(196.0, ring(pillar(196.0) - 3.0, pillar(196.0) - 0.8), IRON)


def summit():
    fill_layer(FLOOR3 - 0.5, T <= 9.33, DECK)
    fill_layer(FLOOR3 - 0.5, ring(8.3, 9.33), IRON)
    fill_layer(FLOOR3 - 0.5, lift_hole(), 0)
    fill_layer(FLOOR3 - 1.5, ring(7.6, 9.33), IRON)
    # Glazed gallery, a mullion every two steps of the face.
    for h in (276.5, 277.5, 278.5):
        ring_line(8.6, h, DARK_GLASS)
    for q in range(4):
        for k in range(-3, 4):
            s = k * 2 * math.sqrt(2)
            line(face_point(q, s, 8.7, 276.5), face_point(q, s, 8.7, 278.5), IRON)
        line(face_point(q, 8.9, 8.9, 276.5), face_point(q, 8.9, 8.9, 278.5), IRON)
    fill_layer(279.5, T <= 9.33, IRON)
    # Upper terrace: railing, then the cage of bars bowing in at the top.
    ring_line(8.8, 280.5, FENCE)
    for q in range(4):
        for k in range(-5, 6):
            s = k * math.sqrt(2)
            pts = [face_point(q, s, 7.6, 280.5), face_point(q, s, 7.6, 285.5), face_point(q, s * 0.85, 6.5, 287.5)]
            polyline(pts, FENCE)
        line(face_point(q, 7.6, 7.6, 280.5), face_point(q, 7.6, 7.6, 285.5), IRON)
    ring_line(7.2, 286.5, FENCE)
    ring_line(6.1, 287.5, IRON)
    # Rooms of the top: Eiffel's office and the equipment over it.
    for h in np.arange(280.5, 288.0, 1.0):
        ring_line(4.6, h, IRON)
    for h in (282.5, 283.5):
        fill_layer(h, ring(3.8, 5.4) & (np.minimum(np.abs(U), np.abs(V)) < 2.0) & (grid[:, layer(h), :] != 0), DARK_GLASS)
    fill_layer(288.5, T <= 5.2, IRON)
    fill_layer(289.5, T <= 4.0, IRON)
    # Campanile with the lighthouse lantern.
    for q in range(4):
        polyline([face_point(q, 3.2, 3.2, 290.5), face_point(q, 2.3, 2.3, 293.5), face_point(q, 1.4, 1.4, 296.5)], IRON)
    for h, t in ((290.5, 3.1), (293.5, 2.2)):
        ring_line(t, h, IRON)
    for h in (296.5, 297.5):
        fill_layer(h, T <= 1.3, GLASS)
        fill_layer(h, T <= 0.75, LAMP)
    fill_layer(298.5, T <= 1.6, IRON)
    # Antenna mast: lattice with dipoles, the radome stack, the crown and the top mast.
    for su, sv in ((-0.9, -0.9), (0.9, -0.9), (0.9, 0.9), (-0.9, 0.9)):
        line((su, sv, 299.5), (su, sv, 307.5), FENCE)
    for h in (299.5, 302.5, 305.5):
        ring_line(0.9, h, FENCE)
    for h in (301.5, 304.5, 307.5):
        for q in range(4):
            line(face_point(q, 0.0, 1.0, h), face_point(q, 0.0, 2.6, h), FENCE)
    for h in np.arange(308.5, 320.0, 1.0):
        fill_layer(h, np.hypot(U, V) <= 1.25, IRON)
    fill_layer(320.5, np.hypot(U, V) <= 1.7, IRON)
    for q in range(4):
        line(face_point(q, 0.0, 1.0, 321.5), face_point(q, 0.0, 3.0, 321.5), FENCE)
    line((0.0, 0.0, 321.5), (0.0, 0.0, 329.5), FENCE)


# ---------------------------------------------------------------- night lighting


def _grow(m, r):
    for ax in range(3):
        out = m.copy()
        for d in range(1, r + 1):
            out |= np.roll(m, d, ax) | np.roll(m, -d, ax)
        m = out
    return m


def light_at(points, under=True):
    """Invisible lights at tower-frame points, only into air."""
    for u, v, h in points:
        x, y, z = to_model(u, v, h)
        put([int(math.floor(x))], [int(math.floor(y))], [int(math.floor(z))], LIGHT, under)


def frame_loose_glass():
    """A pane with nothing beside it to join would stand as a thin sliver; make it a frame post."""
    glass = np.isin(grid, [PANE, DARK_GLASS])
    held = glass | np.isin(grid, [IRON, DECK, STONE, GLASS])
    side = np.zeros_like(glass)
    for ax in (0, 2):
        side |= np.roll(held, 1, ax) | np.roll(held, -1, ax)
    grid[glass & ~side] = IRON


def night_lights(step=8):
    """The golden lighting comes from lamps inside the iron, so the lattice gets sparse
    invisible lights in the air just within it, the nearest to the middle of each step cube."""
    ok = (grid == 0) & _grow(grid == IRON, 2)
    for j in range(NY):
        h = j - GY + 0.5
        lim = 0.0 if h < 6.0 or h > 275.0 else leg_outer(h) if h < 116.0 else pillar(h)
        ok[:, j, :] &= T <= lim - 0.5
    i, j, k = np.nonzero(ok)
    c = (step - 1) / 2
    d = (i % step - c) ** 2 + (j % step - c) ** 2 + (k % step - c) ** 2
    key = ((i // step) * 100 + j // step) * 100 + k // step
    order = np.lexsort((d, key))
    first = np.r_[True, key[order][1:] != key[order][:-1]]
    sel = order[first]
    grid[i[sel], j[sel], k[sel]] = LIGHT
    # Lit rooms: first-floor pavilions and gallery, second-floor rooms, the top galleries.
    pts = []
    for q in range(4):
        pts += [face_point(q, s, 25.0, 61.0) for s in (-8.0, 0.0, 8.0) if q != 3]
        pts += [face_point(q, s, 33.0, 61.0) for s in (-24.0, -12.0, 0.0, 12.0, 24.0)]
        pts += [face_point(q, s, 14.0, 118.0) for s in (-8.0, 0.0, 8.0)]
        pts += [face_point(q, s, 6.8, 277.5) for s in (-4.0, 4.0)]
        pts += [face_point(q, 0.0, 3.0, 284.0)]
        # The frieze and the second-floor girder are lit from in front, like their floodlights.
        pts += [face_point(q, k * 2 * PITCH * CW, F1_OUT + 1.2, 54.0) for k in range(-4, 5)]
        pts += [face_point(q, k * 2 * PITCH * CW, 21.7, 112.0) for k in range(-2, 3)]
    light_at(pts)


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
    else:
        raise ValueError(tag)


def _varints(ids):
    out = bytearray()
    for n in ids:
        while True:
            byte = n & 0x7F
            n >>= 7
            if n:
                out.append(byte | 0x80)
            else:
                out.append(byte)
                break
    return out


def sponge():
    used = np.unique(grid)
    remap = np.zeros(len(PALETTE), np.int64)
    names = {}
    for new, old in enumerate(used):
        remap[old] = new
        names[PALETTE[old]] = new
    # Sponge order: index = (y * length + z) * width + x.
    data = _varints(remap[grid.transpose(1, 2, 0)].ravel().tolist())
    blocks = {
        "Palette": (10, {k: (3, v) for k, v in names.items()}),
        "Data": (7, data),
        "BlockEntities": (9, (10, [])),
    }
    schematic = {
        "Version": (3, 3),
        "DataVersion": (3, DATA_VERSION),
        "Width": (2, NX),
        "Height": (2, NY),
        "Length": (2, NZ),
        "Offset": (11, [0, 0, 0]),
        "Blocks": (10, blocks),
    }
    out = io.BytesIO()
    out.write(b"\x0a" + struct.pack(">H", 0))
    _payload(out, 10, {"Schematic": (10, schematic)})
    return gzip.compress(out.getvalue(), mtime=0)


THIN = {"minecraft:jungle_fence": "fence", "minecraft:glass_pane": "pane",
        "minecraft:gray_stained_glass_pane": "pane"}


def connect():
    """Give fences and panes the sides they join, as the game would on placement."""
    for k in ("fence", "pane"):
        pad = np.pad(grid, ((1, 1), (0, 0), (1, 1)))
        sides = {"north": pad[1:-1, :, :-2], "east": pad[2:, :, 1:-1], "south": pad[1:-1, :, 2:],
                 "west": pad[:-2, :, 1:-1]}
        names = list(PALETTE)
        kind = np.array([THIN.get(n.split("[")[0], "") for n in names])
        full = np.array([not any(w in n for w in ("slab", "fence", "pane", "trapdoor", "stairs", "air")) for n in names])
        ids = kind == k
        if k == "pane":
            ids_join = ids | np.array([n == "minecraft:glass" for n in names])
        else:
            ids_join = ids
        here = ids[grid]
        if not here.any():
            continue
        code = np.zeros(grid.shape, np.int32)
        for i, n in enumerate(sides.values()):
            code |= (full[n] | ids_join[n]).astype(np.int32) << i
        for b_old in np.unique(grid[here]):
            base = names[b_old].split("[")[0]
            sel = grid == b_old
            for c in np.unique(code[sel]):
                f = {d: "true" if c >> i & 1 else "false" for i, d in enumerate(sides)}
                state = f"{base}[east={f['east']},north={f['north']},south={f['south']},waterlogged=false,west={f['west']}]"
                grid[sel & (code == c)] = blk(state)


def build():
    for q in range(4):
        leg(q)
    # East and West lifts run to the second floor, the North one to the first.
    leg_lift(3, 110.0, 86.0)
    leg_lift(1, 110.0, 31.0)
    leg_lift(2, 58.0, 17.0)
    for q in range(4):
        ff_face(q)
    first_floor()
    pavilions()
    second_floor()
    upper_pillar()
    summit()
    frame_loose_glass()
    night_lights()
    connect()


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "eiffel_tower.schem")
    build()
    with open(out, "wb") as f:
        f.write(sponge())
    n = int((grid != 0).sum())
    print(f"{out}: {NX}x{NY}x{NZ}, {n} blocks, {len(np.unique(grid)) - 1} states")


if __name__ == "__main__":
    main()
