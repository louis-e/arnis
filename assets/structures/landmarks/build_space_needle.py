#!/usr/bin/env python3
"""Builds space_needle.schem, the Space Needle in Seattle, beside this script.

    python3 assets/structures/landmarks/build_space_needle.py

One block per metre, x east, z south, the tower axis on the middle of block (C, C). Layer GROUND
sits on the plaza; the leg footings go GROUND layers below it. Deps: numpy.

Heights and plan come from USGS 3DEP lidar (WA_KingCo_1_2021, public domain): the six leg
columns (three pairs facing 60, 180 and 300 deg) and their 10.3 m rungs, the SkyLine and 200 ft
platforms, the spiral roof of the base pavilion, the halo (21.1 m radius, the published 138 ft
across, at 156-157 m), the roof cone and the spire to 184.5 m. Where the scan cannot see
(under the top house, the waist), the elevation in the Space Needle renovation fact sheet
(2018, 605/520/500/100 ft marks) and 2023-24 photos on Wikimedia Commons fill in. Paint as
since May 2023: Astronaut White with the roof white again.
"""
import gzip
import io
import math
import os
import re
import struct
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
OUT = os.path.join(HERE, "space_needle.schem")
SCHEMATIC_RS = os.path.join(HERE, "..", "..", "..", "src", "structures", "schematic.rs")
DATA_VERSION = 4438

C = 25
# Tower axis, fitted to the leg columns, roof cone, lantern and spire in the scan.
LAT, LON = 47.620_503_9, -122.349_283_9
W = L = 2 * C + 1
GROUND = 2
TOP = 187
H = GROUND + TOP + 1

LEGS = (60.0, 180.0, 300.0)
GAPS = (0.0, 120.0, 240.0)

WHITE = "smooth_quartz"
WHITE_SLAB = "smooth_quartz_slab"
WHITE_STAIRS = "smooth_quartz_stairs"
DARK_GLASS = "black_stained_glass"
CLEAR_GLASS = "glass"
PANE = "glass_pane"
STEEL = "polished_blackstone"
SHAFT = "polished_blackstone_bricks"
LATTICE = "iron_bars"
RIM_EDGE = "smooth_sandstone"
JOIST = "spruce_slab"
GRATE = "iron_trapdoor"
LANTERN_ROOF = "polished_deepslate_slab"
SOFFIT = "polished_deepslate"
SOFFIT_LIGHT = "polished_diorite"
LANTERN = "polished_deepslate"
LOUVRE = "tinted_glass"
CAB = "smooth_sandstone"
PAVILION_ROOF = "smooth_stone"
PAVILION_SLAB = "smooth_stone_slab"
BEACON = "red_concrete"
LAMP = "sea_lantern"
LIGHT = "light[level=15,waterlogged=false]"
LOUPE = "tinted_glass"
MAST = "blackstone_wall"


class Model:
    def __init__(self):
        self.grid = np.zeros((W, H, L), np.int16)
        self.names = ["air"]
        ix, iy, iz = np.meshgrid(np.arange(W), np.arange(H), np.arange(L), indexing="ij")
        self.e = (ix - C).astype(float)
        self.n = (C - iz).astype(float)
        self.z = iy - GROUND + 0.5
        self.r = np.hypot(self.e, self.n)
        self.phi = np.degrees(np.arctan2(self.e, self.n)) % 360

    def id(self, name):
        if name not in self.names:
            self.names.append(name)
        return self.names.index(name)

    def put(self, mask, name):
        self.grid[mask] = self.id(name)

    def layer(self, z):
        """Grid index of the layer holding height z (m above the plaza)."""
        return GROUND + int(math.floor(z))

    def cell(self, e, n, z):
        return C + int(round(e)), self.layer(z), C - int(round(n))

    def set(self, e, n, z, name):
        x, y, zz = self.cell(e, n, z)
        if 0 <= x < W and 0 <= y < H and 0 <= zz < L:
            self.grid[x, y, zz] = self.id(name)

    def line(self, a, b, name, step=0.2):
        """Face-connected: a diagonal step gets the cells between, so it reads as one member."""
        a, b = np.array(a, float), np.array(b, float)
        prev = None
        for t in np.linspace(0, 1, max(2, int(np.linalg.norm(b - a) / step) + 1)):
            cur = list(self.cell(*(a + (b - a) * t)))
            if prev is not None:
                for k in (0, 2, 1):
                    if prev[k] != cur[k]:
                        prev[k] = cur[k]
                        self.mark(prev, name)
            self.mark(cur, name)
            prev = cur

    def mark(self, cell, name):
        x, y, zz = cell
        if 0 <= x < W and 0 <= y < H and 0 <= zz < L:
            self.grid[x, y, zz] = self.id(name)


def ang(d):
    """Signed angle difference in degrees."""
    return (d + 180) % 360 - 180


def near(phi, centres):
    """Angle to the nearest of `centres`, degrees."""
    return np.min([np.abs(ang(phi - c)) for c in centres], axis=0)


# ---------------------------------------------------------------- legs

RUNGS = [10.3 * k for k in range(1, 9)]
FINS = 32


def leg_rho(z):
    """Centre radius of a leg pair: a straight lean to 86 m, eased into the waist at 115 m."""
    z = np.asarray(z, float)
    s = np.clip((z - 86) / 29, 0, 1)
    ease = (2 * s**3 - 3 * s**2 + 1) * 5.98 - 0.1174 * 29 * (s**3 - 2 * s**2 + s)
    ease += (-2 * s**3 + 3 * s**2) * 4.1
    return np.where(z < 86, 16.08 - 0.1174 * z, ease)


def leg_t(z):
    """Half the spacing of a pair's two columns, which close up into one plate above 92 m."""
    z = np.asarray(z, float)
    return np.where(z < 88, 2.8 - 0.0119 * z, np.maximum(1.6, 1.753 - 0.038 * (z - 88)))


def arm_rho(z):
    """Above the waist each leg forks: the outer arm flares to the skirt collar."""
    return 4.1 + 4.8 * np.clip((z - 115) / 33.5, 0, 1) ** 1.15


def rnd(v):
    return math.floor(v + 0.5)


def layers(z0, z1):
    return [y for y in range(H) if z0 <= y - GROUND + 0.5 < z1]


HW = 1.2  # half-width of a column; each face then steps on its own, in an even rhythm


def member(mask, y, a, b, hw=HW):
    """Cells within hw (in x and z) of the segment a-b, in layer y."""
    pts = np.linspace(a, b, 25)
    for ie in range(rnd(min(a[0], b[0]) - hw) - 1, rnd(max(a[0], b[0]) + hw) + 2):
        for jn in range(rnd(min(a[1], b[1]) - hw) - 1, rnd(max(a[1], b[1]) + hw) + 2):
            if np.min(np.maximum(np.abs(pts[:, 0] - ie), np.abs(pts[:, 1] - jn))) <= hw + 1e-6:
                mask[C + ie, y, C - jn] = True


def leg(p):
    """Leg facing p: column pair with rungs, waist plate, two arms. Returns (leaning, all)."""
    s, c = math.sin(math.radians(p)), math.cos(math.radians(p))
    lean = np.zeros((W, H, L), bool)
    fixed = np.zeros_like(lean)

    def at(rho, t):
        return np.array((rho * s + t * c, rho * c - t * s))

    for y in layers(-GROUND, 92):
        z = y - GROUND + 0.5
        rho, t = float(leg_rho(z)), float(leg_t(z))
        for side in (-1, 1):
            member(lean, y, at(rho, side * t), at(rho, side * t))
        if any(abs(z - zr) < 0.5 for zr in RUNGS):
            member(fixed, y, at(rho, -t), at(rho, t), 1.0)
    for y in layers(92, 121):
        rho = float(leg_rho(y - GROUND + 0.5))
        member(lean, y, at(rho, -1.6), at(rho, 1.6))
    for y in layers(115, 148.6):
        ro = float(arm_rho(y - GROUND + 0.5))
        member(lean, y, at(ro, -0.5), at(ro, 0.5))
    for y in layers(115, 151):
        member(fixed, y, at(3.9, -0.5), at(3.9, 0.5))
    return lean, lean | fixed


def mirror(mask):
    """Mirror east-west about the axis."""
    return mask[::-1, :, :]


def legs(m):
    """The 300 deg leg mirrors the 60 deg one; the south leg is symmetric in itself."""
    lean, every = leg(180.0)
    ne_lean, ne_every = leg(60.0)
    lean |= mirror(lean) | ne_lean | mirror(ne_lean)
    every |= mirror(every) | ne_every | mirror(ne_every)
    m.put(every, WHITE)
    m.leg_lean = lean
    m.leg_all = every


DIRS = {"east": (1, 0), "west": (-1, 0), "south": (0, 1), "north": (0, -1)}


def chamfer_legs(m):
    """Quartz stairs on each step of the leaning members, ledges and overhangs alike."""
    leg_all, g = m.leg_all, m.grid
    white = m.id(WHITE)
    diag = {(1, 1): "east", (1, -1): "east", (-1, 1): "west", (-1, -1): "west"}

    def has(x, y, z):
        return 0 <= x < W and 0 <= z < L and leg_all[x, y, z]

    for x, y, z in np.argwhere(m.leg_lean):
        if g[x, y, z] != white or y == 0 or y + 1 >= H:
            continue
        for half, dy in (("bottom", 1), ("top", -1)):
            if leg_all[x, y + dy, z] or g[x, y + dy, z] != 0:
                continue
            face = next((d for d, (dx, dz) in DIRS.items()
                         if has(x + dx, y + dy, z + dz) and has(x + dx, y, z + dz)), None)
            if face is None:
                face = next((d for (dx, dz), d in diag.items()
                             if has(x + dx, y + dy, z + dz) and has(x + dx, y, z + dz)
                             and has(x, y, z + dz)), None)
            if face:
                g[x, y, z] = m.id(stairs(WHITE_STAIRS, face, half))
                break


def stairs(name, facing, half):
    return f"{name}[facing={facing},half={half},shape=straight,waterlogged=false]"


def stair_shapes(m):
    """Corner shapes where stairs meet at right angles, by Minecraft's own rule."""
    g = m.grid
    info = {}
    for i, n in enumerate(m.names):
        if "_stairs[" in n:
            p = dict(kv.split("=") for kv in n[n.index("[") + 1:-1].split(","))
            info[i] = (n[:n.index("[")], p["facing"], p["half"])
    if not info:
        return
    ccw = {"north": "west", "west": "south", "south": "east", "east": "north"}
    opp = {"north": "south", "south": "north", "east": "west", "west": "east"}

    def at(x, y, z, d):
        dx, dz = DIRS[d]
        if 0 <= x + dx < W and 0 <= z + dz < L:
            return info.get(int(g[x + dx, y, z + dz]))
        return None

    def across(a, b):
        return (DIRS[a][0] == 0) != (DIRS[b][0] == 0)

    def free(x, y, z, me, d):
        o = at(x, y, z, d)
        return o is None or o[1] != me[1] or o[2] != me[2]

    shapes = {}
    for x, y, z in np.argwhere(np.isin(g, list(info))):
        me = info[int(g[x, y, z])]
        shape = "straight"
        back = at(x, y, z, me[1])
        if back and back[2] == me[2] and across(back[1], me[1]) \
                and free(x, y, z, me, opp[back[1]]):
            shape = "outer_left" if back[1] == ccw[me[1]] else "outer_right"
        else:
            front = at(x, y, z, opp[me[1]])
            if front and front[2] == me[2] and across(front[1], me[1]) \
                    and free(x, y, z, me, front[1]):
                shape = "inner_left" if front[1] == ccw[me[1]] else "inner_right"
        shapes[(x, y, z)] = (me, shape)
    for (x, y, z), (me, shape) in shapes.items():
        g[x, y, z] = m.id(f"{me[0]}[facing={me[1]},half={me[2]},shape={shape},waterlogged=false]")


# ---------------------------------------------------------------- core and lifts


def core(m):
    """Steel lattice stair and lift tower, 5 m square, X-braced in 5 m panels."""
    sq = (np.abs(m.e) <= 2) & (np.abs(m.n) <= 2)
    face = sq & ((np.abs(m.e) == 2) | (np.abs(m.n) == 2))
    corner = (np.abs(m.e) == 2) & (np.abs(m.n) == 2)
    span = (m.z > -GROUND) & (m.z < 151)
    m.put(sq & ~face & span, SHAFT)
    m.put(face & span, LATTICE)
    m.put(corner & span, STEEL)
    zi = np.floor(m.z)
    k = np.mod(zi, 5)
    u = np.where(np.abs(m.n) == 2, m.e, m.n)
    brace = face & ~corner & span & ((u + 2 == k) | (2 - u == k))
    m.put(brace, STEEL)
    for zb in range(10, 150, 10):
        m.put(face & (zi == zb), STEEL)


def lifts(m):
    """Two passenger cabs and the freight lift ride outside the core on guide rails."""
    for side, zc in (((0, 1), 118), ((1, 0), 47), ((-1, 0), 83)):
        dx, dn = side
        for t in (-1, 1):
            for z in range(1, 150):
                if dx:
                    m.set(3 * dx, t, z, LATTICE)
                else:
                    m.set(t, 3 * dn, z, LATTICE)
        for z in range(zc, zc + 3):
            for t in (-1, 0, 1):
                name = DARK_GLASS if z == zc + 1 and t == 0 else CAB
                if dx:
                    m.set(3 * dx, t, z, name)
                else:
                    m.set(t, 3 * dn, z, name)


# ---------------------------------------------------------------- platforms


def skyline(m):
    """SkyLine level (1982) at 100 ft: a round centre and three glazed lobes between the legs."""
    off = near(m.phi, GAPS)
    # lobe outline: flat front at 14.3 m over +-28 deg, sides back to 12 m by 40 deg
    front = 14.3 / np.cos(np.radians(np.minimum(off, 28)))
    side = 16.2 - (16.2 - 12.0) * np.clip((off - 28) / 12, 0, 1)
    lobe_r = np.where(off <= 28, front, np.where(off <= 40, side, 0))
    d = m.r * np.cos(np.radians(np.minimum(off, 40)))
    zi = np.floor(m.z)
    in_lobe = m.r <= lobe_r + 0.3
    centre = m.r <= 11.5
    # floor plate and roof, the centre a storey higher round the lift lobby
    m.put((in_lobe | centre) & (zi == 30), WHITE)
    m.put(in_lobe & (m.r <= lobe_r + 0.6) & (zi == 34), WHITE)
    m.put(centre & (zi >= 34) & (zi <= 35), WHITE)
    # glazing band, leaning out as it rises, on a white fascia
    for y, inset in ((31, 1.0), (32, 0.5), (33, 0.0)):
        edge = in_lobe & (m.r <= lobe_r - inset) & (m.r > lobe_r - inset - 1.0)
        m.put(edge & (zi == y), DARK_GLASS)
    m.put(in_lobe & (m.r > lobe_r - 2.2) & (m.r <= lobe_r - 1.0) & (zi == 30), WHITE)
    # the soffit steps back down to the legs, ribbed every 6 deg
    for zz, dmin, dmax in ((29, 10.6, 12.6), (28, 9.2, 11.2)):
        under = in_lobe & (zi == zz) & (d > dmin) & (d < dmax)
        m.put(under, SOFFIT_LIGHT)
        m.put(under & spokes(m.phi, m.r, 60, 0.7), WHITE)
    # centre walls round the core
    m.put(centre & (m.r > 10.5) & (zi >= 31) & (zi <= 33) & ~in_lobe, WHITE)


def deck200(m):
    """The 200 ft platform: a hexagon between the legs on K-braces to the core."""
    verts = []
    for p in LEGS:
        verts += [p - 20, p + 20]
    verts = sorted(v % 360 for v in verts)
    # hexagon with vertices at leg +-20 deg, 9.5 m out
    pts = [(9.5 * math.sin(math.radians(v)), 9.5 * math.cos(math.radians(v))) for v in verts]
    inside = np.ones(m.e.shape, bool)
    for i in range(6):
        (x0, y0), (x1, y1) = pts[i], pts[(i + 1) % 6]
        inside &= (x1 - x0) * (m.n - y0) - (y1 - y0) * (m.e - x0) <= 0.3
    zi = np.floor(m.z)
    m.put(inside & (zi == 60), WHITE)
    hexring = inside & ~(
        np.roll(inside, 1, 0) & np.roll(inside, -1, 0) & np.roll(inside, 1, 2) & np.roll(inside, -1, 2)
    )
    m.put(close_ring(hexring & (zi == 61), 9.0), LATTICE)
    for g in GAPS:
        s, c = math.sin(math.radians(g)), math.cos(math.radians(g))
        for o in (-3.0, 3.0):
            a = (7.0 * s + o * c, 7.0 * c - o * s, 60)
            b = (2.5 * s + o * 0.4 * c, 2.5 * c - o * 0.4 * s, 54)
            m.line(a, b, WHITE)


def pavilion(m):
    """Base pavilion: its roof spirals up from 5.6 m at 85 deg to 9.2 m at 35 deg (lidar)."""
    zi = np.floor(m.z)
    r, phi = m.r, m.phi
    u = np.mod(phi - 85, 360)
    h = 5.6 + 0.0116 * u
    inside = (r <= 21.3) & (u <= 310)
    walls = inside & ((r > 20.4) | (u < 2.5) | (u > 307.5) | ((r > 9.2) & (r <= 10.0)))
    glass = walls & (zi >= 0) & (zi + 1 <= h - 0.5)
    m.put(glass, DARK_GLASS)
    roof = inside & (zi == np.floor(h - 1))
    m.put(roof, PAVILION_ROOF)
    m.put(roof & ((r > 20.4) | (u < 2.5) | (u > 307.5)), STEEL)
    m.put(inside & (zi == np.floor(h - 1) + 1) & (h - np.floor(h) >= 0.5),
          f"{PAVILION_SLAB}[type=bottom,waterlogged=false]")
    m.put((r <= 21.3) & (zi == -1), PAVILION_ROOF)


# ---------------------------------------------------------------- top house


def spokes(phi, r, count, width, offset=0.0):
    """True within width/2 metres of one of `count` evenly spaced radial lines."""
    step = 360 / count
    a = np.mod(phi - offset, step)
    return np.minimum(a, step - a) * np.pi / 180 * r < width / 2


def rays(count, r0, r1, offset=0.0):
    """Cells of `count` one-block radial lines with their radius, mirrored from one octant."""
    def rnd(v):
        return int(math.copysign(math.floor(abs(v) + 0.5), v))

    cells = {}
    step = 360 / count
    for k in range(count):
        deg = offset + k * step
        if deg > 45 + 1e-9:
            break
        a = math.radians(deg)
        for rr in np.arange(r0, r1, 0.2):
            e, n = rnd(rr * math.sin(a)), rnd(rr * math.cos(a))
            for se in (1, -1):
                for sn in (1, -1):
                    for de, dn in ((e, n), (n, e)):
                        cells.setdefault((C + se * de, C - sn * dn), rr)
    return cells


def top_house(m):
    zi = np.floor(m.z)
    r, phi = m.r, m.phi
    slab_top = f"{WHITE_SLAB}[type=top,waterlogged=false]"
    slab_bottom = f"{WHITE_SLAB}[type=bottom,waterlogged=false]"
    # collar ring the legs end in
    m.put((r >= 9.3) & (r <= 10.7) & (zi == 148), WHITE)
    # sunburst skirt: ribs curving down from the 17.6 m rim to the collar, 32 so each reads at 1 m
    white = m.id(WHITE)
    m.put((r > 10.2) & (r <= 17.6) & (zi == 150), SOFFIT)
    for (x, zz), rr in rays(FINS, 10.6, 17.6).items():
        b = 148.2 + 2.4 * ((rr - 10.0) / 7.6) ** 1.6
        y0 = m.layer(b + 0.25)
        if b + 0.25 - math.floor(b + 0.25) >= 0.5:
            m.grid[x, y0, zz] = m.id(slab_top)
            y0 += 1
        for y in range(y0, m.layer(150) + 1):
            m.grid[x, y, zz] = white
    m.put((r <= 10.5) & (zi == 151), SOFFIT)
    m.put((r > 15.9) & (r <= 17.7) & (zi == 151), WHITE)
    # The Loupe, 500 ft: dark glass and its revolving glass floor ring
    m.put((r > 10.5) & (r <= 15.9) & (zi == 151), LOUPE)
    m.put((r <= 15.9) & (r > 15.0) & (zi >= 152) & (zi <= 154), LOUPE)
    # halo: joisted soffit under the outer deck, a grating beyond on 48 joists, a tan rim
    m.put((r <= 18.2) & (zi == 155), SOFFIT)
    for (x, zz), rr in rays(48, 16.0, 18.2).items():
        m.grid[x, m.layer(155), zz] = white
    m.put((r <= 18.2) & (zi == 156), WHITE)
    m.put((r > 18.2) & (r <= 20.3) & (zi == 156),
          f"{GRATE}[facing=north,half=top,open=false,powered=false,waterlogged=false]")
    joist = m.id(f"{JOIST}[type=top,waterlogged=false]")
    for (x, zz), rr in rays(48, 18.2, 20.4, 3.75).items():
        m.grid[x, m.layer(156), zz] = joist
    m.put((r > 20.3) & (r <= 21.1) & (zi == 156), RIM_EDGE)
    # open-air deck at 520 ft behind 48 glass barriers, under a glazed ring
    m.put(close_ring((r > 17.3) & (r <= 18.2) & (zi >= 157) & (zi <= 159), 17.75), PANE)
    m.put((r > 15.4) & (r <= 18.2) & (zi == 160), CLEAR_GLASS)
    # indoor observation level, floor-to-ceiling glass round the lift lobby
    m.put((r <= 15.4) & (r > 14.5) & (zi >= 157) & (zi <= 159), CLEAR_GLASS)
    m.put((r <= 6.0) & (zi >= 152) & (zi <= 159), SHAFT)
    # roof cone, 20 deg (lidar): white half-step rings, a stair where a ring edge halves a cell
    slope, top = 0.354, 160.7 + 0.354 * 15.4
    e, n = m.e, m.n
    ax = np.abs(e) >= np.abs(n)
    toward = np.where(ax, np.where(e > 0, 1, 0), np.where(n > 0, 2, 3))
    shift = 0.25 * slope * np.where(ax, np.abs(e), np.abs(n)) / np.maximum(r, 1e-6)
    faces = ("east", "west", "south", "north")
    for y in range(160, 166):
        frac = top - slope * r - y
        cone = (r <= 15.4) & (zi == y)
        step = cone & (frac + shift >= 0.75) & (frac - shift < 0.75)
        m.put(cone & (frac >= 0.75) & ~step, WHITE)
        m.put(cone & (frac >= 0.25) & (frac < 0.75) & ~step, slab_bottom)
        for k, f in enumerate(faces):
            m.put(step & (toward == k), stairs(WHITE_STAIRS, f, "bottom"))
    # six roof vents
    m.put((r > 9.6) & (r < 11.4) & spokes(phi, r, 6, 1.6, 30) & (zi == 162), WHITE)
    # the white crown: waisted, then flaring to its lip
    for y, rr in {164: 5.2, 165: 5.2, 166: 5.2, 167: 6.6}.items():
        m.put((r <= rr) & (zi == y), WHITE)
    m.put((r > 5.2) & (r <= 6.2) & (zi == 164), slab_bottom)
    m.put((r > 5.2) & (r <= 6.0) & (zi == 166), slab_top)
    m.put((r <= 7.4) & (r > 6.6) & (zi == 167), slab_bottom)
    # mechanical lantern with louvres, its railed roof, the spire foot
    lant = (r <= 5.2) & (zi == 168)
    m.put(lant, LANTERN)
    m.put(lant & (r > 4.2) & (np.mod(np.floor(phi / 15), 2) == 0), LOUVRE)
    m.put((r <= 4.3) & (zi == 169), f"{LANTERN_ROOF}[type=bottom,waterlogged=false]")
    m.put((r <= 5.2) & (r > 4.3) & (zi == 169), LATTICE)
    m.put((r <= 1.5) & (zi == 170), LANTERN)


def spire(m):
    """Lattice mast to the aircraft beacon at 605 ft, a lightning rod above."""
    for z in range(171, 184):
        m.set(0, 0, z, MAST)
        if z < 176:
            for de, dn in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                m.set(de, dn, z, LATTICE)
    m.set(0, 0, 184, BEACON)
    m.set(0, 0, 185, "lightning_rod[facing=up,powered=false,waterlogged=false]")


def lights(m):
    """Night look: ceiling lamps; invisible light on legs, pan, halo, glazing and beacon."""
    g = m.grid

    def lamp_ring(z, rr, count, offset, name):
        for k in range(count):
            a = math.radians(offset + 360 * k / count)
            x, y, zz = m.cell(rr * math.sin(a), rr * math.cos(a), z + 0.5)
            if name == LAMP or g[x, y, zz] == 0:
                g[x, y, zz] = m.id(name)

    # ceiling lamps over the observation level, the Loupe and the SkyLine, roofed over above
    lamp_ring(160, 11.0, 12, 15, LAMP)
    lamp_ring(155, 11.0, 12, 15, LAMP)
    for gap in GAPS:
        for d in (-12, 0, 12):
            lamp_ring(34, 10.8, 1, gap + d, LAMP)
    # glazed levels lit from inside
    lamp_ring(157, 10.5, 8, 0, LIGHT)
    lamp_ring(152, 10.5, 8, 22.5, LIGHT)
    for gap in GAPS:
        for d in (-15, 15):
            lamp_ring(32, 12.5, 1, gap + d, LIGHT)
    lamp_ring(2, 15.5, 8, 0, LIGHT)
    # halo: the deck under the canopy, the grating above and below
    lamp_ring(157, 16.4, 12, 0, LIGHT)
    lamp_ring(157, 19.6, 16, 11.25, LIGHT)
    lamp_ring(154, 19.6, 16, 0, LIGHT)
    # bottom pan and legs, floodlit
    lamp_ring(146, 14.0, 12, 15, LIGHT)
    for p in LEGS:
        s, c = math.sin(math.radians(p)), math.cos(math.radians(p))
        for z in range(8, 146, 6):
            side = 1 if z % 12 == 8 else -1
            rho = float(leg_rho(z)) if z < 115 else float(arm_rho(z))
            t = side * (float(leg_t(z)) if z < 115 else 1.0)
            x, y, zz = m.cell((rho + 2.2) * s + t * c, (rho + 2.2) * c - t * s, z + 0.5)
            if g[x, y, zz] == 0:
                g[x, y, zz] = m.id(LIGHT)
    m.set(1, 0, 184, LIGHT)


CONNECT = ("glass_pane", "iron_bars")


def close_ring(mask, radius):
    """Add the cell beside each diagonal-only step nearer the ring, so panes and bars join."""
    out = mask.copy()
    for x, y, z in np.argwhere(mask):
        for dx, dz in ((1, 1), (1, -1)):
            nx, nz = x + dx, z + dz
            if not (0 <= nx < W and 0 <= nz < L and mask[nx, y, nz]):
                continue
            if mask[nx, y, z] or mask[x, y, nz]:
                continue
            a, b = (nx, z), (x, nz)
            pick = min((a, b), key=lambda c: abs(math.hypot(c[0] - C, c[1] - C) - radius))
            out[pick[0], y, pick[1]] = True
    return out


def connect(m):
    """Give panes, bars and walls the sides Minecraft would join them on."""
    names = [n.split("[")[0] for n in m.names]
    joins = np.array([n in CONNECT or n.endswith("_wall") for n in names])
    partial = ("_slab", "_stairs", "_trapdoor", "_rod", "_pane", "_bars", "_wall", "lantern",
               "light", "_carpet")
    solid = np.array([n != "air" and not n.endswith(partial) for n in names])
    g = m.grid
    links = joins[g] | solid[g]
    dirs = {"east": (1, 0), "west": (-1, 0), "south": (0, 1), "north": (0, -1)}
    for x, y, z in np.argwhere(joins[g]):
        base = names[g[x, y, z]]
        props = []
        for d, (dx, dz) in dirs.items():
            nx, nz = x + dx, z + dz
            on = 0 <= nx < W and 0 <= nz < L and links[nx, y, nz]
            if base.endswith("_wall"):
                props.append(f"{d}={'low' if on else 'none'}")
            else:
                props.append(f"{d}={'true' if on else 'false'}")
        if base.endswith("_wall"):
            props.append("up=true")
        props.append("waterlogged=false")
        g[x, y, z] = m.id(f"{base}[{','.join(sorted(props))}]")


def build():
    m = Model()
    pavilion(m)
    core(m)
    skyline(m)
    deck200(m)
    legs(m)
    lifts(m)
    top_house(m)
    spire(m)
    chamfer_legs(m)
    stair_shapes(m)
    connect(m)
    lights(m)
    return m


# ---------------------------------------------------------------- Sponge v3


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


def sponge(grid, names):
    w, h, l = grid.shape
    palette = {"minecraft:" + n: i for i, n in enumerate(names)}
    data = bytearray()
    for v in grid.transpose(1, 2, 0).ravel():
        v = int(v)
        while True:
            b = v & 0x7F
            v >>= 7
            if v:
                data.append(b | 0x80)
            else:
                data.append(b)
                break
    schematic = {
        "Version": (3, 3),
        "DataVersion": (3, DATA_VERSION),
        "Width": (2, w),
        "Height": (2, h),
        "Length": (2, l),
        "Offset": (11, [0, 0, 0]),
        "Blocks": (10, {
            "Palette": (10, {k: (3, v) for k, v in palette.items()}),
            "Data": (7, data),
            "BlockEntities": (9, (10, [])),
        }),
    }
    out = io.BytesIO()
    out.write(b"\x0a" + struct.pack(">H", 0))
    _payload(out, 10, {"Schematic": (10, schematic)})
    return gzip.compress(out.getvalue(), mtime=0)


def unmapped(names):
    try:
        src = open(SCHEMATIC_RS).read()
    except OSError:
        return []
    known = set(re.findall(r'"([a-z_]+)"', src))
    return sorted({n.split("[")[0] for n in names} - known - {"air"})


def main():
    out = sys.argv[sys.argv.index("--out") + 1] if "--out" in sys.argv else OUT
    m = build()
    used = np.unique(m.grid)
    with open(out, "wb") as f:
        f.write(sponge(m.grid, m.names))
    ys = np.nonzero((m.grid > 0).any(axis=(0, 2)))[0]
    print(f"{out}: {W}x{H}x{L}, {int((m.grid > 0).sum())} blocks, {len(used) - 1} states, "
          f"top {ys.max() + 1 - GROUND} m above the plaza")
    missing = unmapped(m.names)
    if missing:
        print("not in map_structure_block:", ", ".join(missing))


if __name__ == "__main__":
    main()
